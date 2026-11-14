//! Antigravity in the chat: `sandhilld agy-chat`, which runs in an
//! Antigravity conversation's terminal. Sandhill uses Google's CLI `agy`
//! in its headless mode: each message runs `agy` once, with a user NDJSON
//! message on stdin (`--input-format stream-json`) and events on stdout
//! (`--output-format stream-json`). The next message goes on with the
//! same conversation (`--conversation`).
//!
//! - What agy does becomes the conversation entries Claude Code's hooks
//!   make ([`super::chat`]), so the chat shows every agent the same way.
//! - Headless agy can't ask for permission: what it would have asked about
//!   is turned down, and the chat says so. YOLO lets everything through.
//! - Sandhill's tools reach it as an MCP server in agy's own settings,
//!   added once with `agy mcp add sandhill`, as a user would add one.
//! - Headless agy runs without the conversation's terminal ([`headless`]),
//!   and a message whose agy says nothing within [`START_TIMEOUT`] fails
//!   with what it said on stderr (a sign-in it waits for, say).
//!
//! agy is unmodified and signs in on its own; Sandhill never sees its
//! login. Whether Google means its headless mode to be driven by another
//! app on a Google AI plan isn't settled (see docs/design.md).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::AgentOptions;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;

use super::chat::shorten;
use super::{FromHost, Paths, ToHost};
use crate::journal::now_ms;

mod history;
mod native;
pub(crate) mod usage;

/// Replies are sent to the chat in pieces at most this far apart.
const FLUSH: Duration = Duration::from_millis(120);
/// A message waits this long at most for where the user is.
const PLACE_TIMEOUT: Duration = Duration::from_millis(1500);
/// What a plan is carried out with, once the user approves it.
const CARRY_OUT: &str = "Carry out the plan.";
/// How long agy may take over one message (its own default is 5 minutes).
const PRINT_TIMEOUT: &str = "24h";
/// What Sandhill's MCP server is called in agy's settings.
pub const SERVER: &str = "sandhill";
/// agy's commands that only list or add something.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
/// How long agy has to stop after being asked, before it is made to.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// How long agy may take to start a message, until its first event.
const START_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Options {
    /// The `agy` CLI.
    pub agy: PathBuf,
    /// `sandhilld`, which agy starts as Sandhill's MCP server.
    pub executable: PathBuf,
    /// The daemon's socket, for where the user is and for the MCP server.
    pub daemon: PathBuf,
    pub project: Option<String>,
    pub options: AgentOptions,
    /// The first message.
    pub prompt: Option<String>,
    /// The conversation to go on with, when it is started again.
    pub conversation: Option<String>,
    /// What agy is told about Sandhill, with the conversation's first message.
    pub instructions: String,
}

/// Runs Antigravity's conversation until the terminal goes away.
pub async fn run(options: Options) -> Result<i32> {
    let dir = std::env::var_os("SANDHILL_TERMINAL")
        .map(PathBuf::from)
        .context("agy-chat runs in a Sandhill terminal")?;
    let cwd = std::env::current_dir()?;
    let stream = UnixStream::connect(Paths::new(&dir).socket())
        .await
        .context("cannot reach the terminal")?;
    let (from_host, to_host) = stream.into_split();
    let host = Host(tokio::sync::Mutex::new(to_host));
    host.send(&ToHost::Listen).await?;

    let help = crate::agents::help_text(&options.agy).await;
    let bridge = Arc::new(Bridge {
        agy: options.agy.clone(),
        help,
        host,
        dir,
        cwd,
        daemon: options.daemon.clone(),
        project: options.project.clone(),
        instructions: options.instructions.clone(),
        work: tokio::sync::RwLock::new(()),
        plan_cancel: tokio::sync::Notify::new(),
        state: Mutex::new(State {
            conversation: options.conversation.clone(),
            told: options.conversation.is_some(),
            model: options.options.model.clone(),
            effort: options.options.effort.clone(),
            permission: options.options.permission.clone(),
            ..State::default()
        }),
    });
    println!(
        "Antigravity {} · {}",
        crate::agents::version(&options.agy)
            .await
            .unwrap_or_default(),
        bridge.cwd.display()
    );
    if let Err(err) = bridge.add_tools(&options.executable).await {
        let text = format!("Antigravity doesn't have Sandhill's tools: {err:#}");
        println!("{text}");
        bridge.chat(json!({"kind": "error", "text": text})).await;
    }
    if let Some(model) = &options.options.model {
        bridge.set_model(Some(model));
    }
    if let Some(conversation) = &options.conversation {
        bridge.report(json!({"thread": conversation})).await;
    }
    // The conversation has started, as Claude Code's SessionStart hook says.
    let source = if options.conversation.is_some() {
        "resume"
    } else {
        "startup"
    };
    bridge
        .chat(json!({"kind": "session", "source": source, "model": options.options.model}))
        .await;
    tokio::spawn(bridge.clone().list_models());
    if let Some(prompt) = options.prompt.filter(|p| !p.trim().is_empty()) {
        tokio::spawn(bridge.clone().say(prompt, true));
    }

    let mut commands = BufReader::new(from_host).lines();
    let mut pending = std::collections::VecDeque::new();
    loop {
        let command = if let Some(command) = pending.pop_front() {
            Some(command)
        } else {
            match commands.next_line().await {
                Ok(Some(line)) => {
                    serde_json::from_str::<FromHost>(&line)
                        .ok()
                        .and_then(|message| {
                            if let FromHost::Command { command } = message {
                                Some(command)
                            } else {
                                None
                            }
                        })
                }
                _ => {
                    if let Some(pid) = bridge.state.lock().unwrap().pid {
                        signal_group(pid, libc::SIGKILL);
                    }
                    return Ok(1);
                }
            }
        };
        if let Some(command) = command {
            if command["kind"] == "view" {
                if command["mode"] == "terminal" {
                    pending.extend(bridge.clone().native(&mut commands).await);
                } else {
                    bridge.report(json!({"view_mode":"chat"})).await;
                }
            } else {
                tokio::spawn(bridge.clone().command(command));
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

#[derive(Default)]
struct State {
    /// agy's conversation, once it has said which.
    conversation: Option<String>,
    /// agy was told about Sandhill (with the conversation's first message).
    told: bool,
    /// The user's choices; `None` leaves agy's own.
    model: Option<String>,
    effort: Option<String>,
    permission: Option<String>,
    /// The turn agy is working on, and its process group.
    turn: Option<String>,
    pid: Option<u32>,
    /// The user stopped the turn.
    stopping: bool,
    /// Messages sent while a turn runs, for the next turn.
    queued: Vec<String>,
    /// Asks waiting for the user.
    waiting: usize,
    /// The status the app shows: model and effort.
    status: serde_json::Map<String, Value>,
    models: Value,
    /// Native owns the conversation while bridge turns are paused.
    paused: bool,
    view_mode: Option<&'static str>,
    /// Invalidates plan answers when ownership leaves the bridge.
    generation: u64,
}

/// A family must contain distinct, actually returned effort variants.
fn effort_variant(id: &str) -> Option<(&str, &str)> {
    let (base, effort) = id.rsplit_once('-')?;
    ["low", "medium", "high", "xhigh", "max"]
        .contains(&effort)
        .then_some((base, effort))
}

fn family_effort<'a>(models: &Value, id: &'a str) -> Option<&'a str> {
    let (base, effort) = effort_variant(id)?;
    let listed = models.as_array()?;
    if !listed.iter().any(|m| m["id"] == id) {
        return None;
    }
    listed
        .iter()
        .any(|m| {
            m["id"]
                .as_str()
                .and_then(effort_variant)
                .is_some_and(|(stem, level)| stem == base && level != effort)
        })
        .then_some(effort)
}

impl State {
    fn set_choices(&mut self, command: &Value) {
        if command.get("model").is_some() {
            self.model = command["model"].as_str().map(str::to_string);
        }
        if let Some(id) = self.model.as_deref()
            && family_effort(&self.models, id).is_some()
        {
            if let Some(level) = command["effort"].as_str() {
                let base = effort_variant(id).unwrap().0;
                if let Some(id) = self
                    .models
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["id"].as_str())
                    .find(|id| effort_variant(id) == Some((base, level)))
                {
                    self.model = Some(id.into());
                }
            }
            self.effort = None;
        } else if command.get("effort").is_some() {
            self.effort = command["effort"].as_str().map(str::to_string);
        }
        if command.get("permission").is_some() {
            self.permission = command["permission"].as_str().map(str::to_string);
        }
    }

    /// The chosen next-turn model wins over a late init event from the
    /// current turn, so status, resumed options and CLI arguments agree.
    fn model_status(&self) -> serde_json::Map<String, Value> {
        let mut status = self.status.clone();
        let id = self
            .model
            .as_deref()
            .or_else(|| status.get("model_id").and_then(Value::as_str));
        let effort = id
            .and_then(|id| family_effort(&self.models, id))
            .or(self.effort.as_deref())
            .map(str::to_string);
        if let Some(id) = self.model.as_deref() {
            let label = self
                .models
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["id"] == id)
                .map(|m| m["label"].clone())
                .unwrap_or_else(|| json!(model_label(id)));
            status.insert("model_id".into(), json!(id));
            status.insert("model".into(), label);
        }
        status.remove("effort");
        if let Some(effort) = effort {
            status.insert("effort".into(), json!(effort));
        }
        status
    }
}

struct Bridge {
    agy: PathBuf,
    /// `agy --help`, for the flags it takes.
    help: String,
    host: Host,
    /// The terminal's folder.
    dir: PathBuf,
    cwd: PathBuf,
    daemon: PathBuf,
    project: Option<String>,
    instructions: String,
    state: Mutex<State>,
    /// Held through process exit, including prompt/context preparation.
    work: tokio::sync::RwLock<()>,
    plan_cancel: tokio::sync::Notify,
}

/// One message's run of agy, as its events come.
#[derive(Default)]
struct Turn {
    id: String,
    /// Replies being written, by agy's step index.
    replies: Vec<Reply>,
    /// Tool steps shown and not ended, by step index: chat id and tool.
    steps: HashMap<i64, (String, String)>,
    ended: HashSet<i64>,
    /// agy's `result`, at the end.
    result: Option<Value>,
    /// The chat shows this turn: it ends with `stop`.
    shown: bool,
    /// Text sent as replies.
    text: String,
}

#[derive(Default)]
struct Reply {
    step: i64,
    /// Pieces sent so far.
    index: u64,
    pending: String,
    done: bool,
    finished: bool,
}

impl Bridge {
    /// Preserve legacy/user registrations and track our separate replacement.
    async fn add_tools(&self, executable: &Path) -> Result<()> {
        let (server, ready) = crate::lifecycle::antigravity_registration(executable)?;
        if ready {
            return Ok(());
        }
        let exe = executable.to_string_lossy();
        let (ok, said) = self
            .agy_command(&["mcp", "add", &server, "--", &exe, "mcp"])
            .await?;
        if !ok {
            bail!("`agy mcp add {server}` failed: {}", said.trim());
        }
        crate::lifecycle::record_antigravity_mcp(&server, executable)?;
        println!(
            "Added Sandhill's tools to Antigravity (`agy mcp remove {server}` takes them out)."
        );
        Ok(())
    }

    /// Runs one of agy's own commands: whether it succeeded, and what it said.
    async fn agy_command(&self, args: &[&str]) -> Result<(bool, String)> {
        let mut command = account_command(&self.agy);
        command
            .args(args)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        headless(&mut command);
        let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
            .await
            .with_context(|| format!("`agy {}` took too long", args.join(" ")))??;
        let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
        said.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.success(), said))
    }

    /// The models agy offers (`agy models`), for the model menu.
    async fn list_models(self: Arc<Self>) {
        let Ok((true, said)) = self.agy_command(&["models"]).await else {
            return;
        };
        let models = models(&said);
        if models.as_array().is_none_or(|m| m.is_empty()) {
            return;
        }
        let model = {
            let mut state = self.state.lock().unwrap();
            state.models = models;
            state
                .status
                .get("model_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        // Its label, now that agy has listed it.
        if model.is_some() {
            self.set_model(model.as_deref());
        }
        self.report(json!({})).await;
    }

    /// The model the status shows: its label once agy has listed it.
    fn set_model(&self, model: Option<&str>) {
        let mut state = self.state.lock().unwrap();
        let Some(model) = model else {
            state.status.remove("model");
            state.status.remove("model_id");
            return;
        };
        let label = state
            .models
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["id"] == model)
            .map(|m| m["label"].clone())
            .unwrap_or_else(|| json!(model_label(model)));
        state.status.insert("model_id".into(), json!(model));
        state.status.insert("model".into(), label);
    }

    /// What the user did in the chat.
    async fn command(self: Arc<Self>, command: Value) {
        match command["kind"].as_str().unwrap_or("") {
            "say" => {
                if let Some(text) = command["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.say(text.to_string(), true).await;
                }
            }
            "interrupt" => self.interrupt(),
            "set" => {
                self.state.lock().unwrap().set_choices(&command);
                let model = self.state.lock().unwrap().model.clone();
                self.set_model(model.as_deref());
                self.report(json!({})).await;
            }
            _ => {}
        }
    }

    /// Runs agy on the user's message, or keeps it for the next turn while
    /// one runs. `shown`: the message shows in the chat. Boxed: a turn
    /// starts the next one.
    fn say(self: Arc<Self>, text: String, shown: bool) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(self.say_now(text, shown))
    }

    async fn say_now(self: Arc<Self>, text: String, shown: bool) {
        let _work = self.work.read().await;
        let at_ms = now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let usage = text.trim() == "/usage";
        let (args, tell, planning) = {
            let mut state = self.state.lock().unwrap();
            if state.paused || state.turn.is_some() || state.waiting > 0 {
                state.queued.push(text);
                return;
            }
            state.turn = Some(id.clone());
            state.stopping = false;
            let tell = !usage && !state.told;
            if !usage {
                state.told = true;
            }
            let planning = !usage && state.permission.as_deref() == Some("plan");
            (turn_args(&self.help, &state), tell, planning)
        };
        let prompt = if shown && !usage {
            self.prompt().await
        } else {
            Default::default()
        };
        let mut input = String::new();
        if tell {
            input.push_str(&self.instructions);
            input.push_str("\n\n");
        }
        input.push_str(&text);
        if let Some(context) = &prompt.context {
            input.push_str("\n\n");
            input.push_str(context);
        }
        if prompt.report.as_object().is_some_and(|r| !r.is_empty()) {
            self.report(prompt.report.clone()).await;
        }
        let mut turn = Turn {
            id: id.clone(),
            shown,
            ..Turn::default()
        };
        if shown {
            println!("\n› {text}");
            self.chat(json!({"kind": "prompt", "turn": id, "text": text, "place": prompt.place, "at_ms": at_ms}))
                .await;
        }
        self.report(json!({"state": "busy"})).await;
        let errors = if usage {
            match usage::read_usage(&self.agy, Some(&self.cwd)).await {
                Ok(usage) => {
                    self.report(usage.report).await;
                    turn.result = Some(json!({"status":"SUCCESS", "response":usage.text}));
                    Vec::new()
                }
                Err(error) => vec![format!("{error:#}")],
            }
        } else {
            match self.run_agy(&mut turn, args, input).await {
                Ok(errors) => errors,
                Err(err) => vec![format!("{err:#}")],
            }
        };
        let plan = self.end_turn(&mut turn, &errors, planning).await;
        {
            let mut state = self.state.lock().unwrap();
            state.turn = None;
            state.pid = None;
            state.stopping = false;
        }
        match plan {
            // Register before releasing the turn's ownership guard.
            Some(plan) => {
                let generation = {
                    let mut state = self.state.lock().unwrap();
                    if state.paused {
                        None
                    } else {
                        state.waiting += 1;
                        Some(state.generation)
                    }
                };
                if let Some(generation) = generation {
                    tokio::spawn(self.clone().decide_plan(id, plan, generation));
                } else if turn.shown {
                    self.chat(json!({"kind":"stop","turn":id})).await;
                }
            }
            None => {
                // Idle before the turn shows as ended, so a client that sees
                // the end also sees the agent idle.
                self.report(json!({})).await;
                if turn.shown {
                    self.chat(json!({"kind": "stop", "turn": id})).await;
                }
                self.next();
            }
        }
    }

    /// Runs agy once and turns what it says into the chat; returns what it
    /// said on stderr.
    async fn run_agy(
        &self,
        turn: &mut Turn,
        args: Vec<String>,
        input: String,
    ) -> Result<Vec<String>> {
        let mut command = account_command(&self.agy);
        command
            .args(&args)
            .current_dir(&self.cwd)
            // For Sandhill's MCP server, which agy starts with its own
            // environment: the daemon and the project.
            .env("SANDHILL_SOCKET", &self.daemon)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own session and group: stopping it stops what it runs too.
        headless(&mut command);
        if let Some(project) = &self.project {
            command.env("SANDHILL_PROJECT", project);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.agy.display()))?;
        let stopping = {
            let mut state = self.state.lock().unwrap();
            state.pid = child.id();
            state.stopping
        };
        // Stopped while it was starting.
        if stopping && let Some(pid) = child.id() {
            signal_group(pid, libc::SIGINT);
        }
        let mut stdin = child.stdin.take().context("no stdin")?;
        let mut input = serde_json::to_vec(&json!({
            "event": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": input}],
            },
        }))?;
        input.push(b'\n');
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let stderr = child.stderr.take().context("no stderr")?;
        let said = Arc::new(Mutex::new(Vec::new()));
        let errors = tokio::spawn({
            let said = said.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("  {line}");
                    said.lock().unwrap().push(line);
                }
            }
        });
        let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
        let mut tick = tokio::time::interval(FLUSH);
        // agy says `init` as soon as it is up. Silence means it is stuck
        // before the message (a sign-in waiting for a browser, say): it is
        // stopped, and the turn fails with what it said.
        let start = tokio::time::sleep(START_TIMEOUT);
        tokio::pin!(start);
        let mut started = false;
        let mut stalled = false;
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    started = true;
                    self.event(turn, &line).await;
                }
                _ = tick.tick() => self.flush(turn).await,
                _ = &mut start, if !started => {
                    if let Some(pid) = child.id() {
                        signal_group(pid, libc::SIGKILL);
                    }
                    stalled = true;
                    break;
                }
            }
        }
        let status = child.wait().await?;
        // What it runs may keep stderr open after it ends.
        let _ = tokio::time::timeout(STOP_GRACE, errors).await;
        let mut said = std::mem::take(&mut *said.lock().unwrap());
        if stalled {
            let text = stalled_text(&said);
            said.push(text);
        } else if turn.result.is_none() && !status.success() && said.is_empty() {
            said.push(format!("agy exited ({status})"));
        }
        Ok(said)
    }

    /// One line of agy's NDJSON: `{"event": kind, kind: {...}}`.
    async fn event(&self, turn: &mut Turn, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            if !line.trim().is_empty() {
                println!("{line}");
            }
            return;
        };
        let kind = message["event"].as_str().unwrap_or("");
        let body = &message[kind];
        match kind {
            "init" => {
                // Known from the start, so the command line can open the
                // conversation while its first message runs.
                self.conversation(&message["conversation_id"]).await;
                if let Some(model) = body["model"].as_str() {
                    self.set_model(Some(model));
                    self.report(json!({})).await;
                }
            }
            "step_update" => self.step(turn, body).await,
            "result" => {
                self.conversation(&body["conversation_id"]).await;
                turn.result = Some(body.clone());
            }
            _ => {}
        }
    }

    async fn step(&self, turn: &mut Turn, step: &Value) {
        self.conversation(&step["conversation_id"]).await;
        let index = step["step_index"].as_i64().unwrap_or(-1);
        let done = step["state"] == "DONE";
        match step["step_type"].as_str().unwrap_or("") {
            "agent_response" => {
                let at = match turn.replies.iter().position(|r| r.step == index) {
                    Some(at) => at,
                    None => {
                        turn.replies.push(Reply {
                            step: index,
                            ..Reply::default()
                        });
                        turn.replies.len() - 1
                    }
                };
                let reply = &mut turn.replies[at];
                if let Some(delta) = step["text_delta"].as_str() {
                    reply.pending.push_str(delta);
                    turn.text.push_str(delta);
                }
                if done {
                    reply.done = true;
                    self.flush(turn).await;
                }
            }
            "tool" => {
                let info = &step["tool_info"];
                if !turn.steps.contains_key(&index) && !turn.ended.contains(&index) {
                    // What it wrote before the step shows above it.
                    self.flush(turn).await;
                    let name = info["name"]
                        .as_str()
                        .or(step["tool_name"].as_str())
                        .unwrap_or("tool");
                    let (tool, input) = tool_step(name, &info["parameters"]);
                    println!("  ▸ {tool} {}", one_line(&input));
                    let id = format!("{}-{index}", turn.id);
                    turn.shown = true;
                    self.chat(json!({"kind": "step", "turn": turn.id, "id": id, "tool": tool, "input": input}))
                        .await;
                    turn.steps.insert(index, (id, tool));
                }
                if done && let Some((id, tool)) = turn.steps.remove(&index) {
                    turn.ended.insert(index);
                    let mut end = step_end(step, &tool);
                    if end["kind"] == "step_failed" {
                        println!("    failed");
                    }
                    end["turn"] = json!(turn.id);
                    end["id"] = json!(id);
                    self.chat(end).await;
                }
            }
            _ => {}
        }
    }

    /// Notes the conversation agy says it is in, for the next message and
    /// for going on with it after a restart.
    async fn conversation(&self, id: &Value) {
        let Some(id) = id.as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.conversation.as_deref() == Some(id) {
                return;
            }
            state.conversation = Some(id.to_string());
        }
        self.report(json!({"thread": id})).await;
    }

    /// Sends what agy has written of its replies since the last time.
    async fn flush(&self, turn: &mut Turn) {
        let mut pieces = Vec::new();
        for reply in turn.replies.iter_mut() {
            let ending = reply.done && !reply.finished;
            if reply.pending.is_empty() && !ending {
                continue;
            }
            pieces.push((
                reply.step,
                reply.index,
                std::mem::take(&mut reply.pending),
                ending,
            ));
            reply.index += 1;
            reply.finished |= ending;
        }
        for (step, index, text, last) in pieces {
            turn.shown = true;
            self.chat(json!({
                "kind": "text",
                "turn": turn.id,
                "message": format!("{}-r{step}", turn.id),
                "index": index,
                "text": text,
                "final": last,
            }))
            .await;
        }
    }

    /// Ends the turn in the chat: the rest of the reply, steps that never
    /// ended, and what went wrong. Returns the plan to put to the user, in
    /// plan mode.
    async fn end_turn(&self, turn: &mut Turn, errors: &[String], planning: bool) -> Option<String> {
        for reply in turn.replies.iter_mut() {
            reply.done = true;
        }
        self.flush(turn).await;
        let result = turn.result.clone().unwrap_or(Value::Null);
        let status = result["status"].as_str().unwrap_or("");
        let stopped =
            self.state.lock().unwrap().stopping || matches!(status, "CANCELED" | "INTERRUPTED");
        let open: Vec<(String, String)> = turn.steps.drain().map(|(_, step)| step).collect();
        for (id, tool) in open {
            self.chat(json!({"kind": "step_failed", "turn": turn.id, "id": id, "tool": tool, "error": "Stopped", "interrupted": stopped}))
                .await;
        }
        let response = result["response"].as_str().unwrap_or("");
        if turn.text.trim().is_empty() && !response.trim().is_empty() {
            turn.shown = true;
            self.chat(json!({"kind": "text", "turn": turn.id, "message": format!("{}-r", turn.id), "index": 0, "text": response, "final": true}))
                .await;
        }
        if !response.is_empty() || !turn.text.is_empty() {
            println!(
                "{}",
                if response.is_empty() {
                    &turn.text
                } else {
                    response
                }
                .trim_end()
            );
        }
        let mut problems = Vec::new();
        if !stopped && status != "SUCCESS" {
            problems.push(failure(&result, errors));
        }
        if let Some(denied) = denials(&result, errors) {
            problems.push(denied);
        }
        for text in problems {
            turn.shown = true;
            println!("  Error: {text}");
            self.chat(json!({"kind": "error", "turn": turn.id, "text": text}))
                .await;
        }
        let plan = if response.trim().is_empty() {
            turn.text.trim()
        } else {
            response.trim()
        };
        (planning && status == "SUCCESS" && !plan.is_empty()).then(|| plan.to_string())
    }

    /// Starts the messages that came while the turn ran.
    fn next(self: &Arc<Self>) {
        let queued = {
            let mut state = self.state.lock().unwrap();
            if state.paused {
                return;
            }
            std::mem::take(&mut state.queued)
        };
        if !queued.is_empty() {
            tokio::spawn(self.clone().say(queued.join("\n\n"), true));
        }
    }

    /// Stops the turn: agy is asked to (SIGINT), then made to.
    fn interrupt(self: Arc<Self>) {
        self.stop_active(true);
    }

    fn stop_active(self: &Arc<Self>, clear_queue: bool) {
        let pid = {
            let mut state = self.state.lock().unwrap();
            if clear_queue {
                state.queued.clear();
            }
            if state.turn.is_none() {
                return;
            }
            state.stopping = true;
            state.pid
        };
        let Some(pid) = pid else { return };
        signal_group(pid, libc::SIGINT);
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(STOP_GRACE).await;
            if this.state.lock().unwrap().pid == Some(pid) {
                signal_group(pid, libc::SIGKILL);
            }
        });
    }

    /// Asks the user whether to carry out the plan agy made in plan mode,
    /// as Claude Code's plan card does; yes switches to Normal and carries
    /// it out.
    async fn decide_plan(self: Arc<Self>, turn: String, plan: String, generation: u64) {
        let cancelled = self.plan_cancel.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        if self.state.lock().unwrap().generation != generation {
            return;
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let event = json!({
            "kind": "ask",
            "turn": turn,
            "id": id,
            "tool": "ExitPlanMode",
            "input": {"plan": plan},
            "at_ms": now_ms(),
        });
        self.report(json!({})).await;
        let answer = tokio::select! {
            answer = super::ask(&self.dir, &id, event) => answer.ok().flatten(),
            _ = &mut cancelled => {
                self.chat(json!({"kind":"stop","turn":turn})).await;
                return;
            }
        };
        let carry_out = answer.as_ref().is_some_and(|a| a["behavior"] == "allow");
        {
            let mut state = self.state.lock().unwrap();
            if state.generation != generation || state.paused {
                return;
            }
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

    /// What goes with the user's message, as Claude Code's prompt hook
    /// gives it: the text for the agent, and the place for the chat.
    async fn prompt(&self) -> crate::mcp::Prompt {
        let tools = crate::mcp::Tools::new(self.daemon.clone(), self.project.clone())
            .in_terminal(Some(self.dir.clone()));
        tools.prompt(PLACE_TIMEOUT).await
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
            let status = state.model_status();
            let mut report = json!({
                "state": if state.waiting > 0 { "waiting" } else if state.turn.is_some() { "busy" } else { "idle" },
                "permission": state.permission.as_deref().unwrap_or("supervised"),
                "status": status,
                "view_mode": state.view_mode.unwrap_or("chat"),
                // Why the command line failed: only in the report that
                // ends its switch back to chat.
                "view_error": null,
                "thread": state.conversation,
            });
            if !state.models.is_null() {
                report["models"] = state.models.clone();
            }
            if state.waiting > 0 && extra.get("message").is_none() {
                report["message"] = json!("Antigravity is waiting for you");
            }
            for (key, value) in extra.as_object().into_iter().flatten() {
                report[key] = value.clone();
            }
            report
        };
        let _ = self.host.send(&ToHost::Agent { value: report }).await;
    }
}

/// Select the added account for this process only. agy merges this provider
/// override with its other settings; the key stays in inherited GEMINI_API_KEY.
fn account_command(agy: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(agy);
    command.args(account_settings(api_key_account()));
    command
}

fn account_settings(api_key: bool) -> &'static [&'static str] {
    if api_key {
        &["--settings", r#"{"modelProvider":"gemini"}"#]
    } else {
        &[]
    }
}

fn api_key_account() -> bool {
    std::env::var_os("SANDHILL_AGY_API_KEY").as_deref() == Some(std::ffi::OsStr::new("1"))
}

/// agy's arguments for one message, with the user's choices. Flags an
/// older agy doesn't list in its help are left out.
fn turn_args(help: &str, state: &State) -> Vec<String> {
    let has = |flag: &str| help.contains(flag);
    // Output formatting alone leaves agy in its interactive TUI. Streamed
    // input selects headless mode and keeps the prompt out of process argv.
    let mut args = vec![
        "--input-format".to_string(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
    ];
    if has("--print-timeout") {
        args.extend(["--print-timeout".into(), PRINT_TIMEOUT.into()]);
    }
    args.extend(conversation_args(help, state));
    args
}

/// Choices shared by headless turns and the native CLI.
fn conversation_args(help: &str, state: &State) -> Vec<String> {
    let has = |flag: &str| help.contains(flag);
    let mut args = Vec::new();
    if let Some(model) = &state.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &state.effort
        && has("--effort")
        && state
            .model
            .as_deref()
            .and_then(|id| family_effort(&state.models, id))
            .is_none()
    {
        args.extend(["--effort".into(), effort.clone()]);
    }
    if let Some(conversation) = &state.conversation {
        args.extend(["--conversation".into(), conversation.clone()]);
    }
    match state.permission.as_deref() {
        Some("full") => args.push("--dangerously-skip-permissions".into()),
        Some("plan") if has("--mode") => args.extend(["--mode".into(), "plan".into()]),
        Some("edits") if has("--mode") => args.extend(["--mode".into(), "accept-edits".into()]),
        _ => {}
    }
    args
}

/// A tool step as the chat shows it: Sandhill's tools by their MCP names,
/// and agy's commands and new files as Claude Code's are; the rest as agy
/// names them.
fn tool_step(name: &str, parameters: &Value) -> (String, Value) {
    if let Some(tool) = sandhill_tool(name) {
        return (format!("mcp__{SERVER}__{tool}"), shorten(parameters));
    }
    match name {
        "run_command" if parameters["CommandLine"].is_string() => {
            ("Bash".into(), json!({"command": parameters["CommandLine"]}))
        }
        "write_to_file" if parameters["TargetFile"].is_string() => (
            "Write".into(),
            shorten(
                &json!({"file_path": parameters["TargetFile"], "content": parameters["CodeContent"]}),
            ),
        ),
        _ => (name.to_string(), shorten(parameters)),
    }
}

/// "add_cell" for one of Sandhill's tools, however agy writes its name:
/// with the server first, joined by `__`, `_`, `/` or `.`.
fn sandhill_tool(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp").unwrap_or(name);
    let rest = rest.trim_start_matches(['_', '/', '.']);
    let rest = rest
        .strip_prefix("sandhill-managed")
        .or_else(|| rest.strip_prefix(SERVER))?;
    let tool = rest.trim_start_matches(['_', '/', '.']);
    crate::mcp::tool_names()
        .iter()
        .any(|t| t == tool)
        .then(|| tool.to_string())
}

/// How a tool step ended: `step_done` with its output, or `step_failed`
/// with agy's error. Without the turn and the step's id.
fn step_end(step: &Value, tool: &str) -> Value {
    let info = &step["tool_info"];
    let ms = step["duration_seconds"]
        .as_f64()
        .map(|s| (s * 1000.0).round() as u64);
    let error = &info["error"];
    if !error.is_null() {
        let message = error["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                error
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or(error.to_string())
            });
        return json!({"kind": "step_failed", "tool": tool, "error": shorten(&json!(message)), "ms": ms});
    }
    let output = &info["output"];
    let output = match tool {
        "Bash" => {
            json!({"stdout": output.as_str().map(str::to_string).unwrap_or_else(|| text_of(output))})
        }
        _ => output.clone(),
    };
    json!({"kind": "step_done", "tool": tool, "output": shorten(&output), "ms": ms})
}

fn text_of(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}

/// Why a turn failed: agy's error in its result, else what it said on
/// stderr (`AGY_ERROR: {...}` when it can say), with how to sign in when
/// that is what it needs.
fn failure(result: &Value, errors: &[String]) -> String {
    let error = &result["error"];
    let mut text = error["message"]
        .as_str()
        .or(error.as_str())
        .map(str::to_string)
        .or_else(|| {
            errors.iter().find_map(|line| {
                let said = line.strip_prefix("AGY_ERROR:")?.trim();
                let value: Value = serde_json::from_str(said).ok()?;
                Some(
                    value["message"]
                        .as_str()
                        .or(value["error"].as_str())
                        .map(str::to_string)
                        .unwrap_or(said.to_string()),
                )
            })
        })
        .or_else(|| {
            errors
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_else(|| "Antigravity stopped without replying".to_string());
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    let lower = text.to_lowercase();
    if [
        "auth",
        "sign in",
        "signed in",
        "login",
        "log in",
        "keyring",
        "credential",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        text.push_str(
            " Sign Antigravity in by running `agy` once in a terminal on this machine, or set it up with a Gemini API key.",
        );
    }
    text
}

/// What agy turned down because it couldn't ask, from its result's
/// `denied_actions` or its notices on stderr, as one line for the chat.
fn denials(result: &Value, errors: &[String]) -> Option<String> {
    let mut denied: Vec<String> = result["denied_actions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| a.as_str().map(str::to_string).unwrap_or(a.to_string()))
        .collect();
    if denied.is_empty() {
        denied = errors
            .iter()
            .filter(|l| {
                let lower = l.to_lowercase();
                (lower.contains("denied") || lower.contains("not allowed"))
                    && !l.starts_with("AGY_ERROR:")
            })
            .map(|l| l.trim().to_string())
            .collect();
    }
    if denied.is_empty() {
        return None;
    }
    Some(format!(
        "Antigravity can't ask for permission here, so it was turned down: {}. Switch to YOLO to let it, or allow it in Antigravity's own settings.",
        denied.join("; ")
    ))
}

/// The models agy offers, before any conversation (`agy models`), for a
/// new conversation's model menu.
pub async fn list_models(agy: &Path) -> Result<Value> {
    let mut command = account_command(agy);
    command
        .arg("models")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    headless(&mut command);
    if let Some(path) = crate::agents::path_for(agy)? {
        command.env("PATH", path);
    }
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .context("`agy models` took too long")??;
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        bail!("`agy models` failed: {}", said.trim());
    }
    Ok(models(&said))
}

/// The models `agy models` lists, one slug at the start of each line:
/// `{id, label, default}`.
fn models(said: &str) -> Value {
    let mut models = Vec::new();
    for line in said.lines() {
        let line = line.trim_start_matches([' ', '\t', '*', '-', '•', '>']);
        let Some(slug) = line.split_whitespace().next() else {
            continue;
        };
        if !is_slug(slug) || models.iter().any(|m: &Value| m["id"] == slug) {
            continue;
        }
        let default = line.to_lowercase().contains("default");
        models.push(json!({"id": slug, "label": model_label(slug), "default": default}));
    }
    Value::Array(models)
}

/// "gemini-3.6-flash-high", "claude-sonnet-4-6".
fn is_slug(word: &str) -> bool {
    let mut parts = word.split(['-', '.']);
    let first = parts.next().unwrap_or("");
    first.starts_with(|c: char| c.is_ascii_lowercase())
        && word.contains(['-', '.'])
        && word.split(['-', '.']).all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// "Gemini 3.6 Flash High" for Gemini's slugs; others as agy names them.
fn model_label(slug: &str) -> String {
    if !slug.starts_with("gemini-") {
        return slug.to_string();
    }
    slug.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A signal to agy and what it runs (its own process group).
fn signal_group(pid: u32, signal: libc::c_int) {
    // SAFETY: plain syscall; agy was started as the leader of its group.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// Runs agy away from the conversation's terminal: in a session of its
/// own, without a controlling terminal, as the leader of its own process
/// group (for [`signal_group`]). Over SSH, agy on Linux asks the terminal
/// what it is (DA2) even when it only prints; from a background process
/// group of the conversation's terminal that stops it for good (SIGTTOU),
/// and from its foreground the question shows in the terminal.
fn headless(command: &mut tokio::process::Command) {
    // SAFETY: setsid is async-signal-safe, as code between fork and exec
    // must be.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Why a message failed whose agy never started: how long it had, and the
/// first thing it said (agy asks for a sign-in on stderr, then waits).
fn stalled_text(said: &[String]) -> String {
    let mut text = format!(
        "Antigravity didn't start within {} s, so Sandhill stopped it",
        START_TIMEOUT.as_secs()
    );
    if let Some(line) = said.iter().map(|l| l.trim()).find(|l| !l.is_empty()) {
        let line = line.trim_end_matches(':');
        let line = match line.char_indices().nth(200) {
            Some((at, _)) => format!("{}…", &line[..at]),
            None => line.to_string(),
        };
        text.push_str(". It said: ");
        text.push_str(&line);
    }
    text
}

/// A step's input on one line, for the terminal.
fn one_line(input: &Value) -> String {
    let text = input["command"]
        .as_str()
        .or(input["file_path"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string());
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(100) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_accounts_override_only_the_provider_without_a_key_argument() {
        assert!(account_settings(false).is_empty());
        let args = account_settings(true);
        assert_eq!(args[0], "--settings");
        let settings: Value = serde_json::from_str(args[1]).unwrap();
        assert_eq!(settings, json!({"modelProvider": "gemini"}));
    }

    fn variant_listing() -> Value {
        json!([
            {"id": "gemini-3.8-flash-high", "label": "Gemini 3.8 Flash High"},
            {"id": "gemini-3.8-flash-medium", "label": "Gemini 3.8 Flash Medium"},
            {"id": "gemini-3.8-flash-low", "label": "Gemini 3.8 Flash Low"},
            {"id": "claude-sonnet-4-6", "label": "Claude Sonnet 4.6"}
        ])
    }

    #[test]
    fn variant_effort_changes_route_cli_model_and_status_together() {
        let mut state = State {
            model: Some("gemini-3.8-flash-high".into()),
            // Old stored options must not override an encoded variant.
            effort: Some("medium".into()),
            models: variant_listing(),
            ..State::default()
        };
        assert_eq!(state.model_status()["effort"], "high");
        let args = turn_args("--model --effort", &state);
        assert!(
            args.windows(2)
                .any(|a| a == ["--model", "gemini-3.8-flash-high"])
        );
        assert!(!args.iter().any(|a| a == "--effort"));
        for level in ["low", "medium", "high"] {
            state.set_choices(&json!({"effort": level}));
            let id = format!("gemini-3.8-flash-{level}");
            assert_eq!(state.model.as_deref(), Some(id.as_str()));
            let status = state.model_status();
            assert_eq!(status["model_id"], id);
            assert_eq!(status["effort"], level);
            assert_eq!(state.effort, None);
            let args = turn_args("--model --effort", &state);
            assert!(args.windows(2).any(|a| a[0] == "--model" && a[1] == id));
            assert!(!args.iter().any(|a| a == "--effort"));
        }
        // A late init event from the active turn cannot undo a next-turn pick.
        state
            .status
            .insert("model_id".into(), json!("gemini-3.8-flash-low"));
        assert_eq!(state.model_status()["model_id"], "gemini-3.8-flash-high");
        state.set_choices(&json!({"effort": "max"}));
        assert_eq!(state.model.as_deref(), Some("gemini-3.8-flash-high"));
        state.set_choices(&json!({"model": "claude-sonnet-4-6", "effort": "high"}));
        let args = turn_args("--model --effort", &state);
        assert!(args.windows(2).any(|a| a == ["--effort", "high"]));
        assert_eq!(state.model_status()["effort"], "high");
    }

    #[test]
    fn singleton_suffix_and_similar_model_ids_are_not_variant_families() {
        let mut state = State {
            model: Some("gemini-3.8-flash-high".into()),
            models: json!([
                {"id": "gemini-3.8-flash-high"},
                {"id": "gemini-3.8-flash-lite-low"},
                {"id": "gemini-3.8-flash-high-preview"}
            ]),
            ..State::default()
        };
        state.set_choices(&json!({"effort": "low"}));
        assert_eq!(state.model.as_deref(), Some("gemini-3.8-flash-high"));
        assert_eq!(state.effort.as_deref(), Some("low"));
        let args = turn_args("--effort", &state);
        assert!(args.windows(2).any(|a| a == ["--effort", "low"]));
    }

    #[test]
    fn each_message_runs_agy_headless_with_the_users_choices() {
        let help = "--output-format --print-timeout --effort --mode";
        let state = State {
            model: Some("gemini-3.6-flash-high".into()),
            effort: Some("high".into()),
            conversation: Some("c1".into()),
            permission: Some("plan".into()),
            ..State::default()
        };
        assert_eq!(
            turn_args(help, &state),
            [
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--print-timeout",
                "24h",
                "--model",
                "gemini-3.6-flash-high",
                "--effort",
                "high",
                "--conversation",
                "c1",
                "--mode",
                "plan"
            ]
        );
        // YOLO lets everything through; Normal leaves agy's own rules.
        let full = State {
            permission: Some("full".into()),
            ..State::default()
        };
        assert!(turn_args("", &full).contains(&"--dangerously-skip-permissions".to_string()));
        assert_eq!(
            turn_args("", &State::default()),
            [
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json"
            ]
        );
    }

    #[test]
    fn steps_show_as_the_chat_knows_them() {
        for name in [
            "mcp__sandhill__add_cell",
            "mcp_sandhill_add_cell",
            "sandhill/add_cell",
            "sandhill.add_cell",
        ] {
            let (tool, input) = tool_step(name, &json!({"code": "1"}));
            assert_eq!(tool, "mcp__sandhill__add_cell", "{name}");
            assert_eq!(input["code"], "1");
        }
        assert_eq!(
            tool_step("sandhill_nothing", &json!({})).0,
            "sandhill_nothing"
        );
        let (tool, input) = tool_step("run_command", &json!({"CommandLine": "ls", "Cwd": "/p"}));
        assert_eq!((tool.as_str(), &input), ("Bash", &json!({"command": "ls"})));
        assert_eq!(
            tool_step("view_file", &json!({"AbsolutePath": "/a"})).0,
            "view_file"
        );

        let done = json!({"tool_info": {"name": "run_command", "output": "hi\n"}, "duration_seconds": 0.25});
        assert_eq!(
            step_end(&done, "Bash"),
            json!({"kind": "step_done", "tool": "Bash", "output": {"stdout": "hi\n"}, "ms": 250})
        );
        let failed = json!({"tool_info": {"name": "run_command", "error": {"type": "PERMISSION", "message": "Needs approval"}}});
        assert_eq!(step_end(&failed, "Bash")["error"], "Needs approval");
    }

    #[test]
    fn failures_say_why_and_how_to_sign_in() {
        let result = json!({"status": "ERROR", "error": {"message": "model not found"}});
        assert_eq!(failure(&result, &[]), "model not found.");
        let errors = ["AGY_ERROR: {\"message\": \"authentication required\"}".to_string()];
        assert!(
            failure(&Value::Null, &errors)
                .starts_with("authentication required. Sign Antigravity in")
        );
        assert_eq!(failure(&Value::Null, &["boom".into()]), "boom.");

        let notices = ["Tool run_command was denied: allow it with command(ls)".to_string()];
        assert!(
            denials(&json!({}), &notices)
                .unwrap()
                .contains("run_command was denied")
        );
        assert!(
            denials(&json!({"denied_actions": ["command(rm)"]}), &[])
                .unwrap()
                .contains("command(rm)")
        );
        assert_eq!(denials(&json!({}), &["warning: slow".into()]), None);
    }

    #[test]
    fn models_come_from_agy_models() {
        let said = "Available models:\n  gemini-3.6-flash-high (default)\n  gemini-3.1-pro-low   Gemini 3.1 Pro\n* claude-sonnet-4-6\nRun agy --model <slug>\n";
        assert_eq!(
            models(said),
            json!([
                {"id": "gemini-3.6-flash-high", "label": "Gemini 3.6 Flash High", "default": true},
                {"id": "gemini-3.1-pro-low", "label": "Gemini 3.1 Pro Low", "default": false},
                {"id": "claude-sonnet-4-6", "label": "claude-sonnet-4-6", "default": false},
            ])
        );
    }
}
