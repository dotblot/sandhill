//! Shared by the integration tests that start kernels.

use std::path::Path;

/// A test's temporary folder. Kernels outlive their daemon by design, so
/// before the folder is deleted, every process started from it (kernels
/// name their connection file there; test daemons their state folder) is
/// killed with what it started, also when the test failed partway.
pub struct TempDir {
    dir: tempfile::TempDir,
}

pub fn tempdir() -> std::io::Result<TempDir> {
    tempfile::tempdir().map(|dir| TempDir { dir })
}

impl TempDir {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let mut roots = vec![format!("{}/", self.path().display())];
        if let Ok(real) = self.path().canonicalize() {
            roots.push(format!("{}/", real.display()));
        }
        let Ok(listed) = std::process::Command::new("ps")
            .args(["-axo", "pid=,pgid=,command="])
            .output()
        else {
            return;
        };
        let own = std::process::id() as libc::pid_t;
        for line in String::from_utf8_lossy(&listed.stdout).lines() {
            let mut fields = line.split_whitespace();
            let (Some(Ok(pid)), Some(Ok(group))) = (
                fields.next().map(str::parse::<libc::pid_t>),
                fields.next().map(str::parse::<libc::pid_t>),
            ) else {
                continue;
            };
            let command = fields.collect::<Vec<_>>().join(" ");
            if pid == own || !roots.iter().any(|root| command.contains(root.as_str())) {
                continue;
            }
            // A kernel leads its own process group, which holds what it started.
            let target = if pid == group { -pid } else { pid };
            // SAFETY: plain signal to a process this test started.
            unsafe {
                libc::kill(target, libc::SIGKILL);
            }
        }
    }
}
