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

mod connections;
mod document_calls;
mod file_history;
mod help_calls;
mod package_calls;
mod package_scan;
mod project_package_calls;
mod quick_calls;
mod request_helpers;
mod session_calls;
mod session_helpers;
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
    history_limits: Mutex<sandhill_proto::FileHistoryLimits>,
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
    own_sessions: Arc<tokio::sync::RwLock<()>>,
    project_package_plans: Mutex<HashMap<(String, String), sandhill_proto::ProjectFilePlan>>,
    project_setup_plans: Mutex<HashMap<(String, String), crate::quick::project_packages::Work>>,
    own_copy_plan: Mutex<Option<(sandhill_proto::OwnSetup, crate::quick::own::Work)>>,
    pub(crate) maintenance: AtomicBool,
    pub(crate) stopping: AtomicBool,
    pub(crate) analysis_gate: tokio::sync::Mutex<()>,
    skills_gate: tokio::sync::Mutex<()>,
    connectors: crate::connectors::Connectors,
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
        let history_limits = file_history::load_limits(state.root());
        file_history::tidy(&journal, history_limits);
        let pages = Pages::open(state.root())?;
        let daemon = Arc::new(Daemon {
            terminals: Terminals::new(state.terminals_dir(), options.executable.clone()),
            pages,
            state,
            journal: journal.clone(),
            history_limits: Mutex::new(history_limits),
            sessions: Mutex::new(HashMap::new()),
            windows: Windows::default(),
            language_servers: crate::lsp::LanguageServers::default(),
            own_sessions: Arc::new(tokio::sync::RwLock::new(())),
            project_package_plans: Mutex::default(),
            project_setup_plans: Mutex::default(),
            own_copy_plan: Mutex::default(),
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
            skills_gate: tokio::sync::Mutex::new(()),
            connectors: crate::connectors::Connectors::default(),
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
        daemon.start_history_tidy();
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

    async fn call(self: &Arc<Self>, call: Call, project: Option<String>) -> Result<Value> {
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
            Call::Footprint => serde_json::to_value(self.footprint().await?)?,
            Call::PlanUninstall { choice } => {
                serde_json::to_value(self.plan_uninstall(choice).await?)?
            }
            Call::RunUninstall { choice, app_pid } => {
                serde_json::to_value(self.run_uninstall(choice, app_pid).await?)?
            }
            Call::Ping => self.ping()?,
            Call::ListKernelspecs => {
                serde_json::to_value(crate::kernel::list_scoped(None, self.own_folder()).await)?
            }
            Call::ListProjectKernelspecs { path } => {
                let root = PathBuf::from(projects::normalize(&path)?);
                serde_json::to_value(
                    crate::kernel::list_scoped(Some(root), self.own_folder()).await,
                )?
            }
            Call::ListRWithoutIrkernel { project } => self.list_r_without_irkernel(project).await?,
            Call::InstallIrkernel {
                r,
                project,
                library,
            } => self.install_irkernel(r, project, library).await?,
            Call::ListRLibraries { r } => serde_json::to_value(
                blocking(move || crate::irkernel::libraries(Path::new(&r))).await?,
            )?,
            Call::QuickReadiness => self.quick_readiness().await?,
            Call::PlanPackages { change } => self.plan_packages(change).await?,
            Call::RunPackages { change } => self.run_packages(change).await?,
            Call::MissingPackages {
                path,
                kernel,
                session,
                project,
            } => serde_json::to_value(
                self.missing_packages(path, kernel, session, project)
                    .await?,
            )?,
            Call::MissingInError { session, error } => {
                serde_json::to_value(self.missing_in_error(&session, &error).await?)?
            }
            Call::ExplainInstallFailure { log } => {
                let folder = self.state.install_logs_dir();
                serde_json::to_value(
                    blocking(move || {
                        crate::quick::install_failure::explain(&folder, Path::new(&log))
                    })
                    .await?,
                )?
            }
            Call::ListEnvironments => self.list_environments().await?,
            Call::SearchPackages { env, query } => self.search_packages(env, query).await?,
            Call::EnvUpdates { env } => self.env_updates(env).await?,
            Call::PlanEnvAction { action } => self.plan_env_action(action).await?,
            Call::RunEnvAction { action } => self.run_env_action(action).await?,
            Call::ProjectPackageFiles { project } => self.project_package_files(project).await?,
            Call::PlanProjectFile { project, language } => {
                self.plan_project_file(project, language).await?
            }
            Call::WriteProjectFile { project, language } => {
                self.write_project_file(project, language).await?
            }
            Call::PlanProjectSetup { project, file } => {
                self.plan_project_setup(project, file).await?
            }
            Call::RunProjectSetup { project, file } => {
                self.run_project_setup(project, file).await?
            }
            Call::PlanRuntime { runtime } => self.plan_runtime(runtime).await?,
            Call::RunRuntime { runtime } => self.run_runtime(runtime).await?,
            Call::InstallPrefs => self.install_prefs().await?,
            Call::SetInstallPrefs { without_asking } => {
                self.set_install_prefs(without_asking).await?
            }
            Call::SetEnvMode { mode } => self.set_env_mode(mode).await?,
            Call::PlanOwnSetup { setup } => self.plan_own_setup(setup).await?,
            Call::RunOwnSetup { setup } => self.run_own_setup(setup).await?,
            Call::RemoveOwnEnvironment => self.remove_own_environment().await?,
            Call::InstallDeveloperTools => {
                blocking(crate::quick::own::install_developer_tools).await?;
                Value::Null
            }
            Call::RemoveInstalled { items } => self.remove_installed(items).await?,
            Call::QuickInstallAgent { kind, command } => {
                self.quick_install_agent(kind, command).await?
            }
            Call::QuickAddIpykernel { prefix } => self.quick_add_ipykernel(prefix).await?,
            Call::QuickStart { tables } => self.quick_start(tables).await?,
            Call::QuickKeep {
                project,
                parent,
                name,
            } => self.quick_keep(project, parent, name).await?,
            Call::QuickDiscard { project } => self.quick_discard(project).await?,
            Call::QuickAddTables { project, tables } => {
                self.quick_add_tables(project, tables).await?
            }
            Call::QuickListArchive { path } => self.quick_list_archive(path).await?,
            Call::QuickOpenArchives {
                project,
                tables,
                archives,
                combine,
            } => {
                self.quick_open_archives(project, tables, archives, combine)
                    .await?
            }
            Call::QuickCard { project } => self.quick_card(project).await?,
            Call::SaveQuickCard { project, card } => self.save_quick_card(project, card).await?,
            Call::QuickRun {
                session,
                project,
                request,
            } => self.quick_run(session, project, request).await?,
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
            Call::Input { session, value } => {
                self.live(&session)?.input(value).await?;
                Value::Null
            }
            Call::Shutdown { session } => {
                let handle = self.live(&session)?;
                handle.shutdown().await?;
                self.sessions.lock().unwrap().remove(&session);
                Value::Null
            }
            Call::ForceStop { session } => {
                self.force_stop_session(&session).await?;
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
            Call::ReadDocument { path, max_bytes } => {
                self.read_document(path, max_bytes, project).await?
            }
            Call::FileVersions { path } => self.file_versions(path, project).await?,
            Call::FileVersion { path, id } => self.file_version(path, project, id).await?,
            Call::SetFileHistoryLimits {
                days,
                per_file,
                file_mb,
                total_mb,
            } => {
                self.set_file_history_limits(sandhill_proto::FileHistoryLimits {
                    days,
                    per_file,
                    file_mb,
                    total_mb,
                })
                .await?
            }
            Call::WriteNotebook {
                path,
                text,
                expected_mtime_ms,
            } => {
                self.write_notebook(path, text, expected_mtime_ms, project)
                    .await?
            }
            Call::WriteFile {
                path,
                text,
                expected_mtime_ms,
                base64,
                create,
            } => {
                self.write_file(path, text, expected_mtime_ms, base64, create, project)
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
            Call::CompressPath { path, format, task } => serde_json::to_value(
                blocking(move || crate::archives::compress(&path, format, task)).await?,
            )?,
            Call::ExtractPath { path, task } => serde_json::to_value(
                blocking(move || crate::archives::extract(&path, task)).await?,
            )?,
            Call::ArchiveProgress { task } => {
                serde_json::to_value(crate::archives::progress(&task))?
            }
            Call::CancelArchive { task } => serde_json::to_value(crate::archives::cancel(&task))?,
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
            Call::DescribeH5ad { path } => {
                crate::h5ad::describe(&self.options.executable, path).await?
            }
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
                // Sessions not kept live in Sandhill's own storage; they
                // aren't the user's projects.
                let explore = self.state.root().join("explore");
                let real = std::fs::canonicalize(&explore).unwrap_or_else(|_| explore.clone());
                let mut recent = blocking(move || projects::recent(&journal)).await?;
                recent.retain(|p| {
                    let path = Path::new(&p.path);
                    !path.starts_with(&explore) && !path.starts_with(&real)
                });
                serde_json::to_value(recent)?
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
                let own = self.own_folder();
                serde_json::to_value(
                    blocking(move || Ok(projects::setup_scoped(&root, own))).await?,
                )?
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
            Call::UsageStats {
                days,
                tz_offset_minutes,
            } => {
                let root = self.state.root().to_path_buf();
                serde_json::to_value(
                    blocking(move || Ok(crate::usage::stats(&root, days, tz_offset_minutes)))
                        .await?,
                )?
            }
            // A preview only reads the index and writes its own incoming
            // folder, so a slow download doesn't hold up agents' get_skill.
            call @ Call::PreviewSkills { .. } => {
                crate::skills::call(self.state.root(), call).await?
            }
            call @ (Call::ListSkills
            | Call::InstallSkills { .. }
            | Call::SetSkillEnabled { .. }
            | Call::RemoveSkill { .. }
            | Call::ExportSkill { .. }
            | Call::GetSkill { .. }) => {
                let _gate = self.skills_gate.lock().await;
                crate::skills::call(self.state.root(), call).await?
            }
            call @ (Call::ListConnectors
            | Call::AddConnector { .. }
            | Call::BeginConnectorSignIn { .. }
            | Call::FinishConnectorSignIn { .. }
            | Call::SetConnectorEnabled { .. }
            | Call::RemoveConnector { .. }
            | Call::TestConnector { .. }
            | Call::ExportConnector { .. }
            | Call::ConnectorTools { .. }
            | Call::CallConnector { .. }) => self.connectors.call(self.state.root(), call).await?,
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
            Call::SetAgentTools { kind, allowed } => {
                crate::lifecycle::set_agent_tools(&kind, allowed, &self.options.executable).await?;
                Value::Null
            }
            Call::AgentModels { kind } => {
                crate::agents::ensure_offered(&kind)?;
                crate::agents::models(&kind).await?
            }
            Call::AgentCommands { kind, cwd } => {
                crate::agents::ensure_offered(&kind)?;
                serde_json::to_value(
                    crate::agent_commands::list(&kind, &cwd, self.state.root()).await,
                )?
            }
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
            } => {
                crate::agents::ensure_offered(&kind)?;
                serde_json::to_value(
                    crate::accounts::Registry::from_env()
                        .with_state(self.state.root())
                        .add(&kind, &name, api_key.as_deref())?,
                )?
            }
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
            | Call::QuickAgent { .. }
            | Call::SetFocus { .. } => unreachable!("handled earlier"),
        })
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
