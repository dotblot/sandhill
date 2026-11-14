//! Private preview files, with cross-process leases and bounded crash recovery.
//!
//! Only inactive session directories under this dedicated cache are removable.
//! A live process holds the session lease through native PDFKit's lazy reads.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

const MAX_ORPHAN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ORPHAN_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_ORPHAN_FILES: usize = 128;
static NEXT: AtomicU64 = AtomicU64::new(0);
static SESSION: LazyLock<Mutex<Option<Arc<Session>>>> = LazyLock::new(|| Mutex::new(None));

// A fork child can inherit the open file description before exec closes it.
// Closing our descriptor alone would leave its flock held until the child exits.
struct FileLock(File);

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

struct Session {
    path: PathBuf,
    _lease: FileLock,
}

/// The lease must outlive every file; native previews retain this value.
pub(super) struct MediaFile(pub(super) PathBuf, Arc<Session>);

impl MediaFile {
    pub(super) fn create() -> Result<(Self, File)> {
        let mut current = SESSION
            .lock()
            .map_err(|_| anyhow::anyhow!("preview cache lock poisoned"))?;
        let session = match current.as_ref() {
            Some(session) => session.clone(),
            None => {
                let root = root()?;
                // Cleanup and creating/locking the session are one operation
                // across processes; an unlocked new lease is not an orphan.
                let _root_lock = lock_root(&root)?;
                cleanup_locked(&root, false)?;
                let session = Arc::new(Session::open(&root)?);
                *current = Some(session.clone());
                session
            }
        };
        let path = session
            .path
            .join(format!("preview-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("cannot create preview in {}", session.path.display()))?;
        Ok((Self(path, session), file))
    }
}

impl Drop for MediaFile {
    fn drop(&mut self) {
        if self.0.parent() == Some(self.1.path.as_path()) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

impl Session {
    fn open(root: &Path) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_nanos();
        let path = root.join(format!(
            "session-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join(".lease"))
            .with_context(|| format!("cannot create preview lease in {}", path.display()))?;
        lease
            .try_lock()
            .context("cannot lock preview cache session")?;
        Ok(Self {
            path,
            _lease: FileLock(lease),
        })
    }
}

fn private_dir(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => (),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => (),
        Err(err) => return Err(err.into()),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "unsafe preview cache directory: {}",
        path.display()
    );
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "preview cache directory is not private: {}",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
fn root() -> Result<PathBuf> {
    static TEST_ROOT: LazyLock<tempfile::TempDir> =
        LazyLock::new(|| tempfile::tempdir().expect("private preview test cache"));
    Ok(TEST_ROOT.path().to_path_buf())
}

#[cfg(not(test))]
fn root() -> Result<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    #[cfg(target_os = "macos")]
    let base = home.join("Library/Caches");
    #[cfg(not(target_os = "macos"))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".cache"));
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    #[cfg(target_os = "macos")]
    let app = base.join("Sandhill");
    #[cfg(not(target_os = "macos"))]
    let app = base.join("sandhill");
    private_dir(&app)?;
    let root = app.join("media");
    private_dir(&root)?;
    Ok(root)
}

struct SessionFiles {
    path: PathBuf,
    // Holding the lock closes the check/delete race with other cleaners.
    lease: Option<FileLock>,
    active: bool,
    files: Vec<(PathBuf, u64, SystemTime)>,
    unknown: bool,
}

fn session_name(name: &str) -> bool {
    name.strip_prefix("session-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.split('-').count() == 3
            && suffix
                .split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn preview_name(name: &str) -> bool {
    name.strip_prefix("preview-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn scan(root: &Path) -> Result<Vec<SessionFiles>> {
    let metadata = std::fs::symlink_metadata(root)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "unsafe preview cache root"
    );
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !session_name(&entry.file_name().to_string_lossy()) || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let lock_path = path.join(".lease");
        let mut unknown = false;
        let (lease, active) = match std::fs::symlink_metadata(&lock_path) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                let file = OpenOptions::new().read(true).write(true).open(&lock_path)?;
                match file.try_lock() {
                    Ok(()) => (Some(FileLock(file)), false),
                    Err(std::fs::TryLockError::WouldBlock) => (None, true),
                    Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                }
            }
            // A crash between mkdir and lease creation has no outputs to clean.
            _ => {
                unknown = true;
                (None, false)
            }
        };
        let mut files = Vec::new();
        for file in std::fs::read_dir(&path)? {
            let file = file?;
            let name = file.file_name();
            if name == ".lease" {
                continue;
            }
            let meta = std::fs::symlink_metadata(file.path())?;
            if preview_name(&name.to_string_lossy())
                && file.file_type()?.is_file()
                && !meta.file_type().is_symlink()
            {
                files.push((
                    file.path(),
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ));
            } else {
                unknown = true;
            }
        }
        sessions.push(SessionFiles {
            path,
            lease,
            active,
            files,
            unknown,
        });
    }
    Ok(sessions)
}

fn entry(path: &Path, bytes: u64, removable: bool) -> Value {
    let mut value = json!({"id": format!("local-preview:{}", path.display()), "category": "media_previews", "path": path, "bytes": bytes, "removable": removable});
    if !removable {
        value["reason"] = json!("An active preview session owns this file");
    }
    value
}

pub(crate) fn inventory() -> Result<Value> {
    inventory_at(&root()?)
}

fn inventory_at(root: &Path) -> Result<Value> {
    let mut entries = Vec::new();
    let mut active_bytes = 0u64;
    let mut reclaimable_bytes = 0u64;
    let mut protected_bytes = 0u64;
    for session in scan(root)? {
        for (path, bytes, _) in &session.files {
            let active = session.active;
            let removable = session.lease.is_some() && !session.unknown;
            if active {
                active_bytes = active_bytes.saturating_add(*bytes);
            } else if removable {
                reclaimable_bytes = reclaimable_bytes.saturating_add(*bytes);
            } else {
                protected_bytes = protected_bytes.saturating_add(*bytes);
            }
            let mut value = entry(path, *bytes, removable);
            value["active"] = json!(active);
            if session.unknown {
                value["reason"] = json!("Unrecognized cache contents are preserved");
            }
            entries.push(value);
        }
    }
    Ok(json!({
        "entries": entries,
        "active_bytes": active_bytes,
        "reclaimable_bytes": reclaimable_bytes,
        "protected_bytes": protected_bytes,
        "bytes": active_bytes.saturating_add(reclaimable_bytes).saturating_add(protected_bytes)
    }))
}

/// Serializes new session creation with cleanup in every running app process.
/// The file is never removed; changing its inode would split the lock.
fn lock_root(root: &Path) -> Result<FileLock> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(".lock"))?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1,
        "unsafe preview cache lock"
    );
    file.lock()?;
    Ok(FileLock(file))
}

/// Explicit cleanup removes all inactive previews, never active native outputs.
pub(crate) fn clean() -> Result<Value> {
    cleanup_at(&root()?, true)
}

fn cleanup_at(root: &Path, all: bool) -> Result<Value> {
    let _root_lock = lock_root(root)?;
    cleanup_locked(root, all)
}

fn cleanup_locked(root: &Path, all: bool) -> Result<Value> {
    let sessions = scan(root)?;
    let mut candidates = Vec::new();
    let mut retained = 0u64;
    for (index, session) in sessions.iter().enumerate() {
        if session.lease.is_some() && !session.unknown {
            for (path, bytes, modified) in &session.files {
                retained = retained.saturating_add(*bytes);
                candidates.push((index, path, *bytes, *modified));
            }
        }
    }
    candidates.sort_by_key(|(_, _, _, modified)| *modified);
    let now = SystemTime::now();
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut remaining_files = candidates.len();
    for (_, path, bytes, modified) in candidates {
        let expired = now.duration_since(modified).unwrap_or_default() >= MAX_ORPHAN_AGE;
        if !all && !expired && retained <= MAX_ORPHAN_BYTES && remaining_files <= MAX_ORPHAN_FILES {
            continue;
        }
        let mut result = entry(path, bytes, true);
        match std::fs::remove_file(path) {
            Ok(()) => {
                retained = retained.saturating_sub(bytes);
                remaining_files = remaining_files.saturating_sub(1);
                result["removed"] = json!(true);
            }
            Err(err) => {
                result["removed"] = json!(false);
                result["error"] = json!(err.to_string());
                errors.push(format!("{}: {err}", path.display()));
            }
        }
        entries.push(result);
    }
    // Remove only known, empty session containers after their files are gone.
    for session in &sessions {
        if session.lease.is_none() || session.unknown {
            continue;
        }
        let empty = std::fs::read_dir(&session.path)?
            .all(|entry| entry.is_ok_and(|e| e.file_name() == ".lease"));
        if empty {
            // Never unlock an existing pathname and then delete a new owner's
            // lease: the directory is uniquely named and never reused.
            if let Err(err) = std::fs::remove_file(session.path.join(".lease"))
                .and_then(|_| std::fs::remove_dir(&session.path))
            {
                errors.push(format!("{}: {err}", session.path.display()));
            }
        }
    }
    Ok(json!({"success": errors.is_empty(), "entries": entries, "errors": errors}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::fd::{AsRawFd as _, FromRawFd as _};

    struct WaitingChild {
        pid: libc::pid_t,
        release: File,
    }

    impl WaitingChild {
        fn inheriting_locks() -> Self {
            let mut pipe = [0; 2];
            assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
            // Own both descriptors before fork so every parent failure closes them.
            let read = unsafe { File::from_raw_fd(pipe[0]) };
            let release = unsafe { File::from_raw_fd(pipe[1]) };
            let pid = unsafe { libc::fork() };
            if pid == 0 {
                // The suite is multithreaded: only async-signal-safe libc calls
                // are permitted in the child, with no Rust drops or allocation.
                let mut byte = 0u8;
                loop {
                    if unsafe { libc::read(read.as_raw_fd(), (&mut byte as *mut u8).cast(), 1) }
                        == 1
                    {
                        unsafe { libc::_exit(0) };
                    }
                }
            }
            assert!(pid > 0, "fork failed: {}", io::Error::last_os_error());
            drop(read);
            Self { pid, release }
        }

        fn assert_running(&self) {
            assert_eq!(
                unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), libc::WNOHANG) },
                0,
                "child must still retain its inherited descriptors"
            );
        }
    }

    impl Drop for WaitingChild {
        fn drop(&mut self) {
            // Release and reap even if an assertion above unwinds.
            let _ = self.release.write_all(&[1]);
            loop {
                let result = unsafe { libc::waitpid(self.pid, std::ptr::null_mut(), 0) };
                if result == self.pid
                    || (result < 0
                        && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted)
                {
                    break;
                }
            }
        }
    }

    fn output(session: &Path, name: u64, bytes: &[u8]) -> PathBuf {
        let path = session.join(format!("preview-{name}"));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn orphan(root: &Path) -> PathBuf {
        // An unlocked lease models a crashed process without a concurrent
        // test subprocess inheriting a still-locked file descriptor.
        let path = root.join("session-0-0-1");
        std::fs::create_dir(&path).unwrap();
        File::create(path.join(".lease")).unwrap();
        path
    }

    #[test]
    fn cleanup_excludes_live_previews_and_reaps_crash_orphans() {
        let root = tempfile::tempdir().unwrap();
        let live = Session::open(root.path()).unwrap();
        let active = output(&live.path, 1, b"active");
        let orphan = orphan(root.path());
        let stale = output(&orphan, 2, b"orphan");
        let report = inventory_at(root.path()).unwrap();
        assert!(
            report["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["path"] == active.to_str().unwrap() && v["removable"] == false)
        );
        assert_eq!(report["active_bytes"], 6);
        assert_eq!(report["reclaimable_bytes"], 6);
        assert_eq!(report["protected_bytes"], 0);
        assert!(
            cleanup_at(root.path(), true).unwrap()["success"]
                .as_bool()
                .unwrap()
        );
        assert!(active.exists());
        assert!(!stale.exists());
    }

    #[test]
    fn fork_inherited_inventory_leases_do_not_prevent_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let live = Session::open(root.path()).unwrap();
        let active = output(&live.path, 1, b"active");
        let orphan = orphan(root.path());
        let stale = output(&orphan, 2, b"orphan");
        let inventory = scan(root.path()).unwrap();
        let probe = File::options()
            .read(true)
            .write(true)
            .open(orphan.join(".lease"))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let child = WaitingChild::inheriting_locks();
        drop(inventory);

        child.assert_running();
        let report = cleanup_at(root.path(), true).unwrap();
        assert_eq!(report["success"], true, "{report}");
        assert!(
            active.exists(),
            "the parent's live lease must remain protected"
        );
        assert!(
            !stale.exists(),
            "an inherited inventory lease must not hide an orphan"
        );
        assert_eq!(report["entries"].as_array().unwrap().len(), 1);
        assert_eq!(report["entries"][0]["removed"], true);
        probe
            .try_lock()
            .expect("the dropped inventory lease must be unlocked");
        probe.unlock().unwrap();
        child.assert_running();
    }

    #[test]
    fn fork_inherited_session_lease_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let session = Session::open(root.path()).unwrap();
        let stale = output(&session.path, 1, b"orphan");
        assert_eq!(inventory_at(root.path()).unwrap()["active_bytes"], 6);
        let child = WaitingChild::inheriting_locks();
        drop(session);

        child.assert_running();
        let report = cleanup_at(root.path(), true).unwrap();
        assert_eq!(report["success"], true, "{report}");
        assert!(
            !stale.exists(),
            "the child's inherited session lease must not keep it active"
        );
        assert_eq!(report["entries"][0]["removed"], true);
        child.assert_running();
    }

    #[test]
    fn fork_inherited_root_lock_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let lock = lock_root(root.path()).unwrap();
        let probe = File::options()
            .read(true)
            .write(true)
            .open(root.path().join(".lock"))
            .unwrap();
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        let child = WaitingChild::inheriting_locks();
        drop(lock);

        child.assert_running();
        probe
            .try_lock()
            .expect("the parent's root lock scope has ended");
        probe.unlock().unwrap();
        child.assert_running();
    }

    #[test]
    fn bounded_cleanup_removes_oldest_inactive_outputs() {
        let root = tempfile::tempdir().unwrap();
        let old = orphan(root.path());
        let path = output(&old, 1, b"");
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_ORPHAN_BYTES + 1)
            .unwrap();
        let report = cleanup_at(root.path(), false).unwrap();
        assert!(report["success"] == true && !path.exists(), "{report}");
    }

    #[test]
    fn automatic_cleanup_caps_small_orphans_and_expires_old_files() {
        let root = tempfile::tempdir().unwrap();
        let orphan = orphan(root.path());
        for index in 0..=MAX_ORPHAN_FILES {
            output(&orphan, index as u64, b"x");
        }
        let expired = output(&orphan, 1000, b"old");
        let old_time = SystemTime::now() - MAX_ORPHAN_AGE - Duration::from_secs(1);
        File::options()
            .write(true)
            .open(&expired)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old_time))
            .unwrap();
        cleanup_at(root.path(), false).unwrap();
        assert!(!expired.exists());
        assert_eq!(
            inventory_at(root.path()).unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            MAX_ORPHAN_FILES
        );
    }

    #[test]
    fn cleanup_does_not_follow_links_or_remove_unrecognized_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let protected = outside.path().join("protected");
        std::fs::write(&protected, b"user data").unwrap();
        let session = Session::open(root.path()).unwrap();
        std::os::unix::fs::symlink(&protected, session.path.join("preview-1")).unwrap();
        std::fs::write(session.path.join("unexpected"), b"keep").unwrap();
        let owned = output(&session.path, 2, b"conservative preservation");
        drop(session);
        cleanup_at(root.path(), true).unwrap();
        assert!(protected.exists());
        assert!(owned.exists());
        assert!(private_dir(&outside.path().join("missing")).is_ok());
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(outside.path(), &linked).unwrap();
        assert!(private_dir(&linked).is_err());
    }
}
