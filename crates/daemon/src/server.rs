//! The daemon: owns every session and serves clients over a byte stream.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use sandhill_proto::{
    Blob, Call, Event, HistoryPage, Outcome, PROTOCOL_VERSION, PingReply, Request, ServerMessage,
    SessionStatus, Subscribed,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::blobs::BlobStore;
use crate::journal::Journal;
use crate::paths::StateDir;
use crate::session::SessionHandle;
use crate::{files, memory, notebook, outputs, projects, sidecar};

const REPLAY_PAGE: u32 = 500;
const HISTORY_LIMIT: u32 = 5000;

pub struct Daemon {
    state: StateDir,
    journal: Arc<Journal>,
    sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
}

impl Daemon {
    /// Opens the state directory and reconnects to kernels that kept running
    /// while the daemon was down.
    pub async fn open(state: StateDir) -> Result<Arc<Daemon>> {
        state
            .create()
            .context("cannot create the state directory")?;
        let journal = Arc::new(Journal::open(
            &state.db_path(),
            BlobStore::new(state.blobs_dir()),
        )?);
        let daemon = Arc::new(Daemon {
            state,
            journal: journal.clone(),
            sessions: Mutex::new(HashMap::new()),
        });
        for record in journal.sessions()? {
            if record.status == SessionStatus::Ended {
                continue;
            }
            let id = record.id.clone();
            match SessionHandle::reattach(journal.clone(), record).await {
                Ok(handle) => {
                    tracing::info!("reattached to session {id}");
                    daemon.insert(handle);
                }
                Err(err) => tracing::warn!("session {id} could not be reattached: {err:#}"),
            }
        }
        Ok(daemon)
    }

    pub fn state(&self) -> &StateDir {
        &self.state
    }

    /// Stops managing every kernel but leaves them running, as happens when
    /// the daemon exits. Used for restarts and in tests.
    pub fn detach_all(&self) {
        for session in self.sessions.lock().unwrap().values() {
            session.detach();
        }
    }

    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> std::io::Result<()> {
        loop {
            let (stream, _) = listener.accept().await?;
            let (reader, writer) = stream.into_split();
            tokio::spawn(self.clone().serve_connection(reader, writer));
        }
    }

    /// Serves one client until it disconnects.
    pub async fn serve_connection<R, W>(self: Arc<Self>, reader: R, writer: W)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out, out_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let writer_task = tokio::spawn(write_loop(writer, out_rx));
        let subscriptions: Subscriptions = Default::default();
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let request = match serde_json::from_str::<Request>(&line) {
                Ok(request) => request,
                Err(err) => {
                    let id = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("id").and_then(Value::as_u64))
                        .unwrap_or(0);
                    let _ = out.send(response(id, Err(anyhow!("bad request: {err}"))));
                    continue;
                }
            };
            let large = line.len() > memory::LARGE;
            drop(line);
            let daemon = self.clone();
            let out = out.clone();
            let subscriptions = subscriptions.clone();
            tokio::spawn(async move {
                daemon.handle(request, out, subscriptions).await;
                if large {
                    memory::release_free_memory();
                }
            });
        }
        for (_, task) in subscriptions.lock().unwrap().drain() {
            task.abort();
        }
        drop(out);
        let _ = writer_task.await;
    }

    async fn handle(
        self: Arc<Self>,
        request: Request,
        out: mpsc::UnboundedSender<ServerMessage>,
        subscriptions: Subscriptions,
    ) {
        let id = request.id;
        let result = match request.call {
            Call::Subscribe { session, since_seq } => {
                // Answers and starts streaming itself, so the response always
                // precedes the replayed events.
                if let Err(err) = self.subscribe(id, session, since_seq, &out, &subscriptions) {
                    let _ = out.send(response(id, Err(err)));
                }
                return;
            }
            Call::Unsubscribe { session } => {
                if let Some(task) = subscriptions.lock().unwrap().remove(&session) {
                    task.abort();
                }
                Ok(Value::Null)
            }
            call => self.call(call).await,
        };
        let _ = out.send(response(id, result));
    }

    async fn call(&self, call: Call) -> Result<Value> {
        Ok(match call {
            Call::Ping => serde_json::to_value(PingReply {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                home: std::env::var("HOME").unwrap_or_default(),
                hostname: hostname(),
                build: crate::build_id().to_string(),
            })?,
            Call::ListKernelspecs => serde_json::to_value(crate::kernel::list_kernelspecs().await)?,
            Call::StartSession {
                kernel,
                cwd,
                project,
            } => {
                let cwd = match cwd {
                    Some(cwd) => PathBuf::from(cwd),
                    None => std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("/")),
                };
                if !cwd.is_dir() {
                    anyhow::bail!("{} is not a directory", cwd.display());
                }
                let handle = SessionHandle::start(
                    self.journal.clone(),
                    &self.state.kernels_dir(),
                    &kernel,
                    &cwd,
                    project.map(|p| projects::normalize(&p)).transpose()?,
                )
                .await?;
                let id = handle.id.clone();
                self.insert(handle);
                serde_json::to_value(self.session_info(&id)?)?
            }
            Call::ListSessions => {
                let infos = self
                    .journal
                    .sessions()?
                    .iter()
                    .map(|r| self.journal.session_info(r))
                    .collect::<Result<Vec<_>, _>>()?;
                serde_json::to_value(infos)?
            }
            Call::Execute {
                session,
                code,
                cell,
            } => serde_json::to_value(self.live(&session)?.execute(code, cell).await?)?,
            Call::Interrupt { session } => {
                self.live(&session)?.interrupt().await?;
                Value::Null
            }
            Call::Shutdown { session } => {
                let handle = self.live(&session)?;
                handle.shutdown().await?;
                self.sessions.lock().unwrap().remove(&session);
                Value::Null
            }
            Call::History {
                session,
                since_seq,
                limit,
            } => {
                let limit = limit.unwrap_or(HISTORY_LIMIT).min(HISTORY_LIMIT);
                let events = self.journal.events_since(&session, since_seq, limit)?;
                let next_seq = events.last().map_or(since_seq, |e| e.seq);
                serde_json::to_value(HistoryPage { events, next_seq })?
            }
            Call::GetBlob { hash, near } => {
                let mut bytes = self.journal.blobs().get(&hash);
                if bytes.is_err()
                    && let Some(path) = near
                        .and_then(|n| files::resolve(&n).ok())
                        .and_then(|doc| sidecar::blob_path(&doc, &hash))
                {
                    bytes = std::fs::read(path);
                }
                let bytes = bytes.with_context(|| format!("no blob {hash}"))?;
                serde_json::to_value(Blob {
                    hash,
                    data: String::from_utf8_lossy(&bytes).into_owned(),
                })?
            }
            Call::ReadFile { path } => {
                serde_json::to_value(blocking(move || files::read_text(&path)).await?)?
            }
            Call::ReadNotebook { path } => {
                let journal = self.journal.clone();
                let file = blocking(move || {
                    let file = notebook::read_notebook(&path, journal.blobs());
                    memory::release_free_memory();
                    file
                })
                .await?;
                serde_json::to_value(file)?
            }
            Call::WriteNotebook {
                path,
                text,
                expected_mtime_ms,
            } => {
                let journal = self.journal.clone();
                let written = blocking(move || {
                    let written =
                        notebook::write_notebook(&path, &text, expected_mtime_ms, journal.blobs());
                    drop(text);
                    memory::release_free_memory();
                    written
                })
                .await?;
                serde_json::to_value(written)?
            }
            Call::WriteFile {
                path,
                text,
                expected_mtime_ms,
            } => serde_json::to_value(
                blocking(move || files::write_text(&path, &text, expected_mtime_ms)).await?,
            )?,
            Call::ListDir { path } => {
                serde_json::to_value(blocking(move || files::list_dir(&path)).await?)?
            }
            Call::FileOutputs { file } => {
                let journal = self.journal.clone();
                serde_json::to_value(
                    blocking(move || {
                        let file = files::resolve(&file)?.to_string_lossy().into_owned();
                        Ok(outputs::file_outputs(&journal, &file)?)
                    })
                    .await?,
                )?
            }
            Call::OpenProject { path } => {
                let journal = self.journal.clone();
                serde_json::to_value(blocking(move || projects::open(&journal, &path)).await?)?
            }
            Call::ListRecentProjects => {
                let journal = self.journal.clone();
                serde_json::to_value(blocking(move || projects::recent(&journal)).await?)?
            }
            Call::SaveProjectState { path, state } => {
                let journal = self.journal.clone();
                blocking(move || {
                    Ok(journal.save_project_state(&projects::normalize(&path)?, &state)?)
                })
                .await?;
                Value::Null
            }
            Call::ForgetProject { path } => {
                let journal = self.journal.clone();
                blocking(move || Ok(journal.forget_project(&projects::normalize(&path)?)?)).await?;
                Value::Null
            }
            Call::SystemStats { path } => {
                let journal = self.journal.clone();
                serde_json::to_value(
                    blocking(move || {
                        let path = match path {
                            Some(path) => files::resolve(&path)?,
                            None => PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into())),
                        };
                        let kernels: Vec<(String, u32)> = journal
                            .sessions()?
                            .into_iter()
                            .filter(|s| s.status != SessionStatus::Ended)
                            .filter_map(|s| Some((s.id, s.pid?)))
                            .collect();
                        Ok(crate::system::stats(&path, &kernels))
                    })
                    .await?,
                )?
            }
            Call::Subscribe { .. } | Call::Unsubscribe { .. } => unreachable!("handled earlier"),
        })
    }

    fn subscribe(
        &self,
        id: u64,
        session: String,
        since_seq: Option<i64>,
        out: &mpsc::UnboundedSender<ServerMessage>,
        subscriptions: &Subscriptions,
    ) -> Result<()> {
        let known = self.journal.sessions()?.iter().any(|r| r.id == session);
        if !known {
            anyhow::bail!("no session {session}");
        }
        // Subscribe to live events before reading the journal so nothing
        // published in between is missed; duplicates are filtered by seq.
        let live = self
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .filter(|s| s.is_running())
            .map(|s| s.subscribe());
        let from_seq = match since_seq {
            Some(seq) => seq,
            None => self.journal.last_seq(&session)?,
        };
        let _ = out.send(response(
            id,
            Ok(serde_json::to_value(Subscribed {
                session: session.clone(),
                from_seq,
            })?),
        ));
        let task = tokio::spawn(forward_events(
            self.journal.clone(),
            session.clone(),
            from_seq,
            live,
            out.clone(),
        ));
        if let Some(old) = subscriptions.lock().unwrap().insert(session, task) {
            old.abort();
        }
        Ok(())
    }

    fn insert(&self, handle: SessionHandle) {
        self.sessions
            .lock()
            .unwrap()
            .insert(handle.id.clone(), Arc::new(handle));
    }

    fn live(&self, session: &str) -> Result<Arc<SessionHandle>> {
        self.sessions
            .lock()
            .unwrap()
            .get(session)
            .filter(|s| s.is_running())
            .cloned()
            .ok_or_else(|| anyhow!("no running session {session}"))
    }

    fn session_info(&self, id: &str) -> Result<sandhill_proto::SessionInfo> {
        let record = self
            .journal
            .sessions()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| anyhow!("no session {id}"))?;
        Ok(self.journal.session_info(&record)?)
    }
}

type Subscriptions = Arc<Mutex<HashMap<String, JoinHandle<()>>>>;

/// Replays journaled events after `from_seq`, then forwards live ones.
async fn forward_events(
    journal: Arc<Journal>,
    session: String,
    mut last_seq: i64,
    live: Option<broadcast::Receiver<Event>>,
    out: mpsc::UnboundedSender<ServerMessage>,
) {
    let replay = |last_seq: &mut i64| -> bool {
        loop {
            let page = match journal.events_since(&session, *last_seq, REPLAY_PAGE) {
                Ok(page) => page,
                Err(err) => {
                    tracing::error!("replay of {session} failed: {err}");
                    return false;
                }
            };
            let done = page.len() < REPLAY_PAGE as usize;
            for event in page {
                *last_seq = event.seq;
                if out.send(ServerMessage::Event { event }).is_err() {
                    return false;
                }
            }
            if done {
                return true;
            }
        }
    };
    if !replay(&mut last_seq) {
        return;
    }
    let Some(mut live) = live else { return };
    loop {
        match live.recv().await {
            Ok(event) if event.seq > last_seq => {
                last_seq = event.seq;
                if out.send(ServerMessage::Event { event }).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            // A slow client fell behind: catch up from the journal.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if !replay(&mut last_seq) {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => {
                replay(&mut last_seq);
                return;
            }
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut messages: mpsc::UnboundedReceiver<ServerMessage>,
) {
    while let Some(message) = messages.recv().await {
        let Ok(mut line) = serde_json::to_vec(&message) else {
            continue;
        };
        drop(message);
        line.push(b'\n');
        if writer.write_all(&line).await.is_err() || writer.flush().await.is_err() {
            return;
        }
        if line.len() > memory::LARGE {
            drop(line);
            memory::release_free_memory();
        }
    }
}

/// Runs filesystem and SQLite work off the async worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its whole length, and gethostname
    // writes at most that many bytes.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn response(id: u64, result: Result<Value>) -> ServerMessage {
    ServerMessage::Response {
        id,
        outcome: match result {
            Ok(value) => Outcome::Result(value),
            Err(err) => Outcome::Error(format!("{err:#}")),
        },
    }
}
