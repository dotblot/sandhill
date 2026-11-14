//! Read-only hardware discovery. No elevated commands or private Apple APIs.
use sandhill_proto::{GpuStats, GpuStatus};

#[cfg(any(target_os = "linux", test))]
fn percentage(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
}

#[cfg(any(target_os = "linux", test))]
fn temperature(value: f64) -> Option<f64> {
    (value.is_finite() && (-20.0..=150.0).contains(&value)).then_some(value)
}

#[cfg(any(target_os = "linux", test))]
fn parse_nvidia_smi(text: &str) -> Vec<GpuStats> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            let [name, util, used, total, temp] = fields[..] else {
                return None;
            };
            if name.is_empty() {
                return None;
            }
            let mib = |s: &str| s.parse::<u64>().ok()?.checked_mul(1 << 20);
            Some(GpuStats {
                name: name.into(),
                percent: percentage(util),
                memory_used: mib(used),
                memory_total: mib(total).filter(|v| *v > 0),
                temperature_celsius: temp.parse().ok().and_then(temperature),
            })
        })
        .collect()
}

/// Drain both pipes while waiting. Nonblocking reads also bound inherited pipes.
#[cfg(any(target_os = "linux", test))]
fn command_output(program: &str, args: &[&str]) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind, Read};
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::other("missing stdout"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::other("missing stderr"))?;
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: fd belongs to our live pipe. Preserve existing flags.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            // SAFETY: same live fd, setting only nonblocking mode.
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(Error::last_os_error());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output = Vec::new();
        let mut errors = Vec::new();
        loop {
            let mut all_eof = true;
            for (pipe, bytes) in [
                (&mut stdout as &mut dyn Read, &mut output),
                (&mut stderr as &mut dyn Read, &mut errors),
            ] {
                let mut buf = [0u8; 8192];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if bytes.len() + n > 1 << 20 {
                                return Err(Error::other("hardware output too large"));
                            }
                            bytes.extend_from_slice(&buf[..n]);
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            all_eof = false;
                            break;
                        }
                        Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(e),
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::new(ErrorKind::TimedOut, "hardware query timed out"));
                    }
                }
            }
            if let Some(status) = child.try_wait()?
                && all_eof
            {
                if !status.success() {
                    let diagnostics = if errors.is_empty() { &output } else { &errors };
                    let detail: String = String::from_utf8_lossy(diagnostics)
                        .chars()
                        .take(512)
                        .collect();
                    return Err(Error::other(format!(
                        "hardware query failed: {}",
                        detail.trim()
                    )));
                }
                return String::from_utf8(output)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e));
            }
            if Instant::now() >= deadline {
                return Err(Error::new(ErrorKind::TimedOut, "hardware query timed out"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(any(target_os = "linux", test))]
fn read_number(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(any(target_os = "linux", test))]
fn hwmon_temperature(root: &std::path::Path, cpu_only: bool) -> Option<f64> {
    let mut hottest: Option<f64> = None;
    for monitor in std::fs::read_dir(root).ok()?.flatten() {
        let amd_physical_only = if cpu_only {
            let driver = std::fs::read_to_string(monitor.path().join("name")).unwrap_or_default();
            if !matches!(
                driver.trim(),
                "coretemp" | "k10temp" | "zenpower" | "cpu_thermal"
            ) {
                continue;
            }
            matches!(driver.trim(), "k10temp" | "zenpower")
        } else {
            false
        };
        let Ok(entries) = std::fs::read_dir(monitor.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("temp") || !name.ends_with("_input") {
                continue;
            }
            if amd_physical_only {
                // Tctl can contain a fan-control offset, not a physical die
                // temperature. Require an explicitly labelled physical channel.
                let channel = name.trim_end_matches("_input");
                let label =
                    std::fs::read_to_string(monitor.path().join(format!("{channel}_label")))
                        .unwrap_or_default();
                let label = label.trim().to_ascii_lowercase();
                let physical = label == "tdie"
                    || ["tccd", "ccd"].into_iter().any(|prefix| {
                        label
                            .strip_prefix(prefix)
                            .is_some_and(|suffix| suffix.bytes().all(|b| b.is_ascii_digit()))
                    });
                if !physical {
                    continue;
                }
            }
            let Some(value) = std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
                .and_then(|v| temperature(v / 1000.0))
            else {
                continue;
            };
            hottest = Some(hottest.map_or(value, |before| before.max(value)));
        }
    }
    hottest
}

#[cfg(target_os = "linux")]
pub(super) fn cpu_temperature() -> Option<f64> {
    hwmon_temperature(std::path::Path::new("/sys/class/hwmon"), true)
}

#[cfg(target_os = "macos")]
pub(super) fn cpu_temperature() -> Option<f64> {
    // Apple exposes no stable public unprivileged CPU temperature API.
    None
}

#[cfg(any(target_os = "linux", test))]
fn drm_gpus(root: &std::path::Path, have_nvidia: bool) -> std::io::Result<Vec<GpuStats>> {
    let mut gpus = Vec::new();
    for card in std::fs::read_dir(root)? {
        let card = card?;
        let name = card.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name.strip_prefix("card") else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let device = card.path().join("device");
        let vendor = std::fs::read_to_string(device.join("vendor")).unwrap_or_default();
        let vendor_name = match vendor.trim() {
            "0x10de" if have_nvidia => continue,
            "0x10de" => "NVIDIA",
            "0x1002" => "AMD",
            "0x8086" => "Intel",
            _ => "DRM",
        };
        let device_id = std::fs::read_to_string(device.join("device")).unwrap_or_default();
        let model = std::fs::read_to_string(device.join("product_name"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{vendor_name} GPU ({}, {})", name, device_id.trim()));
        let percent = std::fs::read_to_string(device.join("gpu_busy_percent"))
            .ok()
            .and_then(|s| percentage(&s));
        gpus.push(GpuStats {
            name: model,
            percent,
            memory_used: read_number(&device.join("mem_info_vram_used")),
            memory_total: read_number(&device.join("mem_info_vram_total")).filter(|v| *v > 0),
            temperature_celsius: hwmon_temperature(&device.join("hwmon"), false),
        });
    }
    Ok(gpus)
}

#[cfg(target_os = "linux")]
pub(super) fn gpus() -> (Vec<GpuStats>, GpuStatus, Option<String>) {
    let nvidia = command_output(
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
    );
    let mut errors = Vec::new();
    if let Err(error) = &nvidia {
        if error.kind() != std::io::ErrorKind::NotFound {
            errors.push(format!("NVIDIA metrics unavailable: {error}"));
        }
    }
    let mut gpus = nvidia
        .as_ref()
        .ok()
        .map(|s| parse_nvidia_smi(s))
        .unwrap_or_default();
    if nvidia.as_ref().is_ok_and(|text| !text.trim().is_empty()) && gpus.is_empty() {
        errors.push("NVIDIA metrics unavailable: unrecognized nvidia-smi output".into());
    }
    let drm = drm_gpus(std::path::Path::new("/sys/class/drm"), !gpus.is_empty());
    match drm {
        Ok(devices) => gpus.extend(devices),
        Err(error) => errors.push(format!("DRM discovery unavailable: {error}")),
    }
    if nvidia
        .as_ref()
        .err()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        && gpus.iter().any(|gpu| gpu.name.starts_with("NVIDIA"))
    {
        errors.push("NVIDIA metrics unavailable: nvidia-smi is not installed".into());
    }
    let error = (!errors.is_empty()).then(|| errors.join("\n"));
    let status = if error.is_some() {
        GpuStatus::Unavailable
    } else if gpus.is_empty() {
        GpuStatus::Absent
    } else {
        GpuStatus::Available
    };
    (gpus, status, error)
}

#[cfg(target_os = "macos")]
pub(super) fn gpus() -> (Vec<GpuStats>, GpuStatus, Option<String>) {
    let (gpus, status) = apple::gpus();
    let error =
        (status == GpuStatus::Unavailable).then(|| "IOKit GPU discovery unavailable".into());
    (gpus, status, error)
}

/// Public IOKit registry access, also used by system inspection tools. No
/// user-client connection, SMC access, powermetrics, or private HID sensors.
#[cfg(target_os = "macos")]
mod apple {
    use super::*;
    use std::ffi::{CString, c_char, c_void};
    type Cf = *const c_void;
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> Cf;
        fn IOServiceGetMatchingServices(port: u32, matching: Cf, iterator: *mut u32) -> i32;
        fn IOIteratorNext(iterator: u32) -> u32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: Cf, allocator: Cf, options: u32) -> Cf;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Cf);
        fn CFGetTypeID(value: Cf) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFDataGetTypeID() -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFStringCreateWithCString(allocator: Cf, text: *const c_char, encoding: u32) -> Cf;
        fn CFStringGetCString(value: Cf, buffer: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFDataGetLength(value: Cf) -> isize;
        fn CFDataGetBytePtr(value: Cf) -> *const u8;
        fn CFNumberGetValue(value: Cf, kind: isize, out: *mut c_void) -> bool;
        fn CFDictionaryGetValue(dictionary: Cf, key: Cf) -> Cf;
    }
    struct Owned(Cf);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: each Owned wraps exactly one Create-rule CF object.
                unsafe { CFRelease(self.0) };
            }
        }
    }
    fn key(text: &str) -> Owned {
        let text = CString::new(text).expect("static registry key");
        // SAFETY: text is a valid NUL-terminated UTF-8 string; default allocator.
        Owned(unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), 0x08000100) })
    }
    fn number(dictionary: Cf, name: &str) -> Option<f64> {
        if dictionary.is_null() {
            return None;
        }
        let name = key(name);
        // SAFETY: check both runtime types before reading their values.
        unsafe {
            if CFGetTypeID(dictionary) != CFDictionaryGetTypeID() {
                return None;
            }
            let value = CFDictionaryGetValue(dictionary, name.0);
            if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
                return None;
            }
            let mut result = 0f64;
            (CFNumberGetValue(value, 13, (&mut result as *mut f64).cast()) && result.is_finite())
                .then_some(result)
        }
    }
    fn model(entry: u32) -> Option<String> {
        let name = key("model");
        // SAFETY: live registry entry and key. Do not search up to the Mac's
        // platform model, which is not the GPU model.
        let value =
            Owned(unsafe { IORegistryEntryCreateCFProperty(entry, name.0, std::ptr::null(), 0) });
        if value.0.is_null() {
            return None;
        }
        // SAFETY: runtime-checked CFData/CFString; buffer lives through the call.
        unsafe {
            if CFGetTypeID(value.0) == CFDataGetTypeID() {
                let len = CFDataGetLength(value.0);
                if !(1..=4096).contains(&len) {
                    return None;
                }
                let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(value.0), len as usize);
                return Some(
                    String::from_utf8_lossy(bytes)
                        .trim_end_matches('\0')
                        .to_string(),
                );
            }
            if CFGetTypeID(value.0) == CFStringGetTypeID() {
                let mut buf = [0u8; 4096];
                if CFStringGetCString(
                    value.0,
                    buf.as_mut_ptr().cast(),
                    buf.len() as isize,
                    0x08000100,
                ) {
                    return Some(
                        std::ffi::CStr::from_ptr(buf.as_ptr().cast())
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        None
    }
    pub(super) fn gpus() -> (Vec<GpuStats>, GpuStatus) {
        let mut iterator = 0;
        // SAFETY: a static valid C string, returning a Create-rule dictionary.
        let matching = unsafe { IOServiceMatching(c"IOAccelerator".as_ptr()) };
        if matching.is_null() {
            return (Vec::new(), GpuStatus::Unavailable);
        }
        // SAFETY: matching transfers its Create-rule dictionary to IOKit.
        let result = unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) };
        if result != 0 {
            return (Vec::new(), GpuStatus::Unavailable);
        }
        let mut gpus = Vec::new();
        loop {
            // SAFETY: iterator is valid until released below.
            let entry = unsafe { IOIteratorNext(iterator) };
            if entry == 0 {
                break;
            }
            let name = key("PerformanceStatistics");
            // SAFETY: live registry object and key; returned property has Create ownership.
            let metrics = Owned(unsafe {
                IORegistryEntryCreateCFProperty(entry, name.0, std::ptr::null(), 0)
            });
            let percent = number(metrics.0, "Device Utilization %")
                .or_else(|| number(metrics.0, "GPU Activity(%)"))
                .filter(|p| (0.0..=100.0).contains(p));
            let memory_used = number(metrics.0, "In use system memory")
                .filter(|n| *n >= 0.0 && *n < u64::MAX as f64)
                .map(|n| n as u64);
            gpus.push(GpuStats {
                name: model(entry).filter(|s| !s.is_empty()).unwrap_or_else(|| {
                    if cfg!(target_arch = "aarch64") {
                        super::super::cpu_model()
                            .map(|s| format!("{s} GPU"))
                            .unwrap_or_else(|| "Apple GPU".into())
                    } else {
                        "Mac GPU".into()
                    }
                }),
                percent,
                memory_used,
                memory_total: None,
                temperature_celsius: None,
            });
            // SAFETY: releases the iterator-owned reference from IOIteratorNext.
            unsafe { IOObjectRelease(entry) };
        }
        // SAFETY: matching returned this iterator on success.
        unsafe { IOObjectRelease(iterator) };
        let status = if gpus.is_empty() {
            GpuStatus::Absent
        } else {
            GpuStatus::Available
        };
        (gpus, status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nvidia_unknown_is_not_idle_or_missing_device() {
        let devices = parse_nvidia_smi(
            "NVIDIA A100, 84, 33000, 40960, 65\nNVIDIA MIG, [N/A], [Not Supported], 40960, N/A\nbad line\n",
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].percent, Some(84.0));
        assert_eq!(devices[0].memory_total, Some(40960 << 20));
        assert_eq!(devices[0].temperature_celsius, Some(65.0));
        assert_eq!(devices[1].percent, None);
        assert_eq!(devices[1].memory_used, None);
        assert_eq!(devices[1].temperature_celsius, None);
        assert_eq!(percentage("NaN"), None);
        assert_eq!(percentage("101"), None);
        assert_eq!(percentage("0"), Some(0.0));
    }
    #[test]
    fn drm_discovery_keeps_unknown_metrics_and_ignores_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let device = dir.path().join("card0/device");
        std::fs::create_dir_all(device.join("hwmon/hwmon0")).unwrap();
        std::fs::create_dir_all(dir.path().join("card0-HDMI-A-1")).unwrap();
        std::fs::write(device.join("vendor"), "0x1002\n").unwrap();
        std::fs::write(device.join("device"), "0x744c\n").unwrap();
        std::fs::write(device.join("gpu_busy_percent"), "unreadable\n").unwrap();
        std::fs::write(device.join("hwmon/hwmon0/temp1_input"), "51000\n").unwrap();
        let devices = drm_gpus(dir.path(), false).unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].name.starts_with("AMD GPU"));
        assert_eq!(devices[0].percent, None);
        assert_eq!(devices[0].memory_total, None);
        assert_eq!(devices[0].temperature_celsius, Some(51.0));
        assert!(drm_gpus(&dir.path().join("missing"), false).is_err());
        std::fs::write(device.join("gpu_busy_percent"), "0\n").unwrap();
        assert_eq!(drm_gpus(dir.path(), false).unwrap()[0].percent, Some(0.0));
    }
    #[test]
    fn cpu_sensors_do_not_mislabel_acpi_or_gpu_as_cpu() {
        let dir = tempfile::tempdir().unwrap();
        for (sensor, name, value) in [
            ("hwmon0", "coretemp", "62000"),
            ("hwmon1", "amdgpu", "90000"),
            ("hwmon2", "acpitz", "95000"),
        ] {
            let path = dir.path().join(sensor);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("name"), name).unwrap();
            std::fs::write(path.join("temp1_input"), value).unwrap();
        }
        assert_eq!(hwmon_temperature(dir.path(), true), Some(62.0));
        assert_eq!(hwmon_temperature(dir.path(), false), Some(95.0));
    }
    #[test]
    fn amd_cpu_temperature_uses_physical_channels_not_tctl_offsets() {
        for driver in ["k10temp", "zenpower"] {
            let dir = tempfile::tempdir().unwrap();
            let monitor = dir.path().join("hwmon0");
            std::fs::create_dir(&monitor).unwrap();
            std::fs::write(monitor.join("name"), driver).unwrap();
            for (channel, label, value) in [
                (1, "Tctl", "95000"),
                (2, "Tdie", "62000"),
                (3, "Tccd1", "68000"),
                (4, "CCD2", "70000"),
                (5, "Unknown", "99000"),
            ] {
                std::fs::write(monitor.join(format!("temp{channel}_label")), label).unwrap();
                std::fs::write(monitor.join(format!("temp{channel}_input")), value).unwrap();
            }
            assert_eq!(hwmon_temperature(dir.path(), true), Some(70.0), "{driver}");
            for channel in 2..=4 {
                std::fs::remove_file(monitor.join(format!("temp{channel}_input"))).unwrap();
            }
            assert_eq!(
                hwmon_temperature(dir.path(), true),
                None,
                "{driver}: control-only readings are not physical temperatures"
            );
        }
    }
    #[test]
    fn hardware_command_drains_pipe_and_rejects_failure() {
        let output = command_output(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -lt 10000 ]; do printf '1234567890'; i=$((i+1)); done",
            ],
        )
        .unwrap();
        assert_eq!(output.len(), 100000);
        let error =
            command_output("/bin/sh", &["-c", "printf 'NVML mismatch' >&2; exit 1"]).unwrap_err();
        assert!(error.to_string().contains("NVML mismatch"));
        let error = command_output("/bin/sh", &["-c", "printf 'stdout NVML mismatch'; exit 18"])
            .unwrap_err();
        assert!(error.to_string().contains("stdout NVML mismatch"));
    }
}
