//! The daemon: owns every session and serves clients over a byte stream.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::terminal::{Terminals, ToHost};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sandhill_proto::{
    Blob, Call, CellEdited, ChatHistory, Event, FileWritten, HelpPage, HelpSource, HelpTopics,
    HistoryPage, Outcome, PROTOCOL_VERSION, PackageUpdates, Packages, PingReply, PlotRedraw,
    Request, ServerMessage, SessionStatus, Subscribed, TERMINAL_JOB, TableRows, TableSummary,
    Variables,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::blobs::BlobStore;
use crate::chats;
use crate::journal::Journal;
use crate::pages::Pages;
use crate::paths::StateDir;
use crate::session::{self, SessionHandle};
use crate::tunnels::Tunnels;
use crate::windows::Windows;
use crate::{
    cells, fileops, files, help, memory, notebook, outputs, packages, projects, redraw, sidecar,
    tables, variables, windows,
};

mod document_calls;
mod help_calls;
mod session_calls;
mod system_calls;
mod table_calls;
mod terminal_calls;

const REPLAY_PAGE: u32 = 500;
/// How much of its log `read_log` returns unless asked for another amount.
const LOG_TAIL: u64 = 64 * 1024;
const HISTORY_LIMIT: u32 = 5000;
/// How many runs the Console gets back unless it asks for another number.
const CONSOLE_RUNS: u32 = 100;
const MAX_CONSOLE_RUNS: u32 = 1000;
/// How many plots the Plots pane gets back unless it asks for another number.
const PLOTS: u32 = 100;
const MAX_PLOTS: u32 = 500;
/// Pages the Viewer reads back from a session's journal.
const PAGES: u32 = 20;
const MAX_PAGES: u32 = 100;

pub struct Daemon {
    pub(crate) state: StateDir,
    pub(crate) journal: Arc<Journal>,
    pub(crate) sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
    pub(crate) terminals: Terminals,
    windows: Windows,
    pages: Pages,
    pub(crate) language_servers: crate::lsp::LanguageServers,
    next_connection: AtomicU64,
    options: DaemonOptions,
    /// Milliseconds since the Unix epoch.
    started_ms: i64,
    user: String,
    /// This machine's name, for conversations kept with their project.
    machine: String,
    /// Where the agents keep their own conversations here.
    homes: chats::Homes,
    /// Projects opened since the daemon started, whose conversations from
    /// other machines show in the history.
    opened: Mutex<HashSet<String>>,
    /// Git actions under way, by their askpass token: where their prompts
    /// for a password go.
    askers: Mutex<HashMap<String, Asker>>,
    /// Ends `serve`, for `stop_daemon`.
    stop: Arc<Notify>,
    lifecycle_gate: tokio::sync::RwLock<()>,
    pub(crate) maintenance: AtomicBool,
    pub(crate) stopping: AtomicBool,
    pub(crate) analysis_gate: tokio::sync::Mutex<()>,
}

/// The window a git action came from, on its connection.
struct Asker {
    out: mpsc::UnboundedSender<ServerMessage>,
    project: String,
}

/// How long git waits for someone to type a password.
const CREDENTIAL_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// How often conversations are copied to and from their projects.
const CHAT_SYNC: std::time::Duration = std::time::Duration::from_secs(5);

/// What the daemon needs to know about how it runs.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// `sandhilld` itself: terminal hosts, agents' hooks and their MCP
    /// server run it.
    pub executable: PathBuf,
    /// The socket clients reach it on, which agents' MCP servers use.
    pub socket: PathBuf,
    /// What started it, as `ping` reports: `app`, `launchd`, `systemd` or
    /// empty.
    pub started_by: String,
}

impl DaemonOptions {
    pub fn for_state(state: &StateDir) -> Self {
        DaemonOptions {
            executable: crate::version::executable(),
            socket: state.socket_path(),
            started_by: String::new(),
        }
    }
}

impl Daemon {
    /// Opens the state directory and reconnects to kernels that kept running
    /// while the daemon was down.
    pub async fn open(state: StateDir) -> Result<Arc<Daemon>> {
        let options = DaemonOptions::for_state(&state);
        Self::open_with(state, options).await
    }

    pub async fn open_with(state: StateDir, options: DaemonOptions) -> Result<Arc<Daemon>> {
        state
            .create()
            .context("cannot create the state directory")?;
        let journal = Arc::new(Journal::open(
            &state.db_path(),
            BlobStore::new(state.blobs_dir()),
        )?);
        let pages = Pages::open(state.root())?;
        let daemon = Arc::new(Daemon {
            terminals: Terminals::new(state.terminals_dir(), options.executable.clone()),
            pages,
            state,
            journal: journal.clone(),
            sessions: Mutex::new(HashMap::new()),
            windows: Windows::default(),
            language_servers: crate::lsp::LanguageServers::default(),
            next_connection: AtomicU64::new(1),
            options,
            started_ms: crate::journal::now_ms(),
            user: user_name(),
            machine: hostname(),
            homes: chats::Homes::from_env(),
            opened: Mutex::default(),
            askers: Mutex::default(),
            stop: Arc::default(),
            lifecycle_gate: tokio::sync::RwLock::new(()),
            maintenance: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            analysis_gate: tokio::sync::Mutex::new(()),
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

    /// Serves clients until `stop_daemon` asks it to stop.
    pub async fn serve(self: Arc<Self>, listener: UnixListener) -> std::io::Result<()> {
        // Conversations go to and from their projects while it serves.
        let daemon = Arc::downgrade(&self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CHAT_SYNC).await;
                let Some(daemon) = daemon.upgrade() else {
                    return;
                };
                let _guard = daemon.lifecycle_gate.read().await;
                if daemon.maintenance.load(Ordering::Acquire) {
                    continue;
                }
                let syncing = daemon.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    chats::sync(&syncing.terminals, &syncing.homes, &syncing.machine);
                })
                .await;
            }
        });
        let (idle_limit, idle_check) = crate::terminal::idle::timing();
        let daemon = Arc::downgrade(&self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(idle_check).await;
                let Some(daemon) = daemon.upgrade() else {
                    return;
                };
                let _guard = daemon.lifecycle_gate.read().await;
                if !daemon.maintenance.load(Ordering::Acquire)
                    && !daemon.stopping.load(Ordering::Acquire)
                {
                    daemon.terminals.stop_idle(idle_limit).await;
                }
            }
        });
        loop {
            let (stream, _) = tokio::select! {
                accepted = listener.accept() => accepted?,
                _ = self.stop.notified() => return Ok(()),
            };
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
        let stop_reply = Arc::new(Mutex::new(None));
        let writer_task = tokio::spawn(write_loop(
            writer,
            out_rx,
            stop_reply.clone(),
            self.stop.clone(),
        ));
        let subscriptions: Subscriptions = Default::default();
        let tunnels = Tunnels::new(out.clone());
        let connection = self.next_connection.fetch_add(1, Ordering::Relaxed);
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
            // A tunnel's bytes are handled here, not in a task of their own,
            // so they keep their order.
            let request = match request.call {
                call @ (Call::LspStart { .. } | Call::LspSend { .. } | Call::LspStop { .. }) => {
                    // Preserve document synchronization order on the wire.
                    self.clone()
                        .handle(
                            Request {
                                id: request.id,
                                call,
                            },
                            connection,
                            out.clone(),
                            subscriptions.clone(),
                            stop_reply.clone(),
                        )
                        .await;
                    continue;
                }
                Call::OpenTunnel { tunnel, port } => {
                    tunnels.open(request.id, tunnel, port);
                    continue;
                }
                Call::TunnelWrite { tunnel, data } => {
                    tunnels.write(tunnel, &data);
                    continue;
                }
                Call::CloseTunnel { tunnel } => {
                    tunnels.close(tunnel);
                    continue;
                }
                call => Request {
                    id: request.id,
                    call,
                },
            };
            let large = line.len() > memory::LARGE;
            drop(line);
            let daemon = self.clone();
            let out = out.clone();
            let subscriptions = subscriptions.clone();
            let stop_reply = stop_reply.clone();
            tokio::spawn(async move {
                daemon
                    .handle(request, connection, out, subscriptions, stop_reply)
                    .await;
                if large {
                    memory::release_free_memory();
                }
            });
        }
        for (_, task) in subscriptions.lock().unwrap().drain() {
            task.abort();
        }
        drop(tunnels);
        self.windows.disconnected(connection);
        self.language_servers.disconnected(connection).await;
        drop(out);
        let _ = writer_task.await;
    }

    async fn handle(
        self: Arc<Self>,
        request: Request,
        connection: u64,
        out: mpsc::UnboundedSender<ServerMessage>,
        subscriptions: Subscriptions,
        stop_reply: Arc<Mutex<Option<u64>>>,
    ) {
        let id = request.id;
        let shutdown_request =
            matches!(request.call, Call::StopDaemon | Call::PrepareAttach { .. });
        let exclusive = matches!(
            request.call,
            Call::StopResources { .. }
                | Call::CleanStorage { .. }
                | Call::UninstallComponents { .. }
                | Call::StopDaemon
                | Call::PrepareAttach { .. }
        );
        let (_read, _write) = if exclusive {
            (None, Some(self.lifecycle_gate.write().await))
        } else {
            (Some(self.lifecycle_gate.read().await), None)
        };
        if self.stopping.load(Ordering::Acquire) && !matches!(request.call, Call::Ping) {
            let _ = out.send(response(
                id,
                Err(anyhow!(
                    "daemon shutdown is in progress; reconnect after it exits"
                )),
            ));
            return;
        }
        if self.maintenance.load(Ordering::Acquire)
            && !crate::lifecycle::allowed_during_maintenance(&request.call)
        {
            let _ = out.send(response(id, Err(anyhow!("resources are stopped for maintenance; reconnect after an explicit daemon restart"))));
            return;
        }
        if let Err(err) = self.check_trust(&request.call).await {
            let _ = out.send(response(id, Err(err)));
            return;
        }
        let result = match request.call {
            Call::LspStart {
                project,
                language,
                document_kind: _,
                session,
            } => {
                self.start_lsp(project, language, session, connection, out.clone())
                    .await
            }
            Call::LspSend { lsp, message } => self
                .language_servers
                .send(&lsp, connection, message)
                .await
                .map(|()| Value::Null),
            Call::LspStop { lsp } => self
                .language_servers
                .stop(&lsp, connection)
                .await
                .map(|()| Value::Null),
            Call::SetFocus { project, focus } => projects::normalize(&project).map(|project| {
                self.windows.set_focus(connection, &out, project, focus);
                Value::Null
            }),
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
            Call::SubscribeTerminal {
                terminal,
                from,
                chat_from,
            } => {
                match self.terminals.open(&terminal, from, chat_from).await {
                    Ok(stream) => {
                        let _ = out.send(response(
                            id,
                            Ok(serde_json::to_value(stream.subscribed()).unwrap_or_default()),
                        ));
                        let task = tokio::spawn(stream.forward(out.clone()));
                        if let Some(old) = subscriptions
                            .lock()
                            .unwrap()
                            .insert(terminal_key(&terminal), task)
                        {
                            old.abort();
                        }
                    }
                    Err(err) => {
                        let _ = out.send(response(id, Err(err)));
                    }
                }
                return;
            }
            Call::UnsubscribeTerminal { terminal } => {
                if let Some(task) = subscriptions
                    .lock()
                    .unwrap()
                    .remove(&terminal_key(&terminal))
                {
                    task.abort();
                }
                Ok(Value::Null)
            }
            Call::GitRun { path, action } => self.git_run(path, action, &out).await,
            call => self.call(call).await,
        };
        let stopping = shutdown_request && result.is_ok() && self.stopping.load(Ordering::Acquire);
        if stopping {
            *stop_reply.lock().unwrap() = Some(id);
        }
        if out.send(response(id, result)).is_err() && stopping {
            // Even a vanished client must not cancel an accepted shutdown.
            self.stop.notify_one();
        }
    }
    async fn agent_accounts(&self, kinds: Option<&[String]>, force: bool) -> Result<Value> {
        let mut accounts = crate::agents::accounts(kinds, force).await?;
        if let Some(accounts) = accounts.as_array_mut() {
            for terminal in self.terminals.list() {
                if kinds.is_some_and(|kinds| !kinds.contains(&terminal.kind)) {
                    continue;
                }
                if let Some(mut report) = terminal.agent
                    && report.is_object()
                {
                    report["kind"] = json!(terminal.kind);
                    report["feed"] = json!(true);
                    accounts.push(report);
                }
            }
        }
        Ok(accounts)
    }

    /// Runs a git action; prompts for a password go to the window that
    /// asked, on `out`.
    async fn git_run(
        &self,
        path: String,
        action: sandhill_proto::GitAction,
        out: &mpsc::UnboundedSender<ServerMessage>,
    ) -> Result<Value> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let asker = Asker {
            out: out.clone(),
            project: path.clone(),
        };
        self.askers.lock().unwrap().insert(token.clone(), asker);
        let askpass = crate::git::Askpass {
            program: self.options.executable.clone(),
            socket: self.options.socket.clone(),
            token: token.clone(),
        };
        let done = crate::git::ASKPASS
            .scope(askpass, crate::git::run_action(&path, action))
            .await;
        self.askers.lock().unwrap().remove(&token);
        Ok(serde_json::to_value(done?)?)
    }

    /// What the user types for a git action's prompt, in its window.
    async fn git_credential(&self, token: &str, prompt: String) -> Result<Value> {
        let (out, project) = {
            let askers = self.askers.lock().unwrap();
            let asker = askers.get(token).context("that git action has finished")?;
            (asker.out.clone(), asker.project.clone())
        };
        let secret =
            !prompt.to_ascii_lowercase().starts_with("username") && !prompt.contains("(yes/no");
        let params = json!({"prompt": prompt.trim_end(), "secret": secret});
        match self
            .windows
            .ask_on(&out, project, "git_credential", params, CREDENTIAL_WAIT)
            .await
        {
            Some(Ok(Value::String(text))) => Ok(Value::String(text)),
            Some(Ok(_)) => bail!("the window's answer made no sense"),
            Some(Err(crate::windows::Unanswered::Said(message))) => bail!("{message}"),
            Some(Err(crate::windows::Unanswered::Late)) => bail!("no one answered"),
            Some(Err(crate::windows::Unanswered::Closed)) | None => {
                bail!("the Sandhill window closed")
            }
        }
    }

    async fn start_lsp(
        &self,
        project: String,
        language: String,
        session: Option<String>,
        connection: u64,
        out: mpsc::UnboundedSender<ServerMessage>,
    ) -> Result<Value> {
        let project = projects::normalize(&project)?;
        if !Path::new(&project).is_dir() {
            bail!("{project} is not a project directory");
        }
        let language = language.to_ascii_lowercase();
        let records = self.journal.sessions()?;
        let record = if let Some(id) = session {
            let record = records
                .iter()
                .find(|record| record.id == id)
                .context("no such session")?;
            if record.project.as_deref() != Some(project.as_str())
                || record.language.to_ascii_lowercase() != language
            {
                bail!("language session must belong to this project and language");
            }
            Some(record)
        } else {
            records
                .iter()
                .filter(|record| {
                    record.project.as_deref() == Some(project.as_str())
                        && record.language.to_ascii_lowercase() == language
                        && record.status != SessionStatus::Ended
                })
                .max_by_key(|record| record.created_ms)
        };
        let mut interpreter = None;
        let ark = if language == "r" {
            record.map(|record| self.live(&record.id)).transpose()?
        } else {
            None
        };
        if language == "python" {
            let kernel = match record {
                Some(record) => Some(record.kernel.clone()),
                None => {
                    let root = PathBuf::from(&project);
                    blocking(move || Ok(projects::setup(&root).python)).await?
                }
            };
            if let Some(kernel) = kernel {
                let spec =
                    crate::kernel::find_kernelspec(&kernel, Some(Path::new(&project))).await?;
                interpreter = spec
                    .kernelspec
                    .argv
                    .first()
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute());
            }
            if interpreter.is_none() {
                interpreter = [".venv/bin/python", ".venv/bin/python3", "venv/bin/python"]
                    .into_iter()
                    .map(|relative| Path::new(&project).join(relative))
                    .find(|path| path.is_file());
            }
        }
        let start = crate::lsp::Start {
            project,
            language,
            interpreter,
            ark,
        };
        Ok(serde_json::to_value(
            self.language_servers.start(start, connection, out).await?,
        )?)
    }

    /// Refuses a call that starts something acting on the files of an
    /// untrusted project: a kernel (`.Rprofile`, env files), a terminal, job
    /// or agent (`CLAUDE.md`, hooks), a language server, a Quarto render, or
    /// Git (a repository's configuration can name programs to run).
    async fn check_trust(&self, call: &Call) -> Result<()> {
        let paths = |items: [Option<&String>; 2]| -> Vec<PathBuf> {
            items.into_iter().flatten().map(PathBuf::from).collect()
        };
        let (folders, what) = match call {
            Call::StartSession { cwd, project, .. } => {
                (paths([cwd.as_ref(), project.as_ref()]), "start R or Python")
            }
            Call::LspStart { project, .. } => (paths([Some(project), None]), "check code"),
            Call::StartTerminal {
                kind, cwd, project, ..
            } => (
                paths([cwd.as_ref(), project.as_ref()]),
                if kind == sandhill_proto::TERMINAL_SHELL {
                    "use the Terminal"
                } else {
                    "use AI agents"
                },
            ),
            Call::StartJob { cwd, project, .. } => {
                (paths([cwd.as_ref(), project.as_ref()]), "run jobs")
            }
            Call::RestartTerminal { terminal, .. } => match self.terminals.info(terminal) {
                Ok(info) => (vec![PathBuf::from(info.cwd)], "start it again"),
                Err(_) => return Ok(()),
            },
            Call::PreviewDocument { path, project } => {
                (paths([Some(path), project.as_ref()]), "render documents")
            }
            Call::SuggestCell { cwd, .. } => (paths([Some(cwd), None]), "use AI agents"),
            Call::GitStatus { path }
            | Call::GitLog { path, .. }
            | Call::GitShow { path, .. }
            | Call::GitOwnCommits { path }
            | Call::GitDiff { path, .. }
            | Call::GitRun { path, .. }
            | Call::GitWriteMessage { path, .. } => (paths([Some(path), None]), "use Git"),
            _ => return Ok(()),
        };
        self.trusted(folders, what).await
    }

    async fn trusted(&self, folders: Vec<PathBuf>, what: &'static str) -> Result<()> {
        let journal = self.journal.clone();
        blocking(move || {
            for folder in &folders {
                projects::trust::check(&journal, folder, what)?;
            }
            Ok(())
        })
        .await
    }

    /// Tells an agent's run the trusted folder it starts in, so an agent that
    /// can be told per run does not ask about the folder again.
    async fn name_trust(&self, launch: &mut crate::terminal::Launch) -> Result<()> {
        if launch.kind == sandhill_proto::TERMINAL_SHELL {
            return Ok(());
        }
        let journal = self.journal.clone();
        let cwd = launch.cwd.clone();
        if let Some(folder) =
            blocking(move || projects::trust::trusted_root(&journal, &cwd)).await?
        {
            launch
                .env
                .push((projects::trust::TRUSTED_FOLDER.into(), folder));
        }
        Ok(())
    }

    async fn call(&self, call: Call) -> Result<Value> {
        Ok(match call {
            Call::BeginAnalysis { project, spec } => self.begin_analysis(&project, spec).await?,
            Call::CheckAnalysis {
                project,
                analysis,
                checkpoint,
                details,
            } => {
                self.check_analysis(&project, &analysis, &checkpoint, details)
                    .await?
            }
            Call::GetAnalysis { project, analysis } => {
                self.get_analysis(&project, &analysis).await?
            }
            Call::ResourceInventory => self.resource_inventory()?,
            Call::StopResources { force } => self.stop_resources(force).await?,
            Call::StorageInventory => self.storage_inventory()?,
            Call::CleanStorage { categories } => self.clean_storage(&categories).await?,
            Call::UninstallComponents { delete_data } => {
                self.uninstall_components(delete_data).await?
            }
            Call::Ping => self.ping()?,
            Call::ListKernelspecs => {
                serde_json::to_value(crate::kernel::list_kernelspecs(None).await)?
            }
            Call::ListProjectKernelspecs { path } => {
                let root = PathBuf::from(projects::normalize(&path)?);
                serde_json::to_value(crate::kernel::list_kernelspecs(Some(root)).await)?
            }
            Call::ListRWithoutIrkernel { project } => self.list_r_without_irkernel(project).await?,
            Call::InstallIrkernel { r, project } => self.install_irkernel(r, project).await?,
            Call::StartSession {
                kernel,
                cwd,
                project,
            } => self.start_session(kernel, cwd, project).await?,
            Call::ListSessions => {
                let infos = self
                    .journal
                    .sessions()?
                    .iter()
                    .map(|r| self.journal.session_info(r))
                    .collect::<Result<Vec<_>, _>>()?;
                serde_json::to_value(infos)?
            }
            Call::Complete {
                session,
                code,
                cursor_pos,
            } => serde_json::to_value(self.live(&session)?.complete(code, cursor_pos).await?)?,
            Call::Inspect {
                session,
                code,
                cursor_pos,
            } => serde_json::to_value(self.live(&session)?.inspect(code, cursor_pos).await?)?,
            Call::LspStart { .. } | Call::LspSend { .. } | Call::LspStop { .. } => {
                bail!("language server subscriptions require a client connection")
            }
            Call::Execute {
                session,
                code,
                cell,
                quiet,
                by,
            } => serde_json::to_value(self.live(&session)?.execute(code, cell, quiet, by).await?)?,
            Call::SessionPlots { session, limit } => {
                let journal = self.journal.clone();
                let limit = limit.unwrap_or(PLOTS).min(MAX_PLOTS);
                serde_json::to_value(
                    blocking(move || Ok(outputs::session_plots(&journal, &session, limit)?))
                        .await?,
                )?
            }
            Call::SessionPages { session, limit } => {
                let journal = self.journal.clone();
                let limit = limit.unwrap_or(PAGES).min(MAX_PAGES);
                serde_json::to_value(
                    blocking(move || Ok(outputs::session_pages(&journal, &session, limit)?))
                        .await?,
                )?
            }
            Call::PreviewDocument { path, project } => self.preview_document(path, project).await?,
            Call::ServePage { path } => {
                let path = files::resolve(&path)?;
                serde_json::to_value(self.pages.serve(&path).await?)?
            }
            Call::ServeHtml { html, blob } => self.serve_html(html, blob).await?,
            Call::OpenTunnel { .. } | Call::TunnelWrite { .. } | Call::CloseTunnel { .. } => {
                bail!("tunnels are handled by the connection")
            }
            Call::SessionRuns { session, limit } => {
                let journal = self.journal.clone();
                let limit = limit.unwrap_or(CONSOLE_RUNS).min(MAX_CONSOLE_RUNS);
                serde_json::to_value(
                    blocking(move || Ok(outputs::session_runs(&journal, &session, limit)?)).await?,
                )?
            }
            Call::ListVariables { session, path } => self.list_variables(session, path).await?,
            Call::FindVariables {
                session,
                query,
                root,
            } => self.find_variables(session, query, root).await?,
            Call::DeleteVariables { session, names } => {
                if !names.is_empty() {
                    self.quietly(&session, "delete variables", |language| {
                        variables::delete_code(language, &names)
                    })
                    .await?;
                }
                Value::Null
            }
            Call::TableRows {
                session,
                path,
                offset,
                limit,
                sort,
                filters,
                search,
                columns,
                column_offset,
                column_limit,
                cells_only,
            } => {
                self.table_rows(
                    session,
                    path,
                    offset,
                    limit,
                    sort,
                    filters,
                    search,
                    columns,
                    column_offset,
                    column_limit,
                    cells_only,
                )
                .await?
            }
            Call::TableSummary {
                session,
                path,
                filters,
                search,
                column_offset,
                column_limit,
            } => {
                self.table_summary(session, path, filters, search, column_offset, column_limit)
                    .await?
            }
            Call::HelpTopics {
                session,
                query,
                package,
                limit,
            } => self.help_topics(session, query, package, limit).await?,
            Call::HelpPage {
                session,
                topic,
                package,
            } => self.help_page(session, topic, package).await?,
            Call::HelpSource {
                session,
                topic,
                package,
            } => self.help_source(session, topic, package).await?,
            Call::ListPackages { session } => self.list_packages(session).await?,
            Call::PackageUpdates { session } => self.package_updates(session).await?,
            Call::SuggestCell {
                agent,
                cwd,
                language,
                cells,
                output,
            } => {
                self.suggest_cell(agent, cwd, language, cells, output)
                    .await?
            }
            Call::GitStatus { path } => serde_json::to_value(crate::git::status(&path).await?)?,
            Call::GitLog { path, skip, limit } => {
                serde_json::to_value(crate::git::log(&path, skip, limit).await?)?
            }
            Call::GitShow { path, sha } => {
                serde_json::to_value(crate::git::show(&path, &sha).await?)?
            }
            Call::GitOwnCommits { path } => {
                serde_json::to_value(crate::git::own_commits(&path).await?)?
            }
            Call::GitDiff {
                path,
                file,
                staged,
                sha,
            } => {
                serde_json::to_value(crate::git::diff(&path, &file, staged, sha.as_deref()).await?)?
            }
            Call::GitRun { path, action } => {
                serde_json::to_value(crate::git::run_action(&path, action).await?)?
            }
            Call::GitCredential { token, prompt } => self.git_credential(&token, prompt).await?,
            Call::GitWriteMessage { path, agent } => {
                serde_json::to_value(crate::git::write_message(&path, agent.as_deref()).await?)?
            }
            Call::RedrawPlot {
                session,
                run,
                image,
                index,
                file: _,
                format: _,
                width,
                height,
                dpi,
                rasterize: _,
                show,
            } if show => {
                self.redraw_plot_shown(session, run, image, index, width, height, dpi)
                    .await?
            }
            Call::RedrawPlot {
                session,
                run,
                image,
                index,
                file,
                format,
                width,
                height,
                dpi,
                rasterize,
                show: _,
            } => {
                self.redraw_plot_saved(
                    session, run, image, index, file, format, width, height, dpi, rasterize,
                )
                .await?
            }
            Call::TableExport {
                session,
                path,
                sort,
                filters,
                search,
                file,
            } => {
                self.table_export(session, path, sort, filters, search, file)
                    .await?
            }
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
            Call::GetBlob { hash, near } => self.get_blob(hash, near)?,
            Call::ReadFile { path } => {
                serde_json::to_value(blocking(move || files::read_text(&path)).await?)?
            }
            Call::ReadNotebook { path } => self.read_notebook(path).await?,
            Call::WriteNotebook {
                path,
                text,
                expected_mtime_ms,
            } => self.write_notebook(path, text, expected_mtime_ms).await?,
            Call::WriteFile {
                path,
                text,
                expected_mtime_ms,
                base64,
                create,
            } => {
                self.write_file(path, text, expected_mtime_ms, base64, create)
                    .await?
            }
            Call::ListDir { path, sizes, marks } => {
                serde_json::to_value(blocking(move || files::list_dir(&path, sizes, marks)).await?)?
            }
            Call::MakeDir { path } => {
                serde_json::to_value(blocking(move || fileops::make_dir(&path)).await?)?
            }
            Call::MovePath { from, to } => {
                serde_json::to_value(blocking(move || fileops::move_path(&from, &to)).await?)?
            }
            Call::CopyPath { from, to } => {
                serde_json::to_value(blocking(move || fileops::copy_path(&from, &to)).await?)?
            }
            Call::DeletePath { path, trash } => {
                serde_json::to_value(blocking(move || fileops::delete_path(&path, trash)).await?)?
            }
            Call::CompressPath { path, format } => serde_json::to_value(
                blocking(move || crate::compress::compress(&path, format)).await?,
            )?,
            Call::ListFiles { path, limit } => {
                let limit = limit.map_or(fileops::LIST_FILES, |l| l as usize);
                serde_json::to_value(blocking(move || fileops::list_files(&path, limit)).await?)?
            }
            Call::ReadBytes {
                path,
                offset,
                limit,
            } => serde_json::to_value(
                blocking(move || fileops::read_bytes(&path, offset, limit)).await?,
            )?,
            Call::FileOutputs { file } => self.file_outputs(file).await?,
            Call::ClearCellOutputs { file, cell_ids } => {
                self.clear_cell_outputs(file, cell_ids).await?
            }
            Call::OpenProject { path } => {
                let journal = self.journal.clone();
                let info = blocking(move || projects::open(&journal, &path)).await?;
                self.opened.lock().unwrap().insert(info.path.clone());
                chats::adopt(&self.terminals, Path::new(&info.path));
                serde_json::to_value(info)?
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
            Call::GetProjectSettings { path } => {
                let root = PathBuf::from(projects::normalize(&path)?);
                serde_json::to_value(blocking(move || Ok(projects::setup(&root))).await?)?
            }
            Call::SaveProjectSettings { path, settings } => {
                self.save_project_settings(path, settings).await?
            }
            Call::ForgetProject { path } => {
                let journal = self.journal.clone();
                blocking(move || Ok(journal.forget_project(&projects::normalize(&path)?)?)).await?;
                Value::Null
            }
            Call::TrustFolder { path } => {
                let journal = self.journal.clone();
                blocking(move || projects::trust::trust(&journal, &path)).await?;
                Value::Null
            }
            Call::UntrustFolder { path } => {
                let journal = self.journal.clone();
                blocking(move || projects::trust::untrust(&journal, &path)).await?;
                Value::Null
            }
            Call::ListTrustedFolders => {
                let journal = self.journal.clone();
                serde_json::to_value(blocking(move || projects::trust::list(&journal)).await?)?
            }
            Call::SystemStats { path } => self.system_stats(path).await?,
            Call::ReadLog { bytes } => self.read_log(bytes).await?,
            Call::PrepareAttach { build } => self.prepare_attach(&build)?,
            Call::StopDaemon => {
                tracing::info!("daemon shutdown accepted; waiting for reply flush");
                self.stopping.store(true, Ordering::Release);
                Value::Null
            }
            Call::StatFiles { paths } => {
                serde_json::to_value(blocking(move || Ok(files::stat(&paths))).await?)?
            }
            Call::ListAgents => serde_json::to_value(crate::agents::list().await)?,
            Call::AgentModels { kind } => crate::agents::models(&kind).await?,
            Call::AgentAccounts => self.agent_accounts(None, false).await?,
            Call::AgentAccountsQuery { kinds, force } => {
                self.agent_accounts(Some(&kinds), force).await?
            }
            Call::ListAgentAccounts { refresh, ids } => serde_json::to_value(
                crate::accounts::Registry::from_env()
                    .list(refresh, ids.as_deref())
                    .await?,
            )?,
            Call::AddAgentAccount {
                kind,
                name,
                api_key,
            } => serde_json::to_value(crate::accounts::Registry::from_env().add(
                &kind,
                &name,
                api_key.as_deref(),
            )?)?,
            Call::RenameAgentAccount { id, name } => {
                serde_json::to_value(crate::accounts::Registry::from_env().rename(&id, &name)?)?
            }
            Call::RemoveAgentAccount { id } => {
                crate::accounts::Registry::from_env().remove(&id, &self.terminals.list())?;
                Value::Null
            }
            Call::SignInAgentAccount { id, cols, rows } => {
                self.sign_in_agent_account(id, cols, rows).await?
            }
            Call::UseAgentReset { id, credit } => {
                crate::accounts::Registry::from_env()
                    .use_reset(&id, credit.as_deref())
                    .await?
            }
            Call::StartTerminal {
                kind,
                cwd,
                project,
                cols,
                rows,
                options,
                prompt,
            } => {
                self.start_terminal(kind, cwd, project, cols, rows, options, prompt)
                    .await?
            }
            Call::StartJob { job, cwd, project } => self.start_job(job, cwd, project).await?,
            Call::StopTerminal { terminal } => {
                self.terminals.stop(&terminal).await?;
                Value::Null
            }
            Call::RestartTerminal {
                terminal,
                options: _,
                cols: _,
                rows: _,
                prompt: _,
            } if self
                .terminals
                .info(&terminal)
                .is_ok_and(|t| t.kind == TERMINAL_JOB) =>
            {
                self.restart_terminal_job(terminal).await?
            }
            Call::RestartTerminal {
                terminal,
                options,
                cols,
                rows,
                prompt,
            } => {
                self.restart_terminal_agent(terminal, options, cols, rows, prompt)
                    .await?
            }
            Call::RenameTerminal { terminal, name } => {
                serde_json::to_value(self.terminals.rename(&terminal, name.as_deref())?)?
            }
            Call::ListTerminals => {
                let opened: Vec<String> = self.opened.lock().unwrap().iter().cloned().collect();
                for project in opened {
                    chats::adopt(&self.terminals, Path::new(&project));
                }
                serde_json::to_value(self.terminals.list())?
            }
            Call::TerminalInput { terminal, data } => {
                self.terminals
                    .send(&terminal, ToHost::Input { data })
                    .await?;
                Value::Null
            }
            Call::AnswerTerminal {
                terminal,
                id,
                answer,
            } => {
                self.terminals.answer(&terminal, &id, answer).await?;
                Value::Null
            }
            Call::TellTerminal { terminal, command } => {
                self.terminals
                    .send(&terminal, ToHost::Command { command })
                    .await?;
                Value::Null
            }
            Call::ResizeTerminal {
                terminal,
                cols,
                rows,
            } => {
                self.terminals
                    .send(&terminal, ToHost::Resize { cols, rows })
                    .await?;
                Value::Null
            }
            Call::CloseTerminal { terminal } => {
                let project = self.terminals.info(&terminal)?.project;
                self.terminals.close(&terminal).await?;
                if let Some(project) = project {
                    chats::forget(Path::new(&project), &terminal, &self.machine)?;
                }
                Value::Null
            }
            Call::GetFocus { project } => {
                serde_json::to_value(self.windows.focus(&projects::normalize(&project)?))?
            }
            Call::AddCell {
                file,
                code,
                language,
                after,
                run,
                project,
                by,
            } => {
                self.add_cell(file, code, language, after, run, project, by)
                    .await?
            }
            Call::EditCell {
                file,
                cell_id,
                code,
                run,
                project,
                by,
            } => {
                self.edit_cell(file, cell_id, code, run, project, by)
                    .await?
            }
            Call::DeleteCell {
                file,
                cell_id,
                project,
                by,
            } => self.delete_cell(file, cell_id, project, by).await?,
            Call::Answer { id, result, error } => {
                let answer = match (result, error) {
                    (_, Some(error)) => Err(error),
                    (result, None) => Ok(result.unwrap_or(Value::Null)),
                };
                self.windows.answer(id, answer);
                Value::Null
            }
            Call::Subscribe { .. }
            | Call::Unsubscribe { .. }
            | Call::SubscribeTerminal { .. }
            | Call::UnsubscribeTerminal { .. }
            | Call::SetFocus { .. } => unreachable!("handled earlier"),
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

    /// Has the window with `path` open make an edit, if there is one.
    async fn ask_window(
        &self,
        path: &std::path::Path,
        project: Option<String>,
        method: &str,
        params: Value,
    ) -> Result<Option<CellEdited>> {
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let Some(answer) = self
            .windows
            .ask(path, project.as_deref(), method, params)
            .await
        else {
            return Ok(None);
        };
        let mut answer = answer?;
        answer["file"] = json!(path.to_string_lossy());
        answer["in_window"] = json!(true);
        let edited = serde_json::from_value(answer).context("the window's answer made no sense")?;
        Ok(Some(edited))
    }

    /// Keeps the Mac awake while a job runs.
    fn keep_awake(&self, job: &str) {
        if let Some(pid) = self.terminals.pid(job) {
            crate::jobs::keep_awake(pid);
        }
    }

    fn insert(&self, handle: SessionHandle) {
        self.sessions
            .lock()
            .unwrap()
            .insert(handle.id.clone(), Arc::new(handle));
    }

    /// Runs a tool's code quietly in an idle session: the code `code` gives
    /// for the session's language. `what` says what for, in errors.
    async fn quietly(
        &self,
        session: &str,
        what: &str,
        code: impl FnOnce(&str) -> Option<String>,
    ) -> Result<variables::Printed> {
        self.quietly_for(session, what, variables::TIMEOUT, code)
            .await
    }

    /// As [`Self::quietly`], waiting up to `timeout`.
    async fn quietly_for(
        &self,
        session: &str,
        what: &str,
        timeout: std::time::Duration,
        code: impl FnOnce(&str) -> Option<String>,
    ) -> Result<variables::Printed> {
        let info = self.session_info(session)?;
        let code = code(&info.language)
            .with_context(|| format!("cannot {what} in a {} session", info.language))?;
        if info.pending_runs > 0 {
            bail!("the session is busy running code; try again once it is idle");
        }
        let handle = self.live(session)?;
        let printed = variables::run_quietly(&handle, &self.journal, code, timeout)
            .await
            .with_context(|| format!("could not {what}"))?;
        if let Some(error) = &printed.error {
            bail!("could not {what}: {error}");
        }
        Ok(printed)
    }

    /// Draws a plot again into `save`'s file, or says whether it can.
    async fn redraw_plot(
        &self,
        session: &str,
        run: &str,
        image: Option<&str>,
        index: u32,
        save: Option<redraw::Save<'_>>,
    ) -> Result<PlotRedraw> {
        let cannot = |reason: &str| PlotRedraw {
            can: false,
            reason: Some(reason.to_string()),
            png: None,
        };
        let info = self.session_info(session)?;
        if self.live(session).is_err() {
            return Ok(cannot("its session has ended"));
        }
        if self.journal.run_session(run)?.as_deref() != Some(session) {
            return Ok(cannot("it isn't from this session"));
        }
        let events = self.journal.run_events(run, i64::MAX)?;
        let plot = match redraw::find(&events, &info.language, image, index as usize) {
            Ok(plot) => plot,
            Err(reason) => return Ok(cannot(&reason)),
        };
        let Some(save) = save else {
            return Ok(PlotRedraw {
                can: true,
                reason: None,
                png: None,
            });
        };
        if !redraw::FORMATS.contains(&save.format) {
            bail!("a plot can't be saved as {}", save.format);
        }
        let sizes = 0.5..=100.0;
        if !sizes.contains(&save.width)
            || !sizes.contains(&save.height)
            || !(36..=2400).contains(&save.dpi)
        {
            bail!("a plot can't be saved at that size");
        }
        let resolved = files::resolve(save.file)?;
        if let Some(dir) = resolved.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not make the folder for {}", save.file))?;
        }
        let file = resolved.to_string_lossy();
        let save = redraw::Save {
            file: &file,
            ..save
        };
        let printed = self
            .quietly(session, "draw the plot again", |_| {
                Some(redraw::code(&plot, &save))
            })
            .await?;
        if !redraw::saved(&printed.stdout) {
            bail!("could not draw the plot again: nothing was written");
        }
        Ok(PlotRedraw {
            can: true,
            reason: None,
            png: None,
        })
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

/// A connection's subscriptions: sessions by id, terminals by [`terminal_key`].
type Subscriptions = Arc<Mutex<HashMap<String, JoinHandle<()>>>>;

fn terminal_key(terminal: &str) -> String {
    format!("terminal:{terminal}")
}

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
    stop_reply: Arc<Mutex<Option<u64>>>,
    stop: Arc<Notify>,
) {
    while let Some(message) = messages.recv().await {
        let shutdown = matches!(&message, ServerMessage::Response { id, .. }
            if Some(*id) == *stop_reply.lock().unwrap());
        let Ok(mut line) = serde_json::to_vec(&message) else {
            continue;
        };
        drop(message);
        line.push(b'\n');
        let failed = writer.write_all(&line).await.is_err() || writer.flush().await.is_err();
        if shutdown || (failed && stop_reply.lock().unwrap().is_some()) {
            // A response is an acknowledgement, not proof of exit. Notify only
            // after attempting its flush; attach verifies the process exit.
            stop.notify_one();
        }
        if failed {
            return;
        }
        if line.len() > memory::LARGE {
            drop(line);
            memory::release_free_memory();
        }
    }
}

fn on_disk(file: String, edited: cells::Edited) -> CellEdited {
    CellEdited {
        file,
        cell_id: edited.cell_id,
        language: edited.language,
        in_window: false,
        kernel: edited.kernel,
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

/// The user the daemon runs as, from the environment or the password
/// database.
fn user_name() -> String {
    if let Some(user) = ["USER", "LOGNAME"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|user| !user.is_empty())
    {
        return user;
    }
    // SAFETY: getpwuid returns null or a pointer to a record that stays
    // valid until the next call; the name is copied out at once. This runs
    // once, as the daemon opens.
    unsafe {
        let record = libc::getpwuid(libc::getuid());
        if record.is_null() || (*record).pw_name.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr((*record).pw_name)
            .to_string_lossy()
            .into_owned()
    }
}

pub(crate) fn response(id: u64, result: Result<Value>) -> ServerMessage {
    ServerMessage::Response {
        id,
        outcome: match result {
            Ok(value) => Outcome::Result(value),
            Err(err) => Outcome::Error(format!("{err:#}")),
        },
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use sandhill_proto::client::Client;
    use std::time::Duration;

    fn connect(daemon: Arc<Daemon>) -> Client {
        let (server, client) = tokio::io::duplex(8192);
        let (reader, writer) = tokio::io::split(server);
        tokio::spawn(daemon.serve_connection(reader, writer));
        let (reader, writer) = tokio::io::split(client);
        Client::new(reader, writer)
    }

    #[tokio::test]
    async fn accepted_shutdown_rejects_later_destructive_requests() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDir::new(temp.path().join("state"));
        let daemon = Daemon::open(state.clone()).await.unwrap();
        let client = connect(daemon);
        std::fs::create_dir_all(state.root().join("pages/keep")).unwrap();
        std::fs::write(state.root().join("pages/keep/output"), b"preserve").unwrap();
        client.call::<()>(Call::StopDaemon).await.unwrap();
        let error = client
            .call::<Value>(Call::CleanStorage {
                categories: vec!["cache".into()],
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("shutdown is in progress"), "{error}");
        assert!(state.root().join("pages/keep/output").exists());
        assert_eq!(client.ping().await.unwrap().lifecycle, "stopping");
    }

    #[tokio::test]
    async fn attach_cannot_interrupt_maintenance_or_restart_an_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDir::new(temp.path().join("state"));
        let daemon = Daemon::open(state.clone()).await.unwrap();
        let client = connect(daemon.clone());
        // The same exclusive guard held by CleanStorage/UninstallComponents.
        let active = daemon.lifecycle_gate.write().await;
        let mut attach = tokio::spawn(async move {
            client
                .call::<Value>(Call::PrepareAttach {
                    build: "different".into(),
                })
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut attach)
                .await
                .is_err()
        );
        assert!(!daemon.stopping.load(Ordering::Acquire));
        std::fs::write(state.root().join("uninstalled"), b"fixture uninstall").unwrap();
        drop(active);
        let error = attach.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("uninstalled"), "{error}");
        assert!(!daemon.stopping.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn shutdown_notification_waits_for_reply_flush() {
        let (writer, reader) = tokio::io::duplex(1);
        let (out, incoming) = mpsc::unbounded_channel();
        let stop = Arc::new(Notify::new());
        let task = tokio::spawn(write_loop(
            writer,
            incoming,
            Arc::new(Mutex::new(Some(0))),
            stop.clone(),
        ));
        out.send(response(0, Ok(Value::Null))).unwrap();
        // One byte of capacity keeps the acknowledgement from being flushed.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), stop.notified())
                .await
                .is_err()
        );
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await.unwrap();
        assert!(
            line.contains("\"id\":0") && line.contains(":null"),
            "{line}"
        );
        tokio::time::timeout(Duration::from_secs(1), stop.notified())
            .await
            .unwrap();
        drop(out);
        task.await.unwrap();
    }
}
