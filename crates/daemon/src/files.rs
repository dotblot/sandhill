//! File access for clients. The app edits notebooks through the daemon so the
//! same code works whether the files are on the Mac or on a server.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use sandhill_proto::{CONFLICT_ERROR, DirEntry, EntryKind, FileText, FileWritten};

/// Notebooks bigger than this are almost certainly a mistake to open as text.
const MAX_READ: u64 = 256 * 1024 * 1024;

/// Expands a leading `~` and requires an absolute path.
pub fn resolve(path: &str) -> Result<PathBuf> {
    let expanded = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(path),
    };
    if !expanded.is_absolute() {
        bail!("{path} is not an absolute path");
    }
    Ok(expanded)
}

pub fn read_text(path: &str) -> Result<FileText> {
    let resolved = resolve(path)?;
    let meta = std::fs::metadata(&resolved)
        .with_context(|| format!("cannot open {}", resolved.display()))?;
    if meta.is_dir() {
        bail!("{} is a directory", resolved.display());
    }
    if meta.len() > MAX_READ {
        bail!(
            "{} is too large to open ({} bytes)",
            resolved.display(),
            meta.len()
        );
    }
    let bytes = std::fs::read(&resolved)?;
    let text = String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("{} is not a UTF-8 text file", resolved.display()))?;
    Ok(FileText {
        path: resolved.to_string_lossy().into_owned(),
        text,
        mtime_ms: mtime_ms(&meta),
    })
}

/// Writes through a temporary file and a rename, so readers never see a
/// half-written notebook and a crash leaves the old version intact.
pub fn write_text(path: &str, text: &str, expected_mtime_ms: Option<i64>) -> Result<FileWritten> {
    write_with(path, expected_mtime_ms, |file| {
        Ok(file.write_all(text.as_bytes())?)
    })
}

/// Like [`write_text`], with the contents written by `write`.
pub fn write_with(
    path: &str,
    expected_mtime_ms: Option<i64>,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<FileWritten> {
    let resolved = resolve(path)?;
    let existing = std::fs::metadata(&resolved).ok();
    if let (Some(expected), Some(meta)) = (expected_mtime_ms, &existing)
        && mtime_ms(meta) != expected
    {
        bail!(
            "{CONFLICT_ERROR} {} changed on disk since it was opened",
            resolved.display()
        );
    }
    write_atomic_with(&resolved, write)?;
    let meta = std::fs::metadata(&resolved)?;
    Ok(FileWritten {
        path: resolved.to_string_lossy().into_owned(),
        mtime_ms: mtime_ms(&meta),
    })
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(path, |file| Ok(file.write_all(bytes)?))
}

fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let name = path
        .file_name()
        .with_context(|| format!("{} is not a file path", path.display()))?;
    let tmp = dir.join(format!(
        ".{}.sandhill-{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut file = std::fs::File::create(&tmp)
            .with_context(|| format!("cannot write in {}", dir.display()))?;
        write(&mut file)?;
        file.sync_all()?;
        // Keep the permissions of the file being replaced.
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn list_dir(path: &str) -> Result<Vec<DirEntry>> {
    let resolved = resolve(path)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&resolved)
        .with_context(|| format!("cannot list {}", resolved.display()))?
    {
        let entry = entry?;
        // Follow symlinks so a linked data directory shows as a directory.
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
        });
    }
    entries.sort_by(|a, b| {
        (b.kind == EntryKind::Dir)
            .cmp(&(a.kind == EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_in(dir: &Path, name: &str) -> String {
        dir.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path(), "a.qmd");
        let written = write_text(&path, "# hi\n", None).unwrap();
        let read = read_text(&path).unwrap();
        assert_eq!(read.text, "# hi\n");
        assert_eq!(read.mtime_ms, written.mtime_ms);
    }

    #[test]
    fn stale_writes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path(), "a.qmd");
        let first = write_text(&path, "one", None).unwrap();
        let err = write_text(&path, "two", Some(first.mtime_ms - 1000)).unwrap_err();
        assert!(err.to_string().starts_with(CONFLICT_ERROR), "{err}");
        assert_eq!(read_text(&path).unwrap().text, "one");
        write_text(&path, "two", Some(first.mtime_ms)).unwrap();
        assert_eq!(read_text(&path).unwrap().text, "two");
    }

    #[test]
    fn lists_directories_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.ipynb"), "{}").unwrap();
        std::fs::write(dir.path().join("A.qmd"), "").unwrap();
        std::fs::create_dir(dir.path().join("zdata")).unwrap();
        let names: Vec<_> = list_dir(&dir.path().to_string_lossy())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["zdata", "A.qmd", "b.ipynb"]);
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert!(resolve("notes.qmd").is_err());
        assert!(resolve("~/notes.qmd").unwrap().is_absolute());
    }

    #[test]
    fn binary_files_are_not_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path(), "x.bin");
        std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_text(&path).is_err());
    }
}
