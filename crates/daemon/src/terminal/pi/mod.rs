//! pi in the chat: `sandhilld pi-chat`, which runs in a pi conversation's
//! terminal in place of pi's own interface. pi has no ACP; it runs as
//! `pi --mode rpc` (JSON lines over stdio: commands in; responses, session
//! events and extension dialogs out), and:
//!
//! - the user's messages, model and thinking effort come from the app through
//!   the terminal host ([`super::ToHost::Command`]) and become pi prompts,
//!   `set_model` and `set_thinking_level`, with what the user added with @
//!   and #;
//! - pi's events come back as the chat entries Claude Code's hooks make,
//!   translated in [`events`];
//! - Sandhill's MCP server comes from a small extension loaded for the run
//!   (`-e`, see `extension.js`): pi reads MCP servers only from its config
//!   files, but an extension can register one for its session, so no pi file
//!   is edited. Tools run as they do in pi itself, without Sandhill approval;
//! - the session is resumed with `--session-id`, and its id is the
//!   conversation's resume key (`thread`), so Sandhill's command-line mode
//!   runs pi's own interface on the same session and back.
//!
//! pi is unmodified and uses the user's own model accounts; Sandhill never
//! reads its credentials.

pub mod events;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

pub use super::acp::Options;
use super::{FromHost, Paths, ToHost};
use crate::journal::now_ms;
use events::Translator;

/// Replies are sent to the chat in pieces at most this far apart.
const FLUSH: Duration = Duration::from_millis(120);
/// How long listing models may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long pi may take to stop its turn and exit before its command line.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// What a message waits at most for what a # cell last showed.
const REFS_TIMEOUT: Duration = Duration::from_millis(1500);
/// pi's own interface: Escape stops a turn, Ctrl+C clears the editor and
/// Ctrl+D quits. (pi has no `/exit`; typed, it would be a message.)
const QUIT: [&str; 3] = ["\x1b", "\x03", "\x04"];
const EXTENSION: &str = include_str!("extension.js");

/// Writes Sandhill's pi extension (see `extension.js`) into `dir`, the
/// conversation's terminal folder. It reads its settings from the
/// environment: `SANDHILL_MCP_URL` and `SANDHILL_MCP_TOKEN`.
pub fn write_extension(dir: &Path) -> std::io::Result<PathBuf> {
    let path = dir.join("pi-sandhill.js");
    std::fs::write(&path, EXTENSION)?;
    Ok(path)
}

/// The models pi offers, with their thinking levels, from `pi --mode rpc
/// --no-session` asked `get_state` and `get_available_models`: no session is
/// saved and no model is called.
pub async fn list_models(program: &Path) -> Result<Value> {
    let mut command = Command::new(program);
    command
        .args(["--mode", "rpc", "--no-session"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(program)? {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot start {}", program.display()))?;
    let mut stdin = child.stdin.take().context("no pi stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no pi stdout")?).lines();
    let ask = async {
        let mut send = async |message: Value| -> Result<()> {
            let mut line = serde_json::to_vec(&message)?;
            line.push(b'\n');
            stdin.write_all(&line).await?;
            Ok(stdin.flush().await?)
        };
        send(json!({"id": "state", "type": "get_state"})).await?;
        send(json!({"id": "models", "type": "get_available_models"})).await?;
        let (mut state, mut models) = (None, None);
        while let Some(line) = lines.next_line().await? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match message["type"].as_str() {
                // A user's extension asking at startup is not answered here.
                Some("extension_ui_request") if is_dialog(&message) => {
                    send(json!({"type": "extension_ui_response", "id": message["id"], "cancelled": true}))
                        .await?;
                }
                Some("response") => {
                    if message["success"] != true {
                        bail!(
                            "pi couldn't list its models: {}",
                            message["error"].as_str().unwrap_or("an error")
                        );
                    }
                    match message["id"].as_str() {
                        Some("state") => state = Some(message["data"].clone()),
                        Some("models") => models = Some(message["data"]["models"].clone()),
                        _ => {}
                    }
                }
                _ => {}
            }
            if let (Some(state), Some(models)) = (&state, &models) {
                return Ok(events::models(
                    models,
                    events::model_id(&state["model"]).as_deref(),
                    state["thinkingLevel"].as_str(),
                ));
            }
        }
        bail!("pi stopped before listing its models")
    };
    let models = tokio::time::timeout(LIST_TIMEOUT, ask)
        .await
        .context("pi took too long to list its models");
    let _ = child.start_kill();
    let _ = child.wait().await;
    models?
}

fn is_dialog(request: &Value) -> bool {
    matches!(
        request["method"].as_str(),
        Some("select" | "confirm" | "input" | "editor")
    )
}

/// What pi's answer to a command is for.
enum Request {
    State,
    Models,
    Entries,
    Model(String),
    Effort(String),
    Prompt,
    Abort,
    Stats,
    /// `get_state` after a change: pi's thinking level for its model.
    Refresh,
    Compact,
}

/// What `pi --mode rpc` says while it starts, gathered before the chat
/// goes on.
#[derive(Default)]
struct Startup {
    state: Option<Value>,
    models: Option<Value>,
    entries: Option<Value>,
}

/// A dialog of pi's waiting for the user.
enum Dialog {
    Select(Vec<String>),
    Confirm,
}

struct PendingDialog {
    /// pi's id for the dialog.
    ui: Value,
    dialog: Dialog,
    task: tokio::task::JoinHandle<()>,
}

/// A message sent to pi that it has not yet accepted.
struct Sent {
    text: String,
    place: Option<Value>,
}

/// The running `pi --mode rpc`.
struct Pi {
    child: Child,
    lines: Lines<BufReader<ChildStdout>>,
}

struct Bridge {
    options: Options,
    dir: PathBuf,
    cwd: PathBuf,
    extension: PathBuf,
    mcp_url: String,
    token: String,
    instructions: String,
    host: OwnedWriteHalf,
    stdin: Option<ChildStdin>,
    next: u64,
    pending: HashMap<String, Request>,
    /// Set while pi starts; the chat waits for it.
    startup: Option<Startup>,
    /// The session entry is added once, not after the command line.
    announced: bool,
    session: String,
    /// Turns already in the chat (their `stop`).
    history: HashSet<String>,
    number: u64,
    serial: u64,
    busy: bool,
    stopping: bool,
    queued: VecDeque<String>,
    sent: Option<Sent>,
    events: Translator,
    dialogs: HashMap<String, PendingDialog>,
    answers: mpsc::UnboundedSender<(String, Option<Value>)>,
    models: Value,
    /// What the bridge last set (or pi started with), and the user's choice.
    model: Option<String>,
    effort: Option<String>,
    selected_model: Option<String>,
    selected_effort: Option<String>,
    /// pi's thinking level now, which it can clamp for a model.
    level: Option<String>,
    status: serde_json::Map<String, Value>,
    view_mode: &'static str,
}

/// Runs pi's conversation until pi or the terminal ends; returns pi's exit
/// code.
pub async fn run(options: Options) -> Result<i32> {
    let dir = std::env::var_os("SANDHILL_TERMINAL")
        .map(PathBuf::from)
        .context("pi-chat runs in a Sandhill terminal")?;
    let cwd = std::env::current_dir()?;
    let extension = write_extension(&dir)?;
    // pi reaches Sandhill's tools over loopback HTTP, with a token for this
    // conversation, through the extension's registered server.
    let token = uuid::Uuid::new_v4().simple().to_string();
    let tools = crate::mcp::Tools::new(options.daemon.clone(), options.project.clone())
        .in_terminal(Some(dir.clone()));
    let port = crate::mcp::http::serve(std::sync::Arc::new(tools), token.clone()).await?;
    let socket = UnixStream::connect(Paths::new(&dir).socket())
        .await
        .context("cannot reach the terminal")?;
    let (reader, host) = socket.into_split();
    let mut commands = BufReader::new(reader).lines();
    let (answers, mut answer_rx) = mpsc::unbounded_channel();
    let mut queued = VecDeque::new();
    if let Some(prompt) = options.prompt.clone().filter(|p| !p.trim().is_empty()) {
        queued.push_back(prompt);
    }
    let history = super::chat::read(&dir, 0)
        .into_iter()
        .filter(|entry| entry.event["kind"] == "stop")
        .filter_map(|entry| entry.event["turn"].as_str().map(str::to_string))
        .collect();
    let mut bridge = Bridge {
        instructions: crate::agents::instructions(&options.options),
        session: options.session.clone().unwrap_or_default(),
        selected_model: options.options.model.clone(),
        selected_effort: options.options.effort.clone(),
        options,
        dir,
        cwd,
        extension,
        mcp_url: format!("http://127.0.0.1:{port}/mcp"),
        token,
        host,
        stdin: None,
        next: 1,
        pending: HashMap::new(),
        startup: None,
        announced: false,
        history,
        number: 0,
        serial: 0,
        busy: false,
        stopping: false,
        queued,
        sent: None,
        events: Translator::default(),
        dialogs: HashMap::new(),
        answers,
        models: Value::Null,
        model: None,
        effort: None,
        level: None,
        status: serde_json::Map::new(),
        view_mode: "chat",
    };
    bridge.send_host(ToHost::Listen).await?;
    let mut pi = bridge.start().await?;
    let mut tick = tokio::time::interval(FLUSH);
    let result = async {
        loop {
            tokio::select! {
                line = pi.lines.next_line() => {
                    let Some(line) = line? else {
                        let status = pi.child.wait().await?;
                        bridge.stdin = None;
                        let interrupted = bridge.busy;
                        bridge.finish(interrupted).await?;
                        bridge.chat(json!({"kind": "error", "text": format!("pi exited ({status}).")})).await?;
                        return Ok(status.code().unwrap_or(1));
                    };
                    if let Ok(message) = serde_json::from_str::<Value>(&line) {
                        bridge.message(message).await?;
                    }
                }
                line = commands.next_line() => {
                    // The terminal is going away.
                    let Some(line) = line? else { return Ok(1) };
                    if let Ok(FromHost::Command { command }) = serde_json::from_str(&line) {
                        if command["kind"] == "view" {
                            if command["mode"] == "terminal" {
                                bridge.native(&mut pi, &mut commands).await?;
                            }
                        } else {
                            bridge.command(command).await?;
                        }
                    }
                }
                Some((key, answer)) = answer_rx.recv() => bridge.answer(&key, answer).await?,
                _ = tick.tick() => {
                    let out = bridge.events.flush(false);
                    bridge.send_all(out).await?;
                }
            }
            bridge.advance().await?;
        }
    }
    .await;
    if let Err(error) = &result {
        let _ = bridge
            .chat(json!({"kind": "error", "turn": bridge.events.turn, "text": format!("pi: {error:#}")}))
            .await;
        let _ = bridge.finish(true).await;
    }
    for (_, dialog) in bridge.dialogs.drain() {
        dialog.task.abort();
    }
    let _ = pi.child.kill().await;
    result
}

impl Bridge {
    async fn send_host(&mut self, message: ToHost) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        Ok(self.host.write_all(&bytes).await?)
    }

    async fn chat(&mut self, mut event: Value) -> Result<()> {
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, value| !value.is_null());
            map.entry("at_ms").or_insert_with(|| json!(now_ms()));
        }
        self.send_host(ToHost::Chat { event }).await
    }

    /// Entries from [`events`], also printed to the terminal.
    async fn send_all(&mut self, out: Vec<Value>) -> Result<()> {
        for event in out {
            match event["kind"].as_str() {
                Some("text") => print!("{}", event["text"].as_str().unwrap_or_default()),
                Some("step") if event["tool"] != "reasoning" => {
                    println!("\n  · {}", event["tool"].as_str().unwrap_or_default());
                }
                Some("stop") => {
                    println!();
                    if let Some(turn) = event["turn"].as_str() {
                        self.history.insert(turn.to_string());
                    }
                }
                _ => {}
            }
            self.chat(event).await?;
        }
        Ok(())
    }

    async fn report(&mut self, extra: Value) -> Result<()> {
        let label = self
            .models
            .as_array()
            .into_iter()
            .flatten()
            .find(|model| model["id"].as_str() == self.model.as_deref())
            .map(|model| model["label"].clone())
            .unwrap_or_else(|| json!(self.model));
        let mut status = self.status.clone();
        status.insert("model".into(), label);
        status.insert("model_id".into(), json!(self.model));
        status.insert("effort".into(), json!(self.level));
        let state = if !self.dialogs.is_empty() {
            "waiting"
        } else if self.busy || self.startup.is_some() {
            "busy"
        } else {
            "idle"
        };
        let mut report = json!({
            "state": state,
            "thread": self.session,
            "view_mode": self.view_mode,
            "status": status,
        });
        if !self.models.is_null() {
            report["models"] = self.models.clone();
        }
        for (key, value) in extra.as_object().into_iter().flatten() {
            report[key] = value.clone();
        }
        self.send_host(ToHost::Agent { value: report }).await
    }

    async fn write(&mut self, message: Value) -> Result<()> {
        let stdin = self.stdin.as_mut().context("pi is stopped")?;
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        stdin.write_all(&bytes).await?;
        Ok(stdin.flush().await?)
    }

    async fn call(&mut self, request: Request, mut command: Value) -> Result<()> {
        let id = format!("sandhill-{}", self.next);
        self.next += 1;
        command["id"] = json!(id);
        self.pending.insert(id, request);
        self.write(command).await
    }

    /// The command for pi: RPC mode for the chat, or its own interface.
    fn pi_command(&self, rpc: bool) -> Command {
        let mut command = Command::new(&self.options.program);
        if rpc {
            command.args(["--mode", "rpc"]);
        }
        if !self.session.is_empty() {
            // Opens the session, or starts it with this id if pi has not
            // saved it yet (it saves after the first reply).
            command.arg("--session-id").arg(&self.session);
        }
        command
            .arg("-e")
            .arg(&self.extension)
            .arg("--append-system-prompt")
            .arg(&self.instructions)
            .env("SANDHILL_MCP_URL", &self.mcp_url)
            .env("SANDHILL_MCP_TOKEN", &self.token)
            .current_dir(&self.cwd);
        super::Terminals::scope_child(&mut command, &self.dir);
        command
    }

    /// Starts `pi --mode rpc` and asks for its session, models and saved
    /// turns.
    async fn start(&mut self) -> Result<Pi> {
        let mut command = self.pi_command(true);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.options.program.display()))?;
        self.stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().context("no pi stdout")?).lines();
        self.startup = Some(Startup::default());
        self.pending.clear();
        self.call(Request::State, json!({"type": "get_state"}))
            .await?;
        self.call(Request::Models, json!({"type": "get_available_models"}))
            .await?;
        self.call(Request::Entries, json!({"type": "get_entries"}))
            .await?;
        self.report(json!({})).await?;
        Ok(Pi { child, lines })
    }

    /// Goes on once pi has said all [`Startup`] asks for.
    async fn started(&mut self) -> Result<()> {
        let complete = self
            .startup
            .as_ref()
            .is_some_and(|s| s.state.is_some() && s.models.is_some() && s.entries.is_some());
        let Some(Startup {
            state: Some(state),
            models: Some(models),
            entries: Some(entries),
        }) = self.startup.take_if(|_| complete)
        else {
            return Ok(());
        };
        self.session = state["sessionId"]
            .as_str()
            .context("pi did not name its session")?
            .to_string();
        self.model = events::model_id(&state["model"]);
        self.level = state["thinkingLevel"].as_str().map(str::to_string);
        self.effort = self.level.clone();
        self.models = events::models(&models, self.model.as_deref(), self.level.as_deref());
        let (replayed, number) = events::history(
            entries["entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
            &self.session,
            &self.history,
        );
        self.number = number;
        self.send_all(replayed).await?;
        if !self.announced {
            self.announced = true;
            let source = if self.options.session.is_some() {
                "resume"
            } else {
                "startup"
            };
            self.chat(json!({"kind": "session", "source": source, "model": self.model}))
                .await?;
        }
        self.view_mode = "chat";
        self.report(json!({})).await
    }

    async fn message(&mut self, message: Value) -> Result<()> {
        match message["type"].as_str().unwrap_or_default() {
            "response" => self.response(message).await,
            "extension_ui_request" => self.dialog(message).await,
            "agent_start" => {
                if self.sent.is_some() {
                    self.accept().await
                } else if self.events.turn.is_none() {
                    // Work pi started on its own (an extension's message).
                    self.serial += 1;
                    let turn = format!("{}:auto:{}", self.session, self.serial);
                    let out = self.events.begin(turn);
                    self.send_all(out).await?;
                    self.busy = true;
                    self.report(json!({})).await
                } else {
                    Ok(())
                }
            }
            "agent_settled" => self.finish(message["aborted"] == true).await,
            "thinking_level_changed" => {
                self.level = message["level"].as_str().map(str::to_string);
                self.report(json!({})).await
            }
            "extension_error" => {
                println!(
                    "\n  pi extension error ({}): {}",
                    message["event"].as_str().unwrap_or_default(),
                    message["error"].as_str().unwrap_or_default()
                );
                Ok(())
            }
            _ => {
                if self.sent.is_some() {
                    self.accept().await?;
                }
                let out = self.events.event(&message);
                self.send_all(out).await
            }
        }
    }

    async fn response(&mut self, message: Value) -> Result<()> {
        let Some(request) = message["id"]
            .as_str()
            .and_then(|id| self.pending.remove(id))
        else {
            return Ok(());
        };
        let ok = message["success"] == true;
        let error = message["error"]
            .as_str()
            .unwrap_or("pi refused the command")
            .to_string();
        let data = &message["data"];
        match request {
            Request::State | Request::Models | Request::Entries => {
                if !ok {
                    bail!("pi couldn't start: {error}");
                }
                if let Some(startup) = self.startup.as_mut() {
                    match request {
                        Request::State => startup.state = Some(data.clone()),
                        Request::Models => startup.models = Some(data["models"].clone()),
                        _ => startup.entries = Some(data.clone()),
                    }
                }
                self.started().await?;
            }
            Request::Model(model) => {
                if ok {
                    self.model = Some(model);
                    // pi picks the new model's own level; Sandhill's is set again.
                    self.effort = None;
                    self.call(Request::Refresh, json!({"type": "get_state"}))
                        .await?;
                } else {
                    self.selected_model = self.model.clone();
                    self.queued.clear();
                    self.chat(json!({"kind": "error", "text": format!("pi couldn't switch models: {error}")}))
                        .await?;
                }
                self.report(json!({})).await?;
            }
            Request::Effort(level) => {
                if ok {
                    self.effort = Some(level);
                    self.call(Request::Refresh, json!({"type": "get_state"}))
                        .await?;
                } else {
                    self.selected_effort = self.effort.clone();
                    self.chat(json!({"kind": "error", "text": format!("pi couldn't set the thinking effort: {error}")}))
                        .await?;
                }
                self.report(json!({})).await?;
            }
            Request::Refresh => {
                if ok {
                    self.level = data["thinkingLevel"].as_str().map(str::to_string);
                    self.report(json!({})).await?;
                }
            }
            Request::Prompt => {
                if !ok {
                    self.reject("rejected", Some(error)).await?;
                } else if data["disposition"] == "handled" {
                    // A command an extension handled: no turn followed.
                    self.reject("handled", None).await?;
                } else {
                    self.accept().await?;
                }
            }
            Request::Abort => {}
            Request::Stats => {
                let usage = &data["contextUsage"];
                if let (Some(percent), Some(size)) =
                    (usage["percent"].as_f64(), usage["contextWindow"].as_u64())
                {
                    self.status.insert(
                        "context".into(),
                        json!((percent.clamp(0.0, 100.0) * 10.0).round() / 10.0),
                    );
                    self.status.insert("context_size".into(), json!(size));
                    self.report(json!({})).await?;
                }
            }
            Request::Compact => {
                self.busy = false;
                if !ok {
                    self.chat(json!({"kind": "error", "text": format!("pi couldn't compact the conversation: {error}")}))
                        .await?;
                }
                self.report(json!({})).await?;
                self.call(Request::Stats, json!({"type": "get_session_stats"}))
                    .await?;
            }
        }
        Ok(())
    }

    /// pi took the message: its turn starts.
    async fn accept(&mut self) -> Result<()> {
        let Some(sent) = self.sent.take() else {
            return Ok(());
        };
        self.number += 1;
        let turn = events::turn_id(&self.session, self.number);
        let out = self.events.begin(turn.clone());
        self.send_all(out).await?;
        self.chat(json!({"kind": "prompt", "turn": turn, "text": sent.text, "place": sent.place}))
            .await?;
        println!("\n› {}", sent.text);
        Ok(())
    }

    /// A message pi did not turn into a turn, shown with why.
    async fn reject(&mut self, why: &str, error: Option<String>) -> Result<()> {
        let Some(sent) = self.sent.take() else {
            return Ok(());
        };
        self.serial += 1;
        let turn = format!("{}:{why}:{}", self.session, self.serial);
        self.chat(json!({"kind": "prompt", "turn": turn, "text": sent.text, "place": sent.place}))
            .await?;
        if let Some(error) = error {
            self.chat(json!({"kind": "error", "turn": turn, "text": error}))
                .await?;
        }
        self.send_all(vec![json!({"kind": "stop", "turn": turn})])
            .await?;
        self.busy = false;
        self.stopping = false;
        self.report(json!({})).await
    }

    /// pi settled: the turn ends.
    async fn finish(&mut self, interrupted: bool) -> Result<()> {
        if self.sent.is_some() {
            self.accept().await?;
        }
        let out = self.events.finish(interrupted);
        self.send_all(out).await?;
        self.cancel_dialogs().await?;
        let was_busy = std::mem::take(&mut self.busy);
        self.stopping = false;
        self.report(json!({})).await?;
        if was_busy && self.stdin.is_some() {
            self.call(Request::Stats, json!({"type": "get_session_stats"}))
                .await?;
        }
        Ok(())
    }

    async fn command(&mut self, command: Value) -> Result<()> {
        match command["kind"].as_str() {
            Some("say") => {
                if let Some(text) = command["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.queued.push_back(text.to_string());
                }
            }
            Some("interrupt") => self.interrupt().await?,
            Some("set") => {
                let text = |key: &str| command[key].as_str().map(str::to_string);
                if command.get("model").is_some() {
                    self.selected_model = text("model");
                }
                if command.get("effort").is_some() {
                    self.selected_effort = text("effort");
                }
                self.report(json!({})).await?;
            }
            Some("compact") => {
                if self.busy || self.startup.is_some() {
                    self.chat(json!({"kind": "error", "text": "pi is working; compact the conversation once it has finished."}))
                        .await?;
                } else {
                    self.busy = true;
                    self.report(json!({})).await?;
                    self.call(Request::Compact, json!({"type": "compact"}))
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn interrupt(&mut self) -> Result<()> {
        self.queued.clear();
        self.cancel_dialogs().await?;
        if self.busy && !self.stopping && self.stdin.is_some() {
            self.stopping = true;
            self.call(Request::Abort, json!({"type": "abort"})).await?;
        }
        Ok(())
    }

    /// Applies the user's model and effort, then sends the next message.
    async fn advance(&mut self) -> Result<()> {
        if self.startup.is_some() || self.busy || !self.pending.is_empty() || self.stdin.is_none() {
            return Ok(());
        }
        if let Some(model) = self
            .selected_model
            .clone()
            .filter(|model| Some(model) != self.model.as_ref())
        {
            if let Some((provider, id)) = model.split_once('/') {
                let command = json!({"type": "set_model", "provider": provider, "modelId": id});
                return self.call(Request::Model(model), command).await;
            }
            self.selected_model = self.model.clone();
            self.queued.clear();
            self.chat(json!({"kind": "error", "text": format!("pi names models provider/model; “{model}” is not one.")}))
                .await?;
        }
        if let Some(level) = self
            .selected_effort
            .clone()
            .filter(|level| Some(level) != self.effort.as_ref())
        {
            let command = json!({"type": "set_thinking_level", "level": level});
            return self.call(Request::Effort(level), command).await;
        }
        if let Some(text) = self.queued.pop_front() {
            self.prompt(text).await?;
        }
        Ok(())
    }

    async fn prompt(&mut self, text: String) -> Result<()> {
        self.busy = true;
        self.stopping = false;
        self.report(json!({})).await?;
        let tools =
            crate::mcp::Tools::new(self.options.daemon.clone(), self.options.project.clone())
                .in_terminal(Some(self.dir.clone()));
        let prompt = tools.prompt(REFS_TIMEOUT).await;
        if prompt
            .report
            .as_object()
            .is_some_and(|report| !report.is_empty())
        {
            self.report(prompt.report).await?;
        }
        let mut message = text.clone();
        if let Some(context) = &prompt.context {
            message.push_str("\n\n");
            message.push_str(context);
        }
        self.sent = Some(Sent {
            text,
            place: prompt.place,
        });
        self.call(
            Request::Prompt,
            json!({"type": "prompt", "message": message}),
        )
        .await
    }

    async fn respond(&mut self, id: &Value, mut fields: Value) -> Result<()> {
        fields["type"] = json!("extension_ui_response");
        fields["id"] = id.clone();
        self.write(fields).await
    }

    /// A native extension's dialog from pi.
    async fn dialog(&mut self, request: Value) -> Result<()> {
        let id = request["id"].clone();
        let title = request["title"].as_str().unwrap_or("pi").to_string();
        match request["method"].as_str().unwrap_or_default() {
            "select" => {
                let options: Vec<String> = request["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|o| o.as_str().map(str::to_string))
                    .collect();
                let choices = options
                    .iter()
                    .enumerate()
                    .map(|(index, name)| json!({"optionId": index.to_string(), "name": name, "kind": if index == 0 { "allow_once" } else { "allow_always" }}))
                    .collect::<Vec<_>>();
                let event = json!({"kind": "ask", "turn": self.events.turn, "tool": title,
                    "input": {"title": title}, "options": choices});
                self.ask(id, Dialog::Select(options), event).await
            }
            "confirm" => {
                let event = json!({"kind": "ask", "turn": self.events.turn, "tool": title,
                "input": {"title": title, "message": request["message"]}, "options": [
                    {"optionId": "yes", "name": "Yes", "kind": "allow_once"},
                    {"optionId": "no", "name": "No", "kind": "reject_once"},
                ]});
                self.ask(id, Dialog::Confirm, event).await
            }
            "input" | "editor" => {
                println!("\n  pi asked for text the chat can't give: {title}");
                self.respond(&id, json!({"cancelled": true})).await
            }
            "notify" => {
                let text = request["message"].as_str().unwrap_or_default();
                println!("\n  pi: {text}");
                if request["notifyType"] == "error" {
                    self.chat(json!({"kind": "error", "turn": self.events.turn, "text": format!("pi: {text}")}))
                        .await?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Shows the ask card `event` and waits (in a task) for the user's
    /// answer to pi's dialog `ui`.
    async fn ask(&mut self, ui: Value, dialog: Dialog, mut event: Value) -> Result<()> {
        let key = uuid::Uuid::new_v4().to_string();
        event["id"] = json!(key);
        event["at_ms"] = json!(now_ms());
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, value| !value.is_null());
        }
        let dir = self.dir.clone();
        let answers = self.answers.clone();
        let ask_id = key.clone();
        // Report waiting before the card can become visible.
        let (go, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = ready.await;
            let answer = super::ask(&dir, &ask_id, event).await.ok().flatten();
            let _ = answers.send((ask_id, answer));
        });
        self.dialogs.insert(key, PendingDialog { ui, dialog, task });
        self.report(json!({})).await?;
        let _ = go.send(());
        Ok(())
    }

    async fn answer(&mut self, key: &str, answer: Option<Value>) -> Result<()> {
        let Some(dialog) = self.dialogs.remove(key) else {
            return Ok(());
        };
        let answer = answer.unwrap_or_default();
        match dialog.dialog {
            Dialog::Select(options) => {
                let chosen = answer["option_id"]
                    .as_str()
                    .and_then(|id| id.parse::<usize>().ok())
                    .and_then(|index| options.get(index));
                let fields = match chosen {
                    Some(value) => json!({"value": value}),
                    None => json!({"cancelled": true}),
                };
                self.respond(&dialog.ui, fields).await?;
            }
            Dialog::Confirm => {
                let confirmed = answer["option_id"] == "yes";
                self.respond(&dialog.ui, json!({"confirmed": confirmed}))
                    .await?;
            }
        }
        self.report(json!({})).await
    }

    async fn cancel_dialogs(&mut self) -> Result<()> {
        for (_, dialog) in std::mem::take(&mut self.dialogs) {
            dialog.task.abort();
            if self.stdin.is_some() {
                self.respond(&dialog.ui, json!({"cancelled": true})).await?;
            }
        }
        Ok(())
    }

    /// Command-line mode: pi's own interface on the same session in this
    /// terminal, until the user goes back to the chat; then RPC mode again.
    async fn native(
        &mut self,
        pi: &mut Pi,
        commands: &mut Lines<BufReader<OwnedReadHalf>>,
    ) -> Result<()> {
        if self.startup.is_some() || self.session.is_empty() {
            self.chat(json!({"kind": "error", "text": "pi is still starting; open its command line in a moment."}))
                .await?;
            self.report(json!({"view_mode": "chat"})).await?;
            return Ok(());
        }
        self.view_mode = "switching_terminal";
        self.report(json!({})).await?;
        // Only one pi owns the session: the turn stops, then RPC mode exits.
        self.interrupt().await?;
        let settled = tokio::time::timeout(STOP_TIMEOUT, async {
            while self.busy {
                let Some(line) = pi.lines.next_line().await? else {
                    break;
                };
                if let Ok(message) = serde_json::from_str::<Value>(&line) {
                    self.message(message).await?;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
        if !matches!(settled, Ok(Ok(()))) || self.busy {
            self.finish(true).await?;
        }
        self.cancel_dialogs().await?;
        self.stdin = None;
        self.pending.clear();
        if !matches!(
            tokio::time::timeout(STOP_TIMEOUT, pi.child.wait()).await,
            Ok(Ok(_))
        ) {
            let _ = pi.child.kill().await;
        }
        let mut command = self.pi_command(false);
        self.view_mode = "terminal";
        self.report(json!({})).await?;
        let queued =
            super::codex::native::run_quitting(&mut command, &self.dir, commands, &QUIT).await;
        self.view_mode = "switching_chat";
        self.report(json!({})).await?;
        if let Err(error) = &queued {
            self.chat(
                json!({"kind": "error", "text": format!("pi's command line failed: {error:#}")}),
            )
            .await?;
        }
        *pi = self.start().await?;
        for command in queued.unwrap_or_default() {
            self.command(command).await?;
        }
        Ok(())
    }
}
