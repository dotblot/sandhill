//! The daemon: owns every session and serves clients over a byte stream.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    state: StateDir,
    journal: Arc<Journal>,
    sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
    terminals: Terminals,
    windows: Windows,
    pages: Pages,
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
            next_connection: AtomicU64::new(1),
            options,
            started_ms: crate::journal::now_ms(),
            user: user_name(),
            machine: hostname(),
            homes: chats::Homes::from_env(),
            opened: Mutex::default(),
            askers: Mutex::default(),
            stop: Arc::default(),
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
                let _ = tokio::task::spawn_blocking(move || {
                    chats::sync(&daemon.terminals, &daemon.homes, &daemon.machine);
                })
                .await;
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
        let writer_task = tokio::spawn(write_loop(writer, out_rx));
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
            tokio::spawn(async move {
                daemon.handle(request, connection, out, subscriptions).await;
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
        drop(out);
        let _ = writer_task.await;
    }

    async fn handle(
        self: Arc<Self>,
        request: Request,
        connection: u64,
        out: mpsc::UnboundedSender<ServerMessage>,
        subscriptions: Subscriptions,
    ) {
        let id = request.id;
        let result = match request.call {
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
        let _ = out.send(response(id, result));
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

    async fn call(&self, call: Call) -> Result<Value> {
        Ok(match call {
            Call::Ping => serde_json::to_value(PingReply {
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                home: std::env::var("HOME").unwrap_or_default(),
                hostname: hostname(),
                build: crate::build_id().to_string(),
                user: self.user.clone(),
                started_ms: self.started_ms,
                started_by: self.options.started_by.clone(),
            })?,
            Call::ListKernelspecs => {
                serde_json::to_value(crate::kernel::list_kernelspecs(None).await)?
            }
            Call::ListProjectKernelspecs { path } => {
                let root = PathBuf::from(projects::normalize(&path)?);
                serde_json::to_value(crate::kernel::list_kernelspecs(Some(root)).await)?
            }
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
                let project = project.map(|p| projects::normalize(&p)).transpose()?;
                let env = match project.clone() {
                    Some(root) => {
                        blocking(move || Ok(projects::kernel_env(Path::new(&root)))).await?
                    }
                    None => Vec::new(),
                };
                let handle = SessionHandle::start(
                    self.journal.clone(),
                    &self.state.kernels_dir(),
                    &kernel,
                    &cwd,
                    session::Start { project, env },
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
            Call::PreviewDocument { path, project } => {
                let file = files::resolve(&path)?;
                if !crate::preview::previewable(&file) {
                    bail!("{} is not a Quarto document (.qmd or .Rmd)", file.display());
                }
                if !file.is_file() {
                    bail!("{} does not exist", file.display());
                }
                let project = project.map(|p| projects::normalize(&p)).transpose()?;
                let (id, port) = match crate::preview::running(&self.terminals, &file) {
                    Some(found) => found,
                    None => {
                        let quarto = blocking(|| {
                            Ok(crate::preview::find_quarto(
                                &crate::kernelspecs::Search::from_env(),
                            ))
                        })
                        .await?
                        .with_context(crate::preview::install_advice)?;
                        let port = crate::preview::free_port()?;
                        let mut launch =
                            crate::preview::launch(&quarto, &file, port, project.clone());
                        if let Some(root) = project {
                            launch.env =
                                blocking(move || Ok(crate::preview::env(Path::new(&root)))).await?;
                        }
                        (self.terminals.start(launch).await?.id, port)
                    }
                };
                serde_json::to_value(crate::preview::ready(&self.terminals, &id, port).await?)?
            }
            Call::ServePage { path } => {
                let path = files::resolve(&path)?;
                serde_json::to_value(self.pages.serve(&path).await?)?
            }
            Call::ServeHtml { html, blob } => {
                let html = match (html, blob) {
                    (Some(html), _) => html,
                    (None, Some(hash)) => {
                        let bytes = self
                            .journal
                            .blobs()
                            .get(&hash)
                            .with_context(|| format!("no blob {hash}"))?;
                        String::from_utf8_lossy(&bytes).into_owned()
                    }
                    (None, None) => bail!("give html or a blob"),
                };
                serde_json::to_value(self.pages.serve_html(&html).await?)?
            }
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
            Call::ListVariables { session, path } => {
                let printed = self
                    .quietly(&session, "list the variables", |language| {
                        variables::list_code(language, &path)
                    })
                    .await?;
                let (list, more) = variables::parse(&printed.stdout);
                serde_json::to_value(Variables {
                    session,
                    path,
                    variables: list,
                    more,
                })?
            }
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
                column_offset,
                column_limit,
                cells_only,
            } => {
                let ask = tables::Ask {
                    mode: tables::Mode::Rows,
                    path: &path,
                    sort: &sort,
                    filters: &filters,
                    offset,
                    limit: limit.unwrap_or(tables::ROWS).min(tables::MAX_ROWS),
                    column_offset,
                    column_limit: column_limit
                        .unwrap_or(tables::COLUMNS)
                        .min(tables::MAX_COLUMNS),
                    headers: !cells_only,
                    file: "",
                };
                let printed = self
                    .quietly(&session, "read the table", |language| {
                        tables::code(language, &ask)
                    })
                    .await?;
                let rows = tables::parse_rows(&printed.stdout);
                serde_json::to_value(TableRows {
                    session,
                    path,
                    code: rows.code,
                    matrix: rows.matrix,
                    columns: rows.columns,
                    column_offset,
                    width: rows.width,
                    total: rows.total,
                    rows: rows.rows,
                    offset,
                    labels: rows.labels,
                    values: rows.values,
                })?
            }
            Call::TableSummary {
                session,
                path,
                filters,
                column_offset,
                column_limit,
            } => {
                let ask = tables::Ask {
                    mode: tables::Mode::Summary,
                    path: &path,
                    sort: &[],
                    filters: &filters,
                    offset: 0,
                    limit: 0,
                    column_offset,
                    column_limit: column_limit
                        .unwrap_or(tables::COLUMNS)
                        .min(tables::MAX_COLUMNS),
                    headers: false,
                    file: "",
                };
                let printed = self
                    .quietly(&session, "summarize the table", |language| {
                        tables::code(language, &ask)
                    })
                    .await?;
                let summary = tables::parse_summary(&printed.stdout);
                serde_json::to_value(TableSummary {
                    session,
                    path,
                    rows: summary.rows,
                    columns: summary.columns,
                })?
            }
            Call::HelpTopics {
                session,
                query,
                package,
                limit,
            } => {
                let ask = help::Ask {
                    mode: help::Mode::Topics,
                    query: query.trim(),
                    package: package.as_deref().unwrap_or("").trim(),
                    topic: "",
                    limit: limit.unwrap_or(help::TOPICS).min(help::MAX_TOPICS),
                };
                let printed = self
                    .quietly(&session, "search the help", |language| {
                        help::code(language, &ask)
                    })
                    .await?;
                let found = help::parse_topics(&printed.stdout);
                serde_json::to_value(HelpTopics {
                    session,
                    more: found.matched.saturating_sub(found.topics.len() as u64),
                    topics: found.topics,
                    vignettes: found.vignettes,
                })?
            }
            Call::HelpPage {
                session,
                topic,
                package,
            } => {
                let ask = help::Ask {
                    mode: help::Mode::Page,
                    query: "",
                    package: package.as_deref().unwrap_or("").trim(),
                    topic: topic.trim(),
                    limit: 0,
                };
                let printed = self
                    .quietly(&session, "read the help", |language| {
                        help::code(language, &ask)
                    })
                    .await?;
                let page = help::parse_page(&printed.stdout)
                    .with_context(|| format!("no help for {topic}"))?;
                serde_json::to_value(HelpPage {
                    session,
                    topic: page.topic,
                    package: page.package,
                    title: page.title,
                    version: page.version,
                    html: page.html,
                    kind: page.kind,
                    signature: page.signature,
                    text: page.text,
                    members: page.members,
                    alternatives: page.alternatives,
                })?
            }
            Call::HelpSource {
                session,
                topic,
                package,
            } => {
                let ask = help::Ask {
                    mode: help::Mode::Source,
                    query: "",
                    package: package.as_deref().unwrap_or("").trim(),
                    topic: topic.trim(),
                    limit: 0,
                };
                let printed = self
                    .quietly(&session, "read the source", |language| {
                        help::code(language, &ask)
                    })
                    .await?;
                let source = help::parse_source(&printed.stdout)
                    .with_context(|| format!("no source for {topic}"))?;
                serde_json::to_value(HelpSource {
                    session,
                    topic: source.topic,
                    package: source.package,
                    text: source.text,
                    file: source.file,
                    line: source.line,
                })?
            }
            Call::ListPackages { session } => {
                let printed = self
                    .quietly(&session, "list the packages", packages::code)
                    .await?;
                let listed = packages::parse(&printed.stdout);
                serde_json::to_value(Packages {
                    session,
                    packages: listed.packages,
                    libraries: listed.libraries,
                    cran: listed.cran,
                    bioconductor: listed.bioconductor,
                    conda: listed.conda,
                })?
            }
            Call::PackageUpdates { session } => {
                let printed = self
                    .quietly_for(
                        &session,
                        "check for updates",
                        packages::UPDATES_TIMEOUT,
                        packages::updates_code,
                    )
                    .await?;
                let found = packages::parse_updates(&printed.stdout);
                serde_json::to_value(PackageUpdates {
                    session,
                    updates: found.updates,
                    repos: found.repos,
                    unreached: found.unreached,
                })?
            }
            Call::GitStatus { path } => serde_json::to_value(crate::git::status(&path).await?)?,
            Call::GitLog { path, skip, limit } => {
                serde_json::to_value(crate::git::log(&path, skip, limit).await?)?
            }
            Call::GitShow { path, sha } => {
                serde_json::to_value(crate::git::show(&path, &sha).await?)?
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
                show,
            } if show => {
                // Drawn to a file of the daemon's, then sent back and removed.
                let dir = self.state.root().join("redraw");
                std::fs::create_dir_all(&dir)?;
                let path = dir.join(format!("{}.png", uuid::Uuid::new_v4().simple()));
                let file = path.to_string_lossy().into_owned();
                let save = redraw::Save {
                    file: &file,
                    format: "png",
                    width,
                    height,
                    dpi,
                };
                let drawn = self
                    .redraw_plot(&session, &run, image.as_deref(), index, Some(save))
                    .await;
                let bytes = std::fs::read(&path);
                let _ = std::fs::remove_file(&path);
                let mut drawn = drawn?;
                if drawn.can {
                    drawn.png = Some(BASE64.encode(bytes?));
                }
                serde_json::to_value(drawn)?
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
                show: _,
            } => {
                let save = file.as_deref().map(|file| redraw::Save {
                    file,
                    format: &format,
                    width,
                    height,
                    dpi,
                });
                serde_json::to_value(
                    self.redraw_plot(&session, &run, image.as_deref(), index, save)
                        .await?,
                )?
            }
            Call::TableExport {
                session,
                path,
                sort,
                filters,
                file,
            } => {
                let resolved = files::resolve(&file)?.to_string_lossy().into_owned();
                let ask = tables::Ask {
                    mode: tables::Mode::Export,
                    path: &path,
                    sort: &sort,
                    filters: &filters,
                    offset: 0,
                    limit: 0,
                    column_offset: 0,
                    column_limit: 0,
                    headers: false,
                    file: &resolved,
                };
                let printed = self
                    .quietly(&session, "export the table", |language| {
                        tables::code(language, &ask)
                    })
                    .await?;
                if tables::parse_written(&printed.stdout).is_none() {
                    bail!("could not export the table: nothing was written");
                }
                let meta = std::fs::metadata(&resolved)
                    .with_context(|| format!("could not export the table to {file}"))?;
                serde_json::to_value(FileWritten {
                    path: file,
                    mtime_ms: files::mtime_ms(&meta),
                })?
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
                base64,
                create,
            } => {
                let bytes = if base64 {
                    BASE64
                        .decode(text.trim())
                        .context("the file's bytes are not valid base64")?
                } else {
                    text.into_bytes()
                };
                serde_json::to_value(
                    blocking(move || {
                        if create {
                            fileops::create_file(&path, &bytes)
                        } else {
                            files::write_bytes(&path, &bytes, expected_mtime_ms)
                        }
                    })
                    .await?,
                )?
            }
            Call::ListDir { path, sizes } => {
                serde_json::to_value(blocking(move || files::list_dir(&path, sizes)).await?)?
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
                let root = PathBuf::from(projects::normalize(&path)?);
                if !root.is_dir() {
                    anyhow::bail!("{} is not a folder", root.display());
                }
                let machine = settings.chat_history == ChatHistory::Machine;
                let setup = blocking({
                    let root = root.clone();
                    move || {
                        projects::settings::write(&root, &settings)?;
                        Ok(projects::setup(&root))
                    }
                })
                .await?;
                if machine {
                    chats::move_out(&self.terminals, &root)?;
                }
                serde_json::to_value(setup)?
            }
            Call::ForgetProject { path } => {
                let journal = self.journal.clone();
                blocking(move || Ok(journal.forget_project(&projects::normalize(&path)?)?)).await?;
                Value::Null
            }
            Call::SystemStats { path } => {
                let journal = self.journal.clone();
                let jobs: Vec<(String, u32)> = self
                    .terminals
                    .list()
                    .into_iter()
                    .filter(|t| t.kind == TERMINAL_JOB && t.running)
                    .filter_map(|t| Some((t.id.clone(), self.terminals.pid(&t.id)?)))
                    .collect();
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
                        Ok(crate::system::stats(&path, &kernels, &jobs))
                    })
                    .await?,
                )?
            }
            Call::ReadLog { bytes } => {
                let path = self.state.log_path();
                let tail = blocking(move || crate::logfile::tail(&path, bytes.unwrap_or(LOG_TAIL)))
                    .await
                    .map_err(|err| match self.options.started_by.as_str() {
                        // Then the service manager keeps the log.
                        "launchd" | "systemd" => anyhow!(
                            "{err:#}; sandhilld runs under {}, which keeps its log",
                            self.options.started_by
                        ),
                        _ => err,
                    })?;
                serde_json::to_value(tail)?
            }
            Call::StopDaemon => {
                tracing::info!("stopping, as a client asked");
                // Answer first: the reply goes out before the daemon stops.
                let stop = self.stop.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    stop.notify_one();
                });
                Value::Null
            }
            Call::StatFiles { paths } => {
                serde_json::to_value(blocking(move || Ok(files::stat(&paths))).await?)?
            }
            Call::ListAgents => serde_json::to_value(crate::agents::list().await)?,
            Call::StartTerminal {
                kind,
                cwd,
                project,
                cols,
                rows,
                options,
                prompt,
            } => {
                let project = project.map(|p| projects::normalize(&p)).transpose()?;
                let cwd = match cwd.or_else(|| project.clone()) {
                    Some(cwd) => files::resolve(&cwd)?,
                    None => PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into())),
                };
                if !cwd.is_dir() {
                    anyhow::bail!("{} is not a directory", cwd.display());
                }
                let context = crate::agents::Context {
                    executable: &self.options.executable,
                    socket: &self.options.socket,
                    project: project.as_deref(),
                    cwd: &cwd,
                    prompt: prompt.as_deref(),
                    resume: None,
                };
                let mut launch = crate::agents::launch(&kind, &options, &context).await?;
                launch.size = cols.zip(rows);
                serde_json::to_value(self.terminals.start(launch).await?)?
            }
            Call::StartJob { job, cwd, project } => {
                let project = project.map(|p| projects::normalize(&p)).transpose()?;
                let cwd = cwd.map(|cwd| files::resolve(&cwd)).transpose()?;
                let launch =
                    blocking(move || crate::jobs::launch(&job, cwd.as_deref(), project.as_deref()))
                        .await?;
                serde_json::to_value(self.terminals.start(launch).await?)?
            }
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
                // A job runs again as it was asked to, in the same place.
                let info = self.terminals.info(&terminal)?;
                let job = info.job.unwrap_or_default();
                let cwd = PathBuf::from(&info.cwd);
                let project = info.project;
                let launch =
                    blocking(move || crate::jobs::launch(&job, Some(&cwd), project.as_deref()))
                        .await?;
                serde_json::to_value(self.terminals.restart(&terminal, launch, &[]).await?)?
            }
            Call::RestartTerminal {
                terminal,
                options,
                cols,
                rows,
                prompt,
            } => {
                let info = self.terminals.info(&terminal)?;
                // It may have run on another machine, or have to start afresh.
                let resume = chats::prepare(&info, &self.homes, &self.machine)?;
                let prompt = match &resume.note {
                    Some(note) => Some(chats::with_note(note, prompt.as_deref())),
                    None => prompt,
                };
                let cwd = PathBuf::from(&info.cwd);
                let context = crate::agents::Context {
                    executable: &self.options.executable,
                    socket: &self.options.socket,
                    project: info.project.as_deref(),
                    cwd: &cwd,
                    prompt: prompt.as_deref(),
                    resume: resume.id.as_deref(),
                };
                let options = options.unwrap_or(info.options);
                let mut launch = crate::agents::launch(&info.kind, &options, &context).await?;
                launch.size = cols.zip(rows);
                let keep = &crate::agents::RESUME_KEYS;
                serde_json::to_value(self.terminals.restart(&terminal, launch, keep).await?)?
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
                    chats::forget(Path::new(&project), &terminal)?;
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
            } => {
                let path = windows::normalize(&files::resolve(&file)?);
                let file = path.to_string_lossy().into_owned();
                let mut params = json!({"file": file, "code": code, "language": language, "after": after, "run": run});
                // Left out, not null: no `after` means after the last cell.
                if let Some(map) = params.as_object_mut() {
                    map.retain(|_, v| !v.is_null());
                }
                let edited = match self.ask_window(&path, project, "add_cell", params).await? {
                    Some(edited) => edited,
                    None => {
                        let edited = blocking({
                            let file = file.clone();
                            move || {
                                cells::add_cell(&file, &code, language.as_deref(), after.as_deref())
                            }
                        })
                        .await?;
                        on_disk(file, edited)
                    }
                };
                self.windows.moved_to(&path, &edited.cell_id);
                serde_json::to_value(edited)?
            }
            Call::EditCell {
                file,
                cell_id,
                code,
                run,
                project,
            } => {
                let path = windows::normalize(&files::resolve(&file)?);
                let file = path.to_string_lossy().into_owned();
                let params = json!({"file": file, "cell_id": cell_id, "code": code, "run": run});
                let edited = match self.ask_window(&path, project, "edit_cell", params).await? {
                    Some(edited) => edited,
                    None => {
                        let edited = blocking({
                            let file = file.clone();
                            move || cells::edit_cell(&file, &cell_id, &code)
                        })
                        .await?;
                        on_disk(file, edited)
                    }
                };
                serde_json::to_value(edited)?
            }
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
