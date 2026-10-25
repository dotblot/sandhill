//! One kernel session: an actor that owns the kernel connection, journals
//! every output, and tracks when each run finishes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jupyter_protocol::{
    CommClose, CommId, ExecuteRequest, ExecutionState, InterruptRequest, JupyterMessage,
    JupyterMessageContent, KernelInfoRequest, ShutdownRequest,
};
use sandhill_proto::{
    CellRef, EVENT_RUN_FINISHED, EVENT_RUN_STARTED, EVENT_SESSION_ENDED, EVENT_VIEW, Event,
    RunFinished, RunStarted, SessionStatus,
};
use serde_json::json;
use tokio::process::Child;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::journal::{Journal, NewRun, SessionRecord, now_ms};
use crate::kernel::{self, KernelConnection, KernelMsg};
use crate::{startup, view};

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
        quiet: bool,
        by: Option<String>,
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
        project: Option<String>,
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
            project,
            language_version: None,
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
            let version = wait_until_ready(&mut conn, &mut rx).await?;
            let mut prelude = None;
            if let Some(code) = startup::code(&record.language) {
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
                bail!("{reason} (kernel log: {})", log.display());
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
            ..Begin::default()
        };
        Ok(Self::spawn(record, journal, conn, rx, begin))
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
                let begin = Begin {
                    runs,
                    probe_id: Some(probe_id),
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
            runs: begin.runs,
            probe_id: begin.probe_id,
            prelude: begin.prelude,
            busy: None,
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
    /// The `msg_id` of the code run at start (see `startup`), whose
    /// messages stay out of the journal: IRkernel echoes even silent code.
    prelude: Option<String>,
    /// The request the kernel is busy with, for messages that name none:
    /// Ark opens its comms without a parent.
    busy: Option<String>,
}

impl Actor {
    async fn run(mut self) {
        let mut liveness = tokio::time::interval(LIVENESS_INTERVAL);
        loop {
            tokio::select! {
                Some(msg) = self.kernel_rx.recv() => self.on_kernel_message(msg).await,
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
            Command::Execute {
                code,
                cell,
                quiet,
                by,
                reply,
            } => {
                let _ = reply.send(self.execute(code, cell, quiet, by).await);
            }
            Command::Interrupt { reply } => {
                let _ = reply.send(self.interrupt().await);
            }
            Command::Shutdown { .. } => unreachable!("handled in run()"),
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
            allow_stdin: false,
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

    async fn on_kernel_message(&mut self, msg: KernelMsg) {
        match msg {
            KernelMsg::IoPub(msg) => self.on_iopub(msg).await,
            KernelMsg::Shell(msg) => self.on_shell(msg),
            KernelMsg::Control(_) => {}
        }
    }

    async fn on_iopub(&mut self, msg: JupyterMessage) {
        let parent = msg.parent_header.as_ref().map(|h| h.msg_id.clone());
        if let JupyterMessageContent::Status(status) = &msg.content {
            if status.execution_state == ExecutionState::Busy {
                self.busy = parent.clone();
            } else if status.execution_state == ExecutionState::Idle && self.busy == parent {
                self.busy = None;
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
        match &msg.content {
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
            let (KernelMsg::IoPub(m) | KernelMsg::Shell(m) | KernelMsg::Control(m)) = &msg;
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
