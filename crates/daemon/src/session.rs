//! One kernel session: an actor that owns the kernel connection, journals
//! every output, and tracks when each run finishes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jupyter_protocol::{
    CommClose, CommId, CommOpen, CompleteRequest, ExecuteRequest, ExecutionState, Header,
    InputReply, InspectRequest, InterruptRequest, JupyterMessage, JupyterMessageContent,
    KernelInfoRequest, ReplyStatus, ShutdownRequest,
};
use sandhill_proto::{
    CellRef, CompletionReply, EVENT_INPUT_REPLY, EVENT_INPUT_REQUEST, EVENT_RUN_FINISHED,
    EVENT_RUN_STARTED, EVENT_SESSION_ENDED, EVENT_VIEW, Event, InspectReply, RunFinished,
    RunStarted, SessionStatus,
};
use serde_json::json;
use tokio::process::Child;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::journal::{Journal, NewRun, SessionRecord, now_ms};
use crate::kernel::{self, KernelConnection, KernelMsg};
use crate::{startup, view, viewer};

/// IOPub message types that are outputs and get journaled.
const JOURNALED: &[&str] = &[
    "stream",
    "display_data",
    "update_display_data",
    "execute_input",
    "execute_result",
    "error",
    "clear_output",
];

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// How long a kernel has to stop of itself before a run in it is interrupted.
const SHUTDOWN_INTERRUPT: Duration = Duration::from_secs(1);
const LIVENESS_INTERVAL: Duration = Duration::from_secs(2);
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
const ARK_LSP_TIMEOUT: Duration = Duration::from_secs(5);
const LANGUAGE_SEND_TIMEOUT: Duration = Duration::from_millis(100);

enum Command {
    Execute {
        code: String,
        cell: Option<CellRef>,
        quiet: bool,
        by: Option<String>,
        reply: oneshot::Sender<Result<RunStarted>>,
    },
    BioCheck {
        code: String,
        reply: oneshot::Sender<Result<RunStarted>>,
    },
    Complete {
        code: String,
        cursor_pos: usize,
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<CompletionReply>>,
    },
    Inspect {
        code: String,
        cursor_pos: usize,
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<InspectReply>>,
    },
    StartArkLsp {
        deadline: tokio::time::Instant,
        reply: oneshot::Sender<Result<u16>>,
    },
    Interrupt {
        reply: oneshot::Sender<Result<()>>,
    },
    Input {
        value: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<()>>,
    },
    Force {
        reply: oneshot::Sender<Result<()>>,
    },
}

/// What a new session belongs to and starts with, besides its kernel and
/// folder.
#[derive(Debug, Default, Clone)]
pub struct Start {
    /// The project's folder. R starts there so its `.Rprofile` and
    /// `.Renviron` apply, and kernels of the environments inside it count.
    pub project: Option<String>,
    /// Variables from the project's env files.
    pub env: Vec<(String, String)>,
}

/// Handle held by the daemon. Dropping it does not stop the kernel.
pub struct SessionHandle {
    pub id: String,
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    task: JoinHandle<()>,
}

impl SessionHandle {
    /// Starts a new kernel and waits until it is ready for code.
    ///
    /// A kernel is given free ports to bind, and they are only its own once
    /// it has: something else (another kernel starting, any connection going
    /// out) can take one in between. That says nothing against the kernel,
    /// so it is started again with other ports.
    pub async fn start(
        journal: Arc<Journal>,
        kernels_dir: &Path,
        kernel: &str,
        cwd: &Path,
        start: Start,
    ) -> Result<SessionHandle> {
        let mut tries = 0;
        loop {
            let attempt =
                Self::start_once(journal.clone(), kernels_dir, kernel, cwd, start.clone()).await;
            match attempt {
                Err(err) if tries < 2 && err.to_string().contains("Address already in use") => {
                    tracing::info!("a kernel's port was taken before it bound it; starting again");
                    tries += 1;
                }
                done => return done,
            }
        }
    }

    async fn start_once(
        journal: Arc<Journal>,
        kernels_dir: &Path,
        kernel: &str,
        cwd: &Path,
        start: Start,
    ) -> Result<SessionHandle> {
        let root = start
            .project
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir());
        let spec =
            kernel::find_scoped(kernel, root.as_deref(), kernel::own_folder(kernels_dir)).await?;
        let language = spec.kernelspec.language.clone();
        let ark = is_ark_spec(&language, &spec.kernelspec.argv);
        // R reads `.Rprofile` and `.Renviron` in the folder it starts in,
        // which is how renv loads a project's library: start it in the
        // project's folder, then move to `cwd`.
        let start_in = match &root {
            Some(root) if crate::cells::normalize_language(&language) == "r" => root,
            _ => cwd,
        };
        let interrupt_mode = spec
            .kernelspec
            .interrupt_mode
            .clone()
            .unwrap_or_else(|| "signal".to_string());
        let id = uuid::Uuid::new_v4().simple().to_string();
        let client_session = uuid::Uuid::new_v4().to_string();
        let launched = kernel::launch(spec, start_in, &start.env, kernels_dir, &id).await?;

        let record = SessionRecord {
            id: id.clone(),
            kernel: kernel.to_string(),
            language,
            cwd: cwd.to_string_lossy().into_owned(),
            connection_file: launched.connection_file.clone(),
            client_session: client_session.clone(),
            pid: Some(launched.pid),
            interrupt_mode,
            status: SessionStatus::Starting,
            created_ms: now_ms(),
            ended_ms: None,
            project: start.project,
            language_version: None,
        };
        journal.insert_session(&record)?;

        let mut child = launched.child;
        let (tx, mut rx) = mpsc::channel(1024);
        let started = async {
            // The kernel binds its ports a moment after it starts, and
            // connecting earlier fails, so retry until it is listening.
            // A kernel that exits stops the wait at once: connecting to
            // ports nobody listens on can take a long time to fail.
            let mut conn = loop {
                let attempt =
                    kernel::connect(&launched.connection_info, &client_session, tx.clone());
                let connected = tokio::select! {
                    connected = attempt => connected,
                    status = child.wait() => bail!("kernel exited during startup ({})", status?),
                };
                match connected {
                    Ok(conn) => break conn,
                    Err(err) => {
                        tracing::debug!("kernel not listening yet: {err}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            };
            let version = wait_until_ready(&mut conn, &mut rx).await?;
            let mut prelude = None;
            let chdir = (start_in != cwd).then_some(cwd);
            if let Some(code) = startup::code(&record.language, chdir) {
                let msg = quietly(&code);
                prelude = Some(msg.header.msg_id.clone());
                conn.shell.send(msg).await?;
            }
            anyhow::Ok((conn, version, prelude))
        };
        let (conn, version, prelude) = match tokio::time::timeout(STARTUP_TIMEOUT, started).await {
            Ok(Ok(started)) => started,
            failure => {
                kernel::kill_process_group(launched.pid);
                journal.set_session_status(&id, SessionStatus::Ended)?;
                let log = kernels_dir.join(format!("{id}.log"));
                let reason = match failure {
                    Ok(Err(err)) => err.to_string(),
                    _ => "kernel did not respond in time".to_string(),
                };
                // What the kernel printed says why, more often than not.
                match kernel::log_tail(&log, 12) {
                    Some(said) => bail!(
                        "{reason}. The kernel said:\n{said}\n(kernel log: {})",
                        log.display()
                    ),
                    None => bail!("{reason} (kernel log: {})", log.display()),
                }
            }
        };
        journal.set_session_status(&id, SessionStatus::Running)?;
        let mut record = record;
        record.status = SessionStatus::Running;
        if let Some(version) = version.filter(|v| !v.is_empty()) {
            journal.set_language_version(&id, &version)?;
            record.language_version = Some(version);
        }
        let begin = Begin {
            child: Some(child),
            prelude,
            ark,
            ..Begin::default()
        };
        Ok(Self::spawn(record, journal, conn, rx, begin))
    }

    /// Reconnects to a kernel that kept running while the daemon was down.
    /// Fails (and marks the session ended) if the kernel is gone.
    pub async fn reattach(journal: Arc<Journal>, record: SessionRecord) -> Result<SessionHandle> {
        let attempt = async {
            let pid = record.pid.context("no pid recorded")?;
            // The process recorded at start, by birth as well as PID: a
            // kernel that exited may have its PID reused by any program.
            let identity = match crate::lifecycle::recorded_kernel(&record.connection_file) {
                Some(identity) if identity.alive() => identity,
                Some(_) => bail!("kernel process {pid} is gone"),
                None => crate::lifecycle::ProcessIdentity::read(pid)
                    .context(format!("kernel process {pid} is gone"))?,
            };
            let info = kernel::read_connection_file(&record.connection_file)?;
            if !kernel::heartbeat(&info, Duration::from_secs(5)).await {
                bail!("kernel does not answer its heartbeat");
            }
            let (tx, rx) = mpsc::channel(1024);
            let conn = kernel::connect(&info, &record.client_session, tx).await?;
            // Give the IOPub subscription a moment to take effect so output
            // from a cell that is still running is not missed.
            tokio::time::sleep(Duration::from_millis(300)).await;
            anyhow::Ok((conn, rx, identity))
        };
        match attempt.await {
            Ok((mut conn, rx, identity)) => {
                let mut runs = HashMap::new();
                let unfinished = journal.unfinished_runs(&record.id)?;
                for run in &unfinished {
                    runs.insert(
                        run.msg_id.clone(),
                        RunState {
                            run_id: run.id.clone(),
                            reply: None,
                            idle: false,
                            recovered: true,
                            quiet: false,
                        },
                    );
                }
                let asking = open_question(&journal, &unfinished);
                // Shell requests are handled in order, so when this reply
                // arrives every earlier run has finished.
                let probe = JupyterMessage::new(KernelInfoRequest {}, None);
                let probe_id = probe.header.msg_id.clone();
                conn.shell.send(probe).await?;
                let begin = Begin {
                    runs,
                    probe_id: Some(probe_id),
                    ark: kernel::find_kernelspec(
                        &record.kernel,
                        record.project.as_deref().map(Path::new),
                    )
                    .await
                    .is_ok_and(|spec| {
                        is_ark_spec(&spec.kernelspec.language, &spec.kernelspec.argv)
                    }),
                    asking,
                    kernel: Some(identity),
                    ..Begin::default()
                };
                Ok(Self::spawn(record, journal, conn, rx, begin))
            }
            Err(err) => {
                end_session(
                    &journal,
                    &record.id,
                    &format!("lost while sandhilld was down: {err}"),
                );
                Err(err)
            }
        }
    }

    fn spawn(
        record: SessionRecord,
        journal: Arc<Journal>,
        conn: KernelConnection,
        kernel_rx: mpsc::Receiver<KernelMsg>,
        begin: Begin,
    ) -> SessionHandle {
        let (commands, command_rx) = mpsc::channel(64);
        let (events, _) = broadcast::channel(1024);
        let id = record.id.clone();
        let actor = Actor {
            record,
            journal,
            conn,
            kernel_rx,
            command_rx,
            events: events.clone(),
            child: begin.child,
            kernel: begin.kernel,
            runs: begin.runs,
            probe_id: begin.probe_id,
            prelude: begin.prelude,
            busy: None,
            ark: begin.ark,
            completions: HashMap::new(),
            inspections: HashMap::new(),
            lsp_starts: HashMap::new(),
            asking: begin.asking,
        };
        let task = tokio::spawn(actor.run());
        SessionHandle {
            id,
            commands,
            events,
            task,
        }
    }

    /// Queues code. A `quiet` run stays out of the kernel's history and
    /// execution count, and an error in it doesn't cancel the runs queued
    /// after it: it is a tool looking around, not the user's code. `by`
    /// names the agent that ran the code, if one did.
    pub async fn execute(
        &self,
        code: String,
        cell: Option<CellRef>,
        quiet: bool,
        by: Option<String>,
    ) -> Result<RunStarted> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Execute {
            code,
            cell,
            quiet,
            by,
            reply,
        })
        .await?;
        rx.await.map_err(|_| ended())?
    }

    /// Requests live-kernel completion without executing or recording code.
    /// Cursor offsets are Unicode scalar positions, as required by Jupyter.
    pub async fn complete(&self, code: String, cursor_pos: usize) -> Result<CompletionReply> {
        let deadline = tokio::time::Instant::now() + COMPLETION_TIMEOUT;
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout_at(deadline, async {
            self.send(Command::Complete {
                code,
                cursor_pos,
                deadline,
                reply,
            })
            .await?;
            rx.await.map_err(|_| ended())?
        })
        .await
        .context("kernel completion timed out")?
    }

    /// Requests live-kernel introspection (detail level 0) at a cursor, like
    /// `complete`: never executes or records code. Offsets are Unicode scalars.
    pub async fn inspect(&self, code: String, cursor_pos: usize) -> Result<InspectReply> {
        let deadline = tokio::time::Instant::now() + COMPLETION_TIMEOUT;
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout_at(deadline, async {
            self.send(Command::Inspect {
                code,
                cursor_pos,
                deadline,
                reply,
            })
            .await?;
            rx.await.map_err(|_| ended())?
        })
        .await
        .context("kernel inspection timed out")?
    }

    /// Starts Ark's built-in language server on the daemon's loopback interface.
    /// The caller owns the returned TCP connection and LSP lifecycle.
    pub async fn start_ark_lsp(&self) -> Result<u16> {
        let deadline = tokio::time::Instant::now() + ARK_LSP_TIMEOUT;
        let (reply, rx) = oneshot::channel();
        tokio::time::timeout_at(deadline, async {
            self.send(Command::StartArkLsp { deadline, reply }).await?;
            rx.await.map_err(|_| ended())?
        })
        .await
        .context("Ark language server startup timed out")?
    }

    /// The actor accepts this only while idle, atomically with queuing it.
    pub(crate) async fn bio_check(&self, code: String) -> Result<RunStarted> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::BioCheck { code, reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub async fn interrupt(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Interrupt { reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    /// Answers the question the running code asked.
    pub async fn input(&self, value: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Input { value, reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub async fn shutdown(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Shutdown { reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub async fn force_stop(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Force { reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    /// Stops managing the kernel without touching it, as if the daemon had
    /// exited. A later [`SessionHandle::reattach`] picks it up again.
    pub fn detach(&self) {
        self.task.abort();
    }

    pub(crate) async fn detach_wait(&self) {
        self.task.abort();
        while !self.task.is_finished() {
            tokio::task::yield_now().await;
        }
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands.send(command).await.map_err(|_| ended())
    }
}

fn ended() -> anyhow::Error {
    anyhow!("the session has ended")
}

fn is_ark_spec(language: &str, argv: &[String]) -> bool {
    crate::cells::normalize_language(language) == "r"
        && argv.first().is_some_and(|program| {
            Path::new(program)
                .file_name()
                .is_some_and(|name| name == "ark" || name == "ark.exe")
        })
}

/// What a session's actor starts with besides its connection.
#[derive(Default)]
struct Begin {
    /// The kernel process, when this daemon started it.
    child: Option<Child>,
    /// Runs that were going when the daemon restarted, and the request that
    /// tells when they are done.
    runs: HashMap<String, RunState>,
    probe_id: Option<String>,
    /// The code run at start (see `startup`).
    prelude: Option<String>,
    ark: bool,
    /// The question a run was waiting on when the daemon restarted.
    asking: Option<Asking>,
    /// The kernel process, by birth as well as PID, when this daemon
    /// adopted it after a restart rather than starting it.
    kernel: Option<crate::lifecycle::ProcessIdentity>,
}

struct RunState {
    run_id: String,
    /// `(status, execution_count)` from `execute_reply`.
    reply: Option<(String, Option<u64>)>,
    /// Saw `status: idle` on IOPub for this run.
    idle: bool,
    /// Started before a daemon restart; its reply may never arrive.
    recovered: bool,
    /// A tool's run: a question from it is answered by interrupting it.
    quiet: bool,
}

/// A question the running code asked, waiting for the user's answer.
struct Asking {
    /// The `msg_id` of the run's execute request.
    msg_id: String,
    run_id: String,
    prompt: String,
    password: bool,
    /// The kernel's `input_request` header, the parent of the answer.
    header: Header,
}

struct PendingLanguage<T> {
    reply: oneshot::Sender<Result<T>>,
    deadline: tokio::time::Instant,
}

struct Actor {
    record: SessionRecord,
    journal: Arc<Journal>,
    conn: KernelConnection,
    kernel_rx: mpsc::Receiver<KernelMsg>,
    command_rx: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    child: Option<Child>,
    /// See `Begin::kernel`.
    kernel: Option<crate::lifecycle::ProcessIdentity>,
    /// Keyed by the `msg_id` of the execute request.
    runs: HashMap<String, RunState>,
    probe_id: Option<String>,
    /// The `msg_id` of the code run at start (see `startup`), whose
    /// messages stay out of the journal: IRkernel echoes even silent code.
    prelude: Option<String>,
    /// The request the kernel is busy with, for messages that name none:
    /// Ark opens its comms without a parent.
    busy: Option<String>,
    /// Derived from the actual launch spec, not the user-visible kernel name.
    ark: bool,
    completions: HashMap<String, PendingLanguage<CompletionReply>>,
    inspections: HashMap<String, PendingLanguage<InspectReply>>,
    lsp_starts: HashMap<String, PendingLanguage<u16>>,
    asking: Option<Asking>,
}

impl Actor {
    async fn run(mut self) {
        let mut liveness = tokio::time::interval(LIVENESS_INTERVAL);
        loop {
            let language_deadline = self
                .completions
                .values()
                .map(|pending| pending.deadline)
                .chain(self.inspections.values().map(|pending| pending.deadline))
                .chain(self.lsp_starts.values().map(|pending| pending.deadline))
                .min()
                .unwrap_or_else(|| tokio::time::Instant::now() + STARTUP_TIMEOUT);
            tokio::select! {
                Some(msg) = self.kernel_rx.recv() => self.on_kernel_message(msg).await,
                command = self.command_rx.recv() => match command {
                    Some(Command::Shutdown { reply }) => {
                        let result = self.shutdown().await;
                        let stopped = result.is_ok();
                        let _ = reply.send(result);
                        if stopped {
                            return;
                        }
                    }
                    Some(Command::Force { reply }) => {
                        let result = self.force_stop().await;
                        let stopped = result.is_ok();
                        let _ = reply.send(result);
                        if stopped {
                            return;
                        }
                    }
                    Some(command) => self.on_command(command).await,
                    // The daemon dropped the handle; keep the kernel running.
                    None => return,
                },
                _ = tokio::time::sleep_until(language_deadline) => self.expire_language_requests(),
                _ = liveness.tick() => {
                    if !self.kernel_alive() {
                        self.finish("kernel process exited");
                        return;
                    }
                }
            }
        }
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::Execute {
                code,
                cell,
                quiet,
                by,
                reply,
            } => {
                let _ = reply.send(self.execute(code, cell, quiet, by).await);
            }
            Command::BioCheck { code, reply } => {
                // Pending executes count as busy even before their IOPub status.
                let result = if !self.runs.is_empty()
                    || self.busy.is_some()
                    || self.probe_id.is_some()
                    || self.prelude.is_some()
                {
                    Err(anyhow!("the session is busy; checkpoint not checked"))
                } else {
                    self.execute(code, None, true, Some("Sandhill bio checker".into()))
                        .await
                };
                let _ = reply.send(result);
            }
            Command::Complete {
                code,
                cursor_pos,
                deadline,
                reply,
            } => {
                if reply.is_closed() || deadline <= tokio::time::Instant::now() {
                    return;
                }
                if self.language_busy() {
                    let _ = reply.send(Err(anyhow!("kernel_busy: kernel busy")));
                    return;
                }
                let request = JupyterMessage::new(CompleteRequest { code, cursor_pos }, None);
                let id = request.header.msg_id.clone();
                match self.send_language(request, deadline).await {
                    Ok(()) => {
                        self.completions
                            .insert(id, PendingLanguage { reply, deadline });
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Inspect {
                code,
                cursor_pos,
                deadline,
                reply,
            } => {
                if reply.is_closed() || deadline <= tokio::time::Instant::now() {
                    return;
                }
                if self.language_busy() {
                    let _ = reply.send(Err(anyhow!("kernel_busy: kernel busy")));
                    return;
                }
                let request = JupyterMessage::new(
                    InspectRequest {
                        code,
                        cursor_pos,
                        detail_level: Some(0),
                    },
                    None,
                );
                let id = request.header.msg_id.clone();
                match self.send_language(request, deadline).await {
                    Ok(()) => {
                        self.inspections
                            .insert(id, PendingLanguage { reply, deadline });
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::StartArkLsp { deadline, reply } => {
                if reply.is_closed() || deadline <= tokio::time::Instant::now() {
                    return;
                }
                if !self.ark {
                    let _ = reply.send(Err(anyhow!(
                        "ark_lsp_unsupported: R language service requires an Ark session; IRkernel and other kernels do not support positron.lsp"
                    )));
                    return;
                }
                if self.language_busy() {
                    let _ = reply.send(Err(anyhow!("kernel_busy: kernel busy")));
                    return;
                }
                if !self.lsp_starts.is_empty() {
                    let _ = reply.send(Err(anyhow!(
                        "Ark language server startup already in progress"
                    )));
                    return;
                }
                // Ark MIT, commit 4f88ecc8a7161e34ba036f933769ee7d274474cb:
                // amalthea/src/comm/server_comm.rs and socket/shell.rs.
                // Ark binds the port itself and acknowledges it on this comm.
                let id = uuid::Uuid::new_v4().to_string();
                let request = CommOpen {
                    comm_id: CommId(id.clone()),
                    target_name: "positron.lsp".into(),
                    data: serde_json::Map::from_iter([("ip_address".into(), json!("127.0.0.1"))]),
                    target_module: None,
                };
                match self
                    .send_language(JupyterMessage::new(request, None), deadline)
                    .await
                {
                    Ok(()) => {
                        self.lsp_starts
                            .insert(id, PendingLanguage { reply, deadline });
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Interrupt { reply } => {
                let _ = reply.send(self.interrupt().await);
            }
            Command::Input { value, reply } => {
                let _ = reply.send(self.answer(value).await);
            }
            Command::Shutdown { .. } | Command::Force { .. } => unreachable!("handled in run()"),
        }
    }

    fn language_busy(&self) -> bool {
        !self.runs.is_empty()
            || self.busy.is_some()
            || self.probe_id.is_some()
            || self.prelude.is_some()
    }

    async fn send_language(
        &mut self,
        request: JupyterMessage,
        deadline: tokio::time::Instant,
    ) -> Result<()> {
        let send_deadline = deadline.min(tokio::time::Instant::now() + LANGUAGE_SEND_TIMEOUT);
        tokio::time::timeout_at(send_deadline, self.conn.shell.send(request))
            .await
            .context("kernel language request send timed out")??;
        Ok(())
    }

    fn expire_language_requests(&mut self) {
        let now = tokio::time::Instant::now();
        for (_, pending) in self
            .completions
            .extract_if(|_, pending| pending.deadline <= now || pending.reply.is_closed())
        {
            let _ = pending
                .reply
                .send(Err(anyhow!("kernel completion timed out")));
        }
        for (_, pending) in self
            .inspections
            .extract_if(|_, pending| pending.deadline <= now || pending.reply.is_closed())
        {
            let _ = pending
                .reply
                .send(Err(anyhow!("kernel inspection timed out")));
        }
        for (_, pending) in self
            .lsp_starts
            .extract_if(|_, pending| pending.deadline <= now || pending.reply.is_closed())
        {
            let _ = pending
                .reply
                .send(Err(anyhow!("Ark language server startup timed out")));
        }
    }

    async fn execute(
        &mut self,
        code: String,
        cell: Option<CellRef>,
        quiet: bool,
        by: Option<String>,
    ) -> Result<RunStarted> {
        let request = ExecuteRequest {
            code: code.clone(),
            silent: false,
            store_history: !quiet,
            user_expressions: None,
            // A tool must never leave the session waiting for an answer.
            allow_stdin: !quiet,
            stop_on_error: !quiet,
        };
        let msg = JupyterMessage::new(request, None);
        let msg_id = msg.header.msg_id.clone();
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let run = NewRun {
            cell: cell.as_ref(),
            quiet,
            by: by.as_deref(),
        };
        self.journal
            .insert_run(&run_id, &self.record.id, &msg_id, &code, run)?;
        self.runs.insert(
            msg_id.clone(),
            RunState {
                run_id: run_id.clone(),
                reply: None,
                idle: false,
                recovered: false,
                quiet,
            },
        );
        let mut started = json!({ "code": code, "cell": cell });
        if quiet {
            started["quiet"] = json!(true);
        }
        if let Some(by) = by {
            started["by"] = json!(by);
        }
        self.publish(Some(&run_id), EVENT_RUN_STARTED, started);
        self.conn.shell.send(msg).await?;
        Ok(RunStarted {
            run: run_id,
            msg_id,
        })
    }

    async fn interrupt(&mut self) -> Result<()> {
        if self.record.interrupt_mode == "message" {
            let msg = JupyterMessage::new(InterruptRequest {}, None);
            self.conn.control.send(msg).await?;
        } else if let Some(child) = &self.child {
            // An unreaped child's PID can't be reused.
            let pid = child.id().context("the kernel process has exited")?;
            kernel::send_signal(pid, libc::SIGINT)?;
        } else {
            self.kernel
                .as_ref()
                .context("no kernel process recorded")?
                .interrupt()?;
        }
        Ok(())
    }

    /// Answers the open question: the prompt and the answer go into the
    /// run's output as a console would show them, then the kernel gets it.
    async fn answer(&mut self, value: String) -> Result<()> {
        let asking = self
            .asking
            .take()
            .context("nothing is being asked: the code isn't waiting for input")?;
        let shown = if asking.password { "" } else { value.as_str() };
        self.publish(
            Some(&asking.run_id),
            "stream",
            json!({ "name": "stdout", "text": format!("{}{shown}\n", asking.prompt) }),
        );
        self.publish(Some(&asking.run_id), EVENT_INPUT_REPLY, json!({}));
        let mut reply = JupyterMessage::new(
            InputReply {
                value,
                status: ReplyStatus::Ok,
                error: None,
            },
            None,
        );
        reply.parent_header = Some(asking.header);
        Ok(self.conn.stdin.send(reply).await?)
    }

    async fn shutdown(&mut self) -> Result<()> {
        // A control request can stop IPython's event loops while its
        // first shell request is still starting, leaving it unable to exit.
        // Let our own startup code reach idle before asking it to stop.
        let ready = async {
            while self.prelude.is_some() && self.kernel_alive() {
                tokio::select! {
                    Some(msg) = self.kernel_rx.recv() => self.on_kernel_message(msg).await,
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        };
        tokio::time::timeout(SHUTDOWN_GRACE, ready)
            .await
            .context("The kernel is still getting ready. Try stopping it again in a moment.")?;
        let started = tokio::time::Instant::now();
        let deadline = started + SHUTDOWN_GRACE;
        let msg = JupyterMessage::new(ShutdownRequest { restart: false }, None);
        // A vanished/disconnected kernel must not hang management forever.
        let sent = match tokio::time::timeout_at(deadline, self.conn.control.send(msg)).await {
            Ok(Ok(())) => "sent",
            Ok(Err(_)) => "could not be sent",
            Err(_) => "timed out sending",
        };
        // A kernel in the middle of a run gets to the request only when the
        // run ends: interrupt it, as stopping the kernel gives the run up
        // anyway. An interrupt is not a kill, so this is still graceful. An
        // idle kernel is left alone: interrupting one that is already
        // shutting down can stop it shutting down.
        let busy = !self.runs.is_empty();
        let interrupt_at = tokio::time::Instant::now() + SHUTDOWN_INTERRUPT;
        let mut interrupted = !busy;
        while self.kernel_alive() && tokio::time::Instant::now() < deadline {
            if !interrupted && tokio::time::Instant::now() >= interrupt_at {
                interrupted = true;
                let _ = tokio::time::timeout_at(deadline, self.interrupt()).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if self.kernel_alive() {
            // Says what happened, so a stop that fails only now and then
            // (seen on CI) can be told apart: a request that never got there,
            // or a kernel that got it and was slow to exit.
            bail!(
                "kernel did not stop gracefully; explicit force is required \
                 (shutdown request {sent}; {}; still running after {:.1} s)",
                if busy {
                    "a run was interrupted"
                } else {
                    "it was idle"
                },
                started.elapsed().as_secs_f32(),
            );
        }
        self.finish("shut down");
        Ok(())
    }

    async fn force_stop(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            // An unreaped child cannot have its PID reused.
            if let Some(pid) = child.id() {
                // The retained, unreaped leader pins this process group.
                kernel::kill_process_group(pid);
            }
            child.start_kill()?;
            tokio::time::timeout(SHUTDOWN_GRACE, child.wait())
                .await
                .context("kernel did not exit after explicit force")??;
            self.finish("explicitly force stopped");
            Ok(())
        } else {
            bail!("no retained child handle; identity-checked termination required");
        }
    }

    fn kernel_alive(&mut self) -> bool {
        match (self.child.as_mut(), &self.kernel) {
            // Our own child: try_wait also reaps it, so it never lingers as a zombie.
            (Some(child), _) => matches!(child.try_wait(), Ok(None)),
            // Adopted: the same process, not just a live PID, which another
            // program may hold once the kernel exits.
            (None, Some(kernel)) => kernel.alive(),
            (None, None) => false,
        }
    }

    async fn on_kernel_message(&mut self, msg: KernelMsg) {
        match msg {
            KernelMsg::IoPub(msg) => self.on_iopub(msg).await,
            KernelMsg::Shell(msg) => self.on_shell(msg),
            KernelMsg::Control(_) => {}
            KernelMsg::Stdin(msg) => self.on_stdin(msg).await,
        }
    }

    /// A question from running code. The user's runs show it and wait for
    /// the answer; anything else (a tool's quiet run, the code run at
    /// start) is interrupted instead, as nobody would see the question.
    async fn on_stdin(&mut self, msg: JupyterMessage) {
        let JupyterMessageContent::InputRequest(request) = &msg.content else {
            return;
        };
        let parent = msg.parent_header.as_ref().map(|h| h.msg_id.clone());
        let run = parent
            .as_deref()
            .and_then(|p| self.runs.get(p).map(|run| (p, run)))
            .filter(|(_, run)| !run.quiet);
        let Some((msg_id, run)) = run else {
            tracing::info!("interrupting a question nobody sees: {:?}", request.prompt);
            if let Err(err) = self.interrupt().await {
                tracing::warn!("cannot interrupt the kernel's question: {err:#}");
            }
            return;
        };
        let asking = Asking {
            msg_id: msg_id.to_string(),
            run_id: run.run_id.clone(),
            prompt: request.prompt.clone(),
            password: request.password,
            header: msg.header.clone(),
        };
        self.publish(
            Some(&asking.run_id),
            EVENT_INPUT_REQUEST,
            json!({
                "prompt": asking.prompt,
                "password": asking.password,
                "header": msg.header,
            }),
        );
        self.asking = Some(asking);
    }

    async fn on_iopub(&mut self, msg: JupyterMessage) {
        let parent = msg.parent_header.as_ref().map(|h| h.msg_id.clone());
        match &msg.content {
            JupyterMessageContent::CommMsg(comm) => {
                if comm.data.get("msg_type").and_then(|value| value.as_str())
                    == Some("server_started")
                    && let Some(pending) = self.lsp_starts.remove(&comm.comm_id.0)
                {
                    let result = comm
                        .data
                        .get("content")
                        .and_then(|content| content.get("port"))
                        .and_then(|port| port.as_u64())
                        .and_then(|port| u16::try_from(port).ok())
                        .filter(|port| *port != 0)
                        .context("Ark language server returned an invalid TCP port");
                    let _ = pending.reply.send(result);
                    return;
                }
            }
            JupyterMessageContent::CommClose(comm) => {
                if let Some(pending) = self.lsp_starts.remove(&comm.comm_id.0) {
                    let _ = pending.reply.send(Err(anyhow!(
                        "Ark closed positron.lsp before its language server started"
                    )));
                    return;
                }
            }
            _ => {}
        }
        if let JupyterMessageContent::Status(status) = &msg.content {
            if status.execution_state == ExecutionState::Busy {
                self.busy = parent.clone();
            } else if status.execution_state == ExecutionState::Idle && self.busy == parent {
                self.busy = None;
            }
            if status.execution_state == ExecutionState::Idle && parent == self.prelude {
                self.prelude = None;
            }
            if status.execution_state == ExecutionState::Idle
                && let Some(run) = parent.as_deref().and_then(|p| self.runs.get_mut(p))
            {
                run.idle = true;
                self.maybe_finish_run(parent.as_deref().unwrap());
            }
            return;
        }
        let msg_type = msg.header.msg_type.as_str();
        if !JOURNALED.contains(&msg_type) && msg_type != "comm_open"
            || parent.is_some() && parent == self.prelude
        {
            return;
        }
        let run_id = parent.as_deref().and_then(|p| self.run_id_for(p));
        let content = match serde_json::to_value(&msg.content) {
            Ok(content) => content,
            Err(err) => return tracing::warn!("cannot serialize {msg_type}: {err}"),
        };
        if msg_type == "stream"
            && let Some((left, pages)) = content["text"].as_str().and_then(viewer::take)
        {
            let run_id = run_id.or_else(|| self.busy.as_deref().and_then(|p| self.run_id_for(p)));
            if !left.is_empty() {
                let mut content = content;
                content["text"] = serde_json::Value::String(left);
                self.publish(run_id.as_deref(), msg_type, content);
            }
            for target in pages {
                // Reads the page and the files beside it.
                let shown = tokio::task::spawn_blocking(move || viewer::display(&target))
                    .await
                    .unwrap_or_default();
                self.publish(run_id.as_deref(), "display_data", shown);
            }
            return;
        }
        let content = if matches!(msg_type, "display_data" | "execute_result") {
            viewer::name(content)
        } else {
            content
        };
        if let Some((title, comm)) = view::request(msg_type, &content) {
            let run_id = run_id.or_else(|| self.busy.as_deref().and_then(|p| self.run_id_for(p)));
            self.publish(run_id.as_deref(), EVENT_VIEW, json!({ "title": title }));
            if let Some(comm) = comm {
                let close = CommClose {
                    comm_id: CommId(comm),
                    data: Default::default(),
                };
                if let Err(err) = self.conn.shell.send(JupyterMessage::new(close, None)).await {
                    tracing::warn!("cannot close the data explorer comm: {err}");
                }
            }
        } else if JOURNALED.contains(&msg_type) {
            self.publish(run_id.as_deref(), msg_type, content);
        }
    }

    fn on_shell(&mut self, msg: JupyterMessage) {
        let Some(parent) = msg.parent_header.as_ref().map(|h| h.msg_id.clone()) else {
            return;
        };
        match msg.content {
            JupyterMessageContent::CompleteReply(reply) => {
                if let Some(pending) = self.completions.remove(&parent) {
                    let result = if reply.status == ReplyStatus::Ok {
                        Ok(CompletionReply {
                            matches: reply.matches,
                            cursor_start: reply.cursor_start,
                            cursor_end: reply.cursor_end,
                            metadata: serde_json::Value::Object(reply.metadata),
                        })
                    } else {
                        Err(anyhow!("kernel completion failed: {:?}", reply.error))
                    };
                    let _ = pending.reply.send(result);
                }
            }
            JupyterMessageContent::InspectReply(reply) => {
                if let Some(pending) = self.inspections.remove(&parent) {
                    let result = if reply.status == ReplyStatus::Ok {
                        serde_json::to_value(&reply.data)
                            .map(|data| InspectReply {
                                found: reply.found,
                                data,
                            })
                            .map_err(Into::into)
                    } else {
                        Err(anyhow!("kernel inspection failed: {:?}", reply.error))
                    };
                    let _ = pending.reply.send(result);
                }
            }
            JupyterMessageContent::ExecuteReply(reply) => {
                let status = serde_json::to_value(&reply.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| "unknown".to_string());
                let count = Some(reply.execution_count.0 as u64).filter(|&n| n > 0);
                if let Some(run) = self.runs.get_mut(&parent) {
                    // An aborted request (queued behind an error) runs no
                    // code, and IRkernel sends no idle status for it, so
                    // the reply alone ends the run.
                    if status == "aborted" {
                        run.idle = true;
                    }
                    run.reply = Some((status, count));
                    self.maybe_finish_run(&parent);
                }
            }
            JupyterMessageContent::KernelInfoReply(_)
                if self.probe_id.as_deref() == Some(parent.as_str()) =>
            {
                self.probe_id = None;
                let recovered: Vec<String> = self
                    .runs
                    .iter()
                    .filter(|(_, run)| run.recovered)
                    .map(|(msg_id, _)| msg_id.clone())
                    .collect();
                for msg_id in recovered {
                    if let Some(run) = self.runs.get_mut(&msg_id) {
                        run.idle = true;
                    }
                    self.maybe_finish_run(&msg_id);
                }
            }
            _ => {}
        }
    }

    fn maybe_finish_run(&mut self, msg_id: &str) {
        let Some(run) = self.runs.get(msg_id) else {
            return;
        };
        let (status, count) = match (&run.reply, run.idle, run.recovered) {
            (Some((status, count)), true, _) => (status.clone(), *count),
            // The reply went to the daemon instance that sent the request.
            (None, true, true) => ("unknown".to_string(), None),
            _ => return,
        };
        let run = self.runs.remove(msg_id).unwrap();
        // A run that ends (interrupted, or failed) asks nothing any more.
        if self.asking.as_ref().is_some_and(|a| a.msg_id == msg_id) {
            self.asking = None;
        }
        if let Err(err) = self.journal.finish_run(&run.run_id, &status, count) {
            tracing::error!("cannot record end of run {}: {err}", run.run_id);
        }
        let content = RunFinished {
            status,
            execution_count: count,
        };
        self.publish(
            Some(&run.run_id),
            EVENT_RUN_FINISHED,
            serde_json::to_value(content).unwrap_or_default(),
        );
        let journal = self.journal.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(err) = crate::sidecar::record_run(&journal, &run.run_id) {
                tracing::warn!("cannot save outputs of run {}: {err:#}", run.run_id);
            }
        });
    }

    fn run_id_for(&self, msg_id: &str) -> Option<String> {
        if let Some(run) = self.runs.get(msg_id) {
            return Some(run.run_id.clone());
        }
        self.journal
            .run_by_msg_id(msg_id)
            .ok()
            .flatten()
            .filter(|run| run.session_id == self.record.id)
            .map(|run| run.id)
    }

    fn publish(&self, run_id: Option<&str>, msg_type: &str, content: serde_json::Value) {
        match self
            .journal
            .append(&self.record.id, run_id, msg_type, content)
        {
            Ok(event) => {
                // No receivers is fine: the event is in the journal.
                let _ = self.events.send(event);
            }
            Err(err) => tracing::error!("cannot journal {msg_type}: {err}"),
        }
    }

    fn finish(&mut self, reason: &str) {
        // Close out runs that will never get a reply.
        let pending: Vec<String> = self.runs.keys().cloned().collect();
        for msg_id in pending {
            if let Some(run) = self.runs.get_mut(&msg_id) {
                run.idle = true;
                run.recovered = true;
            }
            self.maybe_finish_run(&msg_id);
        }
        if let Some(event) = end_session(&self.journal, &self.record.id, reason) {
            let _ = self.events.send(event);
        }
        let _ = std::fs::remove_file(&self.record.connection_file);
    }
}

/// An execute request that runs `code` out of sight: no history, no
/// outputs, and no run in the journal.
fn quietly(code: &str) -> JupyterMessage {
    let request = ExecuteRequest {
        code: code.to_string(),
        silent: true,
        store_history: false,
        user_expressions: None,
        allow_stdin: false,
        stop_on_error: false,
    };
    JupyterMessage::new(request, None)
}

/// Sends `kernel_info_request` until a reply *and* a matching IOPub status
/// message arrive. Seeing IOPub traffic proves the subscription is live, so
/// no output from the first cell can be lost. Returns the language version
/// the reply gives.
async fn wait_until_ready(
    conn: &mut KernelConnection,
    rx: &mut mpsc::Receiver<KernelMsg>,
) -> Result<Option<String>> {
    loop {
        let request = JupyterMessage::new(KernelInfoRequest {}, None);
        let request_id = request.header.msg_id.clone();
        conn.shell.send(request).await?;
        let (mut reply, mut iopub_seen) = (None, false);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
        while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            let (KernelMsg::IoPub(m)
            | KernelMsg::Shell(m)
            | KernelMsg::Control(m)
            | KernelMsg::Stdin(m)) = &msg;
            let is_ours =
                m.parent_header.as_ref().map(|h| h.msg_id.as_str()) == Some(request_id.as_str());
            match &msg {
                KernelMsg::Shell(m) if is_ours => {
                    reply = Some(match &m.content {
                        JupyterMessageContent::KernelInfoReply(info) => {
                            Some(version_number(&info.language_info.version))
                        }
                        _ => None,
                    })
                }
                KernelMsg::IoPub(_) if is_ours => iopub_seen = true,
                _ => {}
            }
            if iopub_seen && let Some(version) = reply {
                return Ok(version);
            }
        }
    }
}

/// The number in a kernel's language version: Ark gives R's whole version
/// string, "R version 4.6.1 (2026-06-24)", where IRkernel gives "4.6.1".
fn version_number(said: &str) -> String {
    let start = said.find(|c: char| c.is_ascii_digit());
    let number = start.map(|start| {
        let rest = &said[start..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        rest[..end].trim_end_matches('.')
    });
    match number {
        Some(number) if number.contains('.') => number.to_string(),
        _ => said.trim().to_string(),
    }
}

/// Marks a session ended in the journal and records why.
fn end_session(journal: &Journal, session_id: &str, reason: &str) -> Option<Event> {
    if let Err(err) = journal.set_session_status(session_id, SessionStatus::Ended) {
        tracing::error!("cannot mark session {session_id} ended: {err}");
    }
    journal
        .append(
            session_id,
            None,
            EVENT_SESSION_ENDED,
            json!({ "reason": reason }),
        )
        .map_err(|err| tracing::error!("cannot journal end of {session_id}: {err}"))
        .ok()
}

/// The question a run was left waiting on when the daemon stopped: its last
/// `input_request` with no answer after it.
fn open_question(journal: &Journal, unfinished: &[crate::journal::RunRecord]) -> Option<Asking> {
    for run in unfinished.iter().rev() {
        let events = journal.run_events(&run.id, i64::MAX).ok()?;
        let last = events
            .iter()
            .rev()
            .find(|(msg_type, _)| msg_type == EVENT_INPUT_REQUEST || msg_type == EVENT_INPUT_REPLY);
        let Some((EVENT_INPUT_REQUEST, asked)) = last.map(|(t, c)| (t.as_str(), c)) else {
            continue;
        };
        let header = serde_json::from_value(asked["header"].clone()).ok()?;
        return Some(Asking {
            msg_id: run.msg_id.clone(),
            run_id: run.id.clone(),
            prompt: asked["prompt"].as_str()?.to_string(),
            password: asked["password"].as_bool().unwrap_or(false),
            header,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::version_number;

    #[test]
    fn a_kernel_version_is_its_number() {
        assert_eq!(version_number("R version 4.6.1 (2026-06-24)"), "4.6.1");
        assert_eq!(version_number("4.4.1"), "4.4.1");
        assert_eq!(version_number("3.11.9"), "3.11.9");
        assert_eq!(version_number(" nightly "), "nightly");
    }
}
