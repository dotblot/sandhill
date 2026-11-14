//! Trusting a project folder (Huan, 2026-11-12; `docs/design/folder-trust-plan.md`).
//!
//! Starting R, Python or an agent acts on files in the folder it starts in
//! (`.Rprofile`, env files, `CLAUDE.md`, hooks, a repository's own Git
//! configuration), so a folder opened as a project starts nothing until the
//! user trusts it. Trust is kept in the daemon's journal, on the machine that
//! holds the folder, and never in the folder, whose author could then grant
//! it. A trusted folder's subfolders are trusted with it.
//!
//! Only folders opened as projects are held back: a path outside every
//! project works as it always did (the Console in the home folder, say).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use sandhill_proto::TrustedFolder;

use crate::journal::Journal;

/// The environment variable an agent's run gets, naming the trusted folder
/// it starts in, for the agents that can be told per run (Codex, pi).
pub const TRUSTED_FOLDER: &str = "SANDHILL_TRUSTED_FOLDER";

/// The form paths are compared in: with links resolved, so `/tmp/p` and
/// `/private/tmp/p` are one folder. A missing folder keeps its given path.
fn real(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

fn trusted_in(path: &Path, trusted: &[(String, i64)]) -> bool {
    trusted
        .iter()
        .any(|(folder, _)| path.starts_with(real(folder)))
}

/// Whether `path` is a trusted folder or inside one.
pub fn is_trusted(journal: &Journal, path: &str) -> Result<bool> {
    Ok(trusted_in(&real(path), &journal.trusted_folders()?))
}

/// The trusted folder `path` is in, as trusted.
pub fn trusted_root(journal: &Journal, path: &Path) -> Result<Option<String>> {
    let path = real(&path.to_string_lossy());
    Ok(journal
        .trusted_folders()?
        .into_iter()
        .map(|(folder, _)| folder)
        .filter(|folder| path.starts_with(real(folder)))
        .max_by_key(String::len))
}

/// The untrusted project `path` is in, if it is in one.
pub fn restricted_root(journal: &Journal, path: &Path) -> Result<Option<String>> {
    let path = real(&path.to_string_lossy());
    if trusted_in(&path, &journal.trusted_folders()?) {
        return Ok(None);
    }
    // The innermost project names the folder to trust.
    Ok(journal
        .recent_projects()?
        .into_iter()
        .map(|(project, _)| project)
        .filter(|project| path.starts_with(real(project)))
        .max_by_key(String::len))
}

/// Refuses to start `what` in an untrusted project.
pub fn check(journal: &Journal, path: &Path, what: &str) -> Result<()> {
    if let Some(root) = restricted_root(journal, path)? {
        bail!("Restricted Mode: trust the folder {root} to {what}");
    }
    Ok(())
}

pub fn trust(journal: &Journal, path: &str) -> Result<()> {
    let path = super::normalize(path)?;
    if !Path::new(&path).is_dir() {
        bail!("{path} is not a folder");
    }
    Ok(journal.trust_folder(&real(&path).to_string_lossy())?)
}

/// Stops trusting `path`, as listed or as opened.
pub fn untrust(journal: &Journal, path: &str) -> Result<()> {
    let path = super::normalize(path)?;
    journal.untrust_folder(&path)?;
    Ok(journal.untrust_folder(&real(&path).to_string_lossy())?)
}

pub fn list(journal: &Journal) -> Result<Vec<TrustedFolder>> {
    Ok(journal
        .trusted_folders()?
        .into_iter()
        .map(|(path, trusted_ms)| TrustedFolder {
            exists: Path::new(&path).is_dir(),
            path,
            trusted_ms,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{check, is_trusted, restricted_root, trust, untrust};
    use crate::blobs::BlobStore;
    use crate::journal::Journal;

    fn open(db: &std::path::Path) -> Journal {
        Journal::open(db, BlobStore::new(db.with_extension("blobs"))).unwrap()
    }

    #[test]
    fn opened_projects_wait_for_trust_and_subfolders_follow() {
        let state = tempfile::tempdir().unwrap();
        let journal = open(&state.path().join("journal.sqlite"));
        let work = tempfile::tempdir().unwrap();
        let project = work.path().join("atlas");
        let inner = project.join("scripts");
        let other = work.path().join("other");
        for dir in [&inner, &other] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let path = project.to_string_lossy().into_owned();

        // Never opened as a project: nothing is held back.
        assert!(check(&journal, &inner, "start a kernel").is_ok());

        journal.open_project(&path).unwrap();
        assert!(!is_trusted(&journal, &path).unwrap());
        assert_eq!(
            restricted_root(&journal, &inner).unwrap().as_deref(),
            Some(path.as_str())
        );
        let refused = check(&journal, &inner, "start a kernel").unwrap_err();
        assert!(refused.to_string().starts_with("Restricted Mode:"));
        assert!(check(&journal, &other, "start a kernel").is_ok());

        trust(&journal, &path).unwrap();
        assert!(is_trusted(&journal, &inner.to_string_lossy()).unwrap());
        assert!(check(&journal, &inner, "start a kernel").is_ok());

        untrust(&journal, &path).unwrap();
        assert!(check(&journal, &project, "start a kernel").is_err());
    }

    #[test]
    fn projects_from_before_trust_start_trusted() {
        let state = tempfile::tempdir().unwrap();
        let db = state.path().join("journal.sqlite");
        let work = tempfile::tempdir().unwrap();
        let path = work.path().to_string_lossy().into_owned();
        {
            let journal = open(&db);
            journal.open_project(&path).unwrap();
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch("DROP TABLE trusted_folders; PRAGMA user_version = 5;")
                .unwrap();
        }
        let upgraded = open(&db);
        assert!(is_trusted(&upgraded, &path).unwrap());
    }
}
