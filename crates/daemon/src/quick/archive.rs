//! List and copy only regular, selected table files from an Explore archive.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::MultiGzDecoder;
use sandhill_proto::{
    QuickArchiveCounts, QuickArchiveMember, QuickArchiveSelection, quick_archive_stem,
    quick_table_stem,
};
use zip::ZipArchive;

use crate::archives::{create_new, free_path_in};
use crate::files::resolve;

/// The original name is for display and matching only. Only a safe basename
/// is ever joined to an extraction destination.
fn table_name(member: &str) -> Option<&str> {
    if member.starts_with('/') || member.contains(['\\', ':']) {
        return None;
    }
    let mut parts = member.split('/');
    let name = parts.next_back()?;
    if name.starts_with("._") || parts.any(|p| p == ".." || p.eq_ignore_ascii_case("__MACOSX")) {
        return None;
    }
    quick_table_stem(name).map(|_| name)
}

fn visit(
    path: &str,
    wanted: impl Fn(usize) -> bool,
    mut table: impl FnMut(usize, &str, &mut dyn Read) -> Result<()>,
) -> Result<()> {
    let path = resolve(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("This archive has no name.")?;
    ensure!(
        quick_archive_stem(name).is_some(),
        "Choose a ZIP or TAR archive."
    );
    ensure!(path.is_file(), "{} isn't an archive file.", path.display());
    let file = BufReader::new(File::open(&path)?);
    let suffix = name.rsplit('.').next().unwrap_or_default();
    if suffix.eq_ignore_ascii_case("zip") {
        let mut archive = ZipArchive::new(file)?;
        for index in 0..archive.len() {
            if !wanted(index) {
                continue;
            }
            let Some(name) = archive.name_for_index(index).transpose()? else {
                continue;
            };
            if table_name(&name).is_none() {
                continue;
            }
            let name = name.into_owned();
            let mut entry = archive.by_index(index)?;
            if !entry.is_file()
                || entry
                    .unix_mode()
                    .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
            {
                continue;
            }
            table(index, &name, &mut entry)?;
        }
    } else if suffix.eq_ignore_ascii_case("tar") {
        let mut archive = tar::Archive::new(file);
        visit_tar(archive.entries_with_seek()?, wanted, table)?;
    } else {
        let mut archive = tar::Archive::new(MultiGzDecoder::new(file));
        visit_tar(archive.entries()?, wanted, table)?;
    }
    Ok(())
}

fn visit_tar<R: Read>(
    entries: tar::Entries<'_, R>,
    wanted: impl Fn(usize) -> bool,
    mut table: impl FnMut(usize, &str, &mut dyn Read) -> Result<()>,
) -> Result<()> {
    for (index, entry) in entries.enumerate() {
        let mut entry = entry?;
        if !wanted(index) || !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path_bytes().into_owned();
        if let Ok(name) = std::str::from_utf8(&name)
            && table_name(name).is_some()
        {
            table(index, name, &mut entry)?;
        }
    }
    Ok(())
}

pub(crate) fn list(path: &str) -> Result<Vec<QuickArchiveMember>> {
    let mut members = Vec::new();
    visit(
        path,
        |_| true,
        |index, name, data| {
            members.push(QuickArchiveMember {
                index,
                name: name.to_owned(),
                sample_counts: looks_like_counts(name, data),
            });
            Ok(())
        },
    )?;
    Ok(members)
}

/// A bounded preview for the picker, not validation. R checks every row
/// before it writes the combined file.
fn looks_like_counts(name: &str, data: &mut dyn Read) -> bool {
    let lower = name.to_ascii_lowercase();
    let (name, gzip) = lower
        .strip_suffix(".gz")
        .map_or((lower.as_str(), false), |s| (s, true));
    if name.ends_with(".xls") || name.ends_with(".xlsx") {
        return false;
    }
    let mut gzip_reader;
    let input: &mut dyn Read = if gzip {
        gzip_reader = MultiGzDecoder::new(data);
        &mut gzip_reader
    } else {
        data
    };
    let mut reader = BufReader::new(input.take(16 * 1024));
    let mut line = String::new();
    let mut rows = Vec::new();
    while rows.len() < 8 {
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.trim().is_empty() => {}
            Ok(_) => rows.push(line.trim().trim_start_matches('\u{feff}').to_owned()),
            Err(_) => return false,
        }
        line.clear();
    }
    let Some(first) = rows.first() else {
        return false;
    };
    let separator = if name.ends_with(".csv") {
        Some(',')
    } else if name.ends_with(".tsv") || name.ends_with(".tab") || first.contains('\t') {
        Some('\t')
    } else if first.contains(',') {
        Some(',')
    } else {
        None
    };
    let Some(columns) = rows
        .iter()
        .map(|row| two_fields(row, separator))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    (0..2).any(|i| {
        let is_row = |row: &[&str; 2]| {
            !row[1 - i].is_empty()
                && row[i]
                    .parse::<f64>()
                    .is_ok_and(|v| v.is_finite() && v >= 0. && v.fract() == 0.)
        };
        columns.iter().all(is_row)
            || (columns.len() > 1
                && (matches!(
                    columns[0][i].to_ascii_lowercase().as_str(),
                    "count" | "counts" | "reads" | "raw_count" | "raw_counts"
                ) || matches!(
                    columns[0][1 - i].to_ascii_lowercase().as_str(),
                    "gene" | "gene_id" | "geneid" | "id" | "symbol" | "ensembl" | "entrez"
                ))
                && columns[1..].iter().all(is_row))
    })
}

fn two_fields(row: &str, separator: Option<char>) -> Option<[&str; 2]> {
    let (a, b) = if let Some(separator) = separator {
        let mut split = None;
        let mut quoted = false;
        for (i, c) in row.char_indices() {
            if c == '"' {
                quoted = !quoted;
            } else if c == separator && !quoted && split.replace(i).is_some() {
                return None;
            }
        }
        if quoted {
            return None;
        }
        let sep = split?;
        (&row[..sep], &row[sep + 1..])
    } else {
        let mut fields = row.split_whitespace();
        let pair = (fields.next()?, fields.next()?);
        if fields.next().is_some() {
            return None;
        }
        pair
    };
    Some([a.trim().trim_matches('"'), b.trim().trim_matches('"')])
}

#[derive(Default)]
struct Copies {
    paths: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

impl Copies {
    fn copy(&mut self, data: &mut dyn Read, path: PathBuf) -> Result<()> {
        let mut file =
            create_new(&path).with_context(|| format!("Couldn't create {}.", path.display()))?;
        self.paths.push(path);
        io::copy(data, &mut file)?;
        Ok(())
    }

    fn rollback(self) {
        for path in self.paths {
            let _ = fs::remove_file(path);
        }
        for dir in self.dirs.into_iter().rev() {
            let _ = fs::remove_dir(dir);
        }
    }
}

/// Copies only the chosen entries and rolls them back on any extraction
/// failure. Inputs to a combined table stay in a private folder for steps.R.
pub(super) fn extract(
    folder: &Path,
    mirror: Option<&Path>,
    archives: &[QuickArchiveSelection],
    combine: bool,
) -> Result<(Vec<String>, Option<QuickArchiveCounts>)> {
    let count: usize = archives.iter().map(|a| a.members.len()).sum();
    ensure!(count > 0, "Choose at least one table from the archive.");
    ensure!(
        !combine || count > 1,
        "Choose at least two samples to combine."
    );
    let mut copied = Copies::default();
    let extracted = (|| {
        let input_dir = if combine {
            format!(".sandhill/counts-{}", uuid::Uuid::new_v4().simple())
        } else {
            String::new()
        };
        let dest = folder.join(&input_dir);
        let mirror_dest = mirror.map(|m| m.join(&input_dir));
        if combine {
            for dir in std::iter::once(&dest).chain(mirror_dest.iter()) {
                ensure!(
                    dir.parent()
                        .is_some_and(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())),
                    "The session's storage folder is missing or is a link."
                );
                fs::create_dir(dir)?;
                copied.dirs.push(dir.clone());
            }
        }
        let mut files = Vec::with_capacity(count);
        for source in archives {
            let wanted: HashMap<_, _> = source.members.iter().map(|m| (m.index, &m.name)).collect();
            ensure!(
                wanted.len() == source.members.len(),
                "An archive table was selected twice."
            );
            if wanted.is_empty() {
                continue;
            }
            let started = files.len();
            visit(
                &source.path,
                |index| wanted.contains_key(&index),
                |index, name, data| {
                    ensure!(
                        wanted[&index] == name,
                        "The archive changed. Open it again to choose its tables."
                    );
                    let filename = table_name(name).context("This archive entry isn't a table.")?;
                    let path = free_path_in(&dest, mirror_dest.as_deref(), filename, false)?;
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .context("This table has no name.")?;
                    let relative = Path::new(&input_dir)
                        .join(name)
                        .to_string_lossy()
                        .into_owned();
                    let mirror_path = mirror_dest.as_ref().map(|m| m.join(name));
                    copied.copy(data, path.clone())?;
                    if let Some(mirror_path) = mirror_path {
                        copied.copy(&mut File::open(&path)?, mirror_path)?;
                    }
                    files.push(relative);
                    Ok(())
                },
            )?;
            if files.len() - started != wanted.len() {
                bail!("The archive changed. Open it again to choose its tables.");
            }
        }
        if combine {
            let mirror_results = mirror.map(|m| m.join("results"));
            let output = free_path_in(
                &folder.join("results"),
                mirror_results.as_deref(),
                "combined_counts.csv",
                false,
            )?;
            let file = output.strip_prefix(folder)?.to_string_lossy().into_owned();
            Ok((Vec::new(), Some(QuickArchiveCounts { files, file })))
        } else {
            Ok((files, None))
        }
    })();
    if extracted.is_err() {
        copied.rollback();
    }
    extracted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn zip_filters_unsafe_members_and_extraction_never_replaces_a_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("tables.zip");
        let mut zip = zip::ZipWriter::new(File::create(&source).unwrap());
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("../escape.csv", "not a table"),
            ("before/../../escape.csv", "not a table"),
            ("/escape.csv", "not a table"),
            ("C:\\escape.csv", "not a table"),
            ("__MACOSX/ignore.csv", "not a table"),
            ("folder/._data.csv", "not a table"),
            ("first/data.csv", "gene,count\na,1\n"),
            ("second/data.csv", "gene,count\nb,2\n"),
            ("unticked.tsv", "gene\tcount\nc\t3\n"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.add_symlink("link.csv", "../../outside.csv", options)
            .unwrap();
        zip.finish().unwrap();
        let path = source.to_string_lossy().into_owned();
        let mut members = list(&path).unwrap();
        assert_eq!(
            members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["first/data.csv", "second/data.csv", "unticked.tsv"]
        );
        assert!(members.iter().all(|m| m.sample_counts));
        members.pop();
        let session = root.path().join("session");
        let kept = root.path().join("kept");
        fs::create_dir(&session).unwrap();
        fs::create_dir(&kept).unwrap();
        fs::write(session.join("data.csv"), "keep").unwrap();
        fs::write(kept.join("data 2.csv"), "keep").unwrap();
        let pick = QuickArchiveSelection { path, members };
        let (files, _) =
            extract(&session, Some(&kept), std::slice::from_ref(&pick), false).unwrap();
        assert_eq!(files, ["data 3.csv", "data 4.csv"]);
        assert_eq!(
            fs::read_to_string(session.join(&files[0])).unwrap(),
            "gene,count\na,1\n"
        );
        assert_eq!(
            fs::read_to_string(kept.join(&files[1])).unwrap(),
            "gene,count\nb,2\n"
        );
        assert_eq!(
            fs::read_to_string(session.join("data.csv")).unwrap(),
            "keep"
        );
        assert!(!session.join("unticked.tsv").exists());
        assert!(!root.path().join("escape.csv").exists());

        let mut stale = pick;
        stale.members[1].index = 0;
        assert!(extract(&session, None, &[stale], false).is_err());
        assert!(!session.join("data 2.csv").exists());
    }

    #[test]
    fn tar_and_gzip_tar_list_nested_gzipped_tables() {
        let root = tempfile::tempdir().unwrap();
        let mut member = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        member.write_all(b"ENSG0001\t10\nENSG0002\t12\n").unwrap();
        let member = member.finish().unwrap();
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(member.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "GEO/GSM123_counts.txt.gz", member.as_slice())
            .unwrap();
        let bytes = tar.into_inner().unwrap();
        let tar_path = root.path().join("samples.tar");
        fs::write(&tar_path, &bytes).unwrap();
        let gz_path = root.path().join("samples.tgz");
        let mut gzip = flate2::write::GzEncoder::new(
            File::create(&gz_path).unwrap(),
            flate2::Compression::fast(),
        );
        gzip.write_all(&bytes).unwrap();
        gzip.finish().unwrap();
        for (i, path) in [tar_path, gz_path].iter().enumerate() {
            let path = path.to_string_lossy().into_owned();
            let members = list(&path).unwrap();
            assert_eq!(members.len(), 1);
            assert!(members[0].sample_counts);
            let session = root.path().join(format!("session{i}"));
            fs::create_dir(&session).unwrap();
            fs::write(session.join("GSM123_counts.txt.gz"), b"original").unwrap();
            let (files, _) = extract(
                &session,
                None,
                &[QuickArchiveSelection { path, members }],
                false,
            )
            .unwrap();
            assert_eq!(files, ["GSM123_counts 2.txt.gz"]);
            assert_eq!(fs::read(session.join(&files[0])).unwrap(), member);
        }
    }
}
