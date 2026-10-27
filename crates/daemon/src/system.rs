//! What the machine is doing, for the resource rows in the app's left pane:
//! how busy the CPU is, memory in use, NVIDIA GPUs, the disk a project is
//! on, the memory each kernel holds, and the CPU and memory each job uses.
//! Linux reads `/proc`; macOS asks the kernel through libc.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sandhill_proto::{
    CpuStats, DiskStats, GpuStats, JobUsage, MemoryStats, SessionMemory, SystemStats,
};

/// A CPU percentage needs two readings. Each call measures from the
/// previous one; without a recent one it takes a second reading after this.
const CPU_SAMPLE: Duration = Duration::from_millis(250);
/// A previous reading older than this says little about now.
const CPU_STALE: Duration = Duration::from_secs(30);
/// `nvidia-smi` can hang when a driver is wedged.
const GPU_TIMEOUT: Duration = Duration::from_secs(3);

/// Busy and total CPU time since boot, in the system's ticks.
#[derive(Debug, Clone, Copy)]
struct CpuTimes {
    busy: u64,
    total: u64,
}

static LAST_CPU: Mutex<Option<(Instant, CpuTimes)>> = Mutex::new(None);

/// Each job's CPU time at its last measuring, to measure the next from.
static LAST_JOBS: Mutex<Option<HashMap<String, (Instant, Duration)>>> = Mutex::new(None);

/// What a process and the processes it started hold now, and the CPU time
/// they have used so far (processes that have ended no longer count).
#[derive(Debug, Clone, Copy)]
struct TreeUsage {
    rss: u64,
    cpu: Duration,
}

/// A snapshot of the machine. `path` picks the disk; `kernels` pairs each
/// running session with its kernel's process id, and `jobs` each running
/// job with its program's. Blocks for a moment on the first call, to
/// measure the CPU.
pub fn stats(path: &Path, kernels: &[(String, u32)], jobs: &[(String, u32)]) -> SystemStats {
    SystemStats {
        cpu: cpu(),
        memory: memory().unwrap_or(MemoryStats { total: 0, used: 0 }),
        gpus: gpus(),
        disk: disk(path),
        sessions: kernels
            .iter()
            .filter_map(|(session, pid)| {
                process_tree(*pid).map(|usage| SessionMemory {
                    session: session.clone(),
                    rss: usage.rss,
                })
            })
            .collect(),
        jobs: job_usage(jobs),
    }
}

/// The CPU and memory of each job, the CPU since it was last measured.
fn job_usage(jobs: &[(String, u32)]) -> Vec<JobUsage> {
    let now = Instant::now();
    let mut last = LAST_JOBS.lock().unwrap_or_else(|e| e.into_inner());
    let last = last.get_or_insert_with(HashMap::new);
    let usage: Vec<JobUsage> = jobs
        .iter()
        .filter_map(|(terminal, pid)| {
            let usage = process_tree(*pid)?;
            let cpu = last
                .insert(terminal.clone(), (now, usage.cpu))
                .and_then(|(then, before)| {
                    let wall = now.duration_since(then);
                    (wall >= CPU_SAMPLE && wall <= CPU_STALE).then(|| {
                        let busy = usage.cpu.saturating_sub(before);
                        (busy.as_secs_f64() / wall.as_secs_f64() * 1000.0).round() / 10.0
                    })
                });
            Some(JobUsage {
                terminal: terminal.clone(),
                cpu,
                rss: usage.rss,
            })
        })
        .collect();
    last.retain(|terminal, _| jobs.iter().any(|(t, _)| t == terminal));
    usage
}

fn cpu() -> CpuStats {
    let percent = cpu_percent().unwrap_or(0.0);
    CpuStats {
        percent,
        cores: cores(),
        model: cpu_model().unwrap_or_default(),
        load: load_average().unwrap_or(0.0),
    }
}

fn cpu_percent() -> Option<f64> {
    let previous = *LAST_CPU.lock().unwrap();
    let mut now = cpu_times()?;
    let from = match previous {
        Some((at, times)) if at.elapsed() >= CPU_SAMPLE && at.elapsed() < CPU_STALE => times,
        _ => {
            std::thread::sleep(CPU_SAMPLE);
            std::mem::replace(&mut now, cpu_times()?)
        }
    };
    *LAST_CPU.lock().unwrap() = Some((Instant::now(), now));
    let total = now.total.saturating_sub(from.total);
    let busy = now.busy.saturating_sub(from.busy);
    (total > 0).then(|| (busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

fn cores() -> u32 {
    // SAFETY: sysconf only reads a system setting.
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    u32::try_from(n).ok().filter(|&n| n > 0).unwrap_or(1)
}

// The statvfs fields are u64 on Linux but u32 on macOS.
#[allow(clippy::unnecessary_cast)]
fn disk(path: &Path) -> Option<DiskStats> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut fs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: c_path is a valid C string and fs is large enough for statvfs.
    if unsafe { libc::statvfs(c_path.as_ptr(), fs.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so it filled fs.
    let fs = unsafe { fs.assume_init() };
    let unit = fs.f_frsize as u64;
    let total = fs.f_blocks as u64 * unit;
    let free = fs.f_bfree as u64 * unit;
    Some(DiskStats {
        path: path.to_string_lossy().into_owned(),
        total,
        used: total.saturating_sub(free),
        available: fs.f_bavail as u64 * unit,
    })
}

/// NVIDIA GPUs, from `nvidia-smi`. None on machines without the tool.
fn gpus() -> Vec<GpuStats> {
    let child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Vec::new();
    };
    let deadline = Instant::now() + GPU_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
    }
    parse_nvidia_smi(&out)
}

/// Lines like `NVIDIA A100-SXM4-40GB, 84, 33000, 40960` (memory in MiB).
fn parse_nvidia_smi(text: &str) -> Vec<GpuStats> {
    const MIB: u64 = 1 << 20;
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            let [name, util, used, total] = fields[..] else {
                return None;
            };
            Some(GpuStats {
                name: name.to_string(),
                percent: util.parse().unwrap_or(0.0),
                memory_used: used.parse::<u64>().ok()? * MIB,
                memory_total: total.parse::<u64>().ok()? * MIB,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn cpu_times() -> Option<CpuTimes> {
    parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

/// The first line of /proc/stat: `cpu  user nice system idle iowait irq
/// softirq steal ...`. Idle and iowait count as not busy; guest time is
/// already part of user time.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let ticks: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|t| t.parse().ok())
        .collect();
    if ticks.len() < 4 {
        return None;
    }
    let total: u64 = ticks.iter().sum();
    let idle = ticks[3] + ticks.get(4).copied().unwrap_or(0);
    Some(CpuTimes {
        busy: total - idle,
        total,
    })
}

#[cfg(target_os = "linux")]
fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    info.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, name)| name.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(target_os = "linux")]
fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
fn memory() -> Option<MemoryStats> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// Used memory is what top and free call used: total less what is
/// available, which counts caches the kernel would drop.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(text: &str) -> Option<MemoryStats> {
    let field = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name) && l[name.len()..].starts_with(':'))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable").or_else(|| field("MemFree"))?;
    Some(MemoryStats {
        total,
        used: total.saturating_sub(available),
    })
}

/// Resident memory and CPU time of a process and everything it started (an
/// R kernel running mclapply, say).
#[cfg(target_os = "linux")]
fn process_tree(pid: u32) -> Option<TreeUsage> {
    let page = page_size();
    // SAFETY: sysconf only reads a system setting.
    let ticks = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
        .ok()
        .filter(|&t| t > 0)
        .unwrap_or(100);
    let rss = |pid: u32| -> Option<u64> {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        Some(statm.split_whitespace().nth(1)?.parse::<u64>().ok()? * page)
    };
    let mut total = rss(pid)?;
    // Parent and CPU ticks (user and system) of every process, to find the tree under pid.
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut cpu: HashMap<u32, u64> = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(child) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{child}/stat")) else {
            continue;
        };
        // The command name can hold spaces and parentheses; fields after it don't.
        let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let fields: Vec<&str> = after.split_whitespace().collect();
        if let Some(parent) = fields.get(1).and_then(|p| p.parse().ok()) {
            children.entry(parent).or_default().push(child);
        }
        let time = |i: usize| {
            fields
                .get(i)
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(0)
        };
        cpu.insert(child, time(11) + time(12));
    }
    let mut busy = cpu.get(&pid).copied().unwrap_or(0);
    let mut stack = children.remove(&pid).unwrap_or_default();
    while let Some(p) = stack.pop() {
        total += rss(p).unwrap_or(0);
        busy += cpu.get(&p).copied().unwrap_or(0);
        stack.extend(children.remove(&p).unwrap_or_default());
    }
    Some(TreeUsage {
        rss: total,
        cpu: Duration::from_nanos(busy.saturating_mul(1_000_000_000) / ticks),
    })
}

#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    // SAFETY: sysconf only reads a system setting.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(size).unwrap_or(4096)
}

#[cfg(target_os = "macos")]
fn cpu_times() -> Option<CpuTimes> {
    let mut info = std::mem::MaybeUninit::<libc::host_cpu_load_info>::uninit();
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: info has room for HOST_CPU_LOAD_INFO_COUNT integers, as the
    // flavor requires; the call fills it when it returns success.
    let ok = unsafe {
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        libc::host_statistics(
            host,
            libc::HOST_CPU_LOAD_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    } == libc::KERN_SUCCESS;
    if !ok {
        return None;
    }
    // SAFETY: host_statistics succeeded.
    let ticks = unsafe { info.assume_init() }.cpu_ticks;
    let total: u64 = ticks.iter().map(|&t| u64::from(t)).sum();
    let idle = u64::from(ticks[libc::CPU_STATE_IDLE as usize]);
    Some(CpuTimes {
        busy: total - idle,
        total,
    })
}

#[cfg(target_os = "macos")]
fn sysctl_bytes(name: &str) -> Option<Vec<u8>> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut len = 0usize;
    // SAFETY: a null buffer asks only for the size.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buf holds len bytes.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

#[cfg(target_os = "macos")]
fn cpu_model() -> Option<String> {
    let bytes = sysctl_bytes("machdep.cpu.brand_string")?;
    let text = String::from_utf8_lossy(&bytes);
    Some(text.trim_end_matches('\0').trim().to_string())
}

#[cfg(target_os = "macos")]
fn load_average() -> Option<f64> {
    let mut load = [0f64; 1];
    // SAFETY: load has room for the one value asked for.
    (unsafe { libc::getloadavg(load.as_mut_ptr(), 1) } == 1).then_some(load[0])
}

/// Used memory as Activity Monitor counts it: app memory (anonymous pages
/// that can't be purged), wired memory and the compressor's pages.
#[cfg(target_os = "macos")]
fn memory() -> Option<MemoryStats> {
    let total = sysctl_bytes("hw.memsize")?;
    let total = u64::from_ne_bytes(total.get(..8)?.try_into().ok()?);
    let mut info = std::mem::MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: info has room for HOST_VM_INFO64_COUNT integers, as the flavor
    // requires; the call fills it when it returns success.
    let ok = unsafe {
        #[allow(deprecated)]
        let host = libc::mach_host_self();
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    } == libc::KERN_SUCCESS;
    if !ok {
        return None;
    }
    // SAFETY: host_statistics64 succeeded.
    let info = unsafe { info.assume_init() };
    // SAFETY: sysconf only reads a system setting.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(16384);
    let app = u64::from(info.internal_page_count).saturating_sub(u64::from(info.purgeable_count));
    let pages = app + u64::from(info.wire_count) + u64::from(info.compressor_page_count);
    Some(MemoryStats {
        total,
        used: (pages * page).min(total),
    })
}

/// Resident memory and CPU time of a process and the processes it started.
#[cfg(target_os = "macos")]
fn process_tree(pid: u32) -> Option<TreeUsage> {
    let info = |pid: i32| -> Option<libc::proc_taskinfo> {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
        // SAFETY: info is size bytes long.
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        // SAFETY: proc_pidinfo filled all of info.
        (n == size).then(|| unsafe { info.assume_init() })
    };
    // Task times are in Mach ticks: nanoseconds on Intel, not on Apple silicon.
    #[allow(deprecated)]
    let mut base = std::mem::MaybeUninit::<libc::mach_timebase_info>::zeroed();
    // SAFETY: base is a mach_timebase_info, which the call fills.
    #[allow(deprecated)]
    let ok = unsafe { libc::mach_timebase_info(base.as_mut_ptr()) } == 0;
    // SAFETY: zeroed is a valid mach_timebase_info, filled if ok.
    #[allow(deprecated)]
    let base = unsafe { base.assume_init() };
    let (numer, denom) = if ok && base.denom > 0 {
        (u128::from(base.numer), u128::from(base.denom))
    } else {
        (1, 1)
    };
    let usage = |task: &libc::proc_taskinfo| {
        let ticks = u128::from(task.pti_total_user) + u128::from(task.pti_total_system);
        (task.pti_resident_size, ticks * numer / denom)
    };
    let children = |pid: i32| -> Vec<i32> {
        let mut buf = vec![0i32; 256];
        let bytes = (buf.len() * std::mem::size_of::<i32>()) as i32;
        // SAFETY: buf is bytes long; the call writes at most that.
        let n = unsafe { libc::proc_listchildpids(pid, buf.as_mut_ptr().cast(), bytes) };
        buf.truncate(usize::try_from(n).unwrap_or(0).min(256));
        buf
    };
    let root = i32::try_from(pid).ok()?;
    let (mut total, mut nanos) = usage(&info(root)?);
    let mut stack = children(root);
    while let Some(p) = stack.pop() {
        if let Some((rss, busy)) = info(p).as_ref().map(usage) {
            total += rss;
            nanos += busy;
        }
        stack.extend(children(p));
    }
    Some(TreeUsage {
        rss: total,
        cpu: Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_proc_stat_and_meminfo() {
        let stat = "cpu  100 5 50 800 40 0 5 0 0 0\ncpu0 50 2 25 400 20 0 3 0 0 0\n";
        let t = parse_proc_stat(stat).unwrap();
        assert_eq!(t.total, 1000);
        assert_eq!(t.busy, 160);

        let meminfo =
            "MemTotal:       16000000 kB\nMemFree:  1000000 kB\nMemAvailable:    6000000 kB\n";
        let m = parse_meminfo(meminfo).unwrap();
        assert_eq!(m.total, 16_000_000 * 1024);
        assert_eq!(m.used, 10_000_000 * 1024);
    }

    #[test]
    fn reads_nvidia_smi() {
        let gpus = parse_nvidia_smi("NVIDIA A100-SXM4-40GB, 84, 33000, 40960\nbad line\n");
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "NVIDIA A100-SXM4-40GB");
        assert_eq!(gpus[0].percent, 84.0);
        assert_eq!(gpus[0].memory_total, 40960 << 20);
    }

    #[test]
    fn measures_this_machine() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let s = stats(dir.path(), &[("me".into(), me)], &[("job".into(), me)]);
        assert!(s.cpu.cores >= 1);
        assert!((0.0..=100.0).contains(&s.cpu.percent));
        assert!(s.memory.total > 0 && s.memory.used <= s.memory.total);
        let disk = s.disk.unwrap();
        assert!(disk.total > 0 && disk.used <= disk.total);
        assert_eq!(s.sessions.len(), 1);
        assert!(s.sessions[0].rss > 0);
        assert_eq!(s.jobs.len(), 1);
        assert!(s.jobs[0].rss > 0);
        assert_eq!(s.jobs[0].cpu, None);
        // Busy for a moment: the next measuring says how busy.
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(400) {
            std::hint::black_box(0);
        }
        let again = stats(dir.path(), &[], &[("job".into(), me)]);
        assert!(
            again.jobs[0].cpu.is_some_and(|cpu| cpu > 10.0),
            "{:?}",
            again.jobs
        );
    }
}
