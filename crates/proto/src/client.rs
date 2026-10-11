//! Async client for `sandhilld`, usable over any byte stream: a Unix socket
//! locally, or the stdio of `ssh host sandhilld attach` for a remote server.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::{
    Blob, Call, CellRef, Event, HistoryPage, KernelspecInfo, Outcome, PingReply, Request,
    RunStarted, ServerMessage, SessionInfo, Subscribed,
};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("connection error: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("daemon error: {0}")]
    Remote(String),
    #[error("connection to the daemon closed")]
    Disconnected,
}

pub type Result<T> = std::result::Result<T, ClientError>;

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Outcome>>>>;

pub struct Client {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: Pending,
    events: mpsc::UnboundedReceiver<Event>,
    next_id: AtomicU64,
}

impl Client {
    /// Wraps an already-connected byte stream.
    pub fn new<R, W>(reader: R, writer: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: Pending = Default::default();
        let (event_tx, events) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(reader, pending.clone(), event_tx));
        Client {
            writer: Mutex::new(Box::new(writer)),
            pending,
            events,
            next_id: AtomicU64::new(1),
        }
    }

    pub async fn connect_unix(path: impl AsRef<Path>) -> Result<Self> {
        let stream = tokio::net::UnixStream::connect(path).await?;
        let (r, w) = stream.into_split();
        Ok(Self::new(r, w))
    }

    /// Spawns `command` (for example `ssh host sandhilld attach --start`) and
    /// talks to the daemon over its stdin/stdout. Keep the returned child
    /// alive for as long as the client is in use.
    pub async fn connect_command(mut command: Command) -> Result<(Self, Child)> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or(ClientError::Disconnected)?;
        let stdout = child.stdout.take().ok_or(ClientError::Disconnected)?;
        Ok((Self::new(stdout, stdin), child))
    }

    /// Sends a request and waits for its response.
    pub async fn call<T: DeserializeOwned>(&self, call: Call) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut line = serde_json::to_vec(&Request { id, call })?;
        line.push(b'\n');
        {
            let mut writer = self.writer.lock().await;
            if let Err(err) = async {
                writer.write_all(&line).await?;
                writer.flush().await
            }
            .await
            {
                self.pending.lock().unwrap().remove(&id);
                return Err(err.into());
            }
        }
        match rx.await.map_err(|_| ClientError::Disconnected)? {
            Outcome::Result(value) => Ok(serde_json::from_value(value)?),
            Outcome::Error(message) => Err(ClientError::Remote(message)),
        }
    }

    /// Next event from any subscribed session; `None` once disconnected.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub async fn ping(&self) -> Result<PingReply> {
        self.call(Call::Ping).await
    }

    pub async fn list_kernelspecs(&self) -> Result<Vec<KernelspecInfo>> {
        self.call(Call::ListKernelspecs).await
    }

    pub async fn start_session(&self, kernel: &str, cwd: Option<&str>) -> Result<SessionInfo> {
        self.call(Call::StartSession {
            kernel: kernel.into(),
            cwd: cwd.map(Into::into),
        })
        .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        self.call(Call::ListSessions).await
    }

    pub async fn execute(
        &self,
        session: &str,
        code: &str,
        cell: Option<CellRef>,
    ) -> Result<RunStarted> {
        self.call(Call::Execute {
            session: session.into(),
            code: code.into(),
            cell,
        })
        .await
    }

    pub async fn interrupt(&self, session: &str) -> Result<()> {
        self.call(Call::Interrupt {
            session: session.into(),
        })
        .await
    }

    pub async fn shutdown(&self, session: &str) -> Result<()> {
        self.call(Call::Shutdown {
            session: session.into(),
        })
        .await
    }

    pub async fn subscribe(&self, session: &str, since_seq: Option<i64>) -> Result<Subscribed> {
        self.call(Call::Subscribe {
            session: session.into(),
            since_seq,
        })
        .await
    }

    pub async fn unsubscribe(&self, session: &str) -> Result<()> {
        self.call(Call::Unsubscribe {
            session: session.into(),
        })
        .await
    }

    pub async fn history(
        &self,
        session: &str,
        since_seq: i64,
        limit: Option<u32>,
    ) -> Result<HistoryPage> {
        self.call(Call::History {
            session: session.into(),
            since_seq,
            limit,
        })
        .await
    }

    pub async fn get_blob(&self, hash: &str) -> Result<Blob> {
        self.call(Call::GetBlob { hash: hash.into() }).await
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    pending: Pending,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ServerMessage>(&line) {
            Ok(ServerMessage::Response { id, outcome }) => {
                if let Some(tx) = pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(outcome);
                }
            }
            Ok(ServerMessage::Event { event }) => {
                let _ = events.send(event);
            }
            // A newer daemon may send messages this client doesn't know.
            Err(_) => continue,
        }
    }
    // Dropping the senders wakes every waiting call with `Disconnected`.
    pending.lock().unwrap().clear();
}
