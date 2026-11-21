//! Terminals that outlive the daemon: shells, and the agent CLIs (Claude
//! Code, Codex) running as their unmodified selves.
//!
//! Each terminal has a folder under `<state>/terminals/<id>/`:
//!
//! ```text
//! meta.json    what it runs, where, for which project (written by the daemon)
//! host.json    the terminal host's pid and the program's pid
//! sock         the host's socket, while the program runs
//! out.<n>      the output, in 1 MiB segments (see `log`)
//! agent.json   what the agent last reported, and the conversation's choices
//! exit.json    how the program ended
//! render.json  successful Quarto output paths (written by the host)
//! host.log     the host's own errors
//! ```
//!
//! The daemon starts a [`host`] process per terminal and talks to it over
//! `sock` with one JSON message per line ([`ToHost`], [`FromHost`]). Once
//! the program has ended, the daemon reads the folder instead.

pub mod acp;
pub mod antigravity;
pub mod chat;
pub mod choices;
pub mod codex;
pub mod events;
pub mod host;
pub(crate) mod idle;
pub mod log;
pub mod pi;
pub mod pty;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use sandhill_proto::{
    AgentOptions, ChatEntry, JobSpec, ServerMessage, TerminalEvent, TerminalExit, TerminalInfo,
    TerminalSubscribed,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc};

use crate::journal::now_ms;

/// Output replayed to a client that asks for recent output.
pub const RECENT: u64 = 1 << 20;
const DEFAULT_SIZE: (u16, u16) = (100, 30);
const HOST_START_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// From a client (the daemon, or an agent's hook) to a terminal host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ToHost {
    /// Send the output from byte `from` on (recent output without it), then
    /// follow along. With `chat_from`, also the conversation from that
    /// entry on.
    Subscribe {
        from: Option<u64>,
        #[serde(default)]
        chat_from: Option<u64>,
    },
    Input {
        data: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// End the program, as closing a terminal window does.
    Hangup,
    /// Management stop: no escalation unless force was explicitly confirmed.
    Stop {
        force: bool,
    },
    /// Check inactivity in the host before ending a resumable agent.
    StopIfIdle {
        limit_ms: u64,
    },
    /// What an agent reports about itself; merged into its report.
    Agent {
        value: Value,
    },
    /// An entry for the agent's conversation, from one of its hooks.
    Chat {
        event: Value,
    },
    /// An `ask` entry from a hook that then waits on this connection for
    /// the user's answer ([`FromHost::Answer`]).
    Ask {
        id: String,
        event: Value,
    },
    /// The user's answer to an ask, from the daemon.
    Answer {
        id: String,
        answer: Value,
    },
    /// From the program that drives an agent's chat (Codex's bridge): send
    /// the user's commands ([`FromHost::Command`]) on this connection.
    Listen,
    /// What the user did in the chat, from the daemon, for the listening
    /// program. Kept until one listens.
    Command {
        command: Value,
    },
}

/// From a terminal host to a subscribed client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum FromHost {
    /// The first message of a subscription: where its output starts.
    Start {
        from: u64,
    },
    /// Output at `offset`, base64.
    Output {
        offset: u64,
        data: String,
    },
    Agent {
        value: Value,
    },
    Chat {
        entry: ChatEntry,
    },
    /// To the hook that asked: the user's answer.
    Answer {
        answer: Value,
    },
    /// To a listening program: what the user did in the chat.
    Command {
        command: Value,
    },
    Exit {
        exit: TerminalExit,
    },
}

/// Longest socket path that works on both Linux (108) and macOS (104).
const MAX_SOCKET_PATH: usize = 100;

/// Where sockets go when the terminal's folder is too deep: private to the user.
pub(crate) fn short_socket_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/sandhill-{uid}"))
}

/// The files of one terminal.
#[derive(Debug, Clone)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Paths { dir: dir.into() }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// The host's socket, in its folder. Socket paths are limited to about
    /// 100 bytes, so a deep state folder gets one under /tmp instead, named
    /// by the terminal's id.
    pub fn socket(&self) -> PathBuf {
        let path = self.dir.join("sock");
        if path.as_os_str().len() < MAX_SOCKET_PATH {
            return path;
        }
        let id = self.dir.file_name().unwrap_or_default();
        short_socket_dir().join(id)
    }
    fn meta(&self) -> PathBuf {
        self.dir.join("meta.json")
    }
    pub fn host(&self) -> PathBuf {
        self.dir.join("host.json")
    }
    pub fn exit(&self) -> PathBuf {
        self.dir.join("exit.json")
    }
    fn render_outputs(&self) -> PathBuf {
        self.dir.join("render.json")
    }
    pub fn agent(&self) -> PathBuf {
        self.dir.join("agent.json")
    }
    fn host_log(&self) -> PathBuf {
        self.dir.join("host.log")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Meta {
    id: String,
    kind: String,
    title: String,
    cwd: String,
    #[serde(default)]
    project: Option<String>,
    created_ms: i64,
    /// When the program last started, if it was restarted.
    #[serde(default)]
    started_ms: i64,
    #[serde(default)]
    options: AgentOptions,
    /// The program and its arguments, for the record.
    #[serde(default)]
    command: Vec<String>,
    /// The daemon build that started it. Agents get that build's tools.
    #[serde(default)]
    build: String,
    /// The name the user gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// For a job, what it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    job: Option<JobSpec>,
}

/// What to run in a new terminal.
#[derive(Debug, Clone)]
pub struct Launch {
    pub kind: String,
    pub title: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Added to the daemon's environment.
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub project: Option<String>,
    pub options: AgentOptions,
    pub size: Option<(u16, u16)>,
    /// For a job, what it runs.
    pub job: Option<JobSpec>,
}

/// A conversation from another machine, to make an ended terminal of (see
/// [`Terminals::adopt`]).
#[derive(Debug, Clone)]
pub struct Adopted {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub name: Option<String>,
    /// The project's folder on this machine, where the agent works.
    pub project: String,
    pub options: AgentOptions,
    pub created_ms: i64,
    pub ended_ms: i64,
    /// What finds the agent's conversation again ([`crate::agents::RESUME_KEYS`]).
    pub report: serde_json::Map<String, Value>,
}

/// Every terminal under `<state>/terminals`.
pub struct Terminals {
    root: PathBuf,
    /// `sandhilld` itself, which runs the terminal hosts.
    executable: PathBuf,
    /// One connection per terminal for input and resizing.
    inputs: Mutex<HashMap<String, OwnedWriteHalf>>,
}

impl Terminals {
    pub fn new(root: PathBuf, executable: PathBuf) -> Self {
        Terminals {
            root,
            executable,
            inputs: Mutex::new(HashMap::new()),
        }
    }

    fn paths(&self, id: &str) -> Result<Paths> {
        if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("no terminal {id}");
        }
        let paths = Paths::new(self.root.join(id));
        if !paths.meta().is_file() {
            bail!("no terminal {id}");
        }
        Ok(paths)
    }

    /// Starts `launch` in a new terminal.
    pub async fn start(&self, launch: Launch) -> Result<TerminalInfo> {
        self.start_with(launch, self.own_folder()).await
    }

    /// An explicit install may target a different environment, and AI
    /// agent installers shouldn't change anyone's computing environment.
    pub(crate) async fn start_unmanaged(&self, launch: Launch) -> Result<TerminalInfo> {
        self.start_with(launch, None).await
    }

    fn own_folder(&self) -> Option<PathBuf> {
        crate::own_env::chosen(&self.root.parent()?.join(crate::quick::record::FILE))
    }

    /// Bridge children may start again without a new terminal host, so
    /// they reapply the current mode before launching their agent.
    pub(super) fn scope_child(command: &mut tokio::process::Command, terminal_dir: &Path) {
        if let Some(state) = terminal_dir.parent().and_then(Path::parent)
            && let Some(folder) = crate::own_env::chosen(&state.join(crate::quick::record::FILE))
        {
            crate::own_env::apply_to_shell(command.as_std_mut(), &folder);
        }
    }

    async fn start_with(&self, launch: Launch, own: Option<PathBuf>) -> Result<TerminalInfo> {
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let paths = Paths::new(self.root.join(&id));
        std::fs::create_dir_all(paths.dir())?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(paths.dir(), std::fs::Permissions::from_mode(0o700))?;
        }
        let now = now_ms();
        let meta = Meta {
            id: id.clone(),
            kind: launch.kind.clone(),
            title: launch.title.clone(),
            cwd: launch.cwd.to_string_lossy().into_owned(),
            project: launch.project.clone(),
            created_ms: now,
            started_ms: now,
            options: launch.options.clone(),
            command: command_of(&launch),
            build: crate::build_id().to_string(),
            name: None,
            job: launch.job.clone(),
        };
        std::fs::write(paths.meta(), serde_json::to_vec_pretty(&meta)?)?;
        write_report(&paths, serde_json::Map::new(), &launch.options)?;
        if let Err(err) = self.spawn_host(&paths, &launch, own.as_deref()).await {
            // A host that failed leaves nothing behind; one still starting
            // keeps its folder.
            if !host_alive(&paths) {
                let _ = std::fs::remove_dir_all(paths.dir());
            }
            return Err(err);
        }
        self.info(&id)
    }

    /// Starts a terminal's program again with `launch`, in the same folder:
    /// its output goes on from where it was, and its conversation and what
    /// `keep` names of the agent's report stay. A program still running is
    /// ended first.
    pub async fn restart(&self, id: &str, launch: Launch, keep: &[&str]) -> Result<TerminalInfo> {
        let paths = self.paths(id)?;
        if self.info(id)?.running {
            self.send(id, ToHost::Hangup).await?;
            let ended = async {
                while host_alive(&paths) && !paths.exit().exists() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            };
            if tokio::time::timeout(CLOSE_TIMEOUT, ended).await.is_err() {
                bail!("{} did not end", launch.title);
            }
            // Its host lets go of the socket after writing how it ended.
            let gone = async {
                while paths.socket().exists() && host_alive(&paths) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, gone).await;
        }
        self.inputs.lock().await.remove(id);
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(paths.meta())?)?;
        let previous_account = meta.options.account.as_deref().unwrap_or(&meta.kind);
        let next_account = launch.options.account.as_deref().unwrap_or(&launch.kind);
        let switched = if crate::agents::is_agent(&launch.kind) && previous_account != next_account
        {
            Some(crate::accounts::Registry::from_env().account(next_account)?)
        } else {
            None
        };
        meta.started_ms = now_ms();
        meta.options = launch.options.clone();
        meta.command = command_of(&launch);
        meta.build = crate::build_id().to_string();
        meta.job = launch.job.clone();
        std::fs::write(paths.meta(), serde_json::to_vec_pretty(&meta)?)?;
        // What the old program reported no longer holds, except what
        // finds its conversation again.
        let kept: serde_json::Map<String, Value> = read_json::<Value>(&paths.agent())
            .and_then(|agent| agent.as_object().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|(key, _)| keep.contains(&key.as_str()))
            .collect();
        write_report(&paths, kept, &launch.options)?;
        for file in [paths.exit(), paths.host(), paths.render_outputs()] {
            let _ = std::fs::remove_file(file);
        }
        if let Some(account) = switched {
            chat::ChatLog::open(paths.dir())?.append(serde_json::json!({
                "kind": "account", "account": account.id, "name": account.name, "at_ms": now_ms()
            }))?;
        }
        self.spawn_host(&paths, &launch, self.own_folder().as_deref())
            .await?;
        self.info(id)
    }

    /// Names a terminal (a conversation, in the app's history), or takes
    /// its name away with `None` or a blank name.
    pub fn rename(&self, id: &str, name: Option<&str>) -> Result<TerminalInfo> {
        let paths = self.paths(id)?;
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(paths.meta())?)?;
        meta.name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string);
        crate::files::write_atomic(&paths.meta(), &serde_json::to_vec_pretty(&meta)?)?;
        self.info(id)
    }

    /// A terminal's folder.
    pub fn dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.paths(id)?.dir)
    }

    /// Makes an ended terminal for a conversation that ran on another
    /// machine, with its chat, so it shows in the history and can go on.
    pub fn adopt(&self, adopted: Adopted, chat: &Path) -> Result<()> {
        let id = &adopted.id;
        if id.is_empty() || id.len() > 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{id} is no terminal id");
        }
        let paths = Paths::new(self.root.join(id));
        if paths.meta().exists() {
            bail!("terminal {id} exists");
        }
        std::fs::create_dir_all(paths.dir())?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(paths.dir(), std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::copy(chat, chat::path(paths.dir()))?;
        write_report(&paths, adopted.report, &adopted.options)?;
        let exit = TerminalExit {
            code: None,
            signal: None,
            ended_ms: adopted.ended_ms,
            idle: false,
        };
        std::fs::write(paths.exit(), serde_json::to_vec(&exit)?)?;
        // Last, as it makes the terminal.
        let meta = Meta {
            id: id.clone(),
            kind: adopted.kind,
            title: adopted.title,
            cwd: adopted.project.clone(),
            project: Some(adopted.project),
            created_ms: adopted.created_ms,
            started_ms: adopted.created_ms,
            options: adopted.options,
            command: Vec::new(),
            build: crate::build_id().to_string(),
            name: adopted.name,
            job: None,
        };
        crate::files::write_atomic(&paths.meta(), &serde_json::to_vec_pretty(&meta)?)
    }

    /// Moves a terminal to a project's folder, after the folder moved.
    pub fn move_to(&self, id: &str, project: &str) -> Result<()> {
        let paths = self.paths(id)?;
        let mut meta: Meta = serde_json::from_slice(&std::fs::read(paths.meta())?)?;
        meta.cwd = project.to_string();
        meta.project = Some(project.to_string());
        crate::files::write_atomic(&paths.meta(), &serde_json::to_vec_pretty(&meta)?)
    }

    /// Sets one thing an agent reported, as which conversation it goes on with.
    pub fn set_report(&self, id: &str, key: &str, value: Value) -> Result<()> {
        let paths = self.paths(id)?;
        let mut report = read_json::<Value>(&paths.agent())
            .and_then(|r| r.as_object().cloned())
            .unwrap_or_default();
        report.insert(key.to_string(), value);
        crate::files::write_atomic(&paths.agent(), &serde_json::to_vec(&report)?)
    }

    /// Starts the terminal host for `launch` in its folder and waits until
    /// it serves.
    async fn spawn_host(&self, paths: &Paths, launch: &Launch, own: Option<&Path>) -> Result<()> {
        let (cols, rows) = launch.size.unwrap_or(DEFAULT_SIZE);
        let log = std::fs::File::create(paths.host_log())?;
        // Under the lock while Sandhill's own R and Python are chosen: the
        // shell, a job or an agent and everything they start.
        let lock = match (own, self.root.parent()) {
            (Some(_), Some(state)) => {
                crate::quick::own_lock::current_async(state.join(crate::quick::record::FILE)).await
            }
            _ => None,
        };
        let keep: Vec<&Path> = std::iter::once(launch.cwd.as_path())
            .chain(launch.project.as_deref().map(Path::new))
            .collect();
        let run = lock_run(launch);
        let locked = lock
            .as_deref()
            .and_then(|lock| lock.command(&self.executable, &keep, run));
        let side = lock
            .as_deref()
            .filter(|_| crate::agents::is_agent(&launch.kind))
            .and_then(|lock| lock.host_lock(run, locked.is_some()));
        let mut host = match locked {
            Some(locked) => tokio::process::Command::from(locked),
            None => tokio::process::Command::new(&self.executable),
        };
        match side {
            Some(side) => host.env(crate::quick::own_lock::HOST_LOCK, side),
            None => host.env_remove(crate::quick::own_lock::HOST_LOCK),
        };
        host.arg("terminal-host")
            .arg("--dir")
            .arg(paths.dir())
            .arg("--cwd")
            .arg(&launch.cwd)
            .arg("--cols")
            .arg(cols.to_string())
            .arg("--rows")
            .arg(rows.to_string())
            .arg("--")
            .arg(&launch.program);
        if let Some(folder) = own
            && launch
                .program
                .file_name()
                .is_some_and(|name| name == "bash")
        {
            // Bash skips --rcfile for a login shell. Let our rc file run
            // the login files first, then restore the environment.
            host.arg("--rcfile").arg(folder.join("shell/bashrc"));
            if launch.args.first().is_some_and(|arg| arg == "-l") {
                host.args(&launch.args[1..]).env("SANDHILL_BASH_LOGIN", "1");
            } else {
                host.args(&launch.args);
            }
        } else {
            host.args(&launch.args);
        }
        host.env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("SANDHILL_TERMINAL", paths.dir())
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            // Its own process group, like kernels, so it outlives the daemon.
            .process_group(0);
        if let Some(folder) = own {
            crate::own_env::apply_to_shell(host.as_std_mut(), folder);
        }
        let mut child = host.spawn().context("cannot start the terminal host")?;
        let started = async {
            loop {
                // The host listens first, then starts the program and says
                // which it is. A program that ended at once has started too,
                // though its host may have let go of the socket already.
                let gone = matches!(child.try_wait(), Ok(Some(_)));
                if paths.host().exists()
                    && (paths.exit().exists() || UnixStream::connect(paths.socket()).await.is_ok())
                {
                    return Ok(());
                }
                if gone {
                    let log = std::fs::read_to_string(paths.host_log()).unwrap_or_default();
                    bail!("{}", log.trim());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let result = tokio::time::timeout(HOST_START_TIMEOUT, started).await;
        // Reap the host when it exits; it doesn't depend on us otherwise.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(err.context(format!("cannot start {}", launch.title))),
            Err(_) => bail!("the terminal for {} did not start in time", launch.title),
        }
    }

    /// Every terminal, oldest first.
    pub fn list(&self) -> Vec<TerminalInfo> {
        let mut list: Vec<TerminalInfo> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| self.info(entry.file_name().to_str()?).ok())
            .collect();
        list.sort_by_key(|t| t.created_ms);
        list
    }

    pub fn info(&self, id: &str) -> Result<TerminalInfo> {
        let paths = self.paths(id)?;
        let meta: Meta = serde_json::from_slice(&std::fs::read(paths.meta())?)?;
        let exit = read_json::<TerminalExit>(&paths.exit());
        let running = exit.is_none() && host_alive(&paths);
        let outdated = outdated(&meta);
        let output_paths = exit
            .as_ref()
            .map(|exit| render_output_paths(&paths, exit))
            .unwrap_or_default();
        Ok(TerminalInfo {
            id: meta.id,
            kind: meta.kind,
            title: meta.title,
            cwd: meta.cwd,
            project: meta.project,
            created_ms: meta.created_ms,
            started_ms: match meta.started_ms {
                0 => meta.created_ms,
                ms => ms,
            },
            running,
            exit,
            end: log::bounds(paths.dir()).1,
            options: meta.options,
            agent: read_json::<Value>(&paths.agent()),
            outdated,
            name: meta.name,
            job: meta.job,
            output_paths,
        })
    }

    /// The process id of a terminal's program while it runs.
    pub fn pid(&self, id: &str) -> Option<u32> {
        let paths = self.paths(id).ok()?;
        if paths.exit().exists() {
            return None;
        }
        let host = read_json::<Value>(&paths.host())?;
        u32::try_from(host.get("pid")?.as_u64()?).ok()
    }

    /// The program a terminal runs and its arguments.
    pub fn command(&self, id: &str) -> Result<Vec<String>> {
        let paths = self.paths(id)?;
        let meta: Meta = serde_json::from_slice(&std::fs::read(paths.meta())?)?;
        Ok(meta.command)
    }

    /// The last `bytes` of a terminal's output, as text.
    pub fn output_tail(&self, id: &str, bytes: u64) -> Result<String> {
        let paths = self.paths(id)?;
        let (start, end) = log::bounds(paths.dir());
        let (_, out) = log::read(paths.dir(), end.saturating_sub(bytes).max(start), end)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Sends input, a resize or a hangup to a running terminal.
    pub async fn send(&self, id: &str, message: ToHost) -> Result<()> {
        let paths = self.paths(id)?;
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        let mut inputs = self.inputs.lock().await;
        for attempt in 0..2 {
            if !inputs.contains_key(id) {
                let stream = UnixStream::connect(paths.socket())
                    .await
                    .map_err(|_| anyhow!("terminal {id} has ended"))?;
                inputs.insert(id.to_string(), stream.into_split().1);
            }
            let writer = inputs.get_mut(id).expect("inserted above");
            match writer.write_all(&line).await {
                Ok(()) => return Ok(()),
                // The host went away (or was replaced): reconnect once.
                Err(err) if attempt == 0 => {
                    tracing::debug!("terminal {id}: {err}; reconnecting");
                    inputs.remove(id);
                }
                Err(err) => return Err(err.into()),
            }
        }
        unreachable!()
    }

    /// Answers an agent's `ask`: the hook waiting for it gets `answer`.
    pub async fn answer(&self, id: &str, ask: &str, answer: Value) -> Result<()> {
        self.send(
            id,
            ToHost::Answer {
                id: ask.to_string(),
                answer,
            },
        )
        .await
    }

    /// Opens a terminal's output from byte `from` (recent output without
    /// it), and its conversation from entry `chat_from` if asked. Forward it
    /// with [`TerminalStream::forward`].
    pub async fn open(
        &self,
        id: &str,
        from: Option<u64>,
        chat_from: Option<u64>,
    ) -> Result<TerminalStream> {
        let paths = self.paths(id)?;
        let deadline = tokio::time::Instant::now() + HOST_START_TIMEOUT;
        loop {
            if let Ok(stream) = UnixStream::connect(paths.socket()).await {
                let (reader, mut writer) = stream.into_split();
                // Existing hosts clamp the requested offset to their atomic
                // subscription snapshot and return it in Start. Attach at
                // that end, then read history ourselves: even an old host
                // needs no restart or new protocol to distinguish replay.
                let mut line = serde_json::to_vec(&ToHost::Subscribe {
                    from: Some(u64::MAX),
                    chat_from,
                })?;
                line.push(b'\n');
                writer.write_all(&line).await?;
                let mut lines = BufReader::new(reader).lines();
                if let Some(first) = lines.next_line().await?
                    && let Ok(FromHost::Start { from: end }) = serde_json::from_str(&first)
                {
                    let recent = from.is_none();
                    let from = from.unwrap_or(end.saturating_sub(RECENT)).min(end);
                    let (start, bytes) = log::read(paths.dir(), from, end)?;
                    let (start, bytes) = if recent && start > 0 {
                        host::after_line_break(start, bytes)
                    } else {
                        (start, bytes)
                    };
                    return Ok(TerminalStream {
                        id: id.to_string(),
                        from: start,
                        source: Source::Live {
                            replay: bytes,
                            lines,
                            _writer: writer,
                            paths,
                        },
                    });
                }
                // The program ended as we connected; read the files instead.
                break;
            }
            // Listed but still starting, as when a window lists the
            // terminals while one starts: follow it once its host is up.
            let starting =
                paths.meta().exists() && !paths.host().exists() && !paths.exit().exists();
            if !starting || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let end = log::bounds(paths.dir()).1;
        let from = from.unwrap_or(end.saturating_sub(RECENT)).min(end);
        let (start, bytes) = log::read(paths.dir(), from, end)?;
        Ok(TerminalStream {
            id: id.to_string(),
            from: start,
            source: Source::Ended {
                bytes,
                chat: chat_from.map_or_else(Vec::new, |from| chat::read(paths.dir(), from)),
                agent: read_json(&paths.agent()),
                exit: read_json(&paths.exit()).unwrap_or(TerminalExit {
                    code: None,
                    signal: None,
                    ended_ms: now_ms(),
                    idle: false,
                }),
                output_paths: read_json::<TerminalExit>(&paths.exit())
                    .map(|exit| render_output_paths(&paths, &exit))
                    .unwrap_or_default(),
            },
        })
    }

    /// Ends the program if it runs, keeping the terminal and its output.
    pub async fn stop(&self, id: &str) -> Result<()> {
        if self.info(id)?.running {
            self.send(id, ToHost::Hangup).await?;
        }
        Ok(())
    }

    /// Requests an atomic activity/state check in each running agent's host.
    /// Hosts retain activity across daemon restarts, without polling log files.
    pub(crate) async fn stop_idle(&self, limit: Duration) {
        for terminal in self.list() {
            if terminal.running && idle::is_agent(&terminal.kind) {
                let result = self
                    .send(
                        &terminal.id,
                        ToHost::StopIfIdle {
                            limit_ms: limit.as_millis().try_into().unwrap_or(u64::MAX),
                        },
                    )
                    .await;
                if let Err(error) = result {
                    tracing::debug!("idle check for {}: {error:#}", terminal.id);
                }
            }
        }
    }

    /// Removes an ended terminal.
    pub fn remove(&self, id: &str) -> Result<()> {
        let paths = self.paths(id)?;
        if self.info(id)?.running {
            bail!("terminal {id} is running");
        }
        if let Ok(mut inputs) = self.inputs.try_lock() {
            inputs.remove(id);
        }
        std::fs::remove_dir_all(paths.dir())?;
        Ok(())
    }

    /// Ends the program if it runs, waits for it, and removes the terminal.
    pub async fn close(&self, id: &str) -> Result<()> {
        let paths = self.paths(id)?;
        if self.info(id)?.running {
            self.send(id, ToHost::Hangup).await?;
            let ended = async {
                while paths.socket().exists() && host_alive(&paths) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            };
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, ended).await;
        }
        self.inputs.lock().await.remove(id);
        std::fs::remove_dir_all(paths.dir())?;
        Ok(())
    }
}

/// A terminal's output on its way to a client.
pub struct TerminalStream {
    id: String,
    /// Where the output starts.
    pub from: u64,
    source: Source,
}

enum Source {
    Live {
        lines: Lines<BufReader<OwnedReadHalf>>,
        replay: Vec<u8>,
        /// Kept so the host keeps the subscription open.
        _writer: OwnedWriteHalf,
        paths: Paths,
    },
    Ended {
        bytes: Vec<u8>,
        chat: Vec<ChatEntry>,
        agent: Option<Value>,
        exit: TerminalExit,
        output_paths: Vec<String>,
    },
}

impl TerminalStream {
    pub fn subscribed(&self) -> TerminalSubscribed {
        TerminalSubscribed {
            terminal: self.id.clone(),
            from: self.from,
        }
    }

    /// Sends the output to a client until the program ends or the client
    /// goes away.
    pub async fn forward(self, out: mpsc::UnboundedSender<ServerMessage>) {
        let id = self.id;
        let send = |event: TerminalEvent| {
            out.send(ServerMessage::Terminal {
                terminal: TerminalEvent {
                    terminal: id.clone(),
                    ..event
                },
            })
            .is_ok()
        };
        match self.source {
            Source::Ended {
                bytes,
                chat,
                agent,
                exit,
                output_paths,
            } => {
                let engine = base64::engine::general_purpose::STANDARD;
                for (i, piece) in bytes.chunks(64 * 1024).enumerate() {
                    if !send(TerminalEvent {
                        offset: Some(self.from + (i * 64 * 1024) as u64),
                        data: Some(engine.encode(piece)),
                        replay: true,
                        ..Default::default()
                    }) {
                        return;
                    }
                }
                for entry in chat {
                    if !send(TerminalEvent {
                        chat: Some(entry),
                        ..Default::default()
                    }) {
                        return;
                    }
                }
                if agent.is_some() {
                    send(TerminalEvent {
                        agent,
                        ..Default::default()
                    });
                }
                send(TerminalEvent {
                    exit: Some(exit),
                    output_paths,
                    ..Default::default()
                });
            }
            Source::Live {
                mut lines,
                paths,
                replay,
                ..
            } => {
                let engine = base64::engine::general_purpose::STANDARD;
                for (i, piece) in replay.chunks(64 * 1024).enumerate() {
                    if !send(TerminalEvent {
                        offset: Some(self.from + (i * 64 * 1024) as u64),
                        data: Some(engine.encode(piece)),
                        replay: true,
                        ..Default::default()
                    }) {
                        return;
                    }
                }
                while let Ok(Some(line)) = lines.next_line().await {
                    let event = match serde_json::from_str::<FromHost>(&line) {
                        Ok(FromHost::Output { offset, data }) => TerminalEvent {
                            offset: Some(offset),
                            data: Some(data),
                            ..Default::default()
                        },
                        Ok(FromHost::Agent { value }) => TerminalEvent {
                            agent: Some(value),
                            ..Default::default()
                        },
                        Ok(FromHost::Chat { entry }) => TerminalEvent {
                            chat: Some(entry),
                            ..Default::default()
                        },
                        Ok(FromHost::Exit { exit }) => {
                            send(TerminalEvent {
                                output_paths: render_output_paths(&paths, &exit),
                                exit: Some(exit),
                                ..Default::default()
                            });
                            return;
                        }
                        Ok(
                            FromHost::Start { .. }
                            | FromHost::Answer { .. }
                            | FromHost::Command { .. },
                        )
                        | Err(_) => continue,
                    };
                    if !send(event) {
                        return;
                    }
                }
                // The host went away without saying how the program ended
                // (it was killed): report what is known.
                let exit = read_json(&paths.exit()).unwrap_or(TerminalExit {
                    code: None,
                    signal: None,
                    ended_ms: now_ms(),
                    idle: false,
                });
                send(TerminalEvent {
                    output_paths: render_output_paths(&paths, &exit),
                    exit: Some(exit),
                    ..Default::default()
                });
            }
        }
    }
}

/// Reports to the terminal an agent runs in, from one of its hooks
/// (`sandhilld terminal-event`). `SANDHILL_TERMINAL` names the terminal.
pub async fn report(dir: &Path, value: Value) -> Result<()> {
    tell(dir, &ToHost::Agent { value }).await
}

/// Adds an entry to the conversation of the terminal an agent runs in.
pub async fn add_chat(dir: &Path, event: Value) -> Result<()> {
    tell(dir, &ToHost::Chat { event }).await
}

/// Adds an `ask` to the conversation and waits for the user's answer.
/// `None` if the terminal went away first.
pub async fn ask(dir: &Path, id: &str, event: Value) -> Result<Option<Value>> {
    let paths = Paths::new(dir);
    let stream = UnixStream::connect(paths.socket())
        .await
        .with_context(|| format!("no terminal at {}", dir.display()))?;
    let (reader, mut writer) = stream.into_split();
    let mut line = serde_json::to_vec(&ToHost::Ask {
        id: id.to_string(),
        event,
    })?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    // The write side stays open: closing it would read as giving up.
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if let Ok(FromHost::Answer { answer }) = serde_json::from_str(&line) {
            return Ok(Some(answer));
        }
    }
    Ok(None)
}

async fn tell(dir: &Path, message: &ToHost) -> Result<()> {
    let paths = Paths::new(dir);
    let mut stream = UnixStream::connect(paths.socket())
        .await
        .with_context(|| format!("no terminal at {}", dir.display()))?;
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stream.write_all(&line).await?;
    stream.shutdown().await?;
    Ok(())
}

/// The report a program starts with: `kept` from the last one and the
/// conversation's choices. None at all when that is empty.
fn write_report(
    paths: &Paths,
    mut kept: serde_json::Map<String, Value>,
    options: &AgentOptions,
) -> Result<()> {
    if let Some(code) = code_to(options.code.as_deref()) {
        kept.insert("code".into(), code.into());
    }
    if let Some(pin) = &options.pin {
        kept.insert("pin".into(), pin.clone().into());
    }
    if !options.refs.is_empty() {
        kept.insert("refs".into(), options.refs.clone().into());
    }
    if kept.is_empty() {
        let _ = std::fs::remove_file(paths.agent());
    } else {
        std::fs::write(paths.agent(), serde_json::to_vec(&kept)?)?;
    }
    Ok(())
}

/// The program and its arguments, for the record.
fn command_of(launch: &Launch) -> Vec<String> {
    let mut command = vec![launch.program.to_string_lossy().into_owned()];
    command.extend(launch.args.iter().cloned());
    command
}

/// On a Mac an agent's own sandbox can't run inside the lock, so an agent
/// Sandhill starts runs under one or the other.
fn lock_run(launch: &Launch) -> crate::quick::own_lock::Run {
    use crate::quick::own_lock::{Run, own_sandbox};
    let config = claude_config(launch);
    let permission = launch.options.permission.as_deref();
    if own_sandbox(&launch.kind, permission, &launch.cwd, config.as_deref()) {
        Run::SandboxedAgent
    } else {
        Run::Host
    }
}

/// Claude Code's settings folder: an added account's, else the usual one.
fn claude_config(launch: &Launch) -> Option<PathBuf> {
    launch
        .env
        .iter()
        .find(|(key, _)| key == "CLAUDE_CONFIG_DIR")
        .map(|(_, folder)| PathBuf::from(folder))
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
}

fn host_alive(paths: &Paths) -> bool {
    let Some(host) = read_json::<Value>(&paths.host()) else {
        // Written right after the host starts; a terminal without it is
        // still starting.
        return paths.socket().exists();
    };
    host.get("host")
        .and_then(Value::as_u64)
        .is_some_and(|pid| crate::kernel::pid_alive(pid as u32))
}

/// Where agents' code goes, as the report keeps it: `console`, or `None`
/// for the document.
pub fn code_to(code: Option<&str>) -> Option<&'static str> {
    (code == Some(CODE_TO_CONSOLE)).then_some(CODE_TO_CONSOLE)
}

pub const CODE_TO_CONSOLE: &str = "console";

/// Whether the user has the agent in `dir` send its code to the Console.
pub fn code_to_console(dir: &Path) -> bool {
    read_json::<Value>(&Paths::new(dir).agent())
        .is_some_and(|report| report["code"] == CODE_TO_CONSOLE)
}

/// The agent in the terminal in `dir`, as the Console names it: "Claude",
/// "Codex", "Antigravity", "Posit Assistant".
pub fn agent_name(dir: &Path) -> Option<&'static str> {
    let meta: Meta = read_json(&Paths::new(dir).meta())?;
    match meta.kind.as_str() {
        crate::agents::CLAUDE => Some("Claude"),
        crate::agents::CODEX => Some("Codex"),
        crate::agents::ANTIGRAVITY => Some("Antigravity"),
        crate::agents::OPENSCIENCE => Some("OpenScience"),
        crate::agents::OMP => Some("Oh My Pi"),
        crate::agents::PI => Some("pi"),
        crate::agents::POSIT => Some("Posit Assistant"),
        _ => None,
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The host saves these before announcing success, independently of daemon
/// subscriptions and terminal log retention. Failed/interrupted runs expose none.
fn render_output_paths(paths: &Paths, exit: &TerminalExit) -> Vec<String> {
    if exit.code == Some(0) && exit.signal.is_none() {
        read_json(&paths.render_outputs()).unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Whether an agent was started by another build than this one. Agents
/// started before builds were recorded are older than any build that
/// records them.
fn outdated(meta: &Meta) -> bool {
    let ours = crate::build_id();
    crate::agents::is_agent(&meta.kind) && !ours.is_empty() && meta.build != ours
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_run_under_the_lock_unless_the_agent_has_its_own_sandbox() {
        use crate::quick::own_lock::{Run, test_lock};
        let launch = |kind: &str, permission: Option<&str>| Launch {
            kind: kind.into(),
            title: String::new(),
            program: "/bin/zsh".into(),
            args: Vec::new(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), "/account/claude".into())],
            cwd: "/work/project".into(),
            project: Some("/work".into()),
            options: AgentOptions {
                permission: permission.map(str::to_string),
                ..Default::default()
            },
            size: None,
            job: None,
        };
        assert_eq!(
            claude_config(&launch("claude", None)).as_deref(),
            Some(Path::new("/account/claude"))
        );
        let mac = test_lock(vec!["/opt/homebrew".into()], false);
        let linux = test_lock(vec!["/opt/homebrew".into()], true);
        for (kind, permission, own_sandbox) in [
            (sandhill_proto::TERMINAL_SHELL, None, false),
            (sandhill_proto::TERMINAL_JOB, None, false),
            (sandhill_proto::TERMINAL_PREVIEW, None, false),
            (crate::agents::CODEX, Some("edits"), true),
            (crate::agents::CODEX, Some("full"), false),
            (crate::agents::PI, Some("edits"), false),
        ] {
            let launch = launch(kind, permission);
            let run = lock_run(&launch);
            assert_eq!(
                run == Run::SandboxedAgent,
                own_sandbox,
                "{kind} {permission:?}"
            );
            let keep = [launch.cwd.as_path()];
            let host = mac.command("/sandhilld", &keep, run);
            assert_eq!(host.is_none(), own_sandbox, "{kind} {permission:?}");
            if let Some(host) = &host {
                assert_eq!(host.get_program(), "/usr/bin/sandbox-exec");
                assert_eq!(
                    host.get_args().last(),
                    Some(std::ffi::OsStr::new("/sandhilld"))
                );
            }
            assert!(linux.command("/sandhilld", &keep, run).is_some());
            let side = mac.host_lock(run, host.is_some());
            assert_eq!(side, Some(if own_sandbox { "agent" } else { "on" }));
            assert_eq!(linux.host_lock(run, true), None);
        }
    }

    #[test]
    fn conversation_child_uses_current_mode_and_keeps_the_original_shell_files() {
        let state = tempfile::tempdir().unwrap();
        let dir = state.path().join("terminals/conversation");
        let record = state.path().join(crate::quick::record::FILE);
        let folder = state.path().join("environment");
        let wrapper = folder.join("shell/zsh");
        let user_zdotdir = Path::new("/user/shell");

        let mut command = tokio::process::Command::new("agent");
        command.env("PYTHONPATH", "/outside/python");
        Terminals::scope_child(&mut command, &dir);
        assert_eq!(command.as_std().get_envs().count(), 1);

        crate::quick::install::update_prefs(&record, |prefs| {
            prefs.mode = Some(sandhill_proto::EnvMode::Own);
            prefs.own_folder = folder.to_str().unwrap().into();
        })
        .unwrap();
        command
            .env("PATH", "/node/bin:/usr/bin")
            .env("ZDOTDIR", &wrapper)
            .env("SANDHILL_USER_ZDOTDIR", user_zdotdir)
            .env("BASH_ENV", "/user/bash-startup");
        Terminals::scope_child(&mut command, &dir);
        // Reapplying at a bridge spawn doesn't lose original startup files.
        Terminals::scope_child(&mut command, &dir);
        let vars: HashMap<_, _> = command.as_std().get_envs().collect();
        let key = std::ffi::OsStr::new;
        assert_eq!(vars[key("PYTHONPATH")], None);
        assert_eq!(vars[key("ZDOTDIR")], Some(wrapper.as_os_str()));
        assert_eq!(
            vars[key("SANDHILL_USER_ZDOTDIR")],
            Some(user_zdotdir.as_os_str())
        );
        assert_eq!(
            vars[key("BASH_ENV")],
            Some(folder.join("shell/bashrc").as_os_str())
        );
        assert_eq!(
            vars[key("SANDHILL_USER_BASH_ENV")],
            Some(key("/user/bash-startup"))
        );
        let path = vars[key("PATH")].unwrap();
        let parts: Vec<_> = std::env::split_paths(path).collect();
        assert_eq!(parts[0], folder.join("conda/envs/sandhill-python/bin"));
        assert_eq!(parts[1], folder.join("conda/envs/sandhill-r/bin"));
        assert_eq!(parts[2], folder.join("bin"));
        assert_eq!(parts[3], Path::new("/node/bin"));
        assert_eq!(parts[4], Path::new("/usr/bin"));
        assert_eq!(parts.len(), 5);
    }

    #[tokio::test]
    async fn old_host_snapshot_separates_replay_from_immediately_queued_live_output() {
        use base64::Engine as _;

        let dir = tempfile::Builder::new()
            .prefix("sandhill-replay-")
            .tempdir_in("/tmp")
            .unwrap();
        let id = uuid::Uuid::new_v4().simple().to_string();
        let paths = Paths::new(dir.path().join(&id));
        std::fs::create_dir_all(paths.dir()).unwrap();
        std::fs::write(paths.meta(), b"{}").unwrap();
        let history = b"history\x1b[c\x1b[6n\x1b]10;?\x1b\\";
        let live = b"live\x1b[6n";
        let mut log = log::LogWriter::create(paths.dir()).unwrap();
        log.append(history).unwrap();
        let listener = tokio::net::UnixListener::bind(paths.socket()).unwrap();
        let engine = base64::engine::general_purpose::STANDARD;
        let exit = TerminalExit {
            code: Some(0),
            signal: None,
            ended_ms: 1,
            idle: false,
        };
        let host = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let request = BufReader::new(reader)
                .lines()
                .next_line()
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                serde_json::from_str::<ToHost>(&request).unwrap(),
                ToHost::Subscribe {
                    from: Some(u64::MAX),
                    ..
                }
            ));
            // New bytes already exist on disk when the daemon handles Start.
            // An old host's atomic snapshot, not a later log bounds query,
            // determines the replay boundary.
            log.append(live).unwrap();
            for message in [
                FromHost::Start {
                    from: history.len() as u64,
                },
                FromHost::Output {
                    offset: history.len() as u64,
                    data: base64::engine::general_purpose::STANDARD.encode(live),
                },
                FromHost::Exit { exit },
            ] {
                let mut line = serde_json::to_vec(&message).unwrap();
                line.push(b'\n');
                writer.write_all(&line).await.unwrap();
            }
        });
        let terminals = Terminals::new(dir.path().into(), PathBuf::new());
        let stream = terminals.open(&id, Some(0), None).await.unwrap();
        assert_eq!(stream.subscribed().from, 0);
        let (out, mut events) = mpsc::unbounded_channel();
        stream.forward(out).await;
        let mut pieces = Vec::new();
        while let Some(ServerMessage::Terminal { terminal }) = events.recv().await {
            if let Some(data) = terminal.data {
                pieces.push((
                    terminal.offset,
                    terminal.replay,
                    engine.decode(data).unwrap(),
                ));
            }
        }
        assert_eq!(
            pieces,
            vec![
                (Some(0), true, history.to_vec()),
                (Some(history.len() as u64), false, live.to_vec()),
            ],
        );
        host.await.unwrap();
    }

    #[test]
    fn agents_from_another_build_are_outdated() {
        let meta = |kind: &str, build: &str| Meta {
            id: "t".into(),
            kind: kind.into(),
            title: String::new(),
            cwd: "/".into(),
            project: None,
            created_ms: 0,
            started_ms: 0,
            options: AgentOptions::default(),
            command: Vec::new(),
            build: build.into(),
            name: None,
            job: None,
        };
        let ours = crate::build_id();
        assert!(!ours.is_empty());
        assert!(!outdated(&meta("claude", ours)));
        assert!(outdated(&meta("claude", "0123456789abcdef")));
        // Started before builds were recorded.
        let old: Meta = serde_json::from_value(serde_json::json!({
            "id": "t", "kind": "codex", "title": "Codex", "cwd": "/", "created_ms": 0
        }))
        .unwrap();
        assert!(outdated(&old));
        // A shell or a job has no tools to miss.
        assert!(!outdated(&meta(sandhill_proto::TERMINAL_SHELL, "")));
        assert!(!outdated(&meta(sandhill_proto::TERMINAL_JOB, "")));
    }
}
