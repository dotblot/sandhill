//! The first start may move Sandhill's old directories, but never while
//! a process still has its files open. Until the final journal transaction
//! commits, every move and metadata edit can be rolled back.

use std::ffi::CString;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use sandhill_proto::{BIN_PATH, DAEMON_PATH, INSTALL_PATH, OLD_DAEMON_LAYOUT, STATE_PATH};
use serde_json::Value;

use super::{StateDir, legacy_path};
use crate::journal::Journal;

pub(super) fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn migrate(home: &Path, rename: impl Fn(&Path, &Path) -> io::Result<()>) -> Vec<String> {
    let mut notes = Vec::new();
    if let Err(error) = migrate_inner(home, &rename, &mut notes) {
        notes.push(format!("Sandhill could not move its old files: {error:#}"));
    }
    notes
}

fn migrate_inner(
    home: &Path,
    rename: &impl Fn(&Path, &Path) -> io::Result<()>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let mut recovered = Vec::new();
    recover_copies(home, &mut recovered)?;
    let old = StateDir::new(home.join(legacy_path(STATE_PATH)));
    let new = StateDir::new(home.join(STATE_PATH));
    let current_marker = new.root().join("footprint-v6");
    // No marker is written until all saved paths are updated. An earlier
    // start may have stopped after the rename, so finish the rewrite even
    // when there is nothing left to rename.
    let repair = metadata(old.root())?.is_none()
        && metadata(new.root())?.is_some_and(|m| m.is_dir())
        && metadata(&current_marker)?.is_none();
    let repair_manifests = metadata(&current_marker)?.is_none()
        && metadata(&home.join(legacy_path(BIN_PATH)))?.is_none()
        && metadata(&home.join(BIN_PATH))?.is_some_and(|m| m.is_dir())
        && metadata(&home.join(INSTALL_PATH))?.is_some_and(|m| m.is_dir());
    let mut moves = Vec::new();
    let server_install = std::cell::OnceCell::new();
    let old_install = home.join(legacy_path(INSTALL_PATH));
    for name in ["sandhilld", "ark", "ty", "ruff"] {
        if metadata(&old_install.join(format!(".{name}.lock")))?.is_some() {
            notes.push(
                "Sandhill kept its old files because an earlier install still has them locked."
                    .into(),
            );
            return Ok(());
        }
    }
    for &(old, new) in OLD_DAEMON_LAYOUT {
        let from = home.join(old);
        if metadata(&from)?.is_none() {
            continue;
        }
        let to = home.join(new);
        if metadata(&to)?.is_none() {
            check_dirs(home, &from)?;
            moves.push(Move::new(from, to));
        } else if matches!(new, BIN_PATH | INSTALL_PATH)
            && *server_install.get_or_init(|| server_scaffolding(home))
        {
            check_dirs(home, &from)?;
            check_dirs(home, &to)?;
            for entry in fs::read_dir(&from)? {
                let entry = entry?;
                let name = entry.file_name();
                let target = to.join(&name);
                if metadata(&target)?.is_none() && !component_conflict(home, new, &name)? {
                    moves.push(Move::new(entry.path(), target));
                }
            }
        }
    }
    if moves.is_empty() && !repair && !repair_manifests && recovered.is_empty() {
        return Ok(());
    }

    if check_old_resources(home, &old)? {
        notes.push("Sandhill kept its old files because a kernel, terminal or job is still running. It will try again on a later start.".into());
        return Ok(());
    }

    let moving_state = moves.iter().any(|item| item.to == new.root());
    let paths = (moving_state || repair)
        .then(|| Rebase::new(home))
        .transpose()?;
    let legacy_bin = home.join(legacy_path(BIN_PATH));
    let mut edits = Vec::new();
    let mut steps = Vec::new();
    let result = (|| -> Result<()> {
        for item in &mut moves {
            item.apply(rename)?;
            steps.push(format!(
                "{} {} to {}.",
                if item.retired.is_some() {
                    "Copied and checked"
                } else {
                    "Moved"
                },
                item.from.display(),
                item.to.display()
            ));
        }
        if let Some(paths) = &paths {
            rewrite_state(&new, paths, &mut edits)?;
        }
        // Manifests remain home-relative; only paths of actually moved
        // helpers change. The server daemon has its own legacy cleanup.
        for name in ["ark", "ty", "ruff"] {
            let source = legacy_bin.join(name);
            let current = home.join(BIN_PATH).join(name);
            if metadata(&source)?.is_none() && metadata(&current)?.is_some_and(|m| m.is_file()) {
                let manifest = home.join(INSTALL_PATH).join(format!("{name}.json"));
                rewrite_manifest(&manifest, name, &mut edits)?;
            }
        }
        if let Some(paths) = &paths {
            rewrite_journal(&new.db_path(), paths)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        for edit in edits.into_iter().rev() {
            if let Err(err) = edit.rollback() {
                notes.push(format!(
                    "Sandhill could not restore a saved record: {err:#}"
                ));
            }
        }
        for item in moves.into_iter().rev() {
            if let Err(err) = item.rollback() {
                notes.push(format!(
                    "Sandhill could not put an old folder back: {err:#}"
                ));
            }
        }
        return Err(error);
    }
    notes.extend(steps);
    if paths.is_some() {
        if repair {
            notes.push("Checked the saved paths in Sandhill's new state folder.".into());
        }
        let marker = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&current_marker);
        if let Err(error) = marker
            && error.kind() != io::ErrorKind::AlreadyExists
        {
            notes.push(format!(
                "Sandhill could not record that the move was finished: {error}"
            ));
        }
    }
    for retired in moves
        .into_iter()
        .filter_map(|item| item.retired)
        .chain(recovered)
    {
        if let Err(error) = discard_retired(&retired) {
            notes.push(format!(
                "Sandhill kept an old copy at {}: {error}",
                retired.display()
            ));
        }
    }
    // A failed rmdir cannot remove anyone else's files, even if they were
    // added since the move. Ignore absent or nonempty parents.
    for &(old, _) in OLD_DAEMON_LAYOUT.iter().rev() {
        let mut parent = home.join(old);
        while parent != home {
            match fs::remove_dir(&parent) {
                Ok(()) => notes.push(format!("Removed the empty folder {}.", parent.display())),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) if err.kind() == io::ErrorKind::DirectoryNotEmpty => break,
                Err(err) => {
                    notes.push(format!(
                        "Sandhill left {} in place: {err}",
                        parent.display()
                    ));
                    break;
                }
            }
            if !parent.pop() {
                break;
            }
        }
    }
    Ok(())
}

fn check_old_resources(home: &Path, old: &StateDir) -> Result<bool> {
    if metadata(old.root())?.is_none() {
        return Ok(false);
    }
    check_dirs(home, old.root())?;
    let journal = old.db_path();
    let sessions = match metadata(&journal)? {
        Some(meta) => {
            ensure!(
                meta.is_file() && meta.nlink() == 1,
                "the old journal is linked or is not a file"
            );
            for suffix in ["journal.sqlite-wal", "journal.sqlite-shm"] {
                if let Some(meta) = metadata(&old.root().join(suffix))? {
                    ensure!(
                        meta.is_file() && meta.nlink() == 1,
                        "an old journal file is linked or is not a file"
                    );
                }
            }
            Journal::read_sessions(&journal)?
        }
        None => Vec::new(),
    };
    crate::lifecycle::old_resources_alive(old, &sessions)
}

fn server_scaffolding(home: &Path) -> bool {
    let manifest = home.join(INSTALL_PATH).join("sandhilld.json");
    let executable = home.join(DAEMON_PATH);
    if ![BIN_PATH, INSTALL_PATH]
        .into_iter()
        .all(|dir| check_dirs(home, &home.join(dir)).is_ok())
    {
        return false;
    }
    let Ok(bytes) = read_regular(&manifest) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    let Some(expected) = record["sha256"].as_str() else {
        return false;
    };
    if record["component"] != "sandhilld"
        || record["version"] != 1
        || record["path"] != DAEMON_PATH
        || !executable
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
    {
        return false;
    }
    use sha2::{Digest, Sha256};
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(executable)
    else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        match file.read(&mut bytes) {
            Ok(0) => return sandhill_proto::hex(&hash.finalize()) == expected,
            Ok(count) => hash.update(&bytes[..count]),
            Err(_) => return false,
        }
    }
}

fn component_conflict(home: &Path, folder: &str, name: &std::ffi::OsStr) -> io::Result<bool> {
    let name = match (folder, name.to_str()) {
        (BIN_PATH, Some(name)) => name,
        (INSTALL_PATH, Some(name)) => match name.strip_suffix(".json") {
            Some(name) => name,
            None => return Ok(false),
        },
        _ => return Ok(false),
    };
    if !matches!(name, "ark" | "ty" | "ruff") {
        return Ok(false);
    }
    let (dir, other) = if folder == BIN_PATH {
        (INSTALL_PATH, format!("{name}.json"))
    } else {
        (BIN_PATH, name.to_owned())
    };
    Ok(metadata(&home.join(dir).join(other))?.is_some())
}

fn metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match path.symlink_metadata() {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn check_dirs(home: &Path, path: &Path) -> Result<()> {
    let mut dir = path;
    while dir != home {
        ensure!(
            dir.symlink_metadata()?.is_dir(),
            "{} is not a regular folder",
            dir.display()
        );
        dir = dir
            .parent()
            .context("the old folder is not inside the home folder")?;
    }
    Ok(())
}

const RETIRED_PREFIX: &[u8] = b".sandhill-moving-";

fn retired_path(from: &Path) -> PathBuf {
    let mut name = RETIRED_PREFIX.to_vec();
    name.extend_from_slice(from.file_name().unwrap().as_bytes());
    from.with_file_name(std::ffi::OsStr::from_bytes(&name))
}

fn file_identity(meta: &Metadata) -> String {
    format!("{}:{}", meta.dev(), meta.ino())
}

fn recover_copies(home: &Path, recovered: &mut Vec<PathBuf>) -> Result<()> {
    for &(old, new) in OLD_DAEMON_LAYOUT {
        let from = home.join(old);
        let to = home.join(new);
        recover_copy(home, &from, &to, recovered)?;
        if matches!(new, BIN_PATH | INSTALL_PATH)
            && metadata(&from)?.is_some_and(|item| item.is_dir())
        {
            check_dirs(home, &from)?;
            for entry in fs::read_dir(&from)? {
                let name = entry?.file_name();
                if let Some(name) = name.as_bytes().strip_prefix(RETIRED_PREFIX)
                    && !name.is_empty()
                    && name != b"."
                    && name != b".."
                {
                    let name = std::ffi::OsStr::from_bytes(name);
                    recover_copy(home, &from.join(name), &to.join(name), recovered)?;
                }
            }
        }
    }
    Ok(())
}

/// The source waits beside its old name. If we stopped before publishing
/// the checked copy, restore that source and start over. Otherwise retain
/// it until the rest of this start finishes rewriting the saved paths.
fn recover_copy(home: &Path, from: &Path, to: &Path, recovered: &mut Vec<PathBuf>) -> Result<()> {
    let retired = retired_path(from);
    let Some(meta) = metadata(&retired)? else {
        return Ok(());
    };
    check_dirs(home, &retired)?;
    ensure!(
        meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o777 == 0o700,
        "{} is not a private Sandhill folder",
        retired.display()
    );
    let original = retired.join("original");
    let target_marker = retired.join("target");
    let published = if metadata(&target_marker)?.is_some() {
        let expected = read_regular(&target_marker)?;
        metadata(to)?.is_some_and(|meta| file_identity(&meta).as_bytes() == expected)
    } else {
        false
    };
    if metadata(&original)?.is_none() {
        if metadata(from)?.is_some() || published {
            discard_retired(&retired)?;
        }
        return Ok(());
    }
    ensure!(
        metadata(from)?.is_none(),
        "{} exists beside its saved original",
        from.display()
    );
    if published {
        recovered.push(retired);
    } else {
        rename_exclusive(&original, from)?;
        discard_retired(&retired)?;
    }
    Ok(())
}

struct Move {
    from: PathBuf,
    to: PathBuf,
    applied: bool,
    /// For a cross-device move, the untouched source waits beside its old
    /// name until every copy and every metadata update has been checked.
    retired: Option<PathBuf>,
}

impl Move {
    fn new(from: PathBuf, to: PathBuf) -> Self {
        Self {
            from,
            to,
            applied: false,
            retired: None,
        }
    }

    fn apply(&mut self, rename: &impl Fn(&Path, &Path) -> io::Result<()>) -> Result<()> {
        ensure!(
            metadata(&self.to)?.is_none(),
            "{} already exists",
            self.to.display()
        );
        match rename(&self.from, &self.to) {
            Ok(()) => self.applied = true,
            Err(err) if err.raw_os_error() == Some(libc::EXDEV) => {
                let stage = tempfile::tempdir_in(self.to.parent().unwrap())?;
                let copy = stage.path().join("copy");
                copy_tree(&self.from, &copy)?;
                verify_copy(&self.from, &copy)?;
                if self.to.ends_with(STATE_PATH)
                    && metadata(&copy.join("journal.sqlite"))?.is_some()
                {
                    Journal::read_sessions(&copy.join("journal.sqlite"))?;
                }
                let retired = retired_path(&self.from);
                fs::DirBuilder::new().mode(0o700).create(&retired)?;
                self.retired = Some(retired.clone());
                let mut marker = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(retired.join("target"))?;
                marker.write_all(file_identity(&copy.symlink_metadata()?).as_bytes())?;
                marker.sync_all()?;
                rename_exclusive(&self.from, &retired.join("original"))?;
                rename_exclusive(&copy, &self.to)?;
                self.applied = true;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "could not move {} to {}",
                        self.from.display(),
                        self.to.display()
                    )
                });
            }
        }
        Ok(())
    }

    fn rollback(self) -> Result<()> {
        if let Some(retired) = self.retired {
            let original = retired.join("original");
            if metadata(&original)?.is_some() {
                rename_exclusive(&original, &self.from)?;
            }
            discard_retired(&retired)?;
            if self.applied {
                discard(&self.to)?;
            }
        } else if self.applied {
            rename_exclusive(&self.to, &self.from)?;
        }
        Ok(())
    }
}

fn discard_retired(retired: &Path) -> io::Result<()> {
    // Keep the target identity until the original is fully discarded. A
    // later start can then recognize and finish an interrupted cleanup.
    let original = retired.join("original");
    if metadata(&original)?.is_some() {
        discard(&original)?;
    }
    let marker = retired.join("target");
    if metadata(&marker)?.is_some() {
        fs::remove_file(marker)?;
    }
    fs::remove_dir(retired)
}

fn discard(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let meta = from.symlink_metadata()?;
    let kind = meta.file_type();
    if kind.is_dir() {
        fs::DirBuilder::new().mode(0o700).create(to)?;
        for item in fs::read_dir(from)? {
            let item = item?;
            copy_tree(&item.path(), &to.join(item.file_name()))?;
        }
    } else if kind.is_symlink() {
        symlink(fs::read_link(from)?, to)?;
    } else if kind.is_file() {
        fs::copy(from, to)?;
    } else {
        // Inactive socket directory entries are not saved data. They can
        // neither be copied nor used to reconnect on another filesystem.
        ensure!(
            kind.is_socket(),
            "{} is not a file, folder or link",
            from.display()
        );
        return Ok(());
    }
    if !kind.is_symlink() {
        fs::set_permissions(to, meta.permissions())?;
        let times = fs::FileTimes::new().set_modified(meta.modified()?);
        let file = File::open(to)?;
        file.set_times(times)?;
        file.sync_all()?;
    }
    Ok(())
}

fn verify_copy(from: &Path, to: &Path) -> Result<()> {
    let source = from.symlink_metadata()?;
    if source.file_type().is_socket() {
        ensure!(metadata(to)?.is_none(), "unexpected copy of a socket");
        return Ok(());
    }
    let copy = to.symlink_metadata()?;
    let same = if source.is_dir() && copy.is_dir() {
        let mut count = fs::read_dir(to)?.count();
        for item in fs::read_dir(from)? {
            let item = item?;
            verify_copy(&item.path(), &to.join(item.file_name()))?;
            if !item.file_type()?.is_socket() {
                count = count
                    .checked_sub(1)
                    .context("a copied folder is incomplete")?;
            }
        }
        count == 0
    } else if source.is_symlink() && copy.is_symlink() {
        fs::read_link(from)? == fs::read_link(to)?
    } else {
        source.is_file() && copy.is_file() && source.len() == copy.len()
    };
    ensure!(same, "the copy of {} does not match", from.display());
    Ok(())
}

struct Rebase {
    old: PathBuf,
    old_real: PathBuf,
    new: PathBuf,
    new_real: PathBuf,
}

impl Rebase {
    fn new(home: &Path) -> Result<Self> {
        let absolute = std::path::absolute(home)?;
        let real = home.canonicalize()?;
        let old_path = legacy_path(STATE_PATH);
        Ok(Self {
            old: absolute.join(old_path),
            old_real: real.join(old_path),
            new: absolute.join(STATE_PATH),
            new_real: real.join(STATE_PATH),
        })
    }

    fn path(&self, path: &Path) -> Option<PathBuf> {
        let (rest, root) = if let Ok(rest) = path.strip_prefix(&self.old) {
            (rest, &self.new)
        } else {
            (path.strip_prefix(&self.old_real).ok()?, &self.new_real)
        };
        rest.components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
            .then(|| root.join(rest))
    }

    fn text(&self, value: &str) -> Option<String> {
        Some(self.path(Path::new(value))?.to_string_lossy().into_owned())
    }

    fn command(&self, text: &str) -> Option<String> {
        // Terminal install commands record shell-quoted paths (including
        // paths prefixed by @ for an install worker's saved payload).
        let mut result = None;
        for (old, new) in [(&self.old, &self.new), (&self.old_real, &self.new_real)] {
            let old = old.to_string_lossy().replace('\'', "'\\''");
            let new = new.to_string_lossy().replace('\'', "'\\''");
            let text = result.as_deref().unwrap_or(text);
            let mut changed = text.to_owned();
            for prefix in ["'", "'@"] {
                for suffix in ["/", "'"] {
                    changed = changed.replace(
                        &format!("{prefix}{old}{suffix}"),
                        &format!("{prefix}{new}{suffix}"),
                    );
                }
            }
            if changed != text {
                result = Some(changed);
            }
        }
        result
    }

    fn value(&self, value: &mut Value) -> bool {
        match value {
            Value::String(text) => {
                if let Some(new) = self.text(text) {
                    *text = new;
                    true
                } else {
                    false
                }
            }
            Value::Array(values) => values
                .iter_mut()
                .fold(false, |changed, v| self.value(v) | changed),
            Value::Object(values) => values
                .values_mut()
                .fold(false, |changed, v| self.value(v) | changed),
            _ => false,
        }
    }
}

enum Edit {
    File { path: PathBuf, before: Vec<u8> },
    Link { path: PathBuf, before: PathBuf },
}

impl Edit {
    fn apply(path: &Path, before: Vec<u8>, after: &[u8], edits: &mut Vec<Self>) -> Result<()> {
        replace(path, after)?;
        edits.push(Self::File {
            path: path.into(),
            before,
        });
        Ok(())
    }

    fn rollback(self) -> Result<()> {
        match self {
            Self::File { path, before } => replace(&path, &before),
            Self::Link { path, before } => replace_link(&path, &before),
        }
    }
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.nlink() == 1,
        "{} is linked or is not a regular file",
        path.display()
    );
    let mut bytes = Vec::new();
    (&file).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let meta = path.symlink_metadata()?;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temp.as_file().set_permissions(meta.permissions())?;
    temp.write_all(bytes)?;
    temp.as_file()
        .set_times(fs::FileTimes::new().set_modified(meta.modified()?))?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}

fn replace_link(path: &Path, target: &Path) -> Result<()> {
    let stage = tempfile::tempdir_in(path.parent().unwrap())?;
    let link = stage.path().join("link");
    symlink(target, &link)?;
    fs::rename(link, path)?;
    Ok(())
}

fn rewrite_links(dir: &Path, rebase: &Rebase, edits: &mut Vec<Edit>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            rewrite_links(&path, rebase, edits)?;
        } else if kind.is_symlink() {
            let before = fs::read_link(&path)?;
            if let Some(target) = rebase.path(&before) {
                replace_link(&path, &target)?;
                edits.push(Edit::Link { path, before });
            }
        }
    }
    Ok(())
}

fn rewrite_json(
    path: &Path,
    edits: &mut Vec<Edit>,
    change: impl FnOnce(&mut Value) -> bool,
) -> Result<()> {
    if metadata(path)?.is_some() {
        let before = read_regular(path)?;
        let mut value = serde_json::from_slice(&before)
            .with_context(|| format!("could not read {}", path.display()))?;
        if change(&mut value) {
            Edit::apply(path, before, &serde_json::to_vec(&value)?, edits)?;
        }
    }
    Ok(())
}

fn rewrite_manifest(path: &Path, name: &str, edits: &mut Vec<Edit>) -> Result<()> {
    if metadata(path)?.is_none() {
        return Ok(());
    }
    let old = format!("{}/{name}", legacy_path(BIN_PATH));
    let before = read_regular(path)?;
    let record: Value = serde_json::from_slice(&before)?;
    if record["version"] == 1
        && record["component"] == name
        && record["path"] == old
        && let Some(hash) = record["sha256"].as_str().filter(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
    {
        // Keep the installer's compact format so a later checked upgrade
        // recognizes this as the same ownership record.
        let changed = format!(
            "{{\"version\":1,\"component\":\"{name}\",\"path\":\"{BIN_PATH}/{name}\",\"sha256\":\"{hash}\"}}\n"
        );
        Edit::apply(path, before, changed.as_bytes(), edits)?;
    }
    Ok(())
}

fn rewrite_state(state: &StateDir, rebase: &Rebase, edits: &mut Vec<Edit>) -> Result<()> {
    let record = state.root().join(crate::quick::record::FILE);
    if metadata(&record)?.is_some() {
        let before = read_regular(&record)?;
        let text = std::str::from_utf8(&before)?;
        let mut after = String::with_capacity(before.len());
        for line in text.split_inclusive('\n') {
            let mut fields = line.splitn(5, '\t');
            if let [
                Some(kind),
                Some(name),
                Some(version),
                Some(place),
                Some(rest),
            ] = std::array::from_fn(|_| fields.next())
                && let Some(new) = rebase.text(place)
            {
                use std::fmt::Write as _;
                let _ = write!(after, "{kind}\t{name}\t{version}\t{new}\t{rest}");
            } else {
                after.push_str(line);
            }
        }
        if text != after {
            Edit::apply(&record, before, after.as_bytes(), edits)?;
        }
    }
    rewrite_json(&state.root().join("install-prefs.json"), edits, |value| {
        rebase.value(value)
    })?;
    for name in ["undo", "terminals", "log", "copy-package-index", "explore"] {
        let folder = state.root().join(name);
        let Some(meta) = metadata(&folder)? else {
            continue;
        };
        ensure!(
            meta.is_dir(),
            "{} is not a regular folder",
            folder.display()
        );
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let path = entry.path();
            let file = entry.file_name();
            let file = file.to_string_lossy();
            if matches!(name, "undo" | "terminals" | "explore") {
                ensure!(
                    entry.file_type()?.is_dir(),
                    "{} is not a regular folder",
                    path.display()
                );
                if name == "undo" || name == "explore" {
                    rewrite_links(&path, rebase, edits)?;
                }
                let files: &[_] = match name {
                    "undo" => &["snapshot.json"],
                    "explore"
                        if path
                            .join(".sandhill")
                            .symlink_metadata()
                            .is_ok_and(|m| m.is_dir()) =>
                    {
                        &[".sandhill/quick.json"]
                    }
                    "explore" => &[],
                    _ => &["meta.json", "render.json", "agent.json"],
                };
                for file in files {
                    rewrite_json(&path.join(file), edits, |value| {
                        let mut changed = rebase.value(value);
                        if *file == "meta.json" {
                            if let Some(command) =
                                value.get_mut("command").and_then(Value::as_array_mut)
                            {
                                for arg in command {
                                    if let Some(new) = arg.as_str().and_then(|s| rebase.command(s))
                                    {
                                        *arg = Value::String(new);
                                        changed = true;
                                    }
                                }
                            }
                            if let Some(new) = value["job"]["command"]
                                .as_str()
                                .and_then(|s| rebase.command(s))
                            {
                                value["job"]["command"] = Value::String(new);
                                changed = true;
                            }
                        }
                        changed
                    })?;
                }
            } else if file.ends_with(".meta.json")
                || file.ends_with(".work.json")
                || file == "last-query.json"
            {
                rewrite_json(&path, edits, |value| rebase.value(value))?;
            }
        }
    }
    Ok(())
}

fn rewrite_journal(path: &Path, rebase: &Rebase) -> Result<()> {
    if metadata(path)?.is_none() {
        return Ok(());
    }
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let tx = conn.transaction()?;
    for (table, column, json) in [
        ("sessions", "connection_file", false),
        ("sessions", "cwd", false),
        ("sessions", "project", false),
        ("projects", "path", false),
        ("projects", "state", true),
        ("trusted_folders", "path", false),
        ("runs", "cell_file", false),
        ("file_versions", "project", false),
        ("analyses", "project", false),
        ("analyses", "record", true),
        ("analysis_checks", "record", true),
    ] {
        if !tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            params![table, column],
            |row| row.get::<_, bool>(0),
        )? {
            continue;
        }
        let mut changes = Vec::new();
        {
            let mut select = tx.prepare(&format!(
                "SELECT rowid, {column} FROM {table} WHERE {column} IS NOT NULL"
            ))?;
            let mut rows = select.query([])?;
            while let Some(row) = rows.next()? {
                let old = row.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
                let new = if json {
                    let mut value: Value = serde_json::from_str(old)?;
                    rebase.value(&mut value).then(|| value.to_string())
                } else {
                    rebase.text(old)
                };
                if let Some(new) = new {
                    changes.push((row.get::<_, i64>(0)?, new));
                }
            }
        }
        if !changes.is_empty() {
            let mut update = tx.prepare(&format!(
                "UPDATE {table} SET {column} = ?1 WHERE rowid = ?2"
            ))?;
            for (rowid, new) in changes {
                update.execute(params![new, rowid])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
use crate::quick::test_common as common;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_device_copy_rebases_paths_and_keeps_links_as_links() {
        let temp = common::tempdir().unwrap();
        let home = temp.path();
        StateDir::prepare_home(home).unwrap();
        let old = StateDir::new(home.join(legacy_path(STATE_PATH)));
        old.create().unwrap();
        let journal = Journal::open(
            &old.db_path(),
            crate::blobs::BlobStore::new(old.blobs_dir()),
        )
        .unwrap();
        drop(journal);
        let record = format!(
            "kernel\texample\t1\t{}/kernels/example\t2026-11-20\n",
            old.root().display()
        );
        fs::write(old.root().join(crate::quick::record::FILE), record).unwrap();
        symlink("../kernels", old.blobs_dir().join("link")).unwrap();
        let notes = migrate(home, |_, _| Err(io::Error::from_raw_os_error(libc::EXDEV)));
        assert!(
            notes
                .iter()
                .any(|note| note.starts_with("Copied and checked")),
            "{notes:?}"
        );
        let new = StateDir::new(home.join(STATE_PATH));
        assert!(Journal::read_sessions(&new.db_path()).unwrap().is_empty());
        assert!(
            fs::read_to_string(new.root().join(crate::quick::record::FILE))
                .unwrap()
                .contains(new.root().to_str().unwrap())
        );
        assert_eq!(
            fs::read_link(new.blobs_dir().join("link")).unwrap(),
            Path::new("../kernels")
        );
        assert!(!old.root().exists());
    }

    #[test]
    fn a_failed_move_puts_the_whole_layout_back() {
        let temp = common::tempdir().unwrap();
        let home = temp.path();
        StateDir::prepare_home(home).unwrap();
        let old = home.join(legacy_path(STATE_PATH));
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("only-copy"), b"keep me").unwrap();
        fs::create_dir_all(home.join(legacy_path(BIN_PATH))).unwrap();
        let notes = migrate(home, |from, to| {
            if to == home.join(BIN_PATH) {
                Err(io::Error::from_raw_os_error(libc::EACCES))
            } else {
                fs::rename(from, to)
            }
        });
        assert!(
            notes.iter().any(|note| note.contains("could not move")),
            "{notes:?}"
        );
        assert_eq!(fs::read(old.join("only-copy")).unwrap(), b"keep me");
        assert!(!home.join(STATE_PATH).exists());
        assert!(home.join(legacy_path(BIN_PATH)).is_dir());
    }

    #[test]
    fn cross_device_resume_recovers_before_and_after_publishing_the_copy() {
        for publish in [false, true] {
            let temp = common::tempdir().unwrap();
            let home = temp.path();
            let area = StateDir::prepare_home(home).unwrap();
            let old = home.join(legacy_path(STATE_PATH));
            let new = home.join(STATE_PATH);
            fs::create_dir_all(&old).unwrap();
            fs::write(old.join("history"), b"original bytes").unwrap();
            let stage = area.join("staged-copy");
            copy_tree(&old, &stage).unwrap();
            let retired = retired_path(&old);
            fs::DirBuilder::new().mode(0o700).create(&retired).unwrap();
            fs::write(
                retired.join("target"),
                file_identity(&stage.symlink_metadata().unwrap()),
            )
            .unwrap();
            fs::rename(&old, retired.join("original")).unwrap();
            if publish {
                fs::rename(&stage, &new).unwrap();
            }
            let notes = migrate(home, rename_exclusive);
            assert_eq!(
                fs::read(new.join("history")).unwrap(),
                b"original bytes",
                "{notes:?}"
            );
            assert!(!old.exists());
            assert!(!retired.exists());
        }
    }

    #[test]
    fn finishes_a_state_rename_that_stopped_before_fixing_paths() {
        let temp = common::tempdir().unwrap();
        let home = temp.path();
        StateDir::prepare_home(home).unwrap();
        let old = StateDir::new(home.join(legacy_path(STATE_PATH)));
        let new = StateDir::new(home.join(STATE_PATH));
        old.create().unwrap();
        let record = format!(
            "kernel\told\t1\t{}/kernels/old\t2026-11-20\n",
            old.root().display()
        );
        fs::write(old.root().join(crate::quick::record::FILE), &record).unwrap();
        fs::rename(old.root(), new.root()).unwrap();
        let notes = migrate(home, rename_exclusive);
        assert!(
            notes
                .iter()
                .any(|note| note.contains("Checked the saved paths")),
            "{notes:?}"
        );
        let expected = record.replacen(
            old.root().to_str().unwrap(),
            new.root().to_str().unwrap(),
            1,
        );
        assert_eq!(
            fs::read_to_string(new.root().join(crate::quick::record::FILE)).unwrap(),
            expected
        );
        assert!(migrate(home, rename_exclusive).is_empty());
    }

    #[test]
    fn finishes_install_records_after_an_earlier_bin_rename() {
        let temp = common::tempdir().unwrap();
        let home = temp.path();
        StateDir::prepare_home(home).unwrap();
        let old_bin = home.join(legacy_path(BIN_PATH));
        let old_install = home.join(legacy_path(INSTALL_PATH));
        let new_install = home.join(INSTALL_PATH);
        fs::create_dir_all(&old_bin).unwrap();
        fs::create_dir_all(&old_install).unwrap();
        fs::write(old_bin.join("ark"), b"helper").unwrap();
        let old_ark = format!("{}/ark", legacy_path(BIN_PATH));
        let hash = "a".repeat(64);
        fs::write(old_install.join("ark.json"), format!(
            "{{\"version\":1,\"component\":\"ark\",\"path\":\"{old_ark}\",\"sha256\":\"{hash}\"}}"
        )).unwrap();
        fs::rename(&old_bin, home.join(BIN_PATH)).unwrap();
        let notes = migrate(home, rename_exclusive);
        assert!(!old_install.exists(), "{notes:?}");
        let record: Value =
            serde_json::from_slice(&fs::read(new_install.join("ark.json")).unwrap()).unwrap();
        assert_eq!(record["path"], sandhill_proto::ARK_PATH);
        assert_eq!(record["sha256"], hash);
        assert!(migrate(home, rename_exclusive).is_empty());
    }
}
