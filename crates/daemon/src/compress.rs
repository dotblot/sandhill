//! Packing a file or folder from the file tree into an archive beside it,
//! as Finder's Compress does: a `.zip`, or gzip (`.gz` for a file,
//! `.tar.gz` for a folder, since gzip holds one file). The original stays.
//! Links inside a folder are stored as links, not followed.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, ErrorKind, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::{Compression, GzBuilder};
use sandhill_proto::{ArchiveFormat, FileWritten};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::fileops::written;
use crate::files::resolve;

/// Big buffers: archives of sequencing data run to gigabytes.
const BUFFER: usize = 1 << 20;
/// Zip entries this big need zip64 sizes.
const ZIP64_AT: u64 = u32::MAX as u64;

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
        (ArchiveFormat::Gzip, false) => ".gz",
        (ArchiveFormat::Gzip, true) => ".tar.gz",
    };
    let (target, file) = create_free(dir, &name, ext)?;
    let out = BufWriter::with_capacity(BUFFER, file);
    let packed = match (format, meta.is_dir()) {
        (ArchiveFormat::Zip, _) => write_zip(&path, &name, &meta, out),
        (ArchiveFormat::Gzip, false) => write_gzip(&path, &name, &meta, out),
        (ArchiveFormat::Gzip, true) => write_tar_gz(&path, &name, out),
    };
    if let Err(err) = packed {
        let _ = fs::remove_file(&target);
        return Err(err.context(format!("cannot compress {}", path.display())));
    }
    written(&target)
}

/// "name.zip", then "name 2.zip", "name 3.zip", made only if nothing is
/// there, so two at once never write into the same file.
fn create_free(dir: &Path, name: &str, ext: &str) -> Result<(PathBuf, File)> {
    for n in 1..10_000 {
        let candidate = dir.join(archive_name(name, ext, n));
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
    bail!("no free name for {name}{ext} in {}", dir.display())
}

fn archive_name(name: &str, ext: &str, n: u32) -> String {
    if n == 1 {
        format!("{name}{ext}")
    } else {
        format!("{name} {n}{ext}")
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
