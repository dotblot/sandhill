use std::path::{Path, PathBuf};

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
}

/// Connection files contain the kernel's signing key, so the state directory
/// is private to the user.
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}
