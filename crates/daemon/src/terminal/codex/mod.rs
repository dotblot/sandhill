//! Codex in the chat: `sandhilld codex-chat`, which runs in a Codex
//! conversation's terminal in place of Codex's own interface. It starts
//! `codex app-server`, the protocol Codex offers editors (JSON-RPC over
//! stdio), and drives it:
//!
//! - the user's messages (and mode, model and effort) come from the app
//!   through the terminal host ([`super::ToHost::Command`]) and become
//!   Codex's turns, with where the user is in their document;
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

pub mod items;

use std::collections::HashMap;
use std::path::PathBuf;
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

use super::{FromHost, Paths, ToHost, guard};
use crate::journal::now_ms;

/// Replies are sent to the chat in pieces at most this far apart.
const FLUSH: Duration = Duration::from_millis(120);
/// A message waits this long at most for where the user is.
const PLACE_TIMEOUT: Duration = Duration::from_millis(1500);
/// What a plan is carried out with, once the user approves it.
const CARRY_OUT: &str = "Carry out the plan.";

pub struct Options {
    /// The `codex` CLI.
    pub codex: PathBuf,
    /// More arguments for `codex app-server`, such as `-c` settings.
    pub args: Vec<String>,
    /// The daemon's socket, for where the user is.
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
pub async fn run(options: Options) -> Result<i32> {
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

    // Codex starts MCP servers with few of its variables: Sandhill's is
    // told which conversation it serves, for the document it is about.
    let terminal = serde_json::to_string(&dir.to_string_lossy())?;
    let mut child = tokio::process::Command::new(&options.codex)
        .arg("app-server")
        .args(&options.args)
        .arg("-c")
        .arg(format!(
            "mcp_servers.sandhill.env.SANDHILL_TERMINAL={terminal}"
        ))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot start {}", options.codex.display()))?;
    let rpc = Rpc {
        stdin: tokio::sync::Mutex::new(child.stdin.take().context("no stdin")?),
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
    });
    tokio::spawn(
        bridge
            .clone()
            .read(child.stdout.take().context("no stdout")?),
    );

    if let Err(err) = bridge.start(&options).await {
        let message = format!("Codex couldn't start its conversation: {err:#}");
        println!("{message}");
        bridge.chat(json!({"kind": "error", "text": message})).await;
        let _ = child.start_kill();
    } else if let Some(prompt) = options.prompt.filter(|p| !p.trim().is_empty()) {
        bridge.clone().say(prompt, true).await;
    }

    let mut commands = BufReader::new(from_host).lines();
    loop {
        tokio::select! {
            line = commands.next_line() => match line {
                Ok(Some(line)) => {
                    if let Ok(FromHost::Command { command }) = serde_json::from_str(&line) {
                        tokio::spawn(bridge.clone().command(command));
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
    stdin: tokio::sync::Mutex<ChildStdin>,
    next: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>,
}

impl Rpc {
    async fn write(&self, message: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
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
}

impl Bridge {
    /// Starts Codex's conversation: says who Sandhill is, then starts a
    /// thread in the folder (or resumes the one it had), with Sandhill's
    /// instructions and the mode.
    async fn start(self: &Arc<Self>, options: &Options) -> Result<()> {
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
            // The chat already shows the turns.
            Some(thread) => {
                params["threadId"] = json!(thread);
                params["excludeTurns"] = json!(true);
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
        self.report(json!({"thread": thread})).await;
        // The conversation has started, as Claude Code's SessionStart hook says.
        let source = if options.thread.is_some() {
            "resume"
        } else {
            "startup"
        };
        self.chat(json!({"kind": "session", "source": source, "model": started["model"]}))
            .await;
        // What the app shows besides: the models, the account and its limits.
        tokio::spawn(self.clone().look_around());
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
        let summary = if account.is_null() {
            Value::Null
        } else {
            json!({"type": account["type"], "email": account["email"], "plan": account["planType"]})
        };
        self.report(json!({"account": summary, "auth_required": read["requiresOpenaiAuth"]}))
            .await;
        if account["type"] == "chatgpt"
            && let Ok(limits) = self.rpc.call("account/rateLimits/read", json!({})).await
        {
            self.limits(&limits["rateLimits"]);
        }
    }

    fn limits(&self, snapshot: &Value) {
        let mut state = self.state.lock().unwrap();
        let known = state.status.get("rate_limits").cloned().unwrap_or_default();
        let limits = items::rate_limits(&known, snapshot);
        if limits.as_object().is_some_and(|l| !l.is_empty()) {
            state.status.insert("rate_limits".into(), limits);
        }
    }

    /// What the user did in the chat.
    async fn command(self: Arc<Self>, command: Value) {
        match command["kind"].as_str().unwrap_or("") {
            "say" => {
                if let Some(text) = command["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.say(text.to_string(), true).await;
                }
            }
            "interrupt" => self.interrupt().await,
            "set" => {
                {
                    let mut state = self.state.lock().unwrap();
                    let text = |key: &str| command[key].as_str().map(str::to_string);
                    if command.get("model").is_some() {
                        state.model = text("model");
                        let label = state
                            .models
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|m| Some(&m["id"]) == command.get("model"))
                            .map(|m| m["label"].clone());
                        let id = state.model.clone().unwrap_or(state.default_model.clone());
                        state.status.insert("model_id".into(), json!(id));
                        state
                            .status
                            .insert("model".into(), label.unwrap_or(json!(id)));
                    }
                    if command.get("effort").is_some() {
                        state.effort = text("effort");
                    }
                    if command.get("permission").is_some() {
                        state.permission = text("permission");
                    }
                }
                self.report(json!({})).await;
            }
            "compact" => {
                let thread = self.state.lock().unwrap().thread.clone();
                if let Err(err) = self
                    .rpc
                    .call("thread/compact/start", json!({"threadId": thread}))
                    .await
                {
                    self.chat(json!({"kind": "error", "text": format!("Codex couldn't compact the conversation: {err:#}")}))
                        .await;
                }
            }
            "login" => self.login(command["method"].as_str() == Some("code")).await,
            _ => {}
        }
    }

    /// Starts a turn with the user's message, or keeps it for the next turn
    /// while one runs. `shown`: the message shows in the chat.
    async fn say(self: Arc<Self>, text: String, shown: bool) {
        let at_ms = now_ms();
        let (params, asking) = {
            let mut state = self.state.lock().unwrap();
            if state.turn.is_some() || state.waiting > 0 {
                state.queued.push(text);
                return;
            }
            // Taken now, so a second message waits for this one's turn.
            state.turn = Some(String::new());
            state.plan = None;
            state.errored = false;
            let mut params = items::mode_settings(state.permission.as_deref());
            params["threadId"] = json!(state.thread);
            if let Some(model) = &state.model {
                params["model"] = json!(model);
            }
            if let Some(effort) = &state.effort {
                params["effort"] = json!(effort);
            }
            let planning = state.permission.as_deref() == Some("plan");
            state.planned |= planning;
            if state.planned {
                let model = state.model.clone().unwrap_or(state.default_model.clone());
                params["collaborationMode"] = json!({
                    "mode": if planning { "plan" } else { "default" },
                    "settings": {"model": model, "reasoning_effort": state.effort, "developer_instructions": null},
                });
            }
            let asking = state.permission.as_deref() == Some(guard::ASK);
            (params, asking)
        };
        let prompt = if shown {
            self.prompt().await
        } else {
            Default::default()
        };
        let mut input = vec![json!({"type": "text", "text": text, "text_elements": []})];
        if let Some(context) = &prompt.context {
            input.push(json!({"type": "text", "text": context, "text_elements": []}));
        }
        if prompt.report.as_object().is_some_and(|r| !r.is_empty()) {
            self.report(prompt.report.clone()).await;
        }
        let place = prompt.place;
        if asking {
            input.push(json!({"type": "text", "text": guard::ASK_NOTE, "text_elements": []}));
        }
        let mut params = params;
        params["input"] = json!(input);
        if shown {
            println!("\n› {text}");
        }
        self.report(json!({"state": "busy"})).await;
        match self.rpc.call("turn/start", params).await {
            Ok(started) => {
                let turn = started["turn"]["id"].as_str().unwrap_or("").to_string();
                {
                    let mut state = self.state.lock().unwrap();
                    if state.turn.as_deref().is_none_or(str::is_empty) {
                        state.turn = Some(turn.clone());
                    }
                    if shown {
                        state.shown.push(turn.clone());
                    }
                }
                if shown {
                    self.chat(json!({
                        "kind": "prompt",
                        "turn": turn,
                        "text": text,
                        "place": place,
                        "at_ms": at_ms,
                    }))
                    .await;
                }
            }
            Err(err) => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.turn = None;
                    state.stopping = false;
                }
                let turn = uuid::Uuid::new_v4().to_string();
                if shown {
                    self.chat(
                        json!({"kind": "prompt", "turn": turn, "text": text, "at_ms": at_ms}),
                    )
                    .await;
                }
                self.chat(json!({"kind": "error", "turn": turn, "text": format!("{err:#}")}))
                    .await;
                self.report(json!({})).await;
                self.chat(json!({"kind": "stop", "turn": turn})).await;
            }
        }
    }

    /// What goes with the user's message, as Claude Code's prompt hook
    /// gives it: the text for the agent, and the place for the chat.
    async fn prompt(&self) -> crate::mcp::Prompt {
        let tools = crate::mcp::Tools::new(self.daemon.clone(), self.project.clone())
            .in_terminal(Some(self.dir.clone()));
        tools.prompt(PLACE_TIMEOUT).await
    }

    async fn interrupt(&self) {
        let (thread, turn) = {
            let mut state = self.state.lock().unwrap();
            state.queued.clear();
            if state.turn.is_some() && !state.started {
                // Starting: stopped once Codex has started it.
                state.stopping = true;
                return;
            }
            (state.thread.clone(), state.turn.clone())
        };
        if let Some(turn) = turn {
            let _ = self
                .rpc
                .call(
                    "turn/interrupt",
                    json!({"threadId": thread, "turnId": turn}),
                )
                .await;
        }
    }

    /// Signs Codex in with ChatGPT: in the browser, or with a code on
    /// another device (for a server). Codex keeps the login; the app only
    /// shows where to go.
    async fn login(&self, code: bool) {
        let params = if code {
            json!({"type": "chatgptDeviceCode"})
        } else {
            json!({"type": "chatgpt"})
        };
        match self.rpc.call("account/login/start", params).await {
            Ok(started) => {
                let login = json!({
                    "id": started["loginId"],
                    "url": started.get("authUrl").or(started.get("verificationUrl")),
                    "code": started["userCode"],
                });
                self.report(json!({"login": login})).await;
            }
            Err(err) => {
                self.report(json!({"login": {"error": format!("{err:#}")}}))
                    .await
            }
        }
    }

    /// Reads what Codex says until it exits.
    async fn read(self: Arc<Self>, stdout: ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        let mut tick = tokio::time::interval(FLUSH);
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    self.clone().handle(message).await;
                }
                _ = tick.tick() => self.flush_replies(None).await,
            }
        }
        // Codex is gone: nothing it was asked will be answered.
        self.rpc.pending.lock().unwrap().clear();
    }

    async fn handle(self: Arc<Self>, message: Value) {
        let method = message["method"].as_str().map(str::to_string);
        match (method, message.get("id")) {
            // A request: Codex asks the user something.
            (Some(method), Some(id)) => {
                tokio::spawn(self.request(id.clone(), method, message["params"].clone()));
            }
            (Some(method), None) => self.notification(&method, &message["params"]).await,
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let result = match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("failed").to_string()),
                    None => Ok(message["result"].clone()),
                };
                self.rpc.resolve(id, result);
            }
            (None, None) => {}
        }
    }

    async fn notification(self: &Arc<Self>, method: &str, params: &Value) {
        let turn = params["turnId"].as_str().unwrap_or("").to_string();
        match method {
            "turn/started" => {
                let id = params["turn"]["id"].as_str().unwrap_or("").to_string();
                let stopping = {
                    let mut state = self.state.lock().unwrap();
                    state.turn = Some(id);
                    state.started = true;
                    std::mem::take(&mut state.stopping)
                };
                if stopping {
                    let me = self.clone();
                    tokio::spawn(async move { me.interrupt().await });
                }
                self.report(json!({})).await;
            }
            "item/started" => self.item_started(&turn, &params["item"]).await,
            "item/completed" => self.item_completed(&turn, &params["item"]).await,
            "item/agentMessage/delta" => {
                let id = params["itemId"].as_str().unwrap_or("").to_string();
                let delta = params["delta"].as_str().unwrap_or("");
                let mut state = self.state.lock().unwrap();
                let reply = state.replies.entry(id).or_default();
                reply.turn = turn;
                reply.pending.push_str(delta);
            }
            "turn/plan/updated" => {
                let n = {
                    let mut state = self.state.lock().unwrap();
                    state.todos += 1;
                    state.todos
                };
                let id = format!("{turn}-todo-{n}");
                let input = items::todos(params);
                self.show_turn(&turn);
                self.chat(json!({"kind": "step", "turn": turn, "id": id, "tool": "TodoWrite", "input": input}))
                    .await;
                self.chat(json!({"kind": "step_done", "turn": turn, "id": id, "tool": "TodoWrite", "output": {}}))
                    .await;
            }
            "error" => {
                let text = params["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex failed");
                if params["willRetry"] == true {
                    println!("  ({text}; trying again)");
                } else {
                    self.state.lock().unwrap().errored = true;
                    self.show_turn(&turn);
                    self.chat(json!({"kind": "error", "turn": turn, "text": text}))
                        .await;
                    println!("  Error: {text}");
                }
            }
            "turn/completed" => self.turn_completed(&params["turn"]).await,
            "thread/tokenUsage/updated" => {
                if let Some(context) = items::context(&params["tokenUsage"]) {
                    let mut state = self.state.lock().unwrap();
                    for (key, value) in context.as_object().into_iter().flatten() {
                        state.status.insert(key.clone(), value.clone());
                    }
                }
                self.report(json!({})).await;
            }
            "account/rateLimits/updated" => {
                self.limits(&params["rateLimits"]);
                self.report(json!({})).await;
            }
            "account/login/completed" => {
                if params["success"] == true {
                    self.report(json!({"login": null})).await;
                    let me = self.clone();
                    tokio::spawn(async move { me.account().await });
                } else {
                    let error = params["error"].as_str().unwrap_or("Signing in failed");
                    self.report(json!({"login": {"error": error}})).await;
                }
            }
            "account/updated" => {
                let me = self.clone();
                tokio::spawn(async move { me.account().await });
            }
            "serverRequest/resolved" => {
                // Settled without the user (the turn was stopped): the ask goes.
                let id = params["requestId"].to_string();
                if let Some(waiting) = self.state.lock().unwrap().requests.remove(&id) {
                    waiting.abort();
                }
            }
            "model/rerouted" => {
                let mut state = self.state.lock().unwrap();
                state
                    .status
                    .insert("model".into(), params["toModel"].clone());
            }
            "warning" | "configWarning" | "deprecationNotice" => {
                let text = params["message"]
                    .as_str()
                    .or(params["summary"].as_str())
                    .unwrap_or("");
                println!("  {text}");
            }
            _ => {}
        }
    }

    async fn item_started(&self, turn: &str, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        if item["type"] == "agentMessage" {
            self.state
                .lock()
                .unwrap()
                .replies
                .entry(id)
                .or_default()
                .turn = turn.to_string();
            return;
        }
        if !items::shown_at_start(item) {
            return;
        }
        let Some((tool, input)) = items::step(item, &self.cwd) else {
            return;
        };
        println!("  ▸ {tool} {}", one_line(&input));
        self.show_turn(turn);
        self.state
            .lock()
            .unwrap()
            .steps
            .push((id.clone(), tool.clone(), input.clone()));
        self.chat(json!({"kind": "step", "turn": turn, "id": id, "tool": tool, "input": input}))
            .await;
    }

    async fn item_completed(&self, turn: &str, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => {
                {
                    // A reply that came whole, with no pieces before.
                    let mut state = self.state.lock().unwrap();
                    let reply = state.replies.entry(id.clone()).or_default();
                    if reply.turn.is_empty() {
                        reply.turn = turn.to_string();
                    }
                }
                self.flush_replies(Some((&id, item["text"].as_str().unwrap_or(""))))
                    .await;
                return;
            }
            "plan" => {
                self.state.lock().unwrap().plan = item["text"].as_str().map(str::to_string);
                return;
            }
            "contextCompaction" => {
                self.chat(json!({"kind": "session", "source": "compact"}))
                    .await;
                println!("  (conversation compacted)");
                return;
            }
            _ => {}
        }
        let started = {
            let mut state = self.state.lock().unwrap();
            let at = state.steps.iter().position(|(step, _, _)| *step == id);
            at.map(|at| state.steps.remove(at))
        };
        let tool = match started {
            Some((_, tool, _)) => tool,
            None => {
                // Shown now that it is complete.
                let Some((tool, input)) = items::step(item, &self.cwd) else {
                    return;
                };
                self.show_turn(turn);
                self.chat(
                    json!({"kind": "step", "turn": turn, "id": id, "tool": tool, "input": input}),
                )
                .await;
                tool
            }
        };
        let mut end = items::step_end(item, &tool);
        if end["kind"] == "step_failed" {
            println!("    failed");
        }
        end["turn"] = json!(turn);
        end["id"] = json!(id);
        self.chat(end).await;
    }

    /// Sends what Codex has written of its replies. With `done` (its id and
    /// text), that reply is complete: its last piece goes, and if the
    /// pieces don't add up to what Codex says it wrote, the whole text
    /// replaces them.
    async fn flush_replies(&self, done: Option<(&str, &str)>) {
        let pieces = {
            let mut state = self.state.lock().unwrap();
            let mut pieces = Vec::new();
            for (id, reply) in state.replies.iter_mut() {
                let finished = done.is_some_and(|(done, _)| done == id);
                if reply.pending.is_empty() && !finished {
                    continue;
                }
                let text = std::mem::take(&mut reply.pending);
                reply.sent.push_str(&text);
                pieces.push((reply.turn.clone(), id.clone(), reply.index, text, finished));
                reply.index += 1;
            }
            if let Some((id, full)) = done {
                let reply = state.replies.remove(id).unwrap_or_default();
                if reply.sent != full {
                    // Replace every piece sent: the first with the whole text.
                    pieces.retain(|(_, piece, ..)| piece != id);
                    for index in 0..reply.index.max(1) {
                        let text = if index == 0 { full } else { "" };
                        pieces.push((
                            reply.turn.clone(),
                            id.to_string(),
                            index,
                            text.to_string(),
                            true,
                        ));
                    }
                }
            }
            pieces
        };
        for (turn, id, index, text, finished) in pieces {
            self.show_turn(&turn);
            self.chat(json!({
                "kind": "text",
                "turn": turn,
                "message": id,
                "index": index,
                "text": text,
                "final": finished,
            }))
            .await;
        }
        if let Some((_, full)) = done {
            println!("{}", full.trim_end());
        }
    }

    async fn turn_completed(self: &Arc<Self>, turn: &Value) {
        let id = turn["id"].as_str().unwrap_or("").to_string();
        self.flush_replies(None).await;
        let (plan, errored, shown) = {
            let mut state = self.state.lock().unwrap();
            state.turn = None;
            state.started = false;
            state.stopping = false;
            state.steps.clear();
            state.replies.clear();
            let shown = state.shown.contains(&id);
            (state.plan.take(), state.errored, shown)
        };
        if turn["status"] == "failed" && !errored {
            let text = turn["error"]["message"].as_str().unwrap_or("Codex failed");
            self.chat(json!({"kind": "error", "turn": id, "text": text}))
                .await;
            println!("  Error: {text}");
        }
        let planning = self.state.lock().unwrap().permission.as_deref() == Some("plan");
        match plan {
            // A plan to carry out: the turn ends once the user decides.
            Some(plan) if planning && turn["status"] == "completed" => {
                tokio::spawn(self.clone().decide_plan(id, plan));
            }
            _ => {
                // Idle before the turn shows as ended, so a client that sees
                // the end also sees the agent idle.
                self.report(json!({})).await;
                if shown || turn["status"] == "failed" {
                    self.chat(json!({"kind": "stop", "turn": id})).await;
                }
                self.next();
            }
        }
    }

    /// Starts the messages that came while the turn ran.
    fn next(self: &Arc<Self>) {
        let queued = std::mem::take(&mut self.state.lock().unwrap().queued);
        if !queued.is_empty() {
            tokio::spawn(self.clone().say(queued.join("\n\n"), true));
        }
    }

    /// Asks the user whether to carry out the plan Codex made in plan mode,
    /// as Claude Code's plan card does; yes switches to Normal and carries
    /// it out.
    async fn decide_plan(self: Arc<Self>, turn: String, plan: String) {
        let id = short_id();
        let event = json!({
            "kind": "ask",
            "turn": turn,
            "id": id,
            "tool": "ExitPlanMode",
            "input": {"plan": plan},
            "at_ms": now_ms(),
        });
        self.state.lock().unwrap().waiting += 1;
        self.report(json!({})).await;
        let answer = super::ask(&self.dir, &id, event).await.ok().flatten();
        let carry_out = answer.as_ref().is_some_and(|a| a["behavior"] == "allow");
        {
            let mut state = self.state.lock().unwrap();
            state.waiting -= 1;
            if carry_out {
                state.permission = Some("supervised".into());
            }
        }
        self.report(json!({})).await;
        self.chat(json!({"kind": "stop", "turn": turn})).await;
        if carry_out {
            tokio::spawn(self.clone().say(CARRY_OUT.to_string(), false));
        } else {
            self.next();
        }
    }

    /// Asks the user what Codex asks, and gives Codex the answer.
    async fn request(self: Arc<Self>, id: Value, method: String, params: Value) {
        let steps = self.state.lock().unwrap().steps.clone();
        let Some(ask) = items::ask(&method, &params, &steps) else {
            self.rpc
                .reply_error(id, &format!("Sandhill doesn't handle {method}"))
                .await;
            return;
        };
        let ask_id = short_id();
        let turn = params["turnId"]
            .as_str()
            .map(str::to_string)
            .or_else(|| self.state.lock().unwrap().turn.clone());
        let mut event = json!({
            "kind": "ask",
            "turn": turn,
            "id": ask_id,
            "tool": ask.tool,
            "input": ask.input,
            "step": ask.step,
            "at_ms": now_ms(),
        });
        if !ask.remember.is_empty() {
            event["remember"] = json!(ask.remember);
            event["scopes"] = json!(ask.scopes);
        }
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        println!("  ? {} (waiting for you)", ask.tool);
        let key = id.to_string();
        // Asked once the agent reports waiting, so a client that sees the
        // ask also sees the agent waiting.
        let (go, ready) = tokio::sync::oneshot::channel::<()>();
        let waiting = tokio::spawn({
            let dir = self.dir.clone();
            async move {
                let _ = ready.await;
                super::ask(&dir, &ask_id, event).await
            }
        });
        {
            let mut state = self.state.lock().unwrap();
            state.waiting += 1;
            state.requests.insert(key.clone(), waiting.abort_handle());
        }
        self.report(json!({"message": format!("Codex asks to use {}", short_tool(&ask.tool))}))
            .await;
        let _ = go.send(());
        let answer = waiting.await;
        {
            let mut state = self.state.lock().unwrap();
            state.waiting -= 1;
            state.requests.remove(&key);
        }
        self.report(json!({})).await;
        match answer {
            Ok(Ok(Some(answer))) => {
                let response = items::response(&method, &params, &answer);
                self.rpc.reply(id, response).await;
            }
            // Codex settled it itself.
            Err(_) => {}
            // The terminal is going away: say no.
            Ok(_) => {
                let response = items::response(&method, &params, &json!({"behavior": "deny"}));
                self.rpc.reply(id, response).await;
            }
        }
    }

    /// Notes that the chat shows this turn, so it ends with `stop`.
    fn show_turn(&self, turn: &str) {
        if turn.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if !state.shown.iter().any(|t| t == turn) {
            state.shown.push(turn.to_string());
        }
    }

    async fn chat(&self, mut event: Value) {
        if event.get("at_ms").is_none() {
            event["at_ms"] = json!(now_ms());
        }
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        let _ = self.host.send(&ToHost::Chat { event }).await;
    }

    /// Reports the agent's state with `extra`: busy, waiting for the user or
    /// idle; the mode; and the status the app shows.
    async fn report(&self, extra: Value) {
        let report = {
            let state = self.state.lock().unwrap();
            let busy = state.turn.is_some();
            let mut status = state.status.clone();
            if let Some(effort) = &state.effort {
                status.insert("effort".into(), json!(effort));
            }
            let mut report = json!({
                "state": if state.waiting > 0 { "waiting" } else if busy { "busy" } else { "idle" },
                "permission": state.permission.as_deref().unwrap_or("supervised"),
                "status": status,
            });
            if !state.models.is_null() {
                report["models"] = state.models.clone();
            }
            if state.waiting > 0 && extra.get("message").is_none() {
                report["message"] = json!("Codex is waiting for you");
            }
            for (key, value) in extra.as_object().into_iter().flatten() {
                report[key] = value.clone();
            }
            report
        };
        let _ = self.host.send(&ToHost::Agent { value: report }).await;
    }
}

/// An id for an ask.
fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// "add_cell" for Sandhill's tools, "bash" for a command.
fn short_tool(tool: &str) -> String {
    match tool.rsplit_once("__") {
        Some((_, name)) => name.to_string(),
        None if tool == "Bash" => "a command".to_string(),
        None if tool == "apply_patch" => "edit files".to_string(),
        None => tool.to_string(),
    }
}

/// A step's input on one line, for the terminal.
fn one_line(input: &Value) -> String {
    let text = input["command"]
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            input["changes"].as_array().map(|changes| {
                changes
                    .iter()
                    .filter_map(|c| c["path"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
        .unwrap_or_else(|| input.to_string());
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(100) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}
