//! Archives for the file tree, as Finder's Compress and Archive Utility do
//! it. Packing a file or folder beside itself: a `.zip`; gzip (`.gz` for a
//! file, `.tar.gz` for a folder, since gzip holds one file); or bgzip, the
//! blocked `.gz` that tabix and samtools index. Unpacking `.zip`,
//! `.tar.gz`/`.tgz` and `.gz`/`.bgz` (plain or bgzip) beside the archive:
//! a single item comes out as itself, several go into a folder named after
//! the archive. The original always stays and nothing is replaced: a taken
//! name is numbered ("results 2"). Links inside folders stay links.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, ErrorKind, Write};
use std::num::NonZero;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::MultiGzDecoder;
use flate2::{Compression, GzBuilder};
use noodles_bgzf as bgzf;
use sandhill_proto::{ArchiveFormat, FileWritten};
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

pub fn compress(path: &str, format: ArchiveFormat) -> Result<FileWritten> {
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
    let (target, file) = create_free(dir, |n| format!("{}{ext}", numbered(&name, n, true)))?;
    let out = BufWriter::with_capacity(BUFFER, file);
    let packed = match (format, meta.is_dir()) {
        (ArchiveFormat::Zip, _) => write_zip(&path, &name, &meta, out),
        (ArchiveFormat::Gzip, false) => write_gzip(&path, &name, &meta, out),
        (ArchiveFormat::Gzip, true) => write_tar_gz(&path, &name, out),
        (ArchiveFormat::Bgzip, _) => write_bgzip(&path, out),
    };
    if let Err(err) = packed {
        let _ = fs::remove_file(&target);
        return Err(err.context(format!("cannot compress {}", path.display())));
    }
    written(&target)
}

/// Unpacks an archive beside itself. Returns what came out: the one file
/// or folder inside, or the folder holding several.
pub fn extract(path: &str) -> Result<FileWritten> {
    let path = resolve(path)?;
    let (Some(name), Some(dir)) = (path.file_name(), path.parent()) else {
        bail!("cannot extract {}", path.display());
    };
    let name = name.to_string_lossy().into_owned();
    let Some((kind, stem)) = archive_kind(&name) else {
        bail!("{name} is not a .zip, .tar.gz, .tgz, .gz or .bgz file");
    };
    let out = match kind {
        Kind::Gzip => decompress(&path, dir, stem),
        Kind::Zip | Kind::TarGz => unpack(&path, dir, &name, stem, kind),
    }
    .with_context(|| format!("cannot extract {}", path.display()))?;
    written(&out)
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
fn decompress(path: &Path, dir: &Path, stem: &str) -> Result<PathBuf> {
    let (target, file) = create_free(dir, |n| numbered(stem, n, false))?;
    let copied = (|| {
        let mut input = MultiGzDecoder::new(BufReader::with_capacity(BUFFER, File::open(path)?));
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
fn unpack(path: &Path, dir: &Path, name: &str, stem: &str, kind: Kind) -> Result<PathBuf> {
    let staging = make_staging(dir, name)?;
    let unpacked = (|| {
        let input = BufReader::with_capacity(BUFFER, File::open(path)?);
        // Both refuse entries that would land outside the folder.
        if kind == Kind::Zip {
            ZipArchive::new(input)?.extract(&staging)?;
        } else {
            tar::Archive::new(MultiGzDecoder::new(input)).unpack(&staging)?;
        }
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

fn write_gzip(path: &Path, name: &str, meta: &fs::Metadata, out: BufWriter<File>) -> Result<()> {
    let mut gz = GzBuilder::new()
        .filename(name.as_bytes())
        .mtime(u32::try_from(meta.mtime()).unwrap_or(0))
        .write(out, Compression::default());
    let mut input = BufReader::with_capacity(BUFFER, File::open(path)?);
    io::copy(&mut input, &mut gz)?;
    finish(gz.finish()?)
}

/// BGZF blocks, compressed on several threads, ending with bgzip's empty
/// end-of-file block.
fn write_bgzip(path: &Path, out: BufWriter<File>) -> Result<()> {
    let threads = std::thread::available_parallelism()
        .map_or(1, NonZero::get)
        .min(BGZIP_THREADS);
    let mut writer = bgzf::io::MultithreadedWriter::with_worker_count(
        NonZero::new(threads).unwrap_or(NonZero::<usize>::MIN),
        out,
    );
    let mut input = BufReader::with_capacity(BUFFER, File::open(path)?);
    io::copy(&mut input, &mut writer)?;
    finish(writer.finish()?)
}

fn write_tar_gz(path: &Path, name: &str, out: BufWriter<File>) -> Result<()> {
    let gz = GzBuilder::new().write(out, Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.follow_symlinks(false);
    tar.append_dir_all(name, path)?;
    finish(tar.into_inner()?.finish()?)
}

fn write_zip(path: &Path, name: &str, meta: &fs::Metadata, out: BufWriter<File>) -> Result<()> {
    let mut zip = ZipWriter::new(out);
    add_to_zip(&mut zip, path, name, meta)?;
    finish(zip.finish()?)
}

/// Adds a file, link or folder (and everything in it) under `entry`.
fn add_to_zip(
    zip: &mut ZipWriter<BufWriter<File>>,
    path: &Path,
    entry: &str,
    meta: &fs::Metadata,
) -> Result<()> {
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
            add_to_zip(zip, &child.path(), &child_entry, &child_meta)?;
        }
    } else {
        zip.start_file(entry, options.large_file(meta.len() >= ZIP64_AT))?;
        let mut input = BufReader::with_capacity(BUFFER, File::open(path)?);
        io::copy(&mut input, zip)?;
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
