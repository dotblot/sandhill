//! Async client for `sandhilld`, usable over any byte stream: a Unix socket
//! locally, or the stdio of `ssh host sandhilld attach` for a remote server.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

use crate::{
    AgentInfo, AgentOptions, AppRequest, Blob, Call, CellRef, DirEntry, Event, FileOutputs,
    FileStat, FileText, FileWritten, Focus, GitAction, GitCommitInfo, GitDiff, GitDone, GitLog,
    GitMessage, GitOwnCommits, GitStatus, HelpPage, HelpSource, HelpTopics, HistoryPage, JobSpec,
    KernelspecInfo, Outcome, PackageUpdates, Packages, PageUrl, PingReply, PreviewInfo,
    ProjectInfo, Request, RunStarted, ServerMessage, SessionInfo, SessionPlots, SessionRuns,
    Subscribed, SystemStats, TableFilter, TableRows, TableSort, TableSummary, TerminalEvent,
    TerminalInfo, TerminalSubscribed, VariableMatches, Variables,
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
    #[error("cannot read file: {0}")]
    InvalidFile(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Outcome>>>>;
/// Where each open tunnel's incoming bytes go.
type Tunnels = Arc<std::sync::Mutex<HashMap<u64, mpsc::UnboundedSender<Vec<u8>>>>>;

/// A connection to a port on the daemon's machine (see
/// [`Client::open_tunnel`]). `incoming` ends when that connection does.
pub struct Tunnel {
    pub id: u64,
    pub incoming: mpsc::UnboundedReceiver<Vec<u8>>,
}

pub struct Client {
    writer: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: Pending,
    events: Option<mpsc::UnboundedReceiver<Event>>,
    terminal_events: Option<mpsc::UnboundedReceiver<TerminalEvent>>,
    lsp_messages: Option<mpsc::UnboundedReceiver<crate::LspMessage>>,
    requests: Option<mpsc::UnboundedReceiver<AppRequest>>,
    tunnels: Tunnels,
    next_id: AtomicU64,
    next_tunnel: AtomicU64,
    closed: watch::Receiver<bool>,
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
        let (lsp_tx, lsp_messages) = mpsc::unbounded_channel();
        let tunnels: Tunnels = Default::default();
        let (closed_tx, closed) = watch::channel(false);
        tokio::spawn(read_loop(
            reader,
            pending.clone(),
            Senders {
                events: event_tx,
                terminal_events: terminal_tx,
                requests: request_tx,
                lsp_messages: lsp_tx,
                tunnels: tunnels.clone(),
                closed: closed_tx,
            },
        ));
        Client {
            writer: Mutex::new(Box::new(writer)),
            pending,
            events: Some(events),
            terminal_events: Some(terminal_events),
            lsp_messages: Some(lsp_messages),
            requests: Some(requests),
            tunnels,
            next_id: AtomicU64::new(1),
            next_tunnel: AtomicU64::new(1),
            closed,
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

    /// Waits for the connection's read side to close, including an EOF that
    /// arrived before this call. Multiple waiters do not consume the signal.
    pub async fn wait_closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// Sends a request the daemon doesn't answer.
    async fn send(&self, call: Call) -> Result<()> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut line = serde_json::to_vec(&Request {
            id,
            call,
            project: None,
        })?;
        line.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer.write_all(&line).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Connects to `127.0.0.1:port` on the daemon's machine.
    pub async fn open_tunnel(&self, port: u16) -> Result<Tunnel> {
        let id = self.next_tunnel.fetch_add(1, Ordering::Relaxed);
        let (tx, incoming) = mpsc::unbounded_channel();
        self.tunnels.lock().unwrap().insert(id, tx);
        if let Err(err) = self
            .call::<serde_json::Value>(Call::OpenTunnel { tunnel: id, port })
            .await
        {
            self.tunnels.lock().unwrap().remove(&id);
            return Err(err);
        }
        Ok(Tunnel { id, incoming })
    }

    /// Sends bytes through a tunnel, in order.
    pub async fn tunnel_write(&self, tunnel: u64, data: &[u8]) -> Result<()> {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(data);
        self.send(Call::TunnelWrite { tunnel, data }).await
    }

    pub async fn close_tunnel(&self, tunnel: u64) -> Result<()> {
        self.tunnels.lock().unwrap().remove(&tunnel);
        self.send(Call::CloseTunnel { tunnel }).await
    }

    /// Sends a request and waits for its response.
    pub async fn call<T: DeserializeOwned>(&self, call: Call) -> Result<T> {
        self.call_in_project(None, call).await
    }

    pub async fn call_in_project<T: DeserializeOwned>(
        &self,
        project: Option<String>,
        call: Call,
    ) -> Result<T> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut line = serde_json::to_vec(&Request { id, call, project })?;
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

    pub fn take_lsp_messages(&mut self) -> Option<mpsc::UnboundedReceiver<crate::LspMessage>> {
        self.lsp_messages.take()
    }

    pub async fn next_lsp_message(&mut self) -> Option<crate::LspMessage> {
        self.lsp_messages.as_mut()?.recv().await
    }

    pub async fn complete(
        &self,
        session: &str,
        code: &str,
        cursor_pos: usize,
    ) -> Result<crate::CompletionReply> {
        self.call(Call::Complete {
            session: session.into(),
            code: code.into(),
            cursor_pos,
        })
        .await
    }

    pub async fn lsp_start(
        &self,
        project: &str,
        language: &str,
        document_kind: Option<String>,
        session: Option<String>,
    ) -> Result<crate::LspStarted> {
        self.call(Call::LspStart {
            project: project.into(),
            language: language.into(),
            document_kind,
            session,
        })
        .await
    }

    pub async fn lsp_send(&self, lsp: &str, message: serde_json::Value) -> Result<()> {
        self.call(Call::LspSend {
            lsp: lsp.into(),
            message,
        })
        .await
    }

    pub async fn lsp_stop(&self, lsp: &str) -> Result<()> {
        self.call(Call::LspStop { lsp: lsp.into() }).await
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

    /// Answers the question the session's running code asked.
    pub async fn input(&self, session: &str, value: &str) -> Result<()> {
        self.call(Call::Input {
            session: session.into(),
            value: value.into(),
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
            base64: false,
            create: false,
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
        self.call(Call::ListDir {
            path: path.into(),
            sizes: false,
            marks: false,
        })
        .await
    }

    pub async fn file_outputs(&self, file: &str) -> Result<FileOutputs> {
        self.call(Call::FileOutputs { file: file.into() }).await
    }

    pub async fn clear_cell_outputs(&self, file: &str, cell_ids: &[&str]) -> Result<usize> {
        self.call(Call::ClearCellOutputs {
            file: file.into(),
            cell_ids: cell_ids.iter().map(|c| c.to_string()).collect(),
        })
        .await
    }

    pub async fn session_runs(&self, session: &str, limit: Option<u32>) -> Result<SessionRuns> {
        self.call(Call::SessionRuns {
            session: session.into(),
            limit,
        })
        .await
    }

    pub async fn session_plots(&self, session: &str, limit: Option<u32>) -> Result<SessionPlots> {
        self.call(Call::SessionPlots {
            session: session.into(),
            limit,
        })
        .await
    }

    pub async fn session_pages(&self, session: &str, limit: Option<u32>) -> Result<SessionPlots> {
        self.call(Call::SessionPages {
            session: session.into(),
            limit,
        })
        .await
    }

    pub async fn preview_document(&self, path: &str) -> Result<PreviewInfo> {
        self.call(Call::PreviewDocument {
            path: path.into(),
            project: None,
        })
        .await
    }

    pub async fn serve_page(&self, path: &str) -> Result<PageUrl> {
        self.call(Call::ServePage { path: path.into() }).await
    }

    pub async fn serve_html(&self, html: &str) -> Result<PageUrl> {
        self.call(Call::ServeHtml {
            html: Some(html.into()),
            blob: None,
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

    pub async fn find_variables(
        &self,
        session: &str,
        query: &str,
        root: &[String],
    ) -> Result<VariableMatches> {
        self.call(Call::FindVariables {
            session: session.into(),
            query: query.into(),
            root: root.to_vec(),
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

    #[allow(clippy::too_many_arguments)]
    pub async fn table_rows(
        &self,
        session: &str,
        path: &[String],
        offset: u64,
        limit: Option<u32>,
        sort: &[TableSort],
        filters: &[TableFilter],
        search: &str,
    ) -> Result<TableRows> {
        self.call(Call::TableRows {
            session: session.into(),
            path: path.to_vec(),
            offset,
            limit,
            sort: sort.to_vec(),
            filters: filters.to_vec(),
            search: search.into(),
            columns: None,
            column_offset: 0,
            column_limit: None,
            cells_only: false,
        })
        .await
    }

    pub async fn table_summary(
        &self,
        session: &str,
        path: &[String],
        filters: &[TableFilter],
        search: &str,
    ) -> Result<TableSummary> {
        self.call(Call::TableSummary {
            session: session.into(),
            path: path.to_vec(),
            filters: filters.to_vec(),
            search: search.into(),
            column_offset: 0,
            column_limit: None,
        })
        .await
    }

    pub async fn table_export(
        &self,
        session: &str,
        path: &[String],
        sort: &[TableSort],
        filters: &[TableFilter],
        file: &str,
        search: &str,
    ) -> Result<FileWritten> {
        self.call(Call::TableExport {
            session: session.into(),
            path: path.to_vec(),
            sort: sort.to_vec(),
            filters: filters.to_vec(),
            search: search.into(),
            file: file.into(),
        })
        .await
    }

    pub async fn help_topics(
        &self,
        session: &str,
        query: &str,
        package: Option<&str>,
        limit: Option<u32>,
    ) -> Result<HelpTopics> {
        self.call(Call::HelpTopics {
            session: session.into(),
            query: query.into(),
            package: package.map(Into::into),
            limit,
        })
        .await
    }

    pub async fn help_page(
        &self,
        session: &str,
        topic: &str,
        package: Option<&str>,
    ) -> Result<HelpPage> {
        self.call(Call::HelpPage {
            session: session.into(),
            topic: topic.into(),
            package: package.map(Into::into),
        })
        .await
    }

    pub async fn help_source(
        &self,
        session: &str,
        topic: &str,
        package: Option<&str>,
    ) -> Result<HelpSource> {
        self.call(Call::HelpSource {
            session: session.into(),
            topic: topic.into(),
            package: package.map(Into::into),
        })
        .await
    }

    pub async fn list_packages(&self, session: &str) -> Result<Packages> {
        self.call(Call::ListPackages {
            session: session.into(),
        })
        .await
    }

    pub async fn package_updates(&self, session: &str) -> Result<PackageUpdates> {
        self.call(Call::PackageUpdates {
            session: session.into(),
        })
        .await
    }

    pub async fn git_status(&self, path: &str) -> Result<GitStatus> {
        self.call(Call::GitStatus { path: path.into() }).await
    }

    pub async fn git_log(&self, path: &str, skip: u32, limit: Option<u32>) -> Result<GitLog> {
        self.call(Call::GitLog {
            path: path.into(),
            skip,
            limit,
        })
        .await
    }

    pub async fn git_own_commits(&self, path: &str) -> Result<GitOwnCommits> {
        self.call(Call::GitOwnCommits { path: path.into() }).await
    }

    pub async fn git_show(&self, path: &str, sha: &str) -> Result<GitCommitInfo> {
        self.call(Call::GitShow {
            path: path.into(),
            sha: sha.into(),
        })
        .await
    }

    pub async fn git_diff(
        &self,
        path: &str,
        file: &str,
        staged: bool,
        sha: Option<&str>,
    ) -> Result<GitDiff> {
        self.call(Call::GitDiff {
            path: path.into(),
            file: file.into(),
            staged,
            sha: sha.map(Into::into),
        })
        .await
    }

    pub async fn git_run(&self, path: &str, action: GitAction) -> Result<GitDone> {
        self.call(Call::GitRun {
            path: path.into(),
            action,
        })
        .await
    }

    pub async fn git_write_message(&self, path: &str, agent: Option<&str>) -> Result<GitMessage> {
        self.call(Call::GitWriteMessage {
            path: path.into(),
            agent: agent.map(Into::into),
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

    pub async fn trust_folder(&self, path: &str) -> Result<()> {
        self.call(Call::TrustFolder { path: path.into() }).await
    }

    pub async fn untrust_folder(&self, path: &str) -> Result<()> {
        self.call(Call::UntrustFolder { path: path.into() }).await
    }

    pub async fn list_trusted_folders(&self) -> Result<Vec<crate::TrustedFolder>> {
        self.call(Call::ListTrustedFolders).await
    }

    pub async fn usage_stats(
        &self,
        days: u32,
        tz_offset_minutes: i32,
    ) -> Result<crate::UsageStats> {
        self.call(Call::UsageStats {
            days,
            tz_offset_minutes,
        })
        .await
    }

    pub async fn stat_files(&self, paths: Vec<String>) -> Result<Vec<FileStat>> {
        self.call(Call::StatFiles { paths }).await
    }

    pub async fn list_agents(&self) -> Result<Vec<AgentInfo>> {
        self.call(Call::ListAgents).await
    }

    pub async fn agent_models(&self, kind: &str) -> Result<serde_json::Value> {
        self.call(Call::AgentModels { kind: kind.into() }).await
    }

    pub async fn agent_commands(&self, kind: &str, cwd: &str) -> Result<Vec<serde_json::Value>> {
        self.call(Call::AgentCommands {
            kind: kind.into(),
            cwd: cwd.into(),
        })
        .await
    }

    pub async fn agent_accounts(&self) -> Result<Vec<serde_json::Value>> {
        self.call(Call::AgentAccounts).await
    }

    pub async fn agent_accounts_query(
        &self,
        kinds: Vec<String>,
        force: bool,
    ) -> Result<Vec<serde_json::Value>> {
        self.call(Call::AgentAccountsQuery { kinds, force }).await
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

    /// Runs `job` in the background (see [`Call::StartJob`]).
    pub async fn start_job(&self, job: JobSpec, project: Option<&str>) -> Result<TerminalInfo> {
        self.call(Call::StartJob {
            job,
            cwd: None,
            project: project.map(Into::into),
        })
        .await
    }

    pub async fn stop_terminal(&self, terminal: &str) -> Result<()> {
        self.call(Call::StopTerminal {
            terminal: terminal.into(),
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

/// Where the read loop hands what the daemon pushes.
struct Senders {
    events: mpsc::UnboundedSender<Event>,
    terminal_events: mpsc::UnboundedSender<TerminalEvent>,
    lsp_messages: mpsc::UnboundedSender<crate::LspMessage>,
    requests: mpsc::UnboundedSender<AppRequest>,
    tunnels: Tunnels,
    closed: watch::Sender<bool>,
}

async fn read_loop<R: AsyncRead + Unpin>(reader: R, pending: Pending, to: Senders) {
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
                let _ = to.events.send(event);
            }
            Ok(ServerMessage::Terminal { terminal }) => {
                let _ = to.terminal_events.send(terminal);
            }
            Ok(ServerMessage::LspMessage { lsp, message }) => {
                let _ = to.lsp_messages.send(crate::LspMessage { lsp, message });
            }
            Ok(ServerMessage::Request { request }) => {
                let _ = to.requests.send(request);
            }
            Ok(ServerMessage::Tunnel { tunnel }) => {
                use base64::Engine;
                let mut tunnels = to.tunnels.lock().unwrap();
                let bytes = tunnel
                    .data
                    .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
                match bytes {
                    Some(bytes) if !tunnel.closed => {
                        if let Some(tx) = tunnels.get(&tunnel.id) {
                            let _ = tx.send(bytes);
                        }
                    }
                    // Dropping the sender ends the tunnel's `incoming`.
                    _ => {
                        tunnels.remove(&tunnel.id);
                    }
                }
            }
            // A newer daemon may send messages this client doesn't know.
            Err(_) => continue,
        }
    }
    // Dropping the senders wakes every waiting call with `Disconnected`.
    pending.lock().unwrap().clear();
    to.tunnels.lock().unwrap().clear();
    to.closed.send_replace(true);
}

#[cfg(test)]
mod tests {
    use super::Client;

    #[tokio::test]
    async fn closed_connection_remembers_eof_for_late_and_multiple_waiters() {
        let (stream, peer) = tokio::io::duplex(64);
        let (reader, writer) = tokio::io::split(stream);
        let client = Client::new(reader, writer);
        drop(peer);
        client.wait_closed().await;
        tokio::time::timeout(std::time::Duration::from_secs(1), client.wait_closed())
            .await
            .unwrap();
        let ((), ()) = tokio::join!(client.wait_closed(), client.wait_closed());
    }
}
