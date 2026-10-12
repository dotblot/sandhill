//! The terminal host: one small process per terminal (`sandhilld
//! terminal-host`) that owns the terminal and the program in it. It is not
//! a child of the daemon in any way that matters, so the program keeps
//! running when the daemon restarts, as kernels do; the new daemon finds it
//! by its socket. The host keeps the output on disk ([`super::log`]) and
//! serves it, with input, resizing and agent reports, on a Unix socket in
//! the terminal's folder.

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use sandhill_proto::TerminalExit;
use serde_json::{Value, json};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use super::log::{self, LogWriter};
use super::{FromHost, Paths, RECENT, ToHost, pty};
use crate::journal::now_ms;

/// Output is sent to clients in pieces of at most this many bytes.
const FRAME: usize = 64 * 1024;
/// After the program exits, output still arriving this soon is kept.
const DRAIN: Duration = Duration::from_millis(200);
/// How long a hung-up program gets before it is killed.
const HANGUP_GRACE: Duration = Duration::from_secs(3);

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
    Exit(TerminalExit),
}

struct Shared {
    paths: Paths,
    log: Mutex<LogWriter>,
    live: broadcast::Sender<Live>,
    agent: Mutex<Value>,
    exit: Mutex<Option<TerminalExit>>,
    input: mpsc::UnboundedSender<Vec<u8>>,
    master: Arc<AsyncFd<OwnedFd>>,
    pid: u32,
}

/// Runs the program until it exits, serving clients meanwhile.
pub async fn run(options: HostOptions) -> Result<()> {
    let paths = Paths::new(&options.dir);
    let (program, args) = options.program.split_first().context("no program to run")?;
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(&options.cwd);
    let pty::Pty { master, mut child } = pty::spawn(&mut command, options.cols, options.rows)
        .with_context(|| format!("cannot start {}", program.to_string_lossy()))?;
    let pid = child.id();
    write_json(
        &paths.host(),
        &json!({"host": std::process::id(), "pid": pid}),
    )?;
    pty::set_nonblocking(master.as_raw_fd())?;
    let master = Arc::new(AsyncFd::new(master)?);

    let (input, input_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        log: Mutex::new(LogWriter::create(&options.dir)?),
        live: broadcast::channel(1024).0,
        agent: Mutex::new(Value::Null),
        exit: Mutex::new(None),
        input,
        master: master.clone(),
        pid,
        paths: paths.clone(),
    });
    let _ = std::fs::remove_file(paths.socket());
    let listener = UnixListener::bind(paths.socket())
        .with_context(|| format!("cannot listen on {}", paths.socket().display()))?;
    tokio::spawn(write_input(master.clone(), input_rx));
    tokio::spawn(accept(shared.clone(), listener));

    let (exited_tx, mut exited) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = exited_tx.send(child.wait());
    });

    let mut buf = vec![0u8; 64 * 1024];
    let mut status = None;
    loop {
        tokio::select! {
            read = read_some(&master, &mut buf) => match read {
                Some(n) => shared.publish(&buf[..n]),
                // The terminal closed: everything holding it has exited.
                None => break,
            },
            result = &mut exited, if status.is_none() => {
                status = Some(result.ok().and_then(Result::ok));
                // Keep what is still on its way, then stop even if a
                // background process holds the terminal open.
                while let Ok(Some(n)) =
                    tokio::time::timeout(DRAIN, read_some(&master, &mut buf)).await
                {
                    shared.publish(&buf[..n]);
                }
                break;
            }
        }
    }
    let status = match status {
        Some(status) => status,
        None => exited.await.ok().and_then(Result::ok),
    };
    let exit = TerminalExit {
        code: status.and_then(|s| s.code()),
        signal: status.and_then(|s| std::os::unix::process::ExitStatusExt::signal(&s)),
        ended_ms: now_ms(),
    };
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
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<ToHost>(&line) else {
            continue;
        };
        match message {
            ToHost::Subscribe { from } => {
                if let Some(old) = subscription.take() {
                    old.abort();
                }
                subscription = Some(tokio::spawn(subscribe(shared.clone(), from, out.clone())));
            }
            ToHost::Input { data } => {
                let _ = shared.input.send(data.into_bytes());
            }
            ToHost::Resize { cols, rows } => shared.resize(cols, rows),
            ToHost::Hangup => shared.hang_up(),
            ToHost::Agent { value } => shared.report(value),
        }
    }
    if let Some(task) = subscription {
        task.abort();
    }
    drop(out);
    let _ = writer_task.await;
}

/// Sends the output from `from` on, then follows it live.
async fn subscribe(shared: Arc<Shared>, from: Option<u64>, out: mpsc::UnboundedSender<FromHost>) {
    // Taking the live feed and the end together means nothing falls
    // between what is read from disk and what arrives live.
    let (mut live, end) = {
        let log = shared.log.lock().unwrap();
        (shared.live.subscribe(), log.end())
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
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
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
fn after_line_break(start: u64, bytes: Vec<u8>) -> (u64, Vec<u8>) {
    match bytes.iter().take(4096).position(|&b| b == b'\n') {
        Some(i) => (start + i as u64 + 1, bytes[i + 1..].to_vec()),
        None => (start, bytes),
    }
}

impl Shared {
    fn publish(&self, bytes: &[u8]) {
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

    /// Closes the terminal as a closed window would: SIGHUP, then SIGKILL
    /// for a program that ignores it.
    fn hang_up(&self) {
        let fd = self.master.get_ref().as_raw_fd();
        let groups = [Some(self.pid as libc::pid_t), pty::foreground_group(fd)];
        for group in groups.into_iter().flatten() {
            // SAFETY: signalling process groups in the program's session.
            unsafe { libc::kill(-group, libc::SIGHUP) };
        }
        let pid = self.pid as libc::pid_t;
        tokio::spawn(async move {
            tokio::time::sleep(HANGUP_GRACE).await;
            for group in groups.into_iter().flatten() {
                // SAFETY: as above; harmless if the group is gone.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
            // SAFETY: as above.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        });
    }

    /// Merges what an agent reported (`{state}` from a hook, `{status}` from
    /// its status line) and tells the clients.
    fn report(&self, value: Value) {
        let merged = {
            let mut agent = self.agent.lock().unwrap();
            if !agent.is_object() {
                *agent = json!({});
            }
            if let (Some(agent), Some(update)) = (agent.as_object_mut(), value.as_object()) {
                for (key, value) in update {
                    agent.insert(key.clone(), value.clone());
                }
                agent.insert("ts_ms".into(), json!(now_ms()));
            }
            agent.clone()
        };
        let _ = write_json(&self.paths.agent(), &merged);
        let _ = self.live.send(Live::Agent(merged));
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
