//! An Explore a Table session's folder. It starts in Sandhill's own storage
//! (`<state>/explore/<id>`), with links to the tables where they are,
//! `results/`, and the What we know card in `.sandhill/quick.json`: opening
//! a table makes nothing beside it (Huan, 2026-11-19). Only when the user
//! agrees is it kept, as a folder they name where they choose, with copies
//! of the tables; what the session writes afterwards is copied there too.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use sandhill_proto::{QuickArchiveOpened, QuickArchiveSelection, QuickCard, quick_table_stem};

use super::archive;

const CARD: &str = ".sandhill/quick.json";
/// Sessions left in Sandhill's storage this long without a change (the
/// app quit before asking) are removed.
const STALE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn table_path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        bail!("{} isn't a full path", path.display());
    }
    if !path.is_file() {
        bail!("{} isn't a file", path.display());
    }
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(quick_table_stem)
        .is_none()
    {
        bail!(
            "{} isn't a table Sandhill can explore (CSV, TSV, text or Excel)",
            path.display()
        );
    }
    Ok(path)
}

/// Starts a session in `explore` (Sandhill's storage) with links to
/// `tables`. Returns its folder.
pub fn start(explore: &Path, tables: &[String]) -> Result<PathBuf> {
    let (folder, _) = start_session(explore, tables, &[], false)?;
    Ok(folder)
}

pub(crate) fn open_archives(
    explore: &Path,
    folder: Option<PathBuf>,
    tables: &[String],
    archives: &[QuickArchiveSelection],
    combine: bool,
) -> Result<QuickArchiveOpened> {
    if let Some(folder) = folder {
        if !fs::symlink_metadata(folder.join(".sandhill")).is_ok_and(|m| m.is_dir())
            || !fs::symlink_metadata(folder.join(CARD)).is_ok_and(|m| m.is_file())
        {
            bail!("Open an Explore session to add tables from an archive.");
        }
        let tables = tables
            .iter()
            .map(|t| table_path(t))
            .collect::<Result<Vec<_>>>()?;
        let mut card = card(&folder)?;
        let mut opened = unpack_archives(&folder, &mut card, archives, combine)?;
        opened
            .files
            .extend(add_to_card(&folder, &tables, &mut card)?);
        save_card(&folder, &card)?;
        mirror(&folder)?;
        Ok(opened)
    } else {
        start_session(explore, tables, archives, combine).map(|(_, opened)| opened)
    }
}

fn start_session(
    explore: &Path,
    tables: &[String],
    archives: &[QuickArchiveSelection],
    combine: bool,
) -> Result<(PathBuf, QuickArchiveOpened)> {
    let tables = tables
        .iter()
        .map(|t| table_path(t))
        .collect::<Result<Vec<_>>>()?;
    if tables.is_empty() && archives.is_empty() {
        bail!("no table to open");
    }
    fs::create_dir_all(explore).context("cannot make Sandhill's explore folder")?;
    prune(explore);
    let folder = explore.join(uuid::Uuid::new_v4().simple().to_string());
    let made = (|| {
        fs::create_dir(&folder)?;
        fs::create_dir(folder.join("results"))?;
        fs::create_dir(folder.join(".sandhill"))?;
        let mut card = QuickCard {
            scratch: true,
            ..QuickCard::default()
        };
        let files = add_to_card(&folder, &tables, &mut card)?;
        let mut opened = if archives.is_empty() {
            QuickArchiveOpened {
                folder: folder.to_string_lossy().into_owned(),
                files: Vec::new(),
                combine: None,
            }
        } else {
            unpack_archives(&folder, &mut card, archives, combine)?
        };
        opened.files.extend(files);
        save_card(&folder, &card)?;
        Ok(opened)
    })();
    match made {
        Ok(opened) => Ok((folder, opened)),
        Err(err) => {
            let _ = fs::remove_dir_all(&folder);
            Err(err)
        }
    }
}

fn unpack_archives(
    folder: &Path,
    card: &mut QuickCard,
    archives: &[QuickArchiveSelection],
    combine: bool,
) -> Result<QuickArchiveOpened> {
    let kept = card
        .kept
        .as_deref()
        .map(Path::new)
        .filter(|path| card.scratch && path.is_dir());
    let (files, combine) = archive::extract(folder, kept, archives, combine)?;
    card.sources.extend(
        archives
            .iter()
            .filter(|a| !a.members.is_empty())
            .map(|a| a.path.clone()),
    );
    Ok(QuickArchiveOpened {
        folder: folder.to_string_lossy().into_owned(),
        files,
        combine,
    })
}

/// Removes sessions in `explore` untouched for [`STALE`].
fn prune(explore: &Path) {
    let Ok(entries) = fs::read_dir(explore) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let card = entry.path().join(CARD);
        let touched = fs::metadata(&card)
            .or_else(|_| entry.metadata())
            .and_then(|m| m.modified());
        if touched.is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > STALE)) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Adds more tables to `folder`: linked in a session still in Sandhill's
/// storage, copied into a kept one. Returns their names there.
pub fn add_tables(folder: &Path, tables: &[String]) -> Result<Vec<String>> {
    let tables = tables
        .iter()
        .map(|t| table_path(t))
        .collect::<Result<Vec<_>>>()?;
    let mut card = card(folder)?;
    let names = add_to_card(folder, &tables, &mut card)?;
    save_card(folder, &card)?;
    mirror(folder)?;
    Ok(names)
}

fn add_to_card(folder: &Path, tables: &[PathBuf], card: &mut QuickCard) -> Result<Vec<String>> {
    let names = if card.scratch {
        link_tables(folder, tables)?
    } else {
        copy_tables(folder, tables)?
    };
    card.sources
        .extend(tables.iter().map(|t| t.to_string_lossy().into_owned()));
    if let Some(kept) = card.kept.clone().filter(|_| card.scratch) {
        copy_tables(Path::new(&kept), tables)?;
    }
    Ok(names)
}

fn free_name(folder: &Path, table: &Path) -> Result<PathBuf> {
    let name = table
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("a table has no name")?;
    crate::archives::free_path(folder, &name, false)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn link_tables(folder: &Path, tables: &[PathBuf]) -> Result<Vec<String>> {
    tables
        .iter()
        .map(|table| {
            let to = free_name(folder, table)?;
            std::os::unix::fs::symlink(table, &to)
                .with_context(|| format!("cannot open {}", table.display()))?;
            Ok(file_name(&to))
        })
        .collect()
}

fn copy_tables(folder: &Path, tables: &[PathBuf]) -> Result<Vec<String>> {
    tables
        .iter()
        .map(|table| {
            let to = free_name(folder, table)?;
            fs::copy(table, &to)
                .with_context(|| format!("cannot copy {} into the session", table.display()))?;
            Ok(file_name(&to))
        })
        .collect()
}

/// Keeps the session in `scratch` as `parent/name`: the tables copied, and
/// its results, steps and card. Returns the new folder.
pub fn keep(scratch: &Path, parent: &str, name: &str) -> Result<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains('/') || name.contains(':') {
        bail!("Choose a folder name without / or : that doesn't start with a dot.");
    }
    let parent = PathBuf::from(parent);
    if !parent.is_dir() {
        bail!("{} isn't a folder", parent.display());
    }
    let mut card = card(scratch)?;
    if !card.scratch {
        bail!("this session is already a folder of its own");
    }
    let dest = parent.join(name);
    if fs::symlink_metadata(&dest).is_ok() {
        bail!("A folder named “{name}” is already there. Choose another name.");
    }
    if let Err(err) = fs::create_dir(&dest) {
        if matches!(
            err.kind(),
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem
        ) {
            bail!("Sandhill can't write to this folder. Choose another.");
        }
        return Err(err).with_context(|| format!("cannot make {}", dest.display()));
    }
    let copied = (|| {
        for entry in fs::read_dir(scratch)?.flatten() {
            let from = entry.path();
            let to = dest.join(entry.file_name());
            if from.is_dir() {
                copy_dir(&from, &to)?;
            } else {
                // A link to a table becomes a copy of it.
                fs::copy(&from, &to).with_context(|| format!("cannot copy {}", from.display()))?;
            }
        }
        save_card(
            &dest,
            &QuickCard {
                scratch: false,
                kept: None,
                ..card.clone()
            },
        )
    })();
    if let Err(err) = copied {
        let _ = fs::remove_dir_all(&dest);
        return Err(err);
    }
    card.kept = Some(dest.to_string_lossy().into_owned());
    save_card(scratch, &card)?;
    Ok(dest)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            fs::copy(&path, &target).with_context(|| format!("cannot copy {}", path.display()))?;
        }
    }
    Ok(())
}

/// A kept session's writes, copied from Sandhill's storage to where it was
/// kept: `results/`, `steps.R` and the card. Nothing when not kept.
pub fn mirror(scratch: &Path) -> Result<()> {
    let card = card(scratch)?;
    let Some(kept) = card.kept.clone().filter(|_| card.scratch) else {
        return Ok(());
    };
    let kept = PathBuf::from(kept);
    if !kept.is_dir() {
        return Ok(());
    }
    copy_dir(&scratch.join("results"), &kept.join("results"))?;
    for file in ["steps.R", ".sandhill/quick.R"] {
        let from = scratch.join(file);
        if from.is_file() {
            if let Some(dir) = kept.join(file).parent() {
                fs::create_dir_all(dir)?;
            }
            fs::copy(&from, kept.join(file))?;
        }
    }
    save_card(
        &kept,
        &QuickCard {
            scratch: false,
            kept: None,
            ..card
        },
    )
}

/// Deletes a session in `explore` that wasn't kept (or was, and is done).
pub fn discard(explore: &Path, folder: &Path) -> Result<()> {
    let explore = fs::canonicalize(explore).context("no explore folder")?;
    let folder = fs::canonicalize(folder).context("no such session")?;
    if folder.parent() != Some(explore.as_path()) {
        bail!("only sessions in Sandhill's own storage can be discarded");
    }
    fs::remove_dir_all(&folder).context("cannot remove the session")
}

/// The card, or an empty one when there is none yet.
pub fn card(folder: &Path) -> Result<QuickCard> {
    match fs::read(folder.join(CARD)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} can't be read", folder.join(CARD).display())),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(QuickCard::default()),
        Err(err) => Err(err).context("cannot read the card"),
    }
}

pub fn save_card(folder: &Path, card: &QuickCard) -> Result<()> {
    let path = folder.join(CARD);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(card)?)?;
    fs::rename(&tmp, &path).context("cannot save the card")
}

/// `name` inside `folder`, refusing anything that reaches outside it.
pub fn inside(folder: &Path, name: &str) -> Result<PathBuf> {
    let relative = Path::new(name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("{name} isn't a file in the session folder");
    }
    Ok(folder.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(dir: &Path, name: &str) -> String {
        let path = dir.join(name);
        fs::write(&path, "gene,log2FoldChange\nA,1\n").unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn opening_a_table_makes_nothing_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let de = table(&downloads, "de_results.csv");
        let explore = dir.path().join("state/explore");

        let session = start(&explore, std::slice::from_ref(&de)).unwrap();
        assert_eq!(session.parent(), Some(explore.as_path()));
        assert_eq!(
            fs::read_dir(&downloads).unwrap().count(),
            1,
            "only the table is in Downloads"
        );
        let link = session.join("de_results.csv");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let card = card(&session).unwrap();
        assert!(card.scratch);
        assert_eq!(card.sources, vec![de.clone()]);

        // Not kept: gone without a trace.
        discard(&explore, &session).unwrap();
        assert!(!session.exists());
        assert!(Path::new(&de).is_file(), "the table stays");
    }

    #[test]
    fn keeping_copies_the_session_where_and_as_the_user_says() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        fs::create_dir(&downloads).unwrap();
        let de = table(&downloads, "de_results.csv");
        let explore = dir.path().join("explore");
        let session = start(&explore, std::slice::from_ref(&de)).unwrap();
        fs::write(session.join("steps.R"), "# steps\n").unwrap();
        let parent = downloads.to_string_lossy().into_owned();

        assert!(keep(&session, &parent, "../x").is_err());
        let kept = keep(&session, &parent, "My results").unwrap();
        assert_eq!(kept, downloads.join("My results"));
        let copy = kept.join("de_results.csv");
        assert!(
            !fs::symlink_metadata(&copy)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&copy).unwrap(),
            fs::read_to_string(&de).unwrap()
        );
        assert!(kept.join("steps.R").is_file());
        assert!(!card(&kept).unwrap().scratch);
        assert_eq!(
            card(&session).unwrap().kept,
            Some(kept.to_string_lossy().into_owned())
        );
        let again = keep(&session, &parent, "My results").unwrap_err();
        assert!(again.to_string().contains("already"));

        // What the session writes afterwards follows it there.
        fs::write(session.join("results/table.xlsx"), "x").unwrap();
        mirror(&session).unwrap();
        assert!(kept.join("results/table.xlsx").is_file());
        assert_eq!(
            add_tables(&session, &[table(&downloads, "counts.csv")]).unwrap(),
            vec!["counts.csv".to_string()]
        );
        assert!(kept.join("counts.csv").is_file());
    }

    #[test]
    fn only_tables_are_taken_and_only_sessions_are_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.pdf");
        fs::write(&notes, "x").unwrap();
        let explore = dir.path().join("explore");
        let err = start(&explore, &[notes.to_string_lossy().into_owned()]).unwrap_err();
        assert!(err.to_string().contains("isn't a table"));
        fs::create_dir_all(&explore).unwrap();
        assert!(discard(&explore, dir.path()).is_err());
    }

    #[test]
    fn names_stay_inside_the_session_folder() {
        let folder = Path::new("/tmp/s");
        assert!(inside(folder, "results/plot.xlsx").is_ok());
        assert!(inside(folder, "../secret.csv").is_err());
        assert!(inside(folder, "/etc/passwd").is_err());
    }
}
