//! One kernel session: an actor that owns the kernel connection, journals
//! every output, and tracks when each run finishes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jupyter_protocol::{
    ExecuteRequest, ExecutionState, InterruptRequest, JupyterMessage, JupyterMessageContent,
    KernelInfoRequest, ShutdownRequest,
};
use sandhill_proto::{
    CellRef, EVENT_RUN_FINISHED, EVENT_RUN_STARTED, EVENT_SESSION_ENDED, Event, RunFinished,
    RunStarted, SessionStatus,
};
use serde_json::json;
use tokio::process::Child;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::journal::{Journal, SessionRecord, now_ms};
use crate::kernel::{self, KernelConnection, KernelMsg};

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
const LIVENESS_INTERVAL: Duration = Duration::from_secs(2);

enum Command {
    Execute {
        code: String,
        cell: Option<CellRef>,
        reply: oneshot::Sender<Result<RunStarted>>,
    },
    Interrupt {
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<()>>,
    },
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
    pub async fn start(
        journal: Arc<Journal>,
        kernels_dir: &Path,
        kernel: &str,
        cwd: &Path,
    ) -> Result<SessionHandle> {
        let spec = kernel::find_kernelspec(kernel).await?;
        let language = spec.kernelspec.language.clone();
        let interrupt_mode = spec
            .kernelspec
            .interrupt_mode
            .clone()
            .unwrap_or_else(|| "signal".to_string());
        let id = uuid::Uuid::new_v4().simple().to_string();
        let client_session = uuid::Uuid::new_v4().to_string();
        let launched = kernel::launch(spec, cwd, kernels_dir, &id).await?;

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
        };
        journal.insert_session(&record)?;

        let mut child = launched.child;
        let (tx, mut rx) = mpsc::channel(1024);
        let started = async {
            // The kernel binds its ports a moment after it starts, and
            // connecting earlier fails, so retry until it is listening.
            let mut conn = loop {
                match kernel::connect(&launched.connection_info, &client_session, tx.clone()).await
                {
                    Ok(conn) => break conn,
                    Err(err) => {
                        if let Ok(Some(status)) = child.try_wait() {
                            bail!("kernel exited during startup ({status})");
                        }
                        tracing::debug!("kernel not listening yet: {err}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            };
            wait_until_ready(&mut conn, &mut rx).await?;
            anyhow::Ok(conn)
        };
        let conn = match tokio::time::timeout(STARTUP_TIMEOUT, started).await {
            Ok(Ok(conn)) => conn,
            failure => {
                kernel::kill_process_group(launched.pid);
                journal.set_session_status(&id, SessionStatus::Ended)?;
                let log = kernels_dir.join(format!("{id}.log"));
                let reason = match failure {
                    Ok(Err(err)) => err.to_string(),
                    _ => "kernel did not respond in time".to_string(),
                };
                bail!("{reason} (kernel log: {})", log.display());
            }
        };
        journal.set_session_status(&id, SessionStatus::Running)?;
        let mut record = record;
        record.status = SessionStatus::Running;
        Ok(Self::spawn(
            record,
            journal,
            conn,
            rx,
            Some(child),
            HashMap::new(),
            None,
        ))
    }

    /// Reconnects to a kernel that kept running while the daemon was down.
    /// Fails (and marks the session ended) if the kernel is gone.
    pub async fn reattach(journal: Arc<Journal>, record: SessionRecord) -> Result<SessionHandle> {
        let attempt = async {
            let pid = record.pid.context("no pid recorded")?;
            if !kernel::pid_alive(pid) {
                bail!("kernel process {pid} is gone");
            }
            let info = kernel::read_connection_file(&record.connection_file)?;
            if !kernel::heartbeat(&info, Duration::from_secs(5)).await {
                bail!("kernel does not answer its heartbeat");
            }
            let (tx, rx) = mpsc::channel(1024);
            let conn = kernel::connect(&info, &record.client_session, tx).await?;
            // Give the IOPub subscription a moment to take effect so output
            // from a cell that is still running is not missed.
            tokio::time::sleep(Duration::from_millis(300)).await;
            anyhow::Ok((conn, rx))
        };
        match attempt.await {
            Ok((mut conn, rx)) => {
                let mut runs = HashMap::new();
                for run in journal.unfinished_runs(&record.id)? {
                    runs.insert(
                        run.msg_id.clone(),
                        RunState {
                            run_id: run.id,
                            reply: None,
                            idle: false,
                            recovered: true,
                        },
                    );
                }
                // Shell requests are handled in order, so when this reply
                // arrives every earlier run has finished.
                let probe = JupyterMessage::new(KernelInfoRequest {}, None);
                let probe_id = probe.header.msg_id.clone();
                conn.shell.send(probe).await?;
                Ok(Self::spawn(
                    record,
                    journal,
                    conn,
                    rx,
                    None,
                    runs,
                    Some(probe_id),
                ))
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
        child: Option<Child>,
        runs: HashMap<String, RunState>,
        probe_id: Option<String>,
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
            child,
            runs,
            probe_id,
        };
        let task = tokio::spawn(actor.run());
        SessionHandle {
            id,
            commands,
            events,
            task,
        }
    }

    pub async fn execute(&self, code: String, cell: Option<CellRef>) -> Result<RunStarted> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Execute { code, cell, reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub async fn interrupt(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Interrupt { reply }).await?;
        rx.await.map_err(|_| ended())?
    }

    pub async fn shutdown(&self) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Shutdown { reply }).await?;
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

    async fn send(&self, command: Command) -> Result<()> {
        self.commands.send(command).await.map_err(|_| ended())
    }
}

fn ended() -> anyhow::Error {
    anyhow!("the session has ended")
}

struct RunState {
    run_id: String,
    /// `(status, execution_count)` from `execute_reply`.
    reply: Option<(String, Option<u64>)>,
    /// Saw `status: idle` on IOPub for this run.
    idle: bool,
    /// Started before a daemon restart; its reply may never arrive.
    recovered: bool,
}

struct Actor {
    record: SessionRecord,
    journal: Arc<Journal>,
    conn: KernelConnection,
    kernel_rx: mpsc::Receiver<KernelMsg>,
    command_rx: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    child: Option<Child>,
    /// Keyed by the `msg_id` of the execute request.
    runs: HashMap<String, RunState>,
    probe_id: Option<String>,
}

impl Actor {
    async fn run(mut self) {
        let mut liveness = tokio::time::interval(LIVENESS_INTERVAL);
        loop {
            tokio::select! {
                Some(msg) = self.kernel_rx.recv() => self.on_kernel_message(msg),
                command = self.command_rx.recv() => match command {
                    Some(Command::Shutdown { reply }) => {
                        let result = self.shutdown().await;
                        let _ = reply.send(result);
                        return;
                    }
                    Some(command) => self.on_command(command).await,
                    // The daemon dropped the handle; keep the kernel running.
                    None => return,
                },
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
            Command::Execute { code, cell, reply } => {
                let _ = reply.send(self.execute(code, cell).await);
            }
            Command::Interrupt { reply } => {
                let _ = reply.send(self.interrupt().await);
            }
            Command::Shutdown { .. } => unreachable!("handled in run()"),
        }
    }

    async fn execute(&mut self, code: String, cell: Option<CellRef>) -> Result<RunStarted> {
        let request = ExecuteRequest {
            code: code.clone(),
            silent: false,
            store_history: true,
            user_expressions: None,
            allow_stdin: false,
            stop_on_error: true,
        };
        let msg = JupyterMessage::new(request, None);
        let msg_id = msg.header.msg_id.clone();
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        self.journal
            .insert_run(&run_id, &self.record.id, &msg_id, &code, cell.as_ref())?;
        self.runs.insert(
            msg_id.clone(),
            RunState {
                run_id: run_id.clone(),
                reply: None,
                idle: false,
                recovered: false,
            },
        );
        self.publish(
            Some(&run_id),
            EVENT_RUN_STARTED,
            json!({ "code": code, "cell": cell }),
        );
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
        } else {
            let pid = self.record.pid.context("no pid recorded")?;
            kernel::send_signal(pid, libc::SIGINT)?;
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        let msg = JupyterMessage::new(ShutdownRequest { restart: false }, None);
        let _ = self.conn.control.send(msg).await;
        let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE;
        while self.kernel_alive() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(pid) = self.record.pid {
            kernel::kill_process_group(pid);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait().await;
        }
        self.finish("shut down");
        Ok(())
    }

    fn kernel_alive(&mut self) -> bool {
        match (self.child.as_mut(), self.record.pid) {
            // Our own child: try_wait also reaps it, so it never lingers as a zombie.
            (Some(child), _) => matches!(child.try_wait(), Ok(None)),
            (None, Some(pid)) => kernel::pid_alive(pid),
            (None, None) => false,
        }
    }

    fn on_kernel_message(&mut self, msg: KernelMsg) {
        match msg {
            KernelMsg::IoPub(msg) => self.on_iopub(msg),
            KernelMsg::Shell(msg) => self.on_shell(msg),
            KernelMsg::Control(_) => {}
        }
    }

    fn on_iopub(&mut self, msg: JupyterMessage) {
        let parent = msg.parent_header.as_ref().map(|h| h.msg_id.clone());
        if let JupyterMessageContent::Status(status) = &msg.content {
            if status.execution_state == ExecutionState::Idle
                && let Some(run) = parent.as_deref().and_then(|p| self.runs.get_mut(p))
            {
                run.idle = true;
                self.maybe_finish_run(parent.as_deref().unwrap());
            }
            return;
        }
        let msg_type = msg.header.msg_type.as_str();
        if !JOURNALED.contains(&msg_type) {
            return;
        }
        let run_id = parent.as_deref().and_then(|p| self.run_id_for(p));
        match serde_json::to_value(&msg.content) {
            Ok(content) => self.publish(run_id.as_deref(), msg_type, content),
            Err(err) => tracing::warn!("cannot serialize {msg_type}: {err}"),
        }
    }

    fn on_shell(&mut self, msg: JupyterMessage) {
        let Some(parent) = msg.parent_header.as_ref().map(|h| h.msg_id.clone()) else {
            return;
        };
        match &msg.content {
            JupyterMessageContent::ExecuteReply(reply) => {
                let status = serde_json::to_value(&reply.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| "unknown".to_string());
                let count = Some(reply.execution_count.0 as u64).filter(|&n| n > 0);
                if let Some(run) = self.runs.get_mut(&parent) {
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

/// Sends `kernel_info_request` until a reply *and* a matching IOPub status
/// message arrive. Seeing IOPub traffic proves the subscription is live, so
/// no output from the first cell can be lost.
async fn wait_until_ready(
    conn: &mut KernelConnection,
    rx: &mut mpsc::Receiver<KernelMsg>,
) -> Result<()> {
    loop {
        let request = JupyterMessage::new(KernelInfoRequest {}, None);
        let request_id = request.header.msg_id.clone();
        conn.shell.send(request).await?;
        let (mut replied, mut iopub_seen) = (false, false);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1000);
        while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            let (KernelMsg::IoPub(m) | KernelMsg::Shell(m) | KernelMsg::Control(m)) = &msg;
            let is_ours =
                m.parent_header.as_ref().map(|h| h.msg_id.as_str()) == Some(request_id.as_str());
            match msg {
                KernelMsg::Shell(_) if is_ours => replied = true,
                KernelMsg::IoPub(_) if is_ours => iopub_seen = true,
                _ => {}
            }
            if replied && iopub_seen {
                return Ok(());
            }
        }
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
