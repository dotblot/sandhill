//! The app's per-user paths, fixed before the first settings read.

use std::ffi::{CString, OsString};
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use sandhill_proto::{APP_PATH, CACHE_PATH, SANDHILL_DIR};

use crate::editor::media::cache;

static PATHS: LazyLock<Option<Storage>> = LazyLock::new(|| {
    #[cfg(not(test))]
    let home = PathBuf::from(std::env::var_os("HOME")?);
    #[cfg(test)]
    let home = {
        static TEST_HOME: LazyLock<crate::daemon::test_common::TempDir> =
            LazyLock::new(|| crate::daemon::test_common::tempdir().expect("test home folder"));
        TEST_HOME.path().to_path_buf()
    };
    let home = home.canonicalize().unwrap_or(home);
    Some(Storage::open(&home, cfg!(target_os = "macos"), |key| {
        if cfg!(test) {
            None
        } else {
            std::env::var_os(key)
        }
    }))
});

pub(crate) fn paths() -> Option<&'static Storage> {
    PATHS.as_ref()
}

#[derive(Clone, Copy)]
pub(crate) enum Setting {
    Preferences,
    Windows,
    Servers,
}

pub(crate) struct Storage {
    pub app: PathBuf,
    pub media: PathBuf,
    pub old_settings_dirs: [PathBuf; 2],
    pub notes: Vec<String>,
    old_media: Option<PathBuf>,
    files: [(PathBuf, Option<PathBuf>); 3],
}

impl Storage {
    fn open(home: &Path, mac: bool, xdg: impl Fn(&str) -> Option<OsString>) -> Self {
        let legacy_base = |key: &str, default: &str| {
            let path = xdg(key)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(default));
            resolve_base(path)
        };
        let (old_settings, old_data, old_cache) = if mac {
            let support = resolve_base(home.join("Library/Application Support"));
            (
                support.join("Sandhill"),
                support.join("dev.sandhill.ide"),
                resolve_base(home.join("Library/Caches")).join("Sandhill"),
            )
        } else {
            (
                legacy_base("XDG_CONFIG_HOME", ".config").join("sandhill"),
                legacy_base("XDG_DATA_HOME", ".local/share").join("dev.sandhill.ide"),
                legacy_base("XDG_CACHE_HOME", ".cache").join("sandhill"),
            )
        };
        let app = home.join(APP_PATH);
        let media = home.join(CACHE_PATH).join("media");
        let mut notes = Vec::new();
        let files = [
            ("gpui.json", &old_settings),
            ("gpui-windows.json", &old_settings),
            ("servers.json", &old_data),
        ]
        .map(|(name, dir)| move_setting(&dir.join(name), app.join(name), &mut notes));
        let old_media = move_media(&old_cache.join("media"), &media, &mut notes);
        for dir in [&old_settings, &old_data, &old_cache] {
            remove_empty_dir(dir, &mut notes);
        }
        Self {
            app,
            media,
            old_settings_dirs: [old_settings, old_data],
            old_media,
            files,
            notes,
        }
    }

    pub fn file(&self, setting: Setting) -> &Path {
        &self.files[setting as usize].0
    }

    pub fn settings_files(&self) -> impl Iterator<Item = &Path> {
        self.files
            .iter()
            .flat_map(|(active, retained)| [Some(active.as_path()), retained.as_deref()])
            .flatten()
    }

    pub fn media_root(&self) -> &Path {
        self.old_media.as_deref().unwrap_or(&self.media)
    }

    pub fn write(&self, setting: Setting, bytes: &[u8]) -> io::Result<()> {
        let path = self.file(setting);
        if path.starts_with(&self.app) {
            prepare_new_parent(path)?;
        } else {
            safe_regular_file(path).map_err(io::Error::other)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?
            .write_all(bytes)
    }
}

fn resolve_base(path: PathBuf) -> PathBuf {
    // Resolve the platform's base, not the Sandhill entry we may move.
    path.canonicalize().unwrap_or(path)
}

fn move_setting(old: &Path, new: PathBuf, notes: &mut Vec<String>) -> (PathBuf, Option<PathBuf>) {
    if missing(old) {
        return (new, None);
    }
    let result = (|| {
        if !missing(&new) {
            safe_regular_file(&new).map_err(io::Error::other)?;
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        safe_regular_file(old).map_err(io::Error::other)?;
        prepare_new_parent(&new)?;
        rename_new(old, &new)
    })();
    match result {
        Ok(()) => {
            tracing::info!(from = %old.display(), to = %new.display(), "Moved an app settings file");
            (new, None)
        }
        Err(_) if missing(old) => (new, None),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists && safe_regular_file(&new).is_ok() =>
        {
            note(
                notes,
                format!(
                    "Kept {} because the new file already exists.",
                    old.display()
                ),
            );
            (new, Some(old.to_path_buf()))
        }
        Err(error) => {
            note(
                notes,
                format!("Still using {}. Could not move it: {error}.", old.display()),
            );
            (old.to_path_buf(), None)
        }
    }
}

fn move_media(old: &Path, new: &Path, notes: &mut Vec<String>) -> Option<PathBuf> {
    if missing(old) {
        return None;
    }
    match prepare_new_parent(new).and_then(|()| cache::move_legacy(old, new)) {
        Ok(()) => {
            tracing::info!(from = %old.display(), to = %new.display(), "Moved the media cache");
            None
        }
        Err(_) if missing(old) => None,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            note(
                notes,
                format!(
                    "Kept the old media cache at {} because the new cache already exists.",
                    old.display()
                ),
            );
            None
        }
        Err(error) => {
            note(
                notes,
                format!(
                    "Still using the media cache at {}. Could not move it: {error}.",
                    old.display()
                ),
            );
            Some(old.to_path_buf())
        }
    }
}

fn note(notes: &mut Vec<String>, text: String) {
    tracing::warn!("{text}");
    notes.push(text);
}

fn missing(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}

fn remove_empty_dir(path: &Path, notes: &mut Vec<String>) {
    if let Err(error) = fs::remove_dir(path)
        && !matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
        )
    {
        note(
            notes,
            format!(
                "Could not remove the old folder at {}: {error}.",
                path.display()
            ),
        );
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(path)?.is_dir() {
                Ok(())
            } else {
                Err(io::Error::other(format!(
                    "{} is not a regular folder",
                    path.display()
                )))
            }
        }
        result => result,
    }
}

/// Create the two Sandhill-owned parents, never following a symbolic link.
/// The caller has already resolved the home directory.
pub(crate) fn prepare_new_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().expect("Sandhill file has a parent");
    let root = parent.parent().expect("Sandhill folder has a parent");
    debug_assert_eq!(
        root.file_name().and_then(|name| name.to_str()),
        Some(SANDHILL_DIR)
    );
    ensure_directory(root)?;
    ensure_directory(parent)
}

/// One atomic move, but never replace another app process's file or cache.
pub(crate) fn rename_new(old: &Path, new: &Path) -> io::Result<()> {
    let from = CString::new(old.as_os_str().as_encoded_bytes())?;
    let to = CString::new(new.as_os_str().as_encoded_bytes())?;
    // SAFETY: both path arguments are valid, NUL-terminated strings.
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    // SAFETY: both path arguments are valid, NUL-terminated strings.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn safe_regular_file(path: &Path) -> Result<u64, String> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("Symbolic links are protected".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("Not a regular settings file".into());
    }
    Ok(meta.len())
}

#[cfg(test)]
mod footprint_tests {
    use std::os::unix::fs::PermissionsExt as _;

    use crate::daemon::test_common as common;

    use super::*;

    fn fixture(home: &Path, old_settings: &str, old_data: &str, old_media: &str) -> PathBuf {
        for (dir, file, bytes) in [
            (old_settings, "gpui.json", &b"{\"theme\":\"dark\"}"[..]),
            (old_settings, "gpui-windows.json", &b"{\"windows\":[]}"[..]),
            (old_data, "servers.json", &b"[\"lab\"]"[..]),
        ] {
            let dir = home.join(dir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), bytes).unwrap();
        }
        let media = home.join(old_media);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(media.join("session-1-1-1"))
            .unwrap();
        fs::write(media.join("session-1-1-1/.lease"), []).unwrap();
        fs::write(media.join("session-1-1-1/preview-1"), b"preview").unwrap();
        media
    }

    #[test]
    fn moves_mac_and_linux_files_to_the_same_layout() {
        for (mac, settings, data, media) in [
            (
                true,
                "Library/Application Support/Sandhill",
                "Library/Application Support/dev.sandhill.ide",
                "Library/Caches/Sandhill/media",
            ),
            (
                false,
                ".config/sandhill",
                ".local/share/dev.sandhill.ide",
                ".cache/sandhill/media",
            ),
        ] {
            let home = common::tempdir().unwrap();
            let home = home.path().canonicalize().unwrap();
            let old_media = fixture(&home, settings, data, media);
            let layout = Storage::open(&home, mac, |_| None);
            assert_eq!(layout.app, home.join(APP_PATH));
            assert_eq!(layout.media_root(), home.join(CACHE_PATH).join("media"));
            for (setting, file, bytes) in [
                (
                    Setting::Preferences,
                    "gpui.json",
                    &b"{\"theme\":\"dark\"}"[..],
                ),
                (
                    Setting::Windows,
                    "gpui-windows.json",
                    &b"{\"windows\":[]}"[..],
                ),
                (Setting::Servers, "servers.json", &b"[\"lab\"]"[..]),
            ] {
                assert_eq!(layout.file(setting), layout.app.join(file));
                assert_eq!(fs::read(layout.file(setting)).unwrap(), bytes);
            }
            let preview = layout.media_root().join("session-1-1-1/preview-1");
            assert_eq!(fs::read(preview).unwrap(), b"preview");
            assert!(!home.join(settings).exists());
            assert!(!home.join(data).exists());
            assert!(!old_media.parent().unwrap().exists());
            assert_eq!(
                fs::metadata(home.join(SANDHILL_DIR))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert!(layout.notes.is_empty(), "{:?}", layout.notes);
            assert!(Storage::open(&home, mac, |_| None).notes.is_empty());
        }
    }

    #[test]
    fn keeps_new_settings_and_preserves_old_neighbors() {
        let home = common::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let old_media = fixture(
            &home,
            "custom-config/sandhill",
            "custom-data/dev.sandhill.ide",
            "custom-cache/sandhill/media",
        );
        let old_settings = home.join("custom-config/sandhill");
        fs::write(old_settings.join("other.txt"), b"keep").unwrap();
        let new_windows = home.join(APP_PATH).join("gpui-windows.json");
        prepare_new_parent(&new_windows).unwrap();
        fs::write(&new_windows, b"new windows").unwrap();

        let layout = Storage::open(&home, false, |key| {
            let dir = match key {
                "XDG_CONFIG_HOME" => "custom-config",
                "XDG_DATA_HOME" => "custom-data",
                "XDG_CACHE_HOME" => "custom-cache",
                _ => unreachable!(),
            };
            Some(home.join(dir).into_os_string())
        });

        assert_eq!(layout.file(Setting::Windows), new_windows);
        assert_eq!(fs::read(&new_windows).unwrap(), b"new windows");
        let old_windows = old_settings.join("gpui-windows.json");
        assert_eq!(fs::read(&old_windows).unwrap(), b"{\"windows\":[]}");
        assert!(layout.settings_files().any(|path| path == old_windows));
        assert!(
            layout
                .notes
                .iter()
                .any(|line| line.contains("already exists"))
        );
        assert_eq!(fs::read(old_settings.join("other.txt")).unwrap(), b"keep");
        assert!(!old_settings.join("gpui.json").exists());
        assert!(!old_media.exists());
    }

    #[test]
    fn blocked_destination_uses_old_files_until_the_next_start() {
        let home = common::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let old_media = fixture(
            &home,
            ".config/sandhill",
            ".local/share/dev.sandhill.ide",
            ".cache/sandhill/media",
        );
        let blocked = home.join(SANDHILL_DIR);
        fs::write(&blocked, b"do not replace").unwrap();

        let first = Storage::open(&home, false, |_| None);
        assert_eq!(
            first.file(Setting::Preferences),
            home.join(".config/sandhill/gpui.json")
        );
        assert_eq!(
            first.file(Setting::Servers),
            home.join(".local/share/dev.sandhill.ide/servers.json")
        );
        assert_eq!(first.media_root(), old_media);
        assert_eq!(fs::read(&blocked).unwrap(), b"do not replace");
        assert_eq!(
            first
                .notes
                .iter()
                .filter(|line| line.starts_with("Still using"))
                .count(),
            4
        );
        first.write(Setting::Servers, b"[\"new-lab\"]").unwrap();

        fs::remove_file(&blocked).unwrap();
        let second = Storage::open(&home, false, |_| None);
        assert_eq!(
            second.file(Setting::Preferences),
            home.join(APP_PATH).join("gpui.json")
        );
        assert_eq!(
            fs::read(second.file(Setting::Servers)).unwrap(),
            b"[\"new-lab\"]"
        );
        assert_eq!(second.media_root(), home.join(CACHE_PATH).join("media"));
        assert!(second.notes.is_empty());
    }

    #[test]
    fn keeps_live_preview_paths_until_the_next_start() {
        let home = common::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let old_media = fixture(
            &home,
            ".config/sandhill",
            ".local/share/dev.sandhill.ide",
            ".cache/sandhill/media",
        );
        let lease = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(old_media.join("session-1-1-1/.lease"))
            .unwrap();
        lease.lock().unwrap();

        let first = Storage::open(&home, false, |_| None);
        assert_eq!(first.media_root(), old_media);
        assert!(
            first
                .notes
                .iter()
                .any(|line| line.contains("previews open"))
        );
        assert!(old_media.join("session-1-1-1/preview-1").is_file());

        drop(lease);
        let second = Storage::open(&home, false, |_| None);
        assert_eq!(second.media_root(), home.join(CACHE_PATH).join("media"));
        assert!(!old_media.exists());
        assert!(second.notes.is_empty());
    }
}
