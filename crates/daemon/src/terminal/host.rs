//! The terminal host: one small process per terminal (`sandhilld
//! terminal-host`) that owns the terminal and the program in it. It is not
//! a child of the daemon in any way that matters, so the program keeps
//! running when the daemon restarts, as kernels do; the new daemon finds it
//! by its socket. The host keeps the output on disk ([`super::log`]) and
//! serves it, with input, resizing and agent reports, on a Unix socket in
//! the terminal's folder. It also keeps an agent's conversation
//! ([`super::chat`]), and holds its hooks' questions until the user
//! answers.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use sandhill_proto::{ChatEntry, TerminalExit};
use serde_json::{Value, json};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot};

use super::chat::{self, ChatLog};
use super::log::{self, LogWriter};
use super::{FromHost, Paths, RECENT, ToHost, pty};
use crate::journal::now_ms;

/// Output is sent to clients in pieces of at most this many bytes.
const FRAME: usize = 64 * 1024;
/// After the program exits, output still arriving this soon is kept.
const DRAIN: Duration = Duration::from_millis(200);
/// Commands kept for a program that doesn't listen yet.
const MAX_QUEUED: usize = 64;

pub struct HostOptions {
    pub dir: PathBuf,
    pub cwd: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub program: Vec<OsString>,
}

#[derive(Clone)]
enum Live {
    Output(u64, Arc<Vec<u8>>),
    Agent(Value),
    Chat(ChatEntry),
    Exit(TerminalExit),
}

struct Shared {
    paths: Paths,
    log: Mutex<LogWriter>,
    chat: Mutex<ChatLog>,
    /// Hooks waiting for the user's answer, by the ask's id.
    asks: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// Connections that take the user's commands, and commands that came
    /// before any did.
    listeners: Mutex<Vec<mpsc::UnboundedSender<FromHost>>>,
    queued: Mutex<Vec<Value>>,
    live: broadcast::Sender<Live>,
    agent: Mutex<Value>,
    kind: String,
    /// Locked before agent/child when checking idle retirement.
    activity: Mutex<Option<super::idle::Activity>>,
    exit: Mutex<Option<TerminalExit>>,
    input: mpsc::UnboundedSender<Vec<u8>>,
    master: Arc<AsyncFd<OwnedFd>>,
    pid: u32,
    /// Kept until reaped under this lock; never signal a recycled child PID.
    child: Mutex<Option<std::process::Child>>,
}

/// Runs the program until it exits, serving clients meanwhile.
pub async fn run(options: HostOptions) -> Result<()> {
    let paths = Paths::new(&options.dir);
    let (program, args) = options.program.split_first().context("no program to run")?;
    // Listening before the program starts: what it runs to talk to the
    // terminal (Codex's bridge, Claude Code's hooks) may connect at once.
    let socket = paths.socket();
    if let Some(parent) = socket.parent()
        && parent != paths.dir()
    {
        use std::os::unix::fs::DirBuilderExt;
        use std::os::unix::fs::MetadataExt;
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent);
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let meta = std::fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            bail!("{} is not a private folder of this user", parent.display());
        }
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("cannot listen on {}", socket.display()))?;

    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(&options.cwd);
    let pty::Pty { master, child } = match pty::spawn(&mut command, options.cols, options.rows) {
        Ok(pty) => pty,
        Err(err) => {
            let _ = std::fs::remove_file(&socket);
            return Err(err).with_context(|| format!("cannot start {}", program.to_string_lossy()));
        }
    };
    let pid = child.id();
    write_json(
        &paths.host(),
        &json!({"host": std::process::id(), "pid": pid,
            "host_identity": crate::lifecycle::ProcessIdentity::read(std::process::id()),
            "identity": crate::lifecycle::ProcessIdentity::read(pid),
            "management_stop": true}),
    )?;
    pty::set_nonblocking(master.as_raw_fd())?;
    // SAFETY: `master` is the pseudo-terminal's open descriptor, owned by the
    // `AsyncFd` from here on and closed only when that is dropped.
    let master = Arc::new(unsafe { AsyncFd::register(master) }.map_err(std::io::Error::from)?);

    let (input, input_rx) = mpsc::unbounded_channel();
    let meta = super::read_json::<super::Meta>(&paths.meta());
    let kind = meta.as_ref().map_or("", |meta| meta.kind.as_str());
    let shared = Arc::new(Shared {
        log: Mutex::new(LogWriter::create(&options.dir)?),
        chat: Mutex::new(ChatLog::open(&options.dir)?),
        asks: Mutex::new(HashMap::new()),
        listeners: Mutex::new(Vec::new()),
        queued: Mutex::new(Vec::new()),
        live: broadcast::channel(1024).0,
        // What a restarted program's terminal kept of the agent's report.
        agent: Mutex::new(super::read_json(&paths.agent()).unwrap_or(Value::Null)),
        exit: Mutex::new(None),
        kind: kind.to_string(),
        activity: Mutex::new(super::idle::is_agent(kind).then(super::idle::Activity::new)),
        input,
        master: master.clone(),
        pid,
        child: Mutex::new(Some(child)),
        paths: paths.clone(),
    });
    tokio::spawn(write_input(master.clone(), input_rx));
    tokio::spawn(accept(shared.clone(), listener));

    let reaper = shared.clone();
    let mut exited = Box::pin(async move {
        loop {
            {
                let mut child = reaper.child.lock().unwrap();
                if let Some(status) = child.as_mut().expect("single reaper").try_wait()? {
                    *child = None;
                    return std::io::Result::Ok(status);
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let mut render = meta
        .filter(|meta| meta.kind == sandhill_proto::TERMINAL_JOB)
        .and_then(|meta| meta.job?.path)
        .filter(|file| crate::preview::previewable(Path::new(file)))
        .map(|_| crate::jobs::RenderOutputs::new(options.cwd.clone()));

    let mut buf = vec![0u8; 64 * 1024];
    let mut status = None;
    loop {
        tokio::select! {
            read = read_some(&master, &mut buf) => match read {
                Some(n) => {
                    if let Some(render) = &mut render {
                        render.push(&buf[..n]);
                    }
                    shared.publish(&buf[..n]);
                }
                // The terminal closed: everything holding it has exited.
                None => break,
            },
            result = &mut exited, if status.is_none() => {
                status = Some(result.ok());
                // Keep what is still on its way, then stop even if a
                // background process holds the terminal open.
                while let Ok(Some(n)) =
                    tokio::time::timeout(DRAIN, read_some(&master, &mut buf)).await
                {
                    if let Some(render) = &mut render {
                        render.push(&buf[..n]);
                    }
                    shared.publish(&buf[..n]);
                }
                break;
            }
        }
    }
    let status = match status {
        Some(status) => status,
        None => exited.await.ok(),
    };
    let exit = TerminalExit {
        code: status.and_then(|s| s.code()),
        signal: status.and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s)),
        ended_ms: now_ms(),
        idle: shared
            .activity
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.stopped),
    };
    if exit.code == Some(0)
        && exit.signal.is_none()
        && let Some(render) = render
    {
        write_json(
            &paths.render_outputs(),
            &serde_json::to_value(render.finish())?,
        )?;
    }
    write_json(&paths.exit(), &serde_json::to_value(&exit)?)?;
    *shared.exit.lock().unwrap() = Some(exit.clone());
    let _ = shared.live.send(Live::Exit(exit));
    // New clients read the files from now on; give current ones a moment
    // to receive the end.
    let _ = std::fs::remove_file(paths.socket());
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

/// Reads what the program printed; `None` once the terminal is closed.
async fn read_some(master: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> Option<usize> {
    loop {
        let mut guard = master.readable().await.ok()?;
        match guard.try_io(|fd| {
            // SAFETY: reading into a buffer we own, of its length.
            let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }) {
            Ok(Ok(0)) => return None,
            Ok(Ok(n)) => return Some(n),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            // EIO: the program and everything it started have exited.
            Ok(Err(_)) => return None,
            Err(_would_block) => continue,
        }
    }
}

/// Writes typed input to the terminal, in order.
async fn write_input(master: Arc<AsyncFd<OwnedFd>>, mut input: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = input.recv().await {
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            let Ok(mut guard) = master.writable().await else {
                return;
            };
            match guard.try_io(|fd| {
                // SAFETY: writing from a buffer we own, of its length.
                let n = unsafe { libc::write(fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
                if n < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(n)) => rest = &rest[n..],
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Ok(Err(_)) => return,
                Err(_would_block) => {}
            }
        }
    }
}

async fn accept(shared: Arc<Shared>, listener: UnixListener) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(serve(shared.clone(), stream));
    }
}

/// One client: usually the daemon, for a subscription or for input.
async fn serve(shared: Arc<Shared>, stream: UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let (out, mut out_rx) = mpsc::unbounded_channel::<FromHost>();
    let writer_task = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let Ok(mut line) = serde_json::to_vec(&message) else {
                continue;
            };
            line.push(b'\n');
            if writer.write_all(&line).await.is_err() {
                return;
            }
        }
    });
    let mut subscription: Option<tokio::task::JoinHandle<()>> = None;
    // What this connection's hook asked and still waits for.
    let mut asked = Vec::new();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<ToHost>(&line) else {
            continue;
        };
        match message {
            ToHost::Subscribe { from, chat_from } => {
                if let Some(old) = subscription.take() {
                    old.abort();
                }
                subscription = Some(tokio::spawn(subscribe(
                    shared.clone(),
                    from,
                    chat_from,
                    out.clone(),
                )));
            }
            ToHost::Input { data } => {
                shared.touch();
                let _ = shared.input.send(data.into_bytes());
            }
            ToHost::Resize { cols, rows } => shared.resize(cols, rows),
            ToHost::Hangup => shared.hang_up(),
            ToHost::Stop { force } => {
                shared.stop_program(force);
            }
            ToHost::StopIfIdle { limit_ms } => {
                shared.stop_if_idle(Duration::from_millis(limit_ms));
            }
            ToHost::Agent { value } => shared.report(value),
            ToHost::Chat { event } => shared.add_chat(event),
            ToHost::Ask { id, event } => {
                let (tx, rx) = oneshot::channel();
                shared.asks.lock().unwrap().insert(id.clone(), tx);
                shared.add_chat(event);
                asked.push(id);
                let out = out.clone();
                tokio::spawn(async move {
                    if let Ok(answer) = rx.await {
                        let _ = out.send(FromHost::Answer { answer });
                    }
                });
            }
            ToHost::Answer { id, answer } => {
                shared.touch();
                shared.answer(&id, answer);
            }
            ToHost::Listen => shared.listen(out.clone()),
            ToHost::Command { command } => {
                shared.touch();
                shared.command(command);
            }
        }
    }
    if let Some(task) = subscription {
        task.abort();
    }
    // The hook stopped waiting (the agent gave up on it, or was stopped).
    for id in asked {
        shared.answer(&id, Value::Null);
    }
    shared
        .listeners
        .lock()
        .unwrap()
        .retain(|listener| !listener.same_channel(&out));
    drop(out);
    let _ = writer_task.await;
}

/// Sends the output from `from` on (and the conversation from `chat_from`
/// on, if asked), then follows them live.
async fn subscribe(
    shared: Arc<Shared>,
    from: Option<u64>,
    chat_from: Option<u64>,
    out: mpsc::UnboundedSender<FromHost>,
) {
    // Taking the live feed and the ends together means nothing falls
    // between what is read from disk and what arrives live.
    let (mut live, end, chat_end) = {
        let log = shared.log.lock().unwrap();
        let chat = shared.chat.lock().unwrap();
        (shared.live.subscribe(), log.end(), chat.next())
    };
    let recent = from.is_none();
    let from = from.map_or_else(|| end.saturating_sub(RECENT), |f| f.min(end));
    let Ok((start, bytes)) = log::read(shared.paths.dir(), from, end) else {
        return;
    };
    let (start, bytes) = if recent && start > 0 {
        after_line_break(start, bytes)
    } else {
        (start, bytes)
    };
    if out.send(FromHost::Start { from: start }).is_err() {
        return;
    }
    send_output(&out, start, &bytes);
    let mut next = end;
    // The conversation: what is on disk now, then what comes live.
    let mut next_chat = chat_end;
    if let Some(chat_from) = chat_from
        && !send_chat(&out, &shared, chat_from, chat_end)
    {
        return;
    }
    let agent = shared.agent.lock().unwrap().clone();
    if !agent.is_null() {
        let _ = out.send(FromHost::Agent { value: agent });
    }
    if let Some(exit) = shared.exit.lock().unwrap().clone() {
        let _ = out.send(FromHost::Exit { exit });
        return;
    }
    loop {
        match live.recv().await {
            Ok(Live::Output(offset, bytes)) if offset >= next => {
                next = offset + bytes.len() as u64;
                if !send_output(&out, offset, &bytes) {
                    return;
                }
            }
            Ok(Live::Output(..)) => {}
            Ok(Live::Agent(value)) => {
                let _ = out.send(FromHost::Agent { value });
            }
            Ok(Live::Chat(entry)) if chat_from.is_some() && entry.seq >= next_chat => {
                next_chat = entry.seq + 1;
                if out.send(FromHost::Chat { entry }).is_err() {
                    return;
                }
            }
            Ok(Live::Chat(_)) => {}
            Ok(Live::Exit(exit)) => {
                let _ = out.send(FromHost::Exit { exit });
                return;
            }
            // A slow client fell behind: catch up from disk.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let end = shared.log.lock().unwrap().end();
                let Ok((start, bytes)) = log::read(shared.paths.dir(), next, end) else {
                    return;
                };
                if !send_output(&out, start, &bytes) {
                    return;
                }
                next = end;
                if chat_from.is_some() {
                    let chat_end = shared.chat.lock().unwrap().next();
                    if !send_chat(&out, &shared, next_chat, chat_end) {
                        return;
                    }
                    next_chat = chat_end;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Sends the conversation's entries from `from` up to `end` from disk.
fn send_chat(out: &mpsc::UnboundedSender<FromHost>, shared: &Shared, from: u64, end: u64) -> bool {
    chat::read(shared.paths.dir(), from)
        .into_iter()
        .take_while(|entry| entry.seq < end)
        .all(|entry| out.send(FromHost::Chat { entry }).is_ok())
}

fn send_output(out: &mpsc::UnboundedSender<FromHost>, offset: u64, bytes: &[u8]) -> bool {
    let engine = base64::engine::general_purpose::STANDARD;
    for (i, piece) in bytes.chunks(FRAME).enumerate() {
        let message = FromHost::Output {
            offset: offset + (i * FRAME) as u64,
            data: engine.encode(piece),
        };
        if out.send(message).is_err() {
            return false;
        }
    }
    true
}

/// Recent output starts at an arbitrary byte, which may be inside an
/// escape sequence or a character; start after the next line break instead
/// when there is one near.
pub(super) fn after_line_break(start: u64, bytes: Vec<u8>) -> (u64, Vec<u8>) {
    match bytes.iter().take(4096).position(|&b| b == b'\n') {
        Some(i) => (start + i as u64 + 1, bytes[i + 1..].to_vec()),
        None => (start, bytes),
    }
}

impl Shared {
    fn touch(&self) {
        if let Some(activity) = self.activity.lock().unwrap().as_mut() {
            activity.touch();
        }
    }

    fn stop_if_idle(&self, limit: Duration) {
        let mut activity = self.activity.lock().unwrap();
        let Some(activity) = activity.as_mut() else {
            return;
        };
        let agent = self.agent.lock().unwrap();
        if activity.expired(limit) && super::idle::resumable_idle(&self.kind, &agent) {
            // Record the reason before the reaper can publish the exit.
            activity.stopped = self.stop_program(false);
        }
    }

    fn publish(&self, bytes: &[u8]) {
        self.touch();
        let mut log = self.log.lock().unwrap();
        let offset = log.end();
        if let Err(err) = log.append(bytes) {
            eprintln!("cannot save terminal output: {err}");
        }
        let _ = self
            .live
            .send(Live::Output(offset, Arc::new(bytes.to_vec())));
    }

    fn resize(&self, cols: u16, rows: u16) {
        let fd = self.master.get_ref().as_raw_fd();
        if pty::resize(fd, cols, rows).is_ok() {
            // Redraw even when the size didn't change: a client that just
            // attached wants a fresh screen.
            let group = pty::foreground_group(fd).unwrap_or(self.pid as libc::pid_t);
            // SAFETY: signalling a process group in our own session.
            unsafe { libc::kill(-group, libc::SIGWINCH) };
        }
    }

    /// The same owned-child path is used for closing a terminal and Stop All.
    /// Never schedule a delayed numeric-PID kill after the child was reaped.
    fn hang_up(&self) {
        self.stop_program(false);
    }

    fn stop_program(&self, force: bool) -> bool {
        let child = self.child.lock().unwrap();
        if child.is_none() {
            return false;
        }
        let signal = if force { libc::SIGKILL } else { libc::SIGHUP };
        let _ = pty::signal_foreground(self.master.get_ref().as_raw_fd(), signal);
        // Holding the unreaped child fixes the identity of the process-group
        // leader. This group cannot be recycled while that child exists.
        unsafe { libc::kill(-(self.pid as libc::pid_t), signal) };
        true
    }

    /// Merges what an agent reported (`{state}` from a hook, `{status}` from
    /// its status line) and tells the clients.
    fn report(&self, value: Value) {
        let mut activity = self.activity.lock().unwrap();
        if let Some(activity) = activity.as_mut() {
            activity.touch();
        }
        let merged = {
            let mut agent = self.agent.lock().unwrap();
            if !agent.is_object() {
                *agent = json!({});
            }
            if let (Some(agent), Some(update)) = (agent.as_object_mut(), value.as_object()) {
                // A new state replaces the message that came with the old one.
                if update.contains_key("state") && !update.contains_key("message") {
                    agent.remove("message");
                }
                for (key, value) in update {
                    agent.insert(key.clone(), value.clone());
                }
                agent.insert("ts_ms".into(), json!(now_ms()));
            }
            agent.clone()
        };
        drop(activity);
        let _ = write_json(&self.paths.agent(), &merged);
        crate::accounts::observe(&self.kind, &value);
        let _ = self.live.send(Live::Agent(merged));
    }

    /// Keeps an entry of the agent's conversation and tells the clients.
    fn add_chat(&self, event: Value) {
        self.touch();
        let mut chat = self.chat.lock().unwrap();
        match chat.append(event) {
            Ok(entry) => {
                let _ = self.live.send(Live::Chat(entry));
            }
            Err(err) => eprintln!("cannot save the conversation: {err}"),
        }
    }

    /// Takes the user's commands on `out` from now on, starting with those
    /// that waited.
    fn listen(&self, out: mpsc::UnboundedSender<FromHost>) {
        let mut listeners = self.listeners.lock().unwrap();
        for command in self.queued.lock().unwrap().drain(..) {
            let _ = out.send(FromHost::Command { command });
        }
        listeners.push(out);
    }

    /// Passes a command from the user to the listening program. A mode
    /// switch also sets Sandhill's own guard, which Claude Code's hooks
    /// read, and where the agent's code goes, which the MCP server reads
    /// (Claude Code doesn't listen; its commands just wait here).
    fn command(&self, command: Value) {
        if command["kind"] == "set"
            && let Some(permission) = command.get("permission")
        {
            let guard = super::guard::of(permission.as_str());
            self.report(json!({ "guard": guard }));
        }
        if command["kind"] == "set"
            && let Some(code) = command.get("code")
        {
            self.report(json!({ "code": super::code_to(code.as_str()) }));
        }
        // The message box's choices, which the next message reads: the
        // pinned file, web search and session context, and (with a set or
        // with the message itself) what was added to the message.
        let choices: serde_json::Map<String, Value> = super::choices::KEYS
            .iter()
            .filter(|key| command["kind"] == "set" || **key == "refs")
            .filter_map(|key| Some((key.to_string(), command.get(*key)?.clone())))
            .collect();
        if !choices.is_empty() {
            self.report(Value::Object(choices));
        }
        let mut listeners = self.listeners.lock().unwrap();
        listeners.retain(|out| !out.is_closed());
        if listeners.is_empty() {
            let mut queued = self.queued.lock().unwrap();
            if queued.len() < MAX_QUEUED {
                queued.push(command);
            }
            return;
        }
        for out in listeners.iter() {
            let _ = out.send(FromHost::Command {
                command: command.clone(),
            });
        }
    }

    /// Gives the user's answer to the hook waiting for it, and notes it in
    /// the conversation. A null answer means the hook stopped waiting.
    fn answer(&self, id: &str, answer: Value) {
        let Some(waiting) = self.asks.lock().unwrap().remove(id) else {
            return;
        };
        if !answer.is_null() {
            let _ = waiting.send(answer.clone());
        }
        self.add_chat(json!({"kind": "answered", "id": id, "answer": answer, "at_ms": now_ms()}));
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
