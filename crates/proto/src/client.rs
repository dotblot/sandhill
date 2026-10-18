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
    AgentInfo, AgentOptions, AppRequest, Blob, Call, CellRef, DirEntry, Event, FileOutputs,
    FileStat, FileText, FileWritten, Focus, HistoryPage, KernelspecInfo, Outcome, PingReply,
    ProjectInfo, Request, RunStarted, ServerMessage, SessionInfo, SessionRuns, Subscribed,
    SystemStats, TerminalEvent, TerminalInfo, TerminalSubscribed, Variables,
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
    events: Option<mpsc::UnboundedReceiver<Event>>,
    terminal_events: Option<mpsc::UnboundedReceiver<TerminalEvent>>,
    requests: Option<mpsc::UnboundedReceiver<AppRequest>>,
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
        let (terminal_tx, terminal_events) = mpsc::unbounded_channel();
        let (request_tx, requests) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(
            reader,
            pending.clone(),
            event_tx,
            terminal_tx,
            request_tx,
        ));
        Client {
            writer: Mutex::new(Box::new(writer)),
            pending,
            events: Some(events),
            terminal_events: Some(terminal_events),
            requests: Some(requests),
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
        self.events.as_mut()?.recv().await
    }

    /// Takes the event stream so it can be read on its own task while the
    /// client is shared for calls. The stream ends when the connection does.
    pub fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<Event>> {
        self.events.take()
    }

    /// Next message from any subscribed terminal; `None` once disconnected.
    pub async fn next_terminal_event(&mut self) -> Option<TerminalEvent> {
        self.terminal_events.as_mut()?.recv().await
    }

    /// Takes the terminal messages, as [`Client::take_events`] does for sessions.
    pub fn take_terminal_events(&mut self) -> Option<mpsc::UnboundedReceiver<TerminalEvent>> {
        self.terminal_events.take()
    }

    /// Next request for a window (see [`Call::SetFocus`]); `None` once
    /// disconnected.
    pub async fn next_request(&mut self) -> Option<AppRequest> {
        self.requests.as_mut()?.recv().await
    }

    /// Takes the requests for windows, as [`Client::take_events`] does for
    /// sessions.
    pub fn take_requests(&mut self) -> Option<mpsc::UnboundedReceiver<AppRequest>> {
        self.requests.take()
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
            project: None,
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
            quiet: false,
            by: None,
        })
        .await
    }

    /// Runs code without adding it to the kernel's history or execution
    /// count, for tools that look at a session without changing it.
    pub async fn execute_quietly(&self, session: &str, code: &str) -> Result<RunStarted> {
        self.call(Call::Execute {
            session: session.into(),
            code: code.into(),
            cell: None,
            quiet: true,
            by: None,
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
        self.call(Call::GetBlob {
            hash: hash.into(),
            near: None,
        })
        .await
    }

    pub async fn read_file(&self, path: &str) -> Result<FileText> {
        self.call(Call::ReadFile { path: path.into() }).await
    }

    pub async fn write_file(
        &self,
        path: &str,
        text: &str,
        expected_mtime_ms: Option<i64>,
    ) -> Result<FileWritten> {
        self.call(Call::WriteFile {
            path: path.into(),
            text: text.into(),
            expected_mtime_ms,
        })
        .await
    }

    pub async fn read_notebook(&self, path: &str) -> Result<FileText> {
        self.call(Call::ReadNotebook { path: path.into() }).await
    }

    pub async fn write_notebook(
        &self,
        path: &str,
        text: &str,
        expected_mtime_ms: Option<i64>,
    ) -> Result<FileWritten> {
        self.call(Call::WriteNotebook {
            path: path.into(),
            text: text.into(),
            expected_mtime_ms,
        })
        .await
    }

    pub async fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        self.call(Call::ListDir { path: path.into() }).await
    }

    pub async fn file_outputs(&self, file: &str) -> Result<FileOutputs> {
        self.call(Call::FileOutputs { file: file.into() }).await
    }

    pub async fn session_runs(&self, session: &str, limit: Option<u32>) -> Result<SessionRuns> {
        self.call(Call::SessionRuns {
            session: session.into(),
            limit,
        })
        .await
    }

    pub async fn list_variables(&self, session: &str, path: &[String]) -> Result<Variables> {
        self.call(Call::ListVariables {
            session: session.into(),
            path: path.to_vec(),
        })
        .await
    }

    pub async fn delete_variables(&self, session: &str, names: &[String]) -> Result<()> {
        self.call(Call::DeleteVariables {
            session: session.into(),
            names: names.to_vec(),
        })
        .await
    }

    pub async fn open_project(&self, path: &str) -> Result<ProjectInfo> {
        self.call(Call::OpenProject { path: path.into() }).await
    }

    pub async fn list_recent_projects(&self) -> Result<Vec<ProjectInfo>> {
        self.call(Call::ListRecentProjects).await
    }

    pub async fn system_stats(&self, path: Option<&str>) -> Result<SystemStats> {
        self.call(Call::SystemStats {
            path: path.map(Into::into),
        })
        .await
    }

    pub async fn save_project_state(&self, path: &str, state: serde_json::Value) -> Result<()> {
        self.call(Call::SaveProjectState {
            path: path.into(),
            state,
        })
        .await
    }

    pub async fn forget_project(&self, path: &str) -> Result<()> {
        self.call(Call::ForgetProject { path: path.into() }).await
    }

    pub async fn stat_files(&self, paths: Vec<String>) -> Result<Vec<FileStat>> {
        self.call(Call::StatFiles { paths }).await
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        self.call(Call::ListAgents).await
    }

    pub async fn start_terminal(
        &self,
        kind: &str,
        cwd: Option<&str>,
        size: Option<(u16, u16)>,
        options: AgentOptions,
    ) -> Result<TerminalInfo> {
        self.call(Call::StartTerminal {
            kind: kind.into(),
            cwd: cwd.map(Into::into),
            project: None,
            cols: size.map(|s| s.0),
            rows: size.map(|s| s.1),
            options,
            prompt: None,
        })
        .await
    }

    pub async fn list_terminals(&self) -> Result<Vec<TerminalInfo>> {
        self.call(Call::ListTerminals).await
    }

    pub async fn subscribe_terminal(
        &self,
        terminal: &str,
        from: Option<u64>,
    ) -> Result<TerminalSubscribed> {
        self.call(Call::SubscribeTerminal {
            terminal: terminal.into(),
            from,
            chat_from: None,
        })
        .await
    }

    /// Subscribes to a terminal's output and its agent's conversation.
    pub async fn subscribe_chat(
        &self,
        terminal: &str,
        from: Option<u64>,
        chat_from: u64,
    ) -> Result<TerminalSubscribed> {
        self.call(Call::SubscribeTerminal {
            terminal: terminal.into(),
            from,
            chat_from: Some(chat_from),
        })
        .await
    }

    pub async fn answer_terminal(
        &self,
        terminal: &str,
        id: &str,
        answer: serde_json::Value,
    ) -> Result<()> {
        self.call(Call::AnswerTerminal {
            terminal: terminal.into(),
            id: id.into(),
            answer,
        })
        .await
    }

    pub async fn tell_terminal(&self, terminal: &str, command: serde_json::Value) -> Result<()> {
        self.call(Call::TellTerminal {
            terminal: terminal.into(),
            command,
        })
        .await
    }

    pub async fn unsubscribe_terminal(&self, terminal: &str) -> Result<()> {
        self.call(Call::UnsubscribeTerminal {
            terminal: terminal.into(),
        })
        .await
    }

    pub async fn terminal_input(&self, terminal: &str, data: &str) -> Result<()> {
        self.call(Call::TerminalInput {
            terminal: terminal.into(),
            data: data.into(),
        })
        .await
    }

    pub async fn resize_terminal(&self, terminal: &str, cols: u16, rows: u16) -> Result<()> {
        self.call(Call::ResizeTerminal {
            terminal: terminal.into(),
            cols,
            rows,
        })
        .await
    }

    pub async fn close_terminal(&self, terminal: &str) -> Result<()> {
        self.call(Call::CloseTerminal {
            terminal: terminal.into(),
        })
        .await
    }

    pub async fn set_focus(&self, project: &str, focus: Focus) -> Result<()> {
        self.call(Call::SetFocus {
            project: project.into(),
            focus,
        })
        .await
    }

    pub async fn get_focus(&self, project: &str) -> Result<Option<Focus>> {
        self.call(Call::GetFocus {
            project: project.into(),
        })
        .await
    }

    /// Answers a window request with its result or error.
    pub async fn answer(
        &self,
        id: u64,
        result: std::result::Result<serde_json::Value, String>,
    ) -> Result<()> {
        let (result, error) = match result {
            Ok(value) => (Some(value), None),
            Err(message) => (None, Some(message)),
        };
        self.call(Call::Answer { id, result, error }).await
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    pending: Pending,
    events: mpsc::UnboundedSender<Event>,
    terminal_events: mpsc::UnboundedSender<TerminalEvent>,
    requests: mpsc::UnboundedSender<AppRequest>,
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
            Ok(ServerMessage::Terminal { terminal }) => {
                let _ = terminal_events.send(terminal);
            }
            Ok(ServerMessage::Request { request }) => {
                let _ = requests.send(request);
            }
            // A newer daemon may send messages this client doesn't know.
            Err(_) => continue,
        }
    }
    // Dropping the senders wakes every waiting call with `Disconnected`.
    pending.lock().unwrap().clear();
}
