use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sandhill_proto::{OLD_DAEMON_LAYOUT, SANDHILL_DIR, STATE_PATH};

mod migration;

pub(crate) fn legacy_path(current: &str) -> &str {
    OLD_DAEMON_LAYOUT
        .iter()
        .find_map(|(old, new)| (*new == current).then_some(*old))
        .expect("known Sandhill directory")
}

/// During a postponed move, continue looking in the old place if the
/// requested program or account directory hasn't appeared in the new one.
pub(crate) fn installed_path(home: &Path, dir: &str, name: &str) -> PathBuf {
    let current = home.join(dir).join(name);
    if !current.exists() {
        let old = home.join(legacy_path(dir)).join(name);
        if old.exists() {
            return old;
        }
    }
    current
}

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

    /// `$SANDHILL_STATE_DIR`, otherwise `~/.sandhill/state`.
    pub fn default_location() -> Self {
        Self::location(
            std::env::var_os("SANDHILL_STATE_DIR"),
            std::env::var_os("HOME"),
        )
    }

    fn location(override_dir: Option<OsString>, home: Option<OsString>) -> Self {
        Self::new(override_dir.map(PathBuf::from).unwrap_or_else(|| {
            home.map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(STATE_PATH)
        }))
    }

    #[doc(hidden)]
    pub fn legacy_location() -> Option<Self> {
        if std::env::var_os("SANDHILL_STATE_DIR").is_some() {
            return None;
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(Self::new(home.join(legacy_path(STATE_PATH))))
    }

    /// Select the old location only while it still holds the state.
    #[doc(hidden)]
    pub fn existing_location(self) -> Self {
        if (!self.root.symlink_metadata().is_ok_and(|m| m.is_dir())
            || !self
                .root
                .parent()
                .is_some_and(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir())))
            && let Some(old) = Self::legacy_location()
            && old.root.symlink_metadata().is_ok()
        {
            return old;
        }
        self
    }

    #[doc(hidden)]
    pub fn prepare_home(home: &Path) -> std::io::Result<PathBuf> {
        let folder = home.join(SANDHILL_DIR);
        let mut builder = std::fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
        match builder.create(&folder) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
        if !folder.symlink_metadata()?.is_dir() {
            return Err(std::io::Error::other(
                "Sandhill's folder is not a directory",
            ));
        }
        restrict_permissions(&folder)?;
        Ok(folder)
    }

    /// Called by the daemon start command while it owns the startup locks.
    #[doc(hidden)]
    pub fn migrate_home(home: &Path) -> Vec<String> {
        migration::migrate(home, migration::rename_exclusive)
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
            if !dir.symlink_metadata()?.is_dir() {
                return Err(std::io::Error::other(format!(
                    "{} is not a regular folder",
                    dir.display()
                )));
            }
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
    fn state_location_uses_only_home_and_explicit_override() {
        let home = Some(OsString::from("/home/sandhill-test"));
        let state = StateDir::location(None, home.clone());
        assert_eq!(
            state.root(),
            Path::new("/home/sandhill-test").join(STATE_PATH)
        );
        assert_eq!(state.socket_path(), state.root().join("sandhilld.sock"));
        assert_eq!(state.log_path(), state.root().join("sandhilld.log"));
        assert_eq!(state.db_path(), state.root().join("journal.sqlite"));
        assert_eq!(state.kernels_dir(), state.root().join("kernels"));
        assert_eq!(state.blobs_dir(), state.root().join("blobs"));
        assert_eq!(state.terminals_dir(), state.root().join("terminals"));
        assert_eq!(state.install_logs_dir(), state.root().join("log"));
        assert_eq!(
            StateDir::location(Some(OsString::from("/elsewhere")), home).root(),
            Path::new("/elsewhere")
        );
    }

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
