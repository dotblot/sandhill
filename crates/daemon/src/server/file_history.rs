//! File history at the document IO boundary.

use std::borrow::Cow;
use std::time::Duration;

use sandhill_proto::{FileHistoryLimits, FileVersionReason, NotebookHistory};

use crate::journal::file_history::file_bytes;

use super::*;

const LIMITS_FILE: &str = "file-history.json";

pub(super) fn load_limits(root: &Path) -> FileHistoryLimits {
    match std::fs::read(root.join(LIMITS_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(%err, "could not read file history limits");
            FileHistoryLimits::default()
        }),
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(%err, "could not read file history limits");
            }
            FileHistoryLimits::default()
        }
    }
}

pub(super) fn tidy(journal: &Journal, limits: FileHistoryLimits) {
    if let Err(err) = journal.tidy_file_versions(limits, true) {
        tracing::warn!(%err, "could not tidy file history");
    }
}

fn canonical(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().context("file has no parent directory")?;
            let name = path.file_name().context("invalid file name")?;
            Ok(canonical(parent)?.join(name))
        }
        Err(err) => Err(err.into()),
    }
}

fn history_key(journal: &Journal, project: Option<&str>, path: &str) -> Result<(String, String)> {
    let resolved = files::resolve(path)?;
    let path = canonical(&resolved)?;
    let project = match project {
        Some(project) => projects::normalize(project)?,
        None => journal
            .recent_projects()?
            .into_iter()
            .filter(|(root, _)| path.starts_with(root) || resolved.starts_with(root))
            .max_by_key(|(root, _)| root.len())
            .map(|(root, _)| root)
            .unwrap_or_default(),
    };
    Ok((project, path.to_string_lossy().into_owned()))
}

pub(super) fn is_ipynb(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ipynb"))
}

fn snapshot<'a>(path: &str, text: &'a str) -> Result<Option<Cow<'a, str>>> {
    if is_ipynb(path) {
        let cells: NotebookHistory = serde_json::from_str(text)?;
        return Ok(Some(Cow::Owned(cells.render()?)));
    }
    Ok((!text.as_bytes().contains(&0)).then_some(Cow::Borrowed(text)))
}

pub(super) fn remember(
    journal: &Journal,
    limits: FileHistoryLimits,
    project: Option<&str>,
    path: &str,
    text: &str,
    reason: FileVersionReason,
) {
    let record = || -> Result<()> {
        if !enabled(limits) || !is_ipynb(path) && text.len() as u64 > file_bytes(limits) {
            return Ok(());
        }
        if let Some(text) = snapshot(path, text)? {
            let (project, key) = history_key(journal, project, path)?;
            journal.record_file_version(&project, &key, &text, reason, limits, is_ipynb(path))?;
        }
        Ok(())
    };
    if let Err(err) = record() {
        tracing::warn!(%err, "could not record file history");
    }
}

fn enabled(limits: FileHistoryLimits) -> bool {
    limits.days > 0 && limits.per_file > 0 && limits.file_mb > 0 && limits.total_mb > 0
}

/// Capture the image before a successful first overwrite. Do not record it
/// until the write succeeds. Jupyter outputs are skipped while parsing.
pub(super) fn before_first_write(
    journal: &Journal,
    limits: FileHistoryLimits,
    project: Option<&str>,
    path: &str,
) -> Option<BeforeWrite> {
    if !enabled(limits) {
        return None;
    }
    let (project, key) = history_key(journal, project, path).ok()?;
    let resolved = files::resolve(path).ok()?;
    if journal.has_file_versions(&project, &key).unwrap_or(true)
        || !std::fs::metadata(&resolved).ok()?.is_file()
    {
        return None;
    }
    let text = if is_ipynb(path) {
        let file = std::fs::File::open(&resolved).ok()?;
        let cells: NotebookHistory = serde_json::from_reader(std::io::BufReader::new(file)).ok()?;
        cells.render().ok()?
    } else {
        let text = files::read_text_bounded(path, file_bytes(limits))
            .ok()?
            .text;
        if text.as_bytes().contains(&0) {
            return None;
        }
        text
    };
    Some(BeforeWrite {
        project,
        path: key,
        text,
        notebook: is_ipynb(path),
    })
}

pub(super) struct BeforeWrite {
    project: String,
    path: String,
    text: String,
    notebook: bool,
}

pub(super) fn remember_before_write(
    journal: &Journal,
    limits: FileHistoryLimits,
    prior: Option<BeforeWrite>,
) {
    if let Some(BeforeWrite {
        project,
        path,
        text,
        notebook,
    }) = prior
        && let Err(err) = journal.record_file_version(
            &project,
            &path,
            &text,
            FileVersionReason::Opened,
            limits,
            notebook,
        )
    {
        tracing::warn!(%err, "could not record the file before its first save");
    }
}

impl Daemon {
    pub(super) fn start_history_tidy(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let period = Duration::from_secs(60);
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(daemon) = weak.upgrade() else { return };
                let _guard = daemon.lifecycle_gate.read().await;
                if !daemon.maintenance.load(Ordering::Acquire)
                    && !daemon.stopping.load(Ordering::Acquire)
                {
                    let daemon = daemon.clone();
                    let _ = blocking(move || {
                        let limits = daemon.history_limits.lock().unwrap();
                        daemon.journal.tidy_file_versions(*limits, false)?;
                        Ok(())
                    })
                    .await
                    .inspect_err(|err| tracing::warn!(%err, "could not tidy file history"));
                }
            }
        });
    }

    pub(super) async fn set_file_history_limits(
        self: &Arc<Self>,
        limits: FileHistoryLimits,
    ) -> Result<Value> {
        let daemon = self.clone();
        blocking(move || {
            let mut current = daemon.history_limits.lock().unwrap();
            if *current != limits {
                files::write_atomic(
                    &daemon.state.root().join(LIMITS_FILE),
                    &serde_json::to_vec(&limits)?,
                )?;
                *current = limits;
                daemon.journal.tidy_file_versions(limits, false)?;
            }
            Ok(Value::Null)
        })
        .await
    }

    pub(super) async fn file_versions(
        &self,
        path: String,
        project: Option<String>,
    ) -> Result<Value> {
        let journal = self.journal.clone();
        blocking(move || {
            let (project, path) = history_key(&journal, project.as_deref(), &path)?;
            Ok(serde_json::to_value(
                journal.file_versions(&project, &path)?,
            )?)
        })
        .await
    }

    pub(super) async fn file_version(
        &self,
        path: String,
        project: Option<String>,
        id: i64,
    ) -> Result<Value> {
        let journal = self.journal.clone();
        blocking(move || {
            let (project, path) = history_key(&journal, project.as_deref(), &path)?;
            let text = journal
                .file_version(&project, &path, id)?
                .context("This version is no longer kept")?;
            Ok(Value::String(text))
        })
        .await
    }
}
