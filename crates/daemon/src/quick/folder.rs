//! A Quick Tools session's folder: `<first table's name> (Sandhill)`, made
//! where the user chose, with copies of the tables, `results/`, and the
//! What we know card in `.sandhill/quick.json`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sandhill_proto::QuickCard;

/// The table files Quick Tools reads.
pub const TABLE_EXTENSIONS: &[&str] = &["csv", "tsv", "tab", "txt", "xlsx", "xls"];

const CARD: &str = ".sandhill/quick.json";

fn table_path(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        bail!("{} isn't a full path", path.display());
    }
    if !path.is_file() {
        bail!("{} isn't a file", path.display());
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !TABLE_EXTENSIONS.contains(&extension.as_str()) {
        bail!(
            "{} isn't a table Quick Tools reads (CSV, TSV, text or Excel)",
            path.display()
        );
    }
    Ok(path)
}

/// Makes the session folder in `parent` and copies `tables` into it.
/// Returns the folder.
pub fn create(tables: &[String], parent: &str) -> Result<PathBuf> {
    let tables = tables
        .iter()
        .map(|t| table_path(t))
        .collect::<Result<Vec<_>>>()?;
    let Some(first) = tables.first() else {
        bail!("no table to open");
    };
    let parent = PathBuf::from(parent);
    if !parent.is_dir() {
        bail!("{} isn't a folder", parent.display());
    }
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "table".into());
    let folder = crate::archives::free_path(&parent, &format!("{stem} (Sandhill)"), true)?;
    if let Err(err) = fs::create_dir(&folder) {
        if matches!(
            err.kind(),
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem
        ) {
            bail!("Sandhill can't write to this folder. Choose another.");
        }
        return Err(err).with_context(|| format!("cannot make {}", folder.display()));
    }
    let made = (|| {
        fs::create_dir(folder.join("results"))?;
        fs::create_dir(folder.join(".sandhill"))?;
        copy_tables(&folder, &tables)?;
        save_card(&folder, &QuickCard::default())
    })();
    if let Err(err) = made {
        let _ = fs::remove_dir_all(&folder);
        return Err(err);
    }
    Ok(folder)
}

/// Copies more tables into the session `folder`. Returns their names there.
pub fn add_tables(folder: &Path, tables: &[String]) -> Result<Vec<String>> {
    let tables = tables
        .iter()
        .map(|t| table_path(t))
        .collect::<Result<Vec<_>>>()?;
    copy_tables(folder, &tables)
}

fn copy_tables(folder: &Path, tables: &[PathBuf]) -> Result<Vec<String>> {
    tables
        .iter()
        .map(|table| {
            let name = table
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .context("a table has no name")?;
            let to = crate::archives::free_path(folder, &name, false)?;
            fs::copy(table, &to)
                .with_context(|| format!("cannot copy {} into the session", table.display()))?;
            Ok(to
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(name))
        })
        .collect()
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

    #[test]
    fn a_session_folder_holds_copies_and_an_empty_card() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("de_results.csv");
        fs::write(&table, "gene,log2FoldChange\nA,1\n").unwrap();
        let tables = vec![table.to_string_lossy().into_owned()];
        let parent = dir.path().to_string_lossy().into_owned();

        let first = create(&tables, &parent).unwrap();
        assert_eq!(first, dir.path().join("de_results (Sandhill)"));
        assert!(first.join("de_results.csv").is_file());
        assert!(first.join("results").is_dir());
        assert_eq!(card(&first).unwrap(), QuickCard::default());
        assert!(table.is_file(), "the original stays");

        let second = create(&tables, &parent).unwrap();
        assert_eq!(second, dir.path().join("de_results (Sandhill) 2"));
        assert_eq!(
            add_tables(&second, &tables).unwrap(),
            vec!["de_results 2.csv".to_string()]
        );
    }

    #[test]
    fn only_tables_are_taken() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.pdf");
        fs::write(&notes, "x").unwrap();
        let err = create(
            &[notes.to_string_lossy().into_owned()],
            &dir.path().to_string_lossy(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("isn't a table"));
    }

    #[test]
    fn names_stay_inside_the_session_folder() {
        let folder = Path::new("/tmp/s");
        assert!(inside(folder, "results/plot.xlsx").is_ok());
        assert!(inside(folder, "../secret.csv").is_err());
        assert!(inside(folder, "/etc/passwd").is_err());
    }
}
