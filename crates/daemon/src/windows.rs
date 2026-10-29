//! What project windows tell the daemon about the user, and what the daemon
//! asks them. Each window says where the user is (the file, cell and
//! selection) and which files it has open. Agents read the place with
//! `get_focus`; their edits to a file a window has open go through that
//! window, so they land in the editor (and its undo history) rather than
//! under it.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use sandhill_proto::{AppRequest, Focus, ServerMessage};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::journal::now_ms;

/// How long a window may take to make an edit before the agent hears it
/// didn't answer. Running the cell comes after the answer.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

type Answer = std::result::Result<Value, String>;

/// Why a window gave no answer.
#[derive(Debug)]
pub enum Unanswered {
    /// It closed first.
    Closed,
    /// It took too long.
    Late,
    /// It answered with this error.
    Said(String),
}

#[derive(Default)]
pub struct Windows {
    /// The latest place per project, kept after its window goes.
    focus: Mutex<HashMap<String, Focus>>,
    /// Per connection: where to send it messages, and the files each of its
    /// project windows has open.
    connections: Mutex<HashMap<u64, Connection>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Answer>>>,
    next_request: AtomicU64,
}

struct Connection {
    out: mpsc::UnboundedSender<ServerMessage>,
    open: HashMap<String, Vec<PathBuf>>,
}

impl Windows {
    pub fn set_focus(
        &self,
        connection: u64,
        out: &mpsc::UnboundedSender<ServerMessage>,
        project: String,
        mut focus: Focus,
    ) {
        focus.ts_ms = now_ms();
        let open = focus.open.iter().map(|p| normalize(Path::new(p))).collect();
        self.connections
            .lock()
            .unwrap()
            .entry(connection)
            .or_insert_with(|| Connection {
                out: out.clone(),
                open: HashMap::new(),
            })
            .open
            .insert(project.clone(), open);
        self.focus.lock().unwrap().insert(project, focus);
    }

    pub fn focus(&self, project: &str) -> Option<Focus> {
        self.focus.lock().unwrap().get(project).cloned()
    }

    /// After an agent added a cell to `file`: the user's place there moves
    /// to it, as the window's cursor does, so the agent's next cell goes
    /// below this one rather than above it.
    pub fn moved_to(&self, file: &Path, cell: &str) {
        let file = normalize(file);
        for focus in self.focus.lock().unwrap().values_mut() {
            if focus.file.as_deref().map(|f| normalize(Path::new(f))) == Some(file.clone()) {
                focus.cell = (!cell.is_empty()).then(|| cell.to_string());
                focus.line = None;
                focus.selection = None;
            }
        }
    }

    /// Forgets a connection's windows. Their place is kept.
    pub fn disconnected(&self, connection: u64) {
        self.connections.lock().unwrap().remove(&connection);
    }

    /// Asks the window that has `file` open (preferring `project`'s window)
    /// to run `method`. `None` when no window has it open.
    pub async fn ask(
        &self,
        file: &Path,
        project: Option<&str>,
        method: &str,
        params: Value,
    ) -> Option<Result<Value>> {
        let (window, out) = self.window_with(file, project)?;
        let answer = self
            .ask_on(&out, window, method, params, ANSWER_TIMEOUT)
            .await?;
        Some(answer.map_err(|err| match err {
            Unanswered::Closed => anyhow!("the Sandhill window closed before it answered"),
            Unanswered::Late => anyhow!(
                "the Sandhill window with {} open didn't answer",
                file.display()
            ),
            Unanswered::Said(message) => anyhow!("{message}"),
        }))
    }

    /// Asks the `project` window of the connection that `out` sends to;
    /// `None` when it has gone.
    pub async fn ask_on(
        &self,
        out: &mpsc::UnboundedSender<ServerMessage>,
        project: String,
        method: &str,
        params: Value,
        wait: Duration,
    ) -> Option<std::result::Result<Value, Unanswered>> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let request = AppRequest {
            id,
            project,
            method: method.into(),
            params,
        };
        if out.send(ServerMessage::Request { request }).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return None;
        }
        let answer = tokio::time::timeout(wait, rx).await;
        self.pending.lock().unwrap().remove(&id);
        Some(match answer {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(Unanswered::Said(message)),
            Ok(Err(_)) => Err(Unanswered::Closed),
            Err(_) => Err(Unanswered::Late),
        })
    }

    pub fn answer(&self, id: u64, answer: Answer) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(answer);
        }
    }

    fn window_with(
        &self,
        file: &Path,
        project: Option<&str>,
    ) -> Option<(String, mpsc::UnboundedSender<ServerMessage>)> {
        let file = normalize(file);
        let connections = self.connections.lock().unwrap();
        let mut found = None;
        for connection in connections.values() {
            for (window, open) in &connection.open {
                if !open.contains(&file) {
                    continue;
                }
                if project == Some(window.as_str()) {
                    return Some((window.clone(), connection.out.clone()));
                }
                found.get_or_insert_with(|| (window.clone(), connection.out.clone()));
            }
        }
        found
    }
}

/// `a/./b/../c` as `a/c`, without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn edits_go_to_the_window_with_the_file_open() {
        let windows = std::sync::Arc::new(Windows::default());
        let (out, mut messages) = mpsc::unbounded_channel();
        let focus = Focus {
            file: Some("/p/a.qmd".into()),
            cell: Some("qc".into()),
            open: vec!["/p/a.qmd".into()],
            ..Focus::default()
        };
        windows.set_focus(1, &out, "/p".into(), focus);
        assert_eq!(windows.focus("/p").unwrap().cell.as_deref(), Some("qc"));
        assert!(windows.focus("/p").unwrap().ts_ms > 0);

        // A file no window has open is left to the daemon.
        assert!(
            windows
                .ask(Path::new("/p/b.qmd"), None, "add_cell", json!({}))
                .await
                .is_none()
        );

        let asking = tokio::spawn({
            let windows = windows.clone();
            async move {
                windows
                    .ask(
                        Path::new("/p/./x/../a.qmd"),
                        Some("/p"),
                        "add_cell",
                        json!({"code": "1"}),
                    )
                    .await
            }
        });
        let Some(ServerMessage::Request { request }) = messages.recv().await else {
            panic!("no request");
        };
        assert_eq!(request.project, "/p");
        assert_eq!(request.method, "add_cell");
        windows.answer(request.id, Ok(json!({"cell_id": "chunk-2"})));
        let answer = asking.await.unwrap().unwrap().unwrap();
        assert_eq!(answer["cell_id"], "chunk-2");

        windows.moved_to(Path::new("/p/a.qmd"), "chunk-2");
        assert_eq!(
            windows.focus("/p").unwrap().cell.as_deref(),
            Some("chunk-2")
        );

        // Once the window's connection goes, the place stays but edits don't
        // go to it.
        windows.disconnected(1);
        assert!(windows.focus("/p").is_some());
        assert!(
            windows
                .ask(Path::new("/p/a.qmd"), None, "add_cell", json!({}))
                .await
                .is_none()
        );
    }
}
