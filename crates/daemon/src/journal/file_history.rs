//! File history rows and their content-addressed text.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use sandhill_proto::{FileHistoryLimits, FileVersionInfo, FileVersionReason};
use similar::{ChangeTag, TextDiff};

use super::*;

const MB: u64 = 1_000_000;
const DAY_MS: i64 = 86_400_000;

pub(crate) fn file_bytes(limits: FileHistoryLimits) -> u64 {
    u64::from(limits.file_mb) * MB
}

impl Journal {
    // Keep snapshot blobs in the regular blobs directory but separate from
    // outputs. An unsaved notebook or an active preview can still hold an
    // output reference that isn't in SQLite.
    fn version_blobs(&self) -> BlobStore {
        BlobStore::new(self.blobs.dir().join("file-history"))
    }

    pub(crate) fn has_file_versions(&self, project: &str, path: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_versions WHERE project = ?1 AND path = ?2)",
            params![project, path],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn record_file_version(
        &self,
        project: &str,
        path: &str,
        text: &str,
        reason: FileVersionReason,
        limits: FileHistoryLimits,
        notebook: bool,
    ) -> Result<()> {
        if text.len() as u64 > file_bytes(limits)
            || limits.days == 0
            || limits.per_file == 0
            || limits.total_mb == 0
            || text.len() as u64 > u64::from(limits.total_mb) * MB
        {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let blobs = self.version_blobs();
        let hash = blobs.put(text.as_bytes())?;
        let prev: Option<String> = conn.query_row(
            "SELECT sha256 FROM file_versions WHERE project = ?1 AND path = ?2 ORDER BY id DESC LIMIT 1",
            params![project, path], |row| row.get(0),
        ).optional()?;
        if prev.as_deref() == Some(&hash) {
            return Ok(());
        }
        let previous = prev
            .and_then(|hash| blobs.get(&hash).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default();
        let readable;
        let (before, after) = if notebook {
            let before = if previous.is_empty() {
                String::new()
            } else {
                sandhill_proto::NotebookHistory::from_version(&previous)?.readable_text()
            };
            let after = sandhill_proto::NotebookHistory::from_version(text)?.readable_text();
            readable = (before, after);
            (readable.0.as_str(), readable.1.as_str())
        } else {
            (previous.as_str(), text)
        };
        let (before, after) = (
            before.lines().collect::<Vec<_>>(),
            after.lines().collect::<Vec<_>>(),
        );
        let (added, removed) = TextDiff::configure()
            .timeout(Duration::from_millis(500))
            .diff_slices(&before, &after)
            .iter_all_changes()
            .fold((0_u32, 0_u32), |(add, del), line| match line.tag() {
                ChangeTag::Insert => (add + 1, del),
                ChangeTag::Delete => (add, del + 1),
                ChangeTag::Equal => (add, del),
            });
        let tx = conn.transaction()?;
        // A return to a previous text becomes the newest occurrence, not a
        // second copy at another point in the timeline.
        tx.execute(
            "DELETE FROM file_versions WHERE project = ?1 AND path = ?2 AND sha256 = ?3",
            params![project, path, hash],
        )?;
        tx.execute(
            "INSERT INTO file_versions (project, path, time_ms, saved, size, sha256, added, removed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project, path, now_ms(), reason == FileVersionReason::Saved, text.len() as i64, hash, added, removed],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn file_versions(&self, project: &str, path: &str) -> Result<Vec<FileVersionInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut query = conn.prepare(
            "SELECT id, time_ms, saved, added, removed FROM file_versions
             WHERE project = ?1 AND path = ?2 ORDER BY id DESC",
        )?;
        Ok(query
            .query_map(params![project, path], |row| {
                Ok(FileVersionInfo {
                    id: row.get(0)?,
                    time_ms: row.get(1)?,
                    reason: if row.get(2)? {
                        FileVersionReason::Saved
                    } else {
                        FileVersionReason::Opened
                    },
                    added: row.get(3)?,
                    removed: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn file_version(
        &self,
        project: &str,
        path: &str,
        id: i64,
    ) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let hash: Option<String> = conn
            .query_row(
                "SELECT sha256 FROM file_versions WHERE project = ?1 AND path = ?2 AND id = ?3",
                params![project, path, id],
                |row| row.get(0),
            )
            .optional()?;
        hash.map(|hash| {
            let bytes = self.version_blobs().get(&hash)?;
            String::from_utf8(bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e).into())
        })
        .transpose()
    }

    pub(crate) fn tidy_file_versions(&self, limits: FileHistoryLimits, sweep: bool) -> Result<()> {
        // Hold the journal lock through collection so a writer cannot add a
        // reference between the liveness query and deletion.
        let mut conn = self.conn.lock().unwrap();
        self.check_history_storage()?;
        let tx = conn.transaction()?;
        let mut removed = tx.execute(
            "DELETE FROM file_versions WHERE time_ms < ?1 OR size > ?2 OR ?3 = 0",
            params![
                now_ms().saturating_sub(i64::from(limits.days) * DAY_MS),
                file_bytes(limits) as i64,
                limits.days
            ],
        )?;
        removed += tx.execute(
            "DELETE FROM file_versions WHERE id IN (
                SELECT id FROM (SELECT id, row_number() OVER (PARTITION BY project, path ORDER BY id DESC) AS n
                                FROM file_versions) WHERE n > ?1
             )", [limits.per_file],
        )?;
        // Charge each blob only to its newest reference. Drop the oldest
        // prefix until the remaining distinct text fits on the device.
        removed += tx.execute(
            "DELETE FROM file_versions WHERE id <= (
                SELECT max(id) FROM (
                    SELECT id, sum(size) OVER (ORDER BY id DESC) AS bytes
                    FROM (SELECT max(id) AS id, size FROM file_versions GROUP BY sha256)
                ) WHERE bytes > ?1
             ) OR ?1 = 0",
            [i64::from(limits.total_mb) * MB as i64],
        )?;
        tx.commit()?;
        if removed > 0 || sweep {
            let mut query = conn.prepare("SELECT DISTINCT sha256 FROM file_versions")?;
            let live: HashSet<String> = query
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            sweep_version_blobs(self.version_blobs().dir(), &live)?;
        }
        Ok(())
    }
}

fn sweep_version_blobs(dir: &Path, live: &HashSet<String>) -> std::io::Result<()> {
    let dirs = match std::fs::read_dir(dir) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for prefix in dirs {
        let prefix = prefix?;
        if !prefix.file_type()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(prefix.path())? {
            let entry = entry?;
            let hash = format!(
                "{}{}",
                prefix.file_name().to_string_lossy(),
                entry.file_name().to_string_lossy()
            );
            if entry.file_type()?.is_file() && !live.contains(&hash) {
                std::fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}
