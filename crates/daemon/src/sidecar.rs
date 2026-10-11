//! Saved outputs for Quarto and R Markdown files.
//!
//! A `.qmd` has no place for outputs, so they are kept next to it:
//!
//! ```text
//! analysis.qmd
//! .analysis.qmd.out/
//!   index.json      cell id -> code, status and nbformat outputs
//!   blobs/<sha256>  plots and other large values the outputs refer to
//! ```
//!
//! The daemon updates the sidecar whenever a run from such a file finishes,
//! so the outputs on disk are current even if no app was connected.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use sandhill_proto::BLOB_KEY;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::blobs::BlobStore;
use crate::journal::{CellRun, Journal};

pub const FORMAT_VERSION: u32 = 1;

/// Serializes read-modify-write cycles; runs from an R and a Python session
/// can finish at the same moment for the same file.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub sandhill_outputs: u32,
    pub document: String,
    pub cells: BTreeMap<String, SavedCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedCell {
    pub code: String,
    pub language: String,
    pub kernel: String,
    pub run: String,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u64>,
    pub outputs: Vec<Value>,
}

/// Whether outputs for `document` are kept in a sidecar.
pub fn applies_to(document: &Path) -> bool {
    document
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("qmd") || e.eq_ignore_ascii_case("rmd"))
}

pub fn dir_for(document: &Path) -> Option<PathBuf> {
    let name = document.file_name()?.to_string_lossy();
    Some(document.parent()?.join(format!(".{name}.out")))
}

/// Where the sidecar of `document` keeps the blob `hash`, if it is a valid hash.
pub fn blob_path(document: &Path, hash: &str) -> Option<PathBuf> {
    let valid = hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !valid {
        return None;
    }
    Some(dir_for(document)?.join("blobs").join(hash))
}

/// Writes the outputs of a finished run into its document's sidecar.
/// Does nothing for runs that didn't come from a Quarto or R Markdown cell.
pub fn record_run(journal: &Journal, run_id: &str) -> Result<()> {
    let Some(run) = journal.cell_run(run_id)? else {
        return Ok(());
    };
    let document = PathBuf::from(&run.cell_file);
    if !document.is_absolute() || !applies_to(&document) {
        return Ok(());
    }
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Writes for runs of one cell can finish out of order; only the cell's
    // most recent run belongs in the sidecar.
    let latest = journal
        .latest_cell_runs(&run.cell_file)?
        .into_iter()
        .any(|r| r.run_id == run.run_id);
    if !latest {
        return Ok(());
    }
    let outputs = crate::outputs::run_outputs(journal, run_id, i64::MAX)?;
    record(journal.blobs(), &document, &run, outputs)
}

fn record(store: &BlobStore, document: &Path, run: &CellRun, outputs: Vec<Value>) -> Result<()> {
    let dir = dir_for(document).context("document has no directory")?;
    let index_path = dir.join("index.json");
    let mut index = read_index(&index_path).unwrap_or_default();
    index.sandhill_outputs = FORMAT_VERSION;
    index.document = document
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // An unlabeled chunk is known by its position, so moving it changes its
    // id. Drop the entry it left behind at its old position.
    if is_positional(&run.cell_id) {
        index
            .cells
            .retain(|id, cell| id == &run.cell_id || !is_positional(id) || cell.code != run.code);
    }
    index.cells.insert(
        run.cell_id.clone(),
        SavedCell {
            code: run.code.clone(),
            language: run.language.clone(),
            kernel: run.kernel.clone(),
            run: run.run_id.clone(),
            started_ms: run.started_ms,
            finished_ms: run.finished_ms,
            status: run.status.clone(),
            execution_count: run.execution_count,
            outputs,
        },
    );

    let referenced: BTreeSet<String> = index
        .cells
        .values()
        .flat_map(|c| blob_refs(&c.outputs))
        .collect();
    let blobs_dir = dir.join("blobs");
    std::fs::create_dir_all(&blobs_dir)
        .with_context(|| format!("cannot create {}", blobs_dir.display()))?;
    for hash in &referenced {
        let target = blobs_dir.join(hash);
        if !target.exists()
            && let Ok(bytes) = store.get(hash)
        {
            crate::files::write_atomic(&target, &bytes)?;
        }
    }
    for entry in std::fs::read_dir(&blobs_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !referenced.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let mut text = serde_json::to_string_pretty(&index)?;
    text.push('\n');
    crate::files::write_atomic(&index_path, text.as_bytes())
}

pub fn read_index(path: &Path) -> Option<Index> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(index) => Some(index),
        Err(err) => {
            tracing::warn!("ignoring unreadable {}: {err}", path.display());
            None
        }
    }
}

/// Ids the app gives unlabeled chunks: `chunk-<position>`.
fn is_positional(cell_id: &str) -> bool {
    cell_id
        .strip_prefix("chunk-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn blob_refs(outputs: &[Value]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| o["data"].as_object())
        .flat_map(|data| data.values())
        .filter_map(|v| v[BLOB_KEY].as_str())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::SessionRecord;
    use sandhill_proto::{CellRef, SessionStatus};
    use serde_json::json;

    struct Setup {
        _dir: tempfile::TempDir,
        journal: Journal,
        doc: PathBuf,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            &dir.path().join("j.sqlite"),
            BlobStore::new(dir.path().join("store")),
        )
        .unwrap();
        journal
            .insert_session(&SessionRecord {
                id: "s".into(),
                kernel: "ir".into(),
                language: "R".into(),
                cwd: "/".into(),
                connection_file: "/k.json".into(),
                client_session: "c".into(),
                pid: None,
                interrupt_mode: "signal".into(),
                status: SessionStatus::Running,
                created_ms: 0,
                ended_ms: None,
                project: None,
            })
            .unwrap();
        let doc = dir.path().join("analysis.qmd");
        Setup {
            _dir: dir,
            journal,
            doc,
        }
    }

    fn run(s: &Setup, run_id: &str, cell_id: &str, code: &str, png: &str) {
        let cell = CellRef {
            file: s.doc.to_string_lossy().into_owned(),
            cell_id: cell_id.into(),
        };
        let msg_id = format!("m-{run_id}");
        s.journal
            .insert_run(run_id, "s", &msg_id, code, Some(&cell))
            .unwrap();
        s.journal
            .append(
                "s",
                Some(run_id),
                "display_data",
                json!({"data": {"image/png": png, "text/plain": "plot"}, "metadata": {}}),
            )
            .unwrap();
        s.journal.finish_run(run_id, "ok", Some(1)).unwrap();
        record_run(&s.journal, run_id).unwrap();
    }

    fn index(s: &Setup) -> Index {
        read_index(&dir_for(&s.doc).unwrap().join("index.json")).unwrap()
    }

    #[test]
    fn outputs_and_their_blobs_are_saved_next_to_the_document() {
        let s = setup();
        run(&s, "r1", "setup", "plot(1)", "AAAA");
        let index = index(&s);
        assert_eq!(index.document, "analysis.qmd");
        let cell = &index.cells["setup"];
        assert_eq!(cell.code, "plot(1)");
        assert_eq!(cell.status.as_deref(), Some("ok"));
        let hash = cell.outputs[0]["data"]["image/png"][BLOB_KEY]
            .as_str()
            .unwrap();
        let saved = std::fs::read(blob_path(&s.doc, hash).unwrap()).unwrap();
        assert_eq!(saved, b"AAAA");
    }

    #[test]
    fn replaced_plots_are_pruned() {
        let s = setup();
        run(&s, "r1", "setup", "plot(1)", "AAAA");
        run(&s, "r2", "setup", "plot(2)", "BBBB");
        let blobs = std::fs::read_dir(dir_for(&s.doc).unwrap().join("blobs")).unwrap();
        assert_eq!(blobs.count(), 1);
        assert_eq!(index(&s).cells["setup"].code, "plot(2)");
    }

    #[test]
    fn a_moved_unlabeled_chunk_leaves_no_stale_copy() {
        let s = setup();
        run(&s, "r1", "chunk-2", "plot(1)", "AAAA");
        run(&s, "r2", "chunk-9", "summary(x)", "CCCC");
        // A chunk was inserted above, so the same code is now chunk-3.
        run(&s, "r3", "chunk-3", "plot(1)", "AAAA");
        let ids: Vec<_> = index(&s).cells.into_keys().collect();
        assert_eq!(ids, ["chunk-3", "chunk-9"]);
    }

    #[test]
    fn only_quarto_and_r_markdown_get_a_sidecar() {
        assert!(applies_to(Path::new("/w/a.qmd")));
        assert!(applies_to(Path::new("/w/a.Rmd")));
        assert!(!applies_to(Path::new("/w/a.ipynb")));
        assert!(blob_path(Path::new("/w/a.qmd"), "../x").is_none());
    }
}
