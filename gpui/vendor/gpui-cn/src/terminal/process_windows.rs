//! The processes behind a local terminal on Windows.
//!
//! Windows has no sessions or process groups for a console. The program
//! a terminal starts goes into a job object that kills every process in it
//! when the job closes, so closing the terminal ends the program and every
//! process it started, as herdr ends a pane's process tree. The foreground
//! process is the program's newest descendant, read from a Toolhelp
//! snapshot of the process table, as herdr reads it.

use std::collections::{HashMap, VecDeque};
use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
#[cfg(test)]
use windows_sys::Win32::System::JobObjects::{
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
    QueryInformationJobObject,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// A handle that closes when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned here.
        unsafe { CloseHandle(self.0) };
    }
}

/// A job that kills every process in it when it closes.
pub(crate) struct Job {
    handle: Owned,
}

// SAFETY: a job handle may be used from any thread.
unsafe impl Send for Job {}
// SAFETY: every call on the handle is thread-safe in Win32.
unsafe impl Sync for Job {}

impl Job {
    /// An empty job with kill-on-close set.
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: null attributes and name create an anonymous job.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self {
            handle: Owned(handle),
        };
        // SAFETY: the structure is plain data; zero is a valid start.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(std::mem::size_of_val(&limits)).map_err(io::Error::other)?;
        // SAFETY: the buffer is the structure the class names, of its size.
        if unsafe {
            SetInformationJobObject(
                job.handle.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Puts process `pid` in the job. Every process it starts from then on
    /// is in the job too.
    pub(crate) fn assign(&self, pid: u32) -> io::Result<()> {
        // SAFETY: OpenProcess returns a new handle or null.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let process = Owned(process);
        // SAFETY: both handles are open.
        if unsafe { AssignProcessToJobObject(self.handle.0, process.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Ends every process in the job.
    pub(crate) fn terminate(&self) {
        // SAFETY: the handle is open.
        unsafe { TerminateJobObject(self.handle.0, 1) };
    }

    /// How many processes in the job are running.
    #[cfg(test)]
    pub(crate) fn active(&self) -> u32 {
        // SAFETY: the structure is plain data; zero is a valid start.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        let Ok(size) = u32::try_from(std::mem::size_of_val(&info)) else {
            return 0;
        };
        // SAFETY: the buffer is the structure the class names, of its size.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle.0,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut info).cast(),
                size,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { 0 } else { info.ActiveProcesses }
    }
}

/// A process from the process table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    pub(crate) name: String,
}

/// Every running process, from a Toolhelp snapshot.
fn snapshot() -> Vec<Entry> {
    // SAFETY: a snapshot of all processes needs no process id.
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Vec::new();
    }
    let handle = Owned(handle);
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut out = Vec::new();
    // SAFETY: the entry's size is set, as the API asks.
    let mut ok = unsafe { Process32FirstW(handle.0, &raw mut entry) } != 0;
    while ok {
        let len = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        out.push(Entry {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..len]),
        });
        // SAFETY: as above.
        ok = unsafe { Process32NextW(handle.0, &raw mut entry) } != 0;
    }
    out
}

/// The foreground process of a terminal whose program is `root`: the
/// program's newest descendant, which is what reads the keys in a shell
/// that runs one command at a time, or the program itself.
pub(crate) fn foreground(root: u32) -> Option<Entry> {
    newest_descendant(root, &snapshot())
}

/// The last process a breadth-first walk from `root` reaches: the deepest
/// descendant, and among those the one the table lists last, which is the
/// newest. `root` itself when it has none.
pub(crate) fn newest_descendant(root: u32, entries: &[Entry]) -> Option<Entry> {
    let mut children: HashMap<u32, Vec<&Entry>> = HashMap::new();
    let mut by_pid = HashMap::new();
    for entry in entries {
        by_pid.insert(entry.pid, entry);
        // A process can name itself as its parent in the snapshot.
        if entry.parent != entry.pid {
            children.entry(entry.parent).or_default().push(entry);
        }
    }
    let mut last = *by_pid.get(&root)?;
    let mut queue = VecDeque::from([root]);
    let mut seen = std::collections::HashSet::from([root]);
    while let Some(pid) = queue.pop_front() {
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                last = child;
                queue.push_back(child.pid);
            }
        }
    }
    Some(last.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, parent: u32, name: &str) -> Entry {
        Entry {
            pid,
            parent,
            name: name.to_owned(),
        }
    }

    #[test]
    fn the_foreground_is_the_newest_deepest_descendant() {
        let table = [
            entry(1, 0, "explorer.exe"),
            entry(10, 1, "pwsh.exe"),
            entry(11, 10, "cargo.exe"),
            entry(12, 11, "rustc.exe"),
            entry(13, 10, "git.exe"),
            entry(20, 1, "other.exe"),
        ];
        assert_eq!(newest_descendant(10, &table).unwrap().name, "rustc.exe");
        assert_eq!(newest_descendant(13, &table).unwrap().name, "git.exe");
        assert_eq!(newest_descendant(99, &table), None);
        // A cycle in a stale snapshot ends.
        let cycle = [entry(5, 6, "a.exe"), entry(6, 5, "b.exe")];
        assert_eq!(newest_descendant(5, &cycle).unwrap().name, "b.exe");
    }

    #[cfg(windows)]
    #[test]
    fn closing_the_job_ends_the_process_and_what_it_started() {
        use std::time::{Duration, Instant};
        // The second ping starts after the assignment, so it is in the job.
        let mut child = std::process::Command::new("cmd.exe")
            .args([
                "/d",
                "/c",
                "ping -n 2 127.0.0.1 >nul & ping -n 1000 127.0.0.1 >nul",
            ])
            .spawn()
            .unwrap();
        let job = Job::new().unwrap();
        job.assign(child.id()).unwrap();
        let wait = |done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !done() {
                assert!(Instant::now() < deadline, "timed out");
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&|| job.active() >= 2);
        let foreground = foreground(child.id()).unwrap();
        assert!(
            foreground.name.eq_ignore_ascii_case("ping.exe"),
            "{foreground:?}"
        );
        job.terminate();
        wait(&|| job.active() == 0);
        assert!(child.wait().is_ok());
    }
}
