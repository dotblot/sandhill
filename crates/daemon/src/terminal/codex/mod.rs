//! Codex in the chat: `sandhilld codex-chat`, which runs in a Codex
//! conversation's terminal in place of Codex's own interface. It starts
//! `codex app-server`, the protocol Codex offers editors (JSON-RPC over
//! stdio), and drives it:
//!
//! - the user's messages (and mode, model and effort) come from the app
//!   through the terminal host ([`super::ToHost::Command`]) and become
//!   Codex's turns, with what the user added with @ and #;
//! - what Codex does comes back as the conversation entries Claude Code's
//!   hooks make ([`super::chat`], translated in [`items`]), so the chat
//!   shows both agents the same way;
//! - what Codex asks (to run a command, edit files, use a Sandhill tool,
//!   a question) waits for the user's answer in the app, as a hook's
//!   permission request does.
//!
//! It prints a plain record of the conversation to its terminal, and Codex
//! prints its warnings there. Codex itself is unmodified and signs in on
//! its own; Sandhill never sees its login.

mod commands;
mod events;

pub mod items;
pub(crate) mod native;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use sandhill_proto::AgentOptions;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::oneshot;

use super::{FromHost, Paths, ToHost, ask};
use crate::journal::now_ms;

/// Replies are sent to the chat in pieces at most this far apart.
const FLUSH: Duration = Duration::from_millis(120);
/// A message waits this long at most for what a # cell last showed.
const REFS_TIMEOUT: Duration = Duration::from_millis(1500);
/// What a plan is carried out with, once the user approves it.
const CARRY_OUT: &str = "Carry out the plan.";

pub struct Options {
    /// The `codex` CLI.
    pub codex: PathBuf,
    /// More arguments for `codex app-server`, such as `-c` settings.
    pub args: Vec<String>,
    /// The daemon's socket, for what goes with a message.
    pub daemon: PathBuf,
    pub project: Option<String>,
    pub options: AgentOptions,
    /// The first message.
    pub prompt: Option<String>,
    /// The thread to go on with, when the conversation is started again.
    pub thread: Option<String>,
    /// What Codex is told about Sandhill.
    pub instructions: String,
}

/// Runs Codex's conversation until Codex exits; returns its exit code.
pub async fn run(mut options: Options) -> Result<i32> {
    let dir = std::env::var_os("SANDHILL_TERMINAL")
        .map(PathBuf::from)
        .context("codex-chat runs in a Sandhill terminal")?;
    let cwd = std::env::current_dir()?;
    let stream = UnixStream::connect(Paths::new(&dir).socket())
        .await
        .context("cannot reach the terminal")?;
    let (from_host, to_host) = stream.into_split();
    let host = Host(tokio::sync::Mutex::new(to_host));
    host.send(&ToHost::Listen).await?;

    let mut child = app_server(&options, &dir)
        .spawn()
        .with_context(|| format!("cannot start {}", options.codex.display()))?;
    let rpc = Rpc {
        stdin: tokio::sync::Mutex::new(Some(child.stdin.take().context("no stdin")?)),
        next: AtomicI64::new(1),
        pending: Mutex::new(HashMap::new()),
    };
    let bridge = Arc::new(Bridge {
        rpc,
        host,
        dir,
        cwd,
        daemon: options.daemon.clone(),
        project: options.project.clone(),
        state: Mutex::new(State {
            model: options.options.model.clone(),
            effort: options.options.effort.clone(),
            permission: options.options.permission.clone(),
            ..State::default()
        }),
        tasks: Mutex::new(Vec::new()),
    });
    let mut reader = tokio::spawn(
        bridge
            .clone()
            .read(child.stdout.take().context("no stdout")?),
    );

    if let Err(err) = bridge.start(&options, false).await {
        let message = format!("Codex couldn't start its conversation: {err:#}");
        println!("{message}");
        bridge.chat(json!({"kind": "error", "text": message})).await;
        let _ = child.start_kill();
    } else if let Some(prompt) = options.prompt.clone().filter(|p| !p.trim().is_empty()) {
        bridge.clone().say(prompt, true).await;
    }

    let mut commands = BufReader::new(from_host).lines();
    loop {
        tokio::select! {
            line = commands.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(FromHost::Command { command }) = serde_json::from_str(&line) {
                        if command["kind"] == "view" {
                            if command["mode"] != "terminal" {
                                continue;
                            }
                            bridge.state.lock().unwrap().paused = true;
                            bridge.report(json!({"view_mode": "switching_terminal"})).await;
                            if let Err(err) = bridge.release().await {
                                bridge.state.lock().unwrap().paused = false;
                                bridge.chat(json!({"kind": "error", "text": format!("Codex couldn't open Terminal: {err:#}")})).await;
                                bridge.report(json!({"view_mode": "chat"})).await;
                                bridge.next();
                                continue;
                            }
                            bridge.stop_tasks().await;
                            // EOF asks the stdio app-server to shut down. Never start
                            // another owner while this process is still alive.
                            bridge.rpc.stdin.lock().await.take();
                            if !matches!(tokio::time::timeout(Duration::from_secs(15), child.wait()).await, Ok(Ok(_))) {
                                bridge.chat(json!({"kind": "error", "text": "Codex app-server did not shut down; Terminal was not started."})).await;
                                bridge.report(json!({"view_mode": "chat"})).await;
                                let _ = child.kill().await;
                                return Ok(1);
                            }
                            let _ = reader.await;
                            let mut native_command = bridge.native_command(&options);
                            bridge.report(json!({"view_mode": "terminal"})).await;
                            let queued = native::run(&mut native_command, &bridge.dir, &mut commands).await;
                            bridge.report(json!({"view_mode": "switching_chat"})).await;
                            if let Err(err) = &queued {
                                bridge.chat(json!({"kind": "error", "text": format!("Codex Terminal failed: {err:#}")})).await;
                            }
                            options.thread = Some(bridge.state.lock().unwrap().thread.clone());
                            child = match app_server(&options, &bridge.dir).spawn() {
                                Ok(child) => child,
                                Err(err) => {
                                    bridge.chat(json!({"kind": "error", "text": format!("Codex couldn't return to Chat: {err:#}")})).await;
                                    bridge.report(json!({"view_mode": "chat"})).await;
                                    return Ok(1);
                                }
                            };
                            *bridge.rpc.stdin.lock().await = Some(child.stdin.take().context("no stdin")?);
                            reader = tokio::spawn(bridge.clone().read(child.stdout.take().context("no stdout")?));
                            if let Err(err) = bridge.start(&options, true).await {
                                bridge.chat(json!({"kind": "error", "text": format!("Codex couldn't resume Chat: {err:#}")})).await;
                                bridge.report(json!({"view_mode": "chat"})).await;
                                let _ = child.kill().await;
                                return Ok(1);
                            }
                            bridge.state.lock().unwrap().paused = false;
                            bridge.report(json!({"view_mode": "chat"})).await;
                            bridge.spawn(bridge.clone().look_around());
                            bridge.clone().drain_queued().await;
                            if let Ok(queued) = queued {
                                for command in queued {
                                    bridge.clone().command(command).await;
                                }
                            }
                            bridge.next();
                        } else {
                            bridge.clone().command(command).await;
                        }
                    }
                }
                // The terminal is going away.
                _ => {
                    let _ = child.start_kill();
                    return Ok(1);
                }
            },
            status = child.wait() => {
                let status = status?;
                println!("Codex exited ({status}).");
                return Ok(status.code().unwrap_or(1));
            }
        }
    }
}

/// Both interfaces use the same overrides, without app-server's subcommand:
/// Sandhill's MCP server, and trust in the folder when the user trusted it
/// in Sandhill, so Codex does not ask again. The trust is for this run only;
/// Codex's own configuration is not written.
fn configured_command(options: &Options, dir: &std::path::Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&options.codex);
    command.args(&options.args).arg("-c").arg(format!(
        "mcp_servers.sandhill.env.SANDHILL_TERMINAL={}",
        serde_json::to_string(&dir.to_string_lossy()).unwrap(),
    ));
    if let Ok(folder) = std::env::var(crate::projects::trust::TRUSTED_FOLDER) {
        // The bridge runs in the conversation's folder.
        let cwd = std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_else(|_| folder.clone());
        command.arg("-c").arg(trust_override(&folder, &cwd));
    }
    configure_account(&mut command, None);
    command
}

/// Codex's `projects` table naming `folder` and the run's own `cwd` trusted.
/// The whole table is one value, as a dotted `-c` key cannot hold a path.
pub fn trust_override(folder: &str, cwd: &str) -> String {
    let entry = |path: &str| {
        format!(
            "{}={{trust_level=\"trusted\"}}",
            serde_json::to_string(path).unwrap()
        )
    };
    if folder == cwd {
        format!("projects={{{}}}", entry(folder))
    } else {
        format!("projects={{{},{}}}", entry(folder), entry(cwd))
    }
}

fn app_server(options: &Options, dir: &std::path::Path) -> tokio::process::Command {
    let mut command = configured_command(options, dir);
    command
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Added accounts use their own file-backed login, never the shared keyring.
/// Live bridges inherit their account home; read-only queries select it here.
fn configure_account(command: &mut tokio::process::Command, home: Option<&Path>) {
    if let Some(home) = home {
        command.env("CODEX_HOME", home);
    }
    if home.is_some() || std::env::var("SANDHILL_CODEX_FILE_AUTH").as_deref() == Ok("1") {
        command.arg("-c").arg("cli_auth_credentials_store=\"file\"");
    }
}

/// How long asking a Codex with no conversation for its models may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// The models Codex offers, before any conversation: `codex app-server`
/// started only to ask (`model/list`), for a new conversation's model menu.
pub async fn list_models(codex: &std::path::Path) -> Result<Value> {
    read_only(codex, false, None).await
}

/// Queries only identity and subscription limits, never starting a thread or turn.
pub async fn read_account(codex: &std::path::Path) -> Result<Value> {
    read_account_in(codex, None).await
}

/// Queries an account in its own Codex home without changing the daemon's
/// environment. `None` uses the inherited home and account policy.
pub async fn read_account_in(codex: &Path, home: Option<&Path>) -> Result<Value> {
    read_only(codex, true, home).await
}

/// Uses a banked reset (`account/rateLimitResetCredit/consume`): `credit`, else
/// the one Codex picks. Returns Codex's outcome: `reset`, `nothingToReset`,
/// `noCredit` or `alreadyRedeemed`.
pub async fn use_reset_credit(
    codex: &Path,
    home: Option<&Path>,
    credit: Option<&str>,
) -> Result<String> {
    let mut command = tokio::process::Command::new(codex);
    configure_account(&mut command, home);
    command
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(codex)? {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot start {}", codex.display()))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
    // One attempt, one key: a retry by Codex itself cannot spend a second reset.
    let key = uuid::Uuid::new_v4().to_string();
    let ask = async {
        let mut send = async |message: Value| -> Result<()> {
            let mut line = serde_json::to_vec(&message)?;
            line.push(b'\n');
            stdin.write_all(&line).await?;
            Ok(stdin.flush().await?)
        };
        let client =
            json!({"name": "sandhill", "title": "Sandhill", "version": env!("CARGO_PKG_VERSION")});
        send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"clientInfo": client}})).await?;
        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = message.get("error") {
                anyhow::bail!(
                    "Codex said: {}",
                    error["message"].as_str().unwrap_or("an error")
                );
            }
            match message["id"].as_i64() {
                Some(1) => {
                    send(json!({"jsonrpc": "2.0", "method": "initialized"})).await?;
                    send(json!({"jsonrpc": "2.0", "id": 2,
                        "method": "account/rateLimitResetCredit/consume",
                        "params": {"creditId": credit, "idempotencyKey": key}}))
                    .await?;
                }
                Some(2) => {
                    return message["result"]["outcome"]
                        .as_str()
                        .map(str::to_string)
                        .context("Codex gave no outcome for the reset");
                }
                _ => {}
            }
        }
        anyhow::bail!("Codex stopped before using the reset")
    };
    let outcome = tokio::time::timeout(LIST_TIMEOUT, ask)
        .await
        .context("Codex took too long to use the reset");
    let _ = child.start_kill();
    let _ = child.wait().await;
    outcome?
}

async fn read_only(codex: &Path, account: bool, home: Option<&Path>) -> Result<Value> {
    let mut command = tokio::process::Command::new(codex);
    configure_account(&mut command, home);
    command
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(codex)? {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot start {}", codex.display()))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
    let ask = async {
        let mut send = async |message: Value| -> Result<()> {
            let mut line = serde_json::to_vec(&message)?;
            line.push(b'\n');
            stdin.write_all(&line).await?;
            Ok(stdin.flush().await?)
        };
        let clients =
            json!({"name": "sandhill", "title": "Sandhill", "version": env!("CARGO_PKG_VERSION")});
        send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"clientInfo": clients}})).await?;
        let mut snapshot = Value::Null;
        let mut asked = false;
        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(error) = message.get("error") {
                if account && message["id"] == 3 && !snapshot.is_null() {
                    return Ok(snapshot);
                }
                anyhow::bail!(
                    "Codex said: {}",
                    error["message"].as_str().unwrap_or("an error")
                );
            }
            match message["id"].as_i64() {
                Some(1) if !asked => {
                    send(json!({"jsonrpc": "2.0", "method": "initialized"})).await?;
                    let (method, params) = if account {
                        ("account/read", json!({"refreshToken": false}))
                    } else {
                        ("model/list", json!({}))
                    };
                    send(json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}))
                        .await?;
                    asked = true;
                }
                Some(2) if !account => return Ok(items::models(&message["result"])),
                Some(2) => {
                    let read = &message["result"];
                    snapshot = json!({"account": account_summary(&read["account"]),
                        "observed_at_ms": now_ms()});
                    if read["account"]["type"] != "chatgpt" {
                        return Ok(snapshot);
                    }
                    send(json!({"jsonrpc": "2.0", "id": 3,
                        "method": "account/rateLimits/read", "params": {}}))
                    .await?;
                }
                Some(3) => {
                    snapshot["status"] = json!({"rate_limits":
                        items::rate_limits(&Value::Null, &message["result"]["rateLimits"])});
                    if let Some(credits) = items::reset_credits(&message["result"]) {
                        snapshot["status"]["reset_credits"] = credits;
                    }
                    return Ok(snapshot);
                }
                _ => {}
            }
        }
        anyhow::bail!("Codex stopped before completing its read-only query")
    };
    let models = tokio::time::timeout(LIST_TIMEOUT, ask)
        .await
        .context("Codex took too long to complete its read-only query");
    let _ = child.start_kill();
    let _ = child.wait().await;
    models?
}

fn account_summary(account: &Value) -> Value {
    if account.is_null() {
        Value::Null
    } else {
        json!({"type": account["type"], "email": account["email"], "plan": account["planType"]})
    }
}

/// The connection to the terminal host: entries and reports go out on it.
struct Host(tokio::sync::Mutex<OwnedWriteHalf>);

impl Host {
    async fn send(&self, message: &ToHost) -> Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        self.0.lock().await.write_all(&line).await?;
        Ok(())
    }
}

/// JSON-RPC with `codex app-server`, one message per line.
struct Rpc {
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    next: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>,
}

impl Rpc {
    async fn write(&self, message: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin.as_mut().context("Codex app-server is stopped")?;
        stdin.write_all(&line).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        rx.await
            .map_err(|_| anyhow!("Codex stopped"))?
            .map_err(|message| anyhow!("{message}"))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    async fn reply(&self, id: Value, result: Value) {
        let _ = self
            .write(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }

    async fn reply_error(&self, id: Value, message: &str) {
        let error = json!({"code": -32601, "message": message});
        let _ = self
            .write(json!({"jsonrpc": "2.0", "id": id, "error": error}))
            .await;
    }

    fn resolve(&self, id: i64, result: Result<Value, String>) {
        if let Some(waiting) = self.pending.lock().unwrap().remove(&id) {
            let _ = waiting.send(result);
        }
    }
}

#[derive(Default)]
struct State {
    thread: String,
    /// Blocks every source of automatic/queued work during ownership handoff.
    paused: bool,
    view_mode: String,
    /// Durable turns already represented in Sandhill's chat.
    history: HashSet<String>,
    /// The turn Codex is working on.
    turn: Option<String>,
    /// The user's choices; `None` leaves Codex's own.
    model: Option<String>,
    effort: Option<String>,
    permission: Option<String>,
    /// The model Codex chose, which plan mode needs named.
    default_model: String,
    /// Plan mode was used: from then on every turn says its mode.
    planned: bool,
    /// Messages sent while a turn runs, for the next turn.
    queued: Vec<String>,
    /// Steps shown and not ended: id, tool, input.
    steps: Vec<(String, String, Value)>,
    /// Replies being written, by item id.
    replies: HashMap<String, Reply>,
    /// The plan Codex wrote in the turn (plan mode).
    plan: Option<String>,
    /// Turns the chat shows (with a message or a step), which end with `stop`.
    shown: Vec<String>,
    /// An error was shown for the running turn.
    errored: bool,
    /// Codex has started the turn: it can be stopped.
    started: bool,
    /// The user stopped the turn before Codex started it.
    stopping: bool,
    /// Asks waiting for the user.
    waiting: usize,
    /// Requests waiting for the user, by their JSON-RPC id: Codex can
    /// settle them itself (when the turn is stopped), and the ask goes.
    requests: HashMap<String, tokio::task::AbortHandle>,
    /// The status the app shows: model, effort, context, plan limits.
    status: serde_json::Map<String, Value>,
    models: Value,
    todos: u32,
}

#[derive(Default)]
struct Reply {
    turn: String,
    /// Pieces sent so far.
    index: u64,
    sent: String,
    pending: String,
}

struct Bridge {
    rpc: Rpc,
    host: Host,
    /// The terminal's folder.
    dir: PathBuf,
    cwd: PathBuf,
    daemon: PathBuf,
    project: Option<String>,
    state: Mutex<State>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Bridge {
    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        let state = self.state.lock().unwrap();
        if !state.paused {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.retain(|task| !task.is_finished());
            tasks.push(tokio::spawn(future));
        }
    }

    async fn stop_tasks(&self) {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in &tasks {
            task.abort();
        }
        let requests = std::mem::take(&mut self.state.lock().unwrap().requests);
        for (_, request) in requests {
            request.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.state.lock().unwrap().waiting = 0;
    }

    /// Interrupt before unsubscribing, then let the process close its durable
    /// history. Unsubscribe alone has a grace period and is not exclusive.
    async fn release(self: &Arc<Self>) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let (thread, turn, started) = {
                    let state = self.state.lock().unwrap();
                    (state.thread.clone(), state.turn.clone(), state.started)
                };
                match turn {
                    None => break,
                    Some(turn) if started => {
                        self.rpc
                            .call(
                                "turn/interrupt",
                                json!({"threadId": thread, "turnId": turn}),
                            )
                            .await?;
                        break;
                    }
                    _ => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
            while self.state.lock().unwrap().turn.is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let thread = self.state.lock().unwrap().thread.clone();
            self.rpc
                .call("thread/unsubscribe", json!({"threadId": thread}))
                .await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Codex did not release its active turn")?
    }

    fn native_command(&self, options: &Options) -> tokio::process::Command {
        let state = self.state.lock().unwrap();
        let mut command = configured_command(options, &self.dir);
        let mode = items::mode_settings(state.permission.as_deref());
        let sandbox = match mode["sandboxPolicy"]["type"].as_str() {
            Some("dangerFullAccess") => "danger-full-access",
            Some("workspaceWrite") => "workspace-write",
            _ => "read-only",
        };
        for (key, value) in [
            ("developer_instructions", json!(options.instructions)),
            ("approval_policy", mode["approvalPolicy"].clone()),
            ("approvals_reviewer", mode["approvalsReviewer"].clone()),
            ("sandbox_mode", json!(sandbox)),
        ] {
            command.arg("-c").arg(format!("{key}={value}"));
        }
        if let Some(effort) = &state.effort {
            command
                .arg("-c")
                .arg(format!("model_reasoning_effort={}", json!(effort)));
        }
        command
            .arg("resume")
            .arg(&state.thread)
            .arg("--cd")
            .arg(&self.cwd)
            .arg("--no-daemon");
        if let Some(model) = &state.model {
            command.arg("--model").arg(model);
        }
        command.current_dir(&self.cwd);
        command
    }

    /// Recent Codex versions can return a backwards history cursor instead
    /// of every turn inline. Older versions return all turns directly.
    async fn history(&self, resumed: &Value) -> Result<Value> {
        let mut cursor = resumed["turnsBackwardsCursor"].as_str().map(str::to_owned);
        let mut older = Vec::new();
        while let Some(current) = cursor {
            let page = self
                .rpc
                .call(
                    "thread/turns/list",
                    json!({
                        "threadId": resumed["thread"]["id"],
                        "cursor": current,
                        "sortDirection": "desc",
                        "itemsView": "full",
                        "limit": 100,
                    }),
                )
                .await?;
            older.extend(page["data"].as_array().into_iter().flatten().cloned());
            cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        older.reverse();
        older.extend(
            resumed["thread"]["turns"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
        Ok(json!(older))
    }

    /// Import only new durable turns, through Codex's history API, never its
    /// screen. Reuse live item translation so tools and replies have one shape.
    async fn import_history(self: &Arc<Self>, turns: &Value) {
        for turn in turns.as_array().into_iter().flatten() {
            let Some(id) = turn["id"].as_str() else {
                continue;
            };
            if !self.state.lock().unwrap().history.insert(id.to_string()) {
                continue;
            }
            self.state.lock().unwrap().errored = false;
            for item in turn["items"].as_array().into_iter().flatten() {
                if item["type"] == "userMessage" {
                    let text = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.show_turn(id);
                    self.chat(json!({"kind": "prompt", "turn": id, "text": text}))
                        .await;
                } else if item["type"] == "plan" {
                    let mut message = item.clone();
                    message["type"] = json!("agentMessage");
                    self.item_completed(id, &message).await;
                } else {
                    self.item_completed(id, item).await;
                }
            }
            self.turn_completed(turn).await;
        }
    }

    /// Starts Codex's conversation: says who Sandhill is, then starts a
    /// thread in the folder (or resumes the one it had), with Sandhill's
    /// instructions and the mode.
    async fn start(self: &Arc<Self>, options: &Options, importing: bool) -> Result<()> {
        self.rpc
            .call(
                "initialize",
                json!({
                    "clientInfo": {"name": "sandhill", "title": "Sandhill", "version": env!("CARGO_PKG_VERSION")},
                    // Plan mode is one of the experimental parts.
                    "capabilities": {"experimentalApi": true, "requestAttestation": false},
                }),
            )
            .await?;
        self.rpc.notify("initialized", Value::Null).await?;
        let (model, permission) = {
            let state = self.state.lock().unwrap();
            (state.model.clone(), state.permission.clone())
        };
        let mode = items::mode_settings(permission.as_deref());
        let sandbox = match mode["sandboxPolicy"]["type"].as_str() {
            Some("dangerFullAccess") => "danger-full-access",
            Some("workspaceWrite") => "workspace-write",
            _ => "read-only",
        };
        let mut params = json!({
            "cwd": self.cwd,
            "approvalPolicy": mode["approvalPolicy"],
            "approvalsReviewer": mode["approvalsReviewer"],
            "sandbox": sandbox,
            "developerInstructions": options.instructions,
        });
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        let started = match &options.thread {
            Some(thread) => {
                params["threadId"] = json!(thread);
                params["excludeTurns"] = json!(false);
                self.rpc.call("thread/resume", params).await?
            }
            None => {
                params["serviceName"] = json!("sandhill");
                self.rpc.call("thread/start", params).await?
            }
        };
        let thread = started["thread"]["id"]
            .as_str()
            .context("Codex started no thread")?
            .to_string();
        let effort = started["reasoningEffort"].as_str().map(str::to_string);
        {
            let mut state = self.state.lock().unwrap();
            state.thread = thread.clone();
            state.default_model = started["model"].as_str().unwrap_or("").to_string();
            state
                .status
                .insert("model_id".into(), started["model"].clone());
            state
                .status
                .insert("model".into(), started["model"].clone());
            if state.effort.is_none() {
                state.effort = effort;
            }
        }
        println!(
            "Codex {} · {} · {}",
            started["thread"]["cliVersion"].as_str().unwrap_or(""),
            started["model"].as_str().unwrap_or(""),
            self.cwd.display()
        );
        let mut report = json!({"thread": thread});
        // Codex's own copy of the conversation, which goes with the project (see `chats`).
        if let Some(path) = started["thread"]["path"].as_str() {
            report["transcript"] = json!(path);
        }
        self.report(report).await;
        // The conversation has started, as Claude Code's SessionStart hook says.
        let source = if options.thread.is_some() {
            "resume"
        } else {
            "startup"
        };
        self.chat(json!({"kind": "session", "source": source, "model": started["model"]}))
            .await;
        let history = self.history(&started).await?;
        if importing {
            self.import_history(&history).await;
        } else {
            let mut state = self.state.lock().unwrap();
            for turn in history.as_array().into_iter().flatten() {
                if let Some(id) = turn["id"].as_str() {
                    state.history.insert(id.to_string());
                }
            }
        }
        // What the app shows besides: the models, the account and its limits.
        self.spawn(self.clone().look_around());
        Ok(())
    }

    /// The models Codex offers, and whether it is signed in.
    async fn look_around(self: Arc<Self>) {
        if let Ok(list) = self.rpc.call("model/list", json!({})).await {
            let models = items::models(&list);
            let mut state = self.state.lock().unwrap();
            let id = state.status.get("model_id").cloned().unwrap_or_default();
            if let Some(model) = models
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["id"] == id)
            {
                state.status.insert("model".into(), model["label"].clone());
            }
            state.models = models;
        }
        self.account().await;
        self.report(json!({})).await;
    }

    /// Whether Codex is signed in, and its plan limits if so.
    async fn account(&self) {
        let Ok(read) = self
            .rpc
            .call("account/read", json!({"refreshToken": false}))
            .await
        else {
            return;
        };
        let account = &read["account"];
        let summary = account_summary(account);
        // A refreshed identity must not inherit another account's windows.
        self.state.lock().unwrap().status.remove("rate_limits");
        self.report(json!({"account": summary, "observed_at_ms": now_ms(),
            "auth_required": read["requiresOpenaiAuth"]}))
            .await;
        if account["type"] == "chatgpt"
            && let Ok(limits) = self.rpc.call("account/rateLimits/read", json!({})).await
        {
            self.limits(&limits["rateLimits"]);
            if let Some(credits) = items::reset_credits(&limits) {
                self.state
                    .lock()
                    .unwrap()
                    .status
                    .insert("reset_credits".into(), credits);
            }
        }
    }

    fn limits(&self, snapshot: &Value) {
        let mut state = self.state.lock().unwrap();
        let known = state.status.get("rate_limits").cloned().unwrap_or_default();
        let limits = items::rate_limits(&known, snapshot);
        state.status.insert("rate_limits".into(), limits);
    }
}

/// An id for an ask.
fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_account_query_scopes_home_and_forces_file_auth() {
        let mut command = tokio::process::Command::new("codex");
        command
            .arg("-c")
            .arg("cli_auth_credentials_store=\"keyring\"");
        let home = Path::new("/sandhill-test/accounts/codex/work");
        configure_account(&mut command, Some(home));
        let command = command.as_std();
        let environment = command.get_envs().collect::<Vec<_>>();
        assert_eq!(
            environment,
            vec![(std::ffi::OsStr::new("CODEX_HOME"), Some(home.as_os_str()))]
        );
        let args = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            &args[args.len() - 2..],
            [
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new("cli_auth_credentials_store=\"file\"")
            ]
        );
    }
}
