use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the daemon keeps its state. Everything lives under one directory so
/// tests can point a daemon at a temp dir.
#[derive(Debug, Clone)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        StateDir { root: root.into() }
    }

    /// `$SANDHILL_STATE_DIR`, else `$XDG_STATE_HOME/sandhill`, else
    /// `~/.local/state/sandhill`.
    pub fn default_location() -> Self {
        if let Some(dir) = std::env::var_os("SANDHILL_STATE_DIR") {
            return Self::new(dir);
        }
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::new(base.join("sandhill"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create(&self) -> std::io::Result<()> {
        for dir in [
            self.root.clone(),
            self.kernels_dir(),
            self.blobs_dir(),
            self.terminals_dir(),
            self.install_logs_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
            restrict_permissions(&dir)?;
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("journal.sqlite")
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    /// Connection files and kernel logs.
    pub fn kernels_dir(&self) -> PathBuf {
        self.root.join("kernels")
    }

    /// One folder per terminal (see `terminal`).
    pub fn terminals_dir(&self) -> PathBuf {
        self.root.join("terminals")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.root.join("sandhilld.sock")
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("sandhilld.log")
    }

    pub(crate) fn install_logs_dir(&self) -> PathBuf {
        self.root.join("log")
    }
}

/// Connection files contain the kernel's signing key, so the state directory
/// is private to the user.
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// The `PATH` kernels run with, and that kernels' programs (R, Python) are
/// looked for on: the daemon's own, then on a Mac the usual places R and
/// Python are installed that it lacks. An app opened from the Finder has
/// only the system folders, and reading the login shell's `PATH` can fail
/// or take too long, which left R out: Ark, which looks for R there, aborted,
/// and IRkernel's `R` wasn't found.
pub fn program_path() -> Option<OsString> {
    static PATH: OnceLock<Option<OsString>> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::var_os("PATH");
        if cfg!(target_os = "macos") {
            with_dirs(path, &mac_dirs())
        } else {
            path
        }
    })
    .clone()
}

/// What a Mac login shell adds (`/etc/paths` and `/etc/paths.d`, which
/// path_helper reads), Homebrew's folders and CRAN R's.
fn mac_dirs() -> Vec<PathBuf> {
    let mut files = vec![PathBuf::from("/etc/paths")];
    if let Ok(entries) = std::fs::read_dir("/etc/paths.d") {
        let mut more: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        more.sort();
        files.extend(more);
    }
    let mut dirs: Vec<PathBuf> = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .collect();
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/Library/Frameworks/R.framework/Resources/bin",
        ]
        .map(PathBuf::from),
    );
    dirs
}

/// `path` with the folders of `extra` that exist and it lacks, after its own.
fn with_dirs(path: Option<OsString>, extra: &[PathBuf]) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = path
        .as_ref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for dir in extra {
        if !dirs.contains(dir) && dir.is_dir() {
            dirs.push(dir.clone());
        }
    }
    if dirs.is_empty() {
        return path;
    }
    std::env::join_paths(dirs).ok().or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usual_folders_come_after_the_path_once() {
        let dir = tempfile::tempdir().unwrap();
        let extra = dir.path().to_path_buf();
        let missing = dir.path().join("missing");
        let path = Some(OsString::from("/usr/bin:/bin"));
        let joined = with_dirs(
            path.clone(),
            &[PathBuf::from("/bin"), missing, extra.clone()],
        )
        .unwrap();
        let dirs: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        assert_eq!(dirs, [PathBuf::from("/usr/bin"), "/bin".into(), extra]);
        assert_eq!(with_dirs(path.clone(), &[]), path);
    }
}
