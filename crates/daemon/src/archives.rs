//! Archives for the file tree, as Finder's Compress and Archive Utility do
//! it. Packing a file or folder beside itself: a `.zip`; gzip (`.gz` for a
//! file, `.tar.gz` for a folder, since gzip holds one file); or bgzip, the
//! blocked `.gz` that tabix and samtools index. Unpacking `.zip`,
//! `.tar.gz`/`.tgz` and `.gz`/`.bgz` (plain or bgzip) beside the archive:
//! a single item comes out as itself, several go into a folder named after
//! the archive. The original always stays and nothing is replaced: a taken
//! name is numbered ("results 2"). Links inside folders stay links.
//!
//! Work started with a task id reports how many input bytes it has read
//! ([`progress`]) and stops when asked ([`cancel`]), leaving nothing half
//! made.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::num::NonZero;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context, Result, bail};
use flate2::read::MultiGzDecoder;
use flate2::{Compression, GzBuilder};
use noodles_bgzf as bgzf;
use sandhill_proto::{ArchiveFormat, ArchiveProgress, CANCELLED_ERROR, FileWritten};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::fileops::written;
use crate::files::resolve;

/// Big buffers: archives of sequencing data run to gigabytes.
const BUFFER: usize = 1 << 20;
/// Zip entries this big need zip64 sizes.
const ZIP64_AT: u64 = u32::MAX as u64;
/// The most threads bgzip compresses with: servers are shared.
const BGZIP_THREADS: usize = 8;
/// What a Mac adds to the zips it makes; not part of the data.
const MAC_EXTRAS: &str = "__MACOSX";
/// How many numbered names are tried before giving up.
const NAMES: u32 = 10_000;

/// How far one compression or extraction is, and whether to stop it.
#[derive(Default)]
struct Progress {
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::other(format!("{CANCELLED_ERROR} stopped")));
        }
        Ok(())
    }
}

/// The work under way that the app named, by its task id.
static TASKS: LazyLock<Mutex<HashMap<String, Arc<Progress>>>> = LazyLock::new(Default::default);

/// A task's progress, listed under its id until dropped.
struct Task {
    id: Option<String>,
    progress: Arc<Progress>,
}

impl Task {
    fn start(id: Option<String>) -> Self {
        let progress = Arc::new(Progress::default());
        if let Some(id) = &id {
            TASKS.lock().unwrap().insert(id.clone(), progress.clone());
        }
        Task { id, progress }
    }

    /// The error as the caller sees it: plain "cancelled" when it was
    /// stopped, so the app needn't show it as a failure.
    fn outcome<T>(&self, result: Result<T>, what: &str) -> Result<T> {
        if self.progress.cancelled.load(Ordering::Relaxed) {
            bail!("{CANCELLED_ERROR} {what} stopped");
        }
        result.with_context(|| what.to_string())
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(id) = &self.id {
            TASKS.lock().unwrap().remove(id);
        }
    }
}

/// How far the task `id` is; none once it has finished.
pub fn progress(id: &str) -> Option<ArchiveProgress> {
    let tasks = TASKS.lock().unwrap();
    let p = tasks.get(id)?;
    let total = p.total.load(Ordering::Relaxed);
    Some(ArchiveProgress {
        done: p.done.load(Ordering::Relaxed).min(total),
        total,
    })
}

/// Asks the task `id` to stop. Says whether it was still running.
pub fn cancel(id: &str) -> bool {
    match TASKS.lock().unwrap().get(id) {
        Some(p) => {
            p.cancelled.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// A reader that counts what it reads into the task's progress and fails
/// once the task is cancelled.
struct Counted<R> {
    inner: R,
    progress: Arc<Progress>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.progress.check()?;
        let n = self.inner.read(buf)?;
        self.progress.done.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Seek> Seek for Counted<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

fn counted(path: &Path, progress: &Arc<Progress>) -> io::Result<BufReader<Counted<File>>> {
    Ok(BufReader::with_capacity(
        BUFFER,
        Counted {
            inner: File::open(path)?,
            progress: progress.clone(),
        },
    ))
}

pub fn compress(path: &str, format: ArchiveFormat, task: Option<String>) -> Result<FileWritten> {
    let task = Task::start(task);
    let path = resolve(path)?;
    // The item itself is followed if it's a link; links inside a folder aren't.
    let meta = fs::metadata(&path).with_context(|| format!("cannot find {}", path.display()))?;
    let (Some(name), Some(dir)) = (path.file_name(), path.parent()) else {
        bail!("cannot compress {}", path.display());
    };
    let name = name.to_string_lossy().into_owned();
    let ext = match (format, meta.is_dir()) {
        (ArchiveFormat::Zip, _) => ".zip",
        (ArchiveFormat::Gzip | ArchiveFormat::Bgzip, false) => ".gz",
        (ArchiveFormat::Gzip, true) => ".tar.gz",
        (ArchiveFormat::Bgzip, true) => {
            bail!("bgzip packs one file; compress the folder to .tar.gz instead")
        }
    };
    let total = if meta.is_dir() {
        files_size(&path)
    } else {
        meta.len()
    };
    task.progress.total.store(total, Ordering::Relaxed);
    let (target, file) = create_free(dir, |n| format!("{}{ext}", numbered(&name, n, true)))?;
    let out = BufWriter::with_capacity(BUFFER, file);
    let progress = &task.progress;
    let packed = match (format, meta.is_dir()) {
        (ArchiveFormat::Zip, _) => write_zip(&path, &name, &meta, out, progress),
        (ArchiveFormat::Gzip, false) => write_gzip(&path, &name, &meta, out, progress),
        (ArchiveFormat::Gzip, true) => write_tar_gz(&path, &name, out, progress),
        (ArchiveFormat::Bgzip, _) => write_bgzip(&path, out, progress),
    };
    if packed.is_err() {
        let _ = fs::remove_file(&target);
    }
    task.outcome(packed, &format!("cannot compress {}", path.display()))?;
    written(&target)
}

/// Unpacks an archive beside itself. Returns what came out: the one file
/// or folder inside, or the folder holding several.
pub fn extract(path: &str, task: Option<String>) -> Result<FileWritten> {
    let task = Task::start(task);
    let path = resolve(path)?;
    let (Some(name), Some(dir)) = (path.file_name(), path.parent()) else {
        bail!("cannot extract {}", path.display());
    };
    let name = name.to_string_lossy().into_owned();
    let Some((kind, stem)) = archive_kind(&name) else {
        bail!("{name} is not a .zip, .tar.gz, .tgz, .gz or .bgz file");
    };
    let size = fs::metadata(&path)
        .with_context(|| format!("cannot find {}", path.display()))?
        .len();
    task.progress.total.store(size, Ordering::Relaxed);
    let out = match kind {
        Kind::Gzip => decompress(&path, dir, stem, &task.progress),
        Kind::Zip | Kind::TarGz => unpack(&path, dir, &name, stem, kind, &task.progress),
    };
    let out = task.outcome(out, &format!("cannot extract {}", path.display()))?;
    written(&out)
}

/// The bytes of the files under `dir`, not following links.
fn files_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => files_size(&entry.path()),
            Ok(meta) if meta.is_file() => meta.len(),
            _ => 0,
        })
        .sum()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Zip,
    TarGz,
    /// One file, plain gzip or bgzip.
    Gzip,
}

/// What an archive is, by its name, and its name without the extension.
fn archive_kind(name: &str) -> Option<(Kind, &str)> {
    let lower = name.to_ascii_lowercase();
    [
        (".tar.gz", Kind::TarGz),
        (".tgz", Kind::TarGz),
        (".zip", Kind::Zip),
        (".gz", Kind::Gzip),
        (".bgz", Kind::Gzip),
    ]
    .into_iter()
    .find(|(ext, _)| lower.len() > ext.len() && lower.ends_with(ext))
    // The extension is ASCII, so cutting its length off keeps whole letters.
    .map(|(ext, kind)| (kind, &name[..name.len() - ext.len()]))
}

/// A gzip or bgzip file's one file, written beside it.
fn decompress(path: &Path, dir: &Path, stem: &str, progress: &Arc<Progress>) -> Result<PathBuf> {
    let (target, file) = create_free(dir, |n| numbered(stem, n, false))?;
    let copied = (|| {
        let mut input = MultiGzDecoder::new(counted(path, progress)?);
        let mut out = BufWriter::with_capacity(BUFFER, file);
        io::copy(&mut input, &mut out)?;
        finish(out)
    })();
    if let Err(err) = copied {
        let _ = fs::remove_file(&target);
        return Err(err);
    }
    Ok(target)
}

/// A zip's or tarball's contents, unpacked into a hidden folder first so
/// nothing half-done shows, then moved beside the archive.
fn unpack(
    path: &Path,
    dir: &Path,
    name: &str,
    stem: &str,
    kind: Kind,
    progress: &Arc<Progress>,
) -> Result<PathBuf> {
    let staging = make_staging(dir, name)?;
    let unpacked = (|| {
        let input = counted(path, progress)?;
        // Both refuse entries that would land outside the folder.
        if kind == Kind::Zip {
            ZipArchive::new(input)?.extract(&staging)?;
        } else {
            tar::Archive::new(MultiGzDecoder::new(input)).unpack(&staging)?;
        }
        progress.check()?;
        if fs::symlink_metadata(staging.join(MAC_EXTRAS)).is_ok() {
            fs::remove_dir_all(staging.join(MAC_EXTRAS))?;
        }
        let items: Vec<_> = fs::read_dir(&staging)?.collect::<io::Result<_>>()?;
        if let [item] = &items[..] {
            let is_dir = item.file_type()?.is_dir();
            let to = free_path(dir, &item.file_name().to_string_lossy(), is_dir)?;
            fs::rename(item.path(), &to)?;
            fs::remove_dir(&staging)?;
            Ok(to)
        } else {
            let to = free_path(dir, stem, true)?;
            fs::rename(&staging, &to)?;
            Ok(to)
        }
    })();
    if unpacked.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    unpacked
}

/// ".name.extracting", made new so two extractions never share one.
fn make_staging(dir: &Path, name: &str) -> Result<PathBuf> {
    for n in 1..NAMES {
        let staging = dir.join(numbered(&format!(".{name}.extracting"), n, true));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("cannot make {}", staging.display()));
            }
        }
    }
    bail!("no free folder to extract {name} into")
}

/// The first of `name`, "name 2" and so on that nothing is at.
fn free_path(dir: &Path, name: &str, is_dir: bool) -> Result<PathBuf> {
    (1..NAMES)
        .map(|n| dir.join(numbered(name, n, is_dir)))
        .find(|p| fs::symlink_metadata(p).is_err())
        .with_context(|| format!("no free name for {name} in {}", dir.display()))
}

/// A new file at the first free name `name_for(1)`, `name_for(2)`…, made
/// only if nothing is there, so two at once never write into one file.
fn create_free(dir: &Path, name_for: impl Fn(u32) -> String) -> Result<(PathBuf, File)> {
    for n in 1..NAMES {
        let candidate = dir.join(name_for(n));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("cannot write {}", candidate.display()));
            }
        }
    }
    bail!("no free name for {} in {}", name_for(1), dir.display())
}

/// "name", then as Finder numbers: "name 2" for a whole name (a folder),
/// "reads 2.fastq" for a file.
fn numbered(name: &str, n: u32, whole: bool) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rfind('.').filter(|&dot| dot > 0 && !whole) {
        Some(dot) => format!("{} {n}{}", &name[..dot], &name[dot..]),
        None => format!("{name} {n}"),
    }
}

fn write_gzip(
    path: &Path,
    name: &str,
    meta: &fs::Metadata,
    out: BufWriter<File>,
    progress: &Arc<Progress>,
) -> Result<()> {
    let mut gz = GzBuilder::new()
        .filename(name.as_bytes())
        .mtime(u32::try_from(meta.mtime()).unwrap_or(0))
        .write(out, Compression::default());
    io::copy(&mut counted(path, progress)?, &mut gz)?;
    finish(gz.finish()?)
}

/// BGZF blocks, compressed on several threads, ending with bgzip's empty
/// end-of-file block.
fn write_bgzip(path: &Path, out: BufWriter<File>, progress: &Arc<Progress>) -> Result<()> {
    let threads = std::thread::available_parallelism()
        .map_or(1, NonZero::get)
        .min(BGZIP_THREADS);
    let mut writer = bgzf::io::MultithreadedWriter::with_worker_count(
        NonZero::new(threads).unwrap_or(NonZero::<usize>::MIN),
        out,
    );
    let copied = io::copy(&mut counted(path, progress)?, &mut writer);
    // Finished either way, so its threads end.
    let out = writer.finish()?;
    copied?;
    finish(out)
}

fn write_tar_gz(
    path: &Path,
    name: &str,
    out: BufWriter<File>,
    progress: &Arc<Progress>,
) -> Result<()> {
    let gz = GzBuilder::new().write(out, Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    add_to_tar(&mut tar, path, name, progress)?;
    finish(tar.into_inner()?.finish()?)
}

/// Adds a file, link or folder (and everything in it) under `entry`,
/// reading files through the task's count.
fn add_to_tar<W: Write>(
    tar: &mut tar::Builder<W>,
    path: &Path,
    entry: &str,
    progress: &Arc<Progress>,
) -> Result<()> {
    progress.check()?;
    let meta = fs::symlink_metadata(path)?;
    if meta.is_file() {
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&meta);
        tar.append_data(&mut header, entry, counted(path, progress)?)?;
    } else if meta.is_dir() {
        tar.append_dir(entry, path)?;
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let child_entry = format!("{entry}/{}", child.file_name().to_string_lossy());
            add_to_tar(tar, &child.path(), &child_entry, progress)?;
        }
    } else {
        // Links and the like, as they are.
        tar.append_path_with_name(path, entry)?;
    }
    Ok(())
}

fn write_zip(
    path: &Path,
    name: &str,
    meta: &fs::Metadata,
    out: BufWriter<File>,
    progress: &Arc<Progress>,
) -> Result<()> {
    let mut zip = ZipWriter::new(out);
    add_to_zip(&mut zip, path, name, meta, progress)?;
    finish(zip.finish()?)
}

/// Adds a file, link or folder (and everything in it) under `entry`.
fn add_to_zip(
    zip: &mut ZipWriter<BufWriter<File>>,
    path: &Path,
    entry: &str,
    meta: &fs::Metadata,
    progress: &Arc<Progress>,
) -> Result<()> {
    progress.check()?;
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(meta.mode() & 0o7777)
        .last_modified_time(zip_time(meta.mtime()));
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        zip.add_symlink(entry, target.to_string_lossy(), options)?;
    } else if meta.is_dir() {
        zip.add_directory(format!("{entry}/"), options)?;
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let child_meta = fs::symlink_metadata(child.path())?;
            let child_entry = format!("{entry}/{}", child.file_name().to_string_lossy());
            add_to_zip(zip, &child.path(), &child_entry, &child_meta, progress)?;
        }
    } else {
        zip.start_file(entry, options.large_file(meta.len() >= ZIP64_AT))?;
        io::copy(&mut counted(path, progress)?, zip)?;
    }
    Ok(())
}

/// A file's modification time as zip stores it: local time, from 1980.
fn zip_time(mtime: i64) -> DateTime {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let time = mtime as libc::time_t;
    // SAFETY: both pointers are valid for the call; localtime_r is the
    // thread-safe form.
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return DateTime::default();
    }
    DateTime::from_date_and_time(
        u16::try_from(tm.tm_year + 1900).unwrap_or(1980),
        tm.tm_mon as u8 + 1,
        tm.tm_mday as u8,
        tm.tm_hour as u8,
        tm.tm_min as u8,
        tm.tm_sec.min(59) as u8,
    )
    .unwrap_or_default()
}

/// Writes out what's buffered and makes sure it reached the disk.
fn finish(mut out: BufWriter<File>) -> Result<()> {
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Kind, archive_kind};

    #[test]
    fn archives_are_known_by_their_whole_extension() {
        assert_eq!(
            archive_kind("results.tar.gz"),
            Some((Kind::TarGz, "results"))
        );
        assert_eq!(archive_kind("Results.TGZ"), Some((Kind::TarGz, "Results")));
        assert_eq!(
            archive_kind("reads.fastq.gz"),
            Some((Kind::Gzip, "reads.fastq"))
        );
        assert_eq!(
            archive_kind("calls.vcf.bgz"),
            Some((Kind::Gzip, "calls.vcf"))
        );
        assert_eq!(archive_kind("data.csv.zip"), Some((Kind::Zip, "data.csv")));
        assert_eq!(archive_kind(".gz"), None);
        assert_eq!(archive_kind("notes.txt"), None);
    }
}
