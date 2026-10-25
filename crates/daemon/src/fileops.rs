//! Changing files and folders for the file tree: new folders, renames,
//! copies, the Trash, and reading a file in parts to download it. Also the
//! project's files for the Agents pane's @ menu.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sandhill_proto::{Deleted, EXISTS_ERROR, FileBytes, FileWritten, MAX_READ_BYTES};

use crate::files::{mtime_ms, resolve};

/// How many entries a listing with folder sizes looks at, at most, so a
/// folder of a million files can't stall it. Sizes past that are partial.
const SIZE_BUDGET: u64 = 200_000;

pub fn make_dir(path: &str) -> Result<FileWritten> {
    let path = resolve(path)?;
    refuse_existing(&path)?;
    fs::create_dir_all(&path)
        .with_context(|| format!("cannot make the folder {}", path.display()))?;
    written(&path)
}

/// A new file, made only if nothing is at `path` yet.
pub fn create_file(path: &str, bytes: &[u8]) -> Result<FileWritten> {
    let path = resolve(path)?;
    refuse_existing(&path)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot make the folder {}", dir.display()))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| match err.kind() {
            ErrorKind::AlreadyExists => exists(&path),
            _ => anyhow::Error::new(err).context(format!("cannot write {}", path.display())),
        })?;
    file.write_all(bytes)?;
    file.sync_all()?;
    written(&path)
}

pub fn move_path(from: &str, to: &str) -> Result<FileWritten> {
    let (from, to) = (resolve(from)?, resolve(to)?);
    let meta =
        fs::symlink_metadata(&from).with_context(|| format!("cannot find {}", from.display()))?;
    // Changing only the case of a name is a rename to the same file on macOS.
    let same =
        fs::symlink_metadata(&to).is_ok_and(|t| t.dev() == meta.dev() && t.ino() == meta.ino());
    if !same {
        refuse_existing(&to)?;
    }
    refuse_inside(&from, &to, "move")?;
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)
            .with_context(|| format!("cannot make the folder {}", dir.display()))?;
    }
    match fs::rename(&from, &to) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::CrossesDevices => {
            copy_tree(&from, &to)?;
            remove_tree(&from)?;
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("cannot move {} to {}", from.display(), to.display()));
        }
    }
    written(&to)
}

pub fn copy_path(from: &str, to: &str) -> Result<FileWritten> {
    let (from, to) = (resolve(from)?, resolve(to)?);
    fs::symlink_metadata(&from).with_context(|| format!("cannot find {}", from.display()))?;
    refuse_existing(&to)?;
    refuse_inside(&from, &to, "copy")?;
    if let Err(err) = copy_tree(&from, &to) {
        let _ = remove_tree(&to);
        return Err(err);
    }
    written(&to)
}

pub fn delete_path(path: &str, trash: bool) -> Result<Deleted> {
    let path = resolve(path)?;
    fs::symlink_metadata(&path).with_context(|| format!("cannot find {}", path.display()))?;
    let trashed_to = if trash {
        Some(Trash::here()?.put(&path)?.to_string_lossy().into_owned())
    } else {
        remove_tree(&path)?;
        None
    };
    Ok(Deleted {
        path: path.to_string_lossy().into_owned(),
        trashed_to,
    })
}

pub fn read_bytes(path: &str, offset: u64, limit: Option<u64>) -> Result<FileBytes> {
    let path = resolve(path)?;
    let mut file =
        fs::File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        bail!("{} is a folder", path.display());
    }
    file.seek(SeekFrom::Start(offset))?;
    let limit = limit.unwrap_or(MAX_READ_BYTES).min(MAX_READ_BYTES);
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(FileBytes {
        path: path.to_string_lossy().into_owned(),
        offset,
        data: BASE64.encode(&bytes),
        size: meta.len(),
        mtime_ms: mtime_ms(&meta),
    })
}

/// A shared budget for summing folder sizes in one listing.
pub struct SizeBudget(u64);

impl Default for SizeBudget {
    fn default() -> Self {
        SizeBudget(SIZE_BUDGET)
    }
}

/// The bytes in the files under `dir`. Links inside it aren't followed, so a
/// link back up the tree can't loop.
pub fn dir_size(dir: &Path, budget: &mut SizeBudget) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        if budget.0 == 0 {
            break;
        }
        budget.0 -= 1;
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path(), budget);
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    total
}

/// Folders `list_files` leaves out, besides hidden ones: installed
/// packages and caches, not the user's work.
const SKIPPED_DIRS: [&str; 5] = [
    "node_modules",
    "__pycache__",
    "renv",
    "site-packages",
    "_freeze",
];
/// The most entries `list_files` looks at, so a huge folder answers quickly.
const LIST_BUDGET: u64 = 50_000;
/// The files `list_files` returns unless told.
pub const LIST_FILES: usize = 5000;

/// The files under `root`, as paths relative to it, nearest first and by
/// name within a folder: for the Agents pane's @ menu. Hidden files and
/// folders, and package folders, are left out, and links aren't followed.
pub fn list_files(root: &str, limit: usize) -> Result<Vec<String>> {
    let root = &resolve(root)?;
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let mut files = Vec::new();
    let mut budget = LIST_BUDGET;
    let mut queue = std::collections::VecDeque::from([PathBuf::new()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
        for entry in entries {
            if budget == 0 || files.len() >= limit {
                return Ok(files);
            }
            budget -= 1;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = dir.join(&name);
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_str()) {
                    queue.push_back(path);
                }
            } else if kind.is_file() {
                files.push(path.to_string_lossy().into_owned());
            }
        }
    }
    Ok(files)
}

fn refuse_existing(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        return Err(exists(path));
    }
    Ok(())
}

fn exists(path: &Path) -> anyhow::Error {
    anyhow::anyhow!("{EXISTS_ERROR} {} already exists", path.display())
}

fn refuse_inside(from: &Path, to: &Path, what: &str) -> Result<()> {
    if to.starts_with(from) && to != from {
        bail!("cannot {what} {} into itself", from.display());
    }
    Ok(())
}

fn written(path: &Path) -> Result<FileWritten> {
    let meta = fs::symlink_metadata(path)?;
    Ok(FileWritten {
        path: path.to_string_lossy().into_owned(),
        mtime_ms: mtime_ms(&meta),
    })
}

/// Copies a file, or a folder and everything in it. Links are copied as links.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
    } else if meta.is_dir() {
        fs::create_dir(to).with_context(|| format!("cannot make the folder {}", to.display()))?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        fs::set_permissions(to, meta.permissions())?;
    } else {
        fs::copy(from, to)
            .with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
    }
    Ok(())
}

fn remove_tree(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .with_context(|| format!("cannot delete {}", path.display()))
}

/// The machine's Trash, where Finder or the desktop's file manager can put
/// things back from.
#[derive(Debug)]
enum Trash {
    /// `~/.Trash`: files go in under their own name.
    Mac(PathBuf),
    /// The freedesktop.org trash: `files/` and an `info/<name>.trashinfo`
    /// saying where each came from and when.
    Freedesktop(PathBuf),
}

impl Trash {
    fn here() -> Result<Trash> {
        let home = || {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .context("HOME is not set")
        };
        if cfg!(target_os = "macos") {
            return Ok(Trash::Mac(home()?.join(".Trash")));
        }
        let data = match std::env::var_os("XDG_DATA_HOME") {
            Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
            _ => home()?.join(".local/share"),
        };
        Ok(Trash::Freedesktop(data.join("Trash")))
    }

    /// Moves `path` into the Trash and returns where it went.
    fn put(&self, path: &Path) -> Result<PathBuf> {
        let name = path
            .file_name()
            .with_context(|| format!("cannot move {} to the Trash", path.display()))?
            .to_string_lossy()
            .into_owned();
        let (files, info) = match self {
            Trash::Mac(dir) => (dir.clone(), None),
            Trash::Freedesktop(dir) => (dir.join("files"), Some(dir.join("info"))),
        };
        fs::create_dir_all(&files)
            .with_context(|| format!("cannot make the Trash at {}", files.display()))?;
        if let Some(info) = &info {
            fs::create_dir_all(info)?;
        }
        for n in 1.. {
            let candidate = numbered(&name, n);
            let target = files.join(&candidate);
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            // The info file claims the name, so two deletes can't take the same one.
            let info_file = match &info {
                Some(info) => {
                    let file = info.join(format!("{candidate}.trashinfo"));
                    match OpenOptions::new().write(true).create_new(true).open(&file) {
                        Ok(mut f) => {
                            write!(
                                f,
                                "[Trash Info]\nPath={}\nDeletionDate={}\n",
                                url_path(path),
                                local_time_now()
                            )?;
                            Some(file)
                        }
                        Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
                        Err(err) => return Err(err.into()),
                    }
                }
                None => None,
            };
            return match fs::rename(path, &target) {
                Ok(()) => Ok(target),
                Err(err) => {
                    if let Some(file) = info_file {
                        let _ = fs::remove_file(file);
                    }
                    if err.kind() == ErrorKind::CrossesDevices {
                        bail!(
                            "cannot move {} to the Trash: it is on another disk. Delete it instead.",
                            path.display()
                        );
                    }
                    Err(err).with_context(|| format!("cannot move {} to the Trash", path.display()))
                }
            };
        }
        unreachable!()
    }
}

/// "name", then "name 2.ext", "name 3.ext" and so on, as Finder numbers copies.
fn numbered(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{} {n}{}", &name[..dot], &name[dot..]),
        None => format!("{name} {n}"),
    }
}

/// A path as the trash spec wants it: percent-encoded, slashes kept.
fn url_path(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The local time, as `YYYY-MM-DDThh:mm:ss`.
fn local_time_now() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn files_are_listed_nearest_first_without_hidden_or_package_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "b.R",
            "A.qmd",
            "data/raw/s1.csv",
            "data/z.csv",
            ".git/HEAD",
            "renv/library/x.R",
            "data/.hidden",
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        let files = list_files(&s(root), 100).unwrap();
        assert_eq!(files, ["A.qmd", "b.R", "data/z.csv", "data/raw/s1.csv"]);
        assert_eq!(list_files(&s(root), 2).unwrap(), ["A.qmd", "b.R"]);
        assert!(list_files(&s(&root.join("b.R")), 2).is_err());
    }

    #[test]
    fn copies_are_numbered_like_finders() {
        assert_eq!(numbered("a.qmd", 1), "a.qmd");
        assert_eq!(numbered("a.qmd", 2), "a 2.qmd");
        assert_eq!(numbered("data", 3), "data 3");
        assert_eq!(numbered(".env", 2), ".env 2");
    }

    #[test]
    fn trash_paths_are_percent_encoded() {
        assert_eq!(
            url_path(Path::new("/w/my data/ä.csv")),
            "/w/my%20data/%C3%A4.csv"
        );
    }

    #[test]
    fn the_freedesktop_trash_records_where_things_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::Freedesktop(dir.path().join("Trash"));
        for _ in 0..2 {
            let file = dir.path().join("notes.md");
            fs::write(&file, "x").unwrap();
            trash.put(&file).unwrap();
            assert!(!file.exists());
        }
        let files = dir.path().join("Trash/files");
        assert!(files.join("notes.md").exists());
        assert!(files.join("notes 2.md").exists());
        let info = fs::read_to_string(dir.path().join("Trash/info/notes 2.md.trashinfo")).unwrap();
        assert!(
            info.starts_with(&format!(
                "[Trash Info]\nPath={}\nDeletionDate=20",
                url_path(&dir.path().join("notes.md"))
            )),
            "{info}"
        );
    }

    #[test]
    fn the_mac_trash_takes_folders_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::Mac(dir.path().join(".Trash"));
        let folder = dir.path().join("results");
        fs::create_dir_all(folder.join("deep")).unwrap();
        assert_eq!(
            trash.put(&folder).unwrap(),
            dir.path().join(".Trash/results")
        );
        assert!(dir.path().join(".Trash/results/deep").is_dir());
    }

    #[test]
    fn nothing_is_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.R");
        let b = dir.path().join("b.R");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        for err in [
            create_file(&s(&a), b"new").unwrap_err(),
            make_dir(&s(&a)).unwrap_err(),
            move_path(&s(&a), &s(&b)).unwrap_err(),
            copy_path(&s(&a), &s(&b)).unwrap_err(),
        ] {
            assert!(err.to_string().starts_with(EXISTS_ERROR), "{err}");
        }
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
    }

    #[test]
    fn a_folder_is_copied_whole_but_not_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(data.join("raw")).unwrap();
        fs::write(data.join("raw/s1.fastq"), "@r1\nACGT\n+\nIIII\n").unwrap();
        std::os::unix::fs::symlink("raw/s1.fastq", data.join("latest")).unwrap();
        copy_path(&s(&data), &s(&dir.path().join("data copy"))).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("data copy/raw/s1.fastq")).unwrap(),
            "@r1\nACGT\n+\nIIII\n"
        );
        assert_eq!(
            fs::read_link(dir.path().join("data copy/latest")).unwrap(),
            Path::new("raw/s1.fastq")
        );
        assert!(copy_path(&s(&data), &s(&data.join("raw/again"))).is_err());

        let mut budget = SizeBudget::default();
        assert_eq!(dir_size(&data, &mut budget), 16);
    }
}
