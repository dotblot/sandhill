//! Generic stdio ACP chat transport, for OpenScience (`openscience acp`) and
//! Oh My Pi (`omp acp`). Provider configuration is confined to process
//! startup; updates use the same durable chat and guard as other agents.
//!
//! Sandhill's MCP tools are served over loopback HTTP with a per-conversation
//! bearer token (`crate::mcp::http`): OpenScience reads the server from its
//! configuration, omp from the ACP session request. Models and thinking
//! effort come from the session's `models` (ACP's unstable model API) or its
//! `configOptions` (model and `thought_level` selects). Oh My Pi's own screen
//! can take over the conversation (`view` commands), as Codex's does.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::AgentOptions;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::mpsc;

use super::{FromHost, Paths, ToHost, chat::shorten, guard};
use crate::agents::{OMP, OPENSCIENCE};
use crate::journal::now_ms;

pub struct Options {
    pub agent: String,
    pub program: PathBuf,
    pub executable: PathBuf,
    pub daemon: PathBuf,
    pub project: Option<String>,
    pub options: AgentOptions,
    pub prompt: Option<String>,
    pub session: Option<String>,
}

const INSTRUCTIONS: &str = "You are running in Sandhill. Run analysis code through Sandhill's run_code tool in the user's live R/Python session; plots and variables stay in Sandhill. Prefer Sandhill tools get_focus, list_variables, view_table and get_viewer to inspect the user's work. Do not start separate Python or R processes for analysis.";
const FLUSH: Duration = Duration::from_millis(120);
/// How long listing models may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the ACP agent gets to release its session before the command
/// line takes it.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);
/// Thinking levels, lowest first, as these agents name them. omp's `off`
/// and `auto` are not levels the effort slider offers.
const LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
/// Oh My Pi's command-line extension that offers Sandhill's tools.
const OMP_EXTENSION: &str = include_str!("omp-sandhill.js");

type Messages = Lines<BufReader<ChildStdout>>;
type Commands = Lines<BufReader<OwnedReadHalf>>;

/// The agent's name in messages.
fn display(agent: &str) -> &'static str {
    match agent {
        OMP => "Oh My Pi",
        _ => "OpenScience",
    }
}

/// Lists models without starting a session or spending a prompt:
/// OpenScience's `models --flat`, omp's `models --json` with its default
/// model and thinking level (`config get`).
pub async fn list_models(agent: &str, program: &Path) -> Result<Value> {
    if agent == OMP {
        // The catalogue first: in a new home omp creates its databases on
        // first use, which concurrent first runs may contend for.
        let models = cli(agent, program, &["models", "--json"]).await?;
        let models: Value =
            serde_json::from_str(&models).context("Oh My Pi listed models in an unknown form")?;
        let (roles, thinking) = tokio::join!(
            cli(agent, program, &["config", "get", "modelRoles", "--json"]),
            cli(
                agent,
                program,
                &["config", "get", "defaultThinkingLevel", "--json"]
            ),
        );
        let setting = |output: Result<String>| {
            output
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .map(|setting| setting["value"].clone())
                .unwrap_or_default()
        };
        return Ok(omp_models(
            &models,
            &setting(roles),
            setting(thinking).as_str(),
        ));
    }
    Ok(parse_models(
        &cli(agent, program, &["models", "--flat"]).await?,
    ))
}

async fn cli(agent: &str, program: &Path, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(Stdio::null()).kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(program)? {
        command.env("PATH", path);
    }
    let output = tokio::time::timeout(LIST_TIMEOUT, command.output())
        .await
        .with_context(|| format!("{} took too long to list models", display(agent)))??;
    if !output.status.success() {
        bail!(
            "{} couldn't list models: {}",
            display(agent),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_models(text: &str) -> Value {
    let mut seen = HashSet::new();
    json!(
        text.lines()
            .map(str::trim)
            .filter(|line| {
                line.split_once('/')
                    .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
                    && !line.chars().any(char::is_whitespace)
                    && seen.insert((*line).to_string())
            })
            .map(|id| json!({"id": id, "label": id}))
            .collect::<Vec<_>>()
    )
}

/// omp's chat models (`provider/model` selectors) with their thinking
/// levels. The default model is `modelRoles.default`, which may carry its
/// level (`provider/model:high`); other models start at
/// `defaultThinkingLevel`.
fn omp_models(listing: &Value, roles: &Value, thinking: Option<&str>) -> Value {
    let role = roles["default"].as_str().unwrap_or_default();
    let (default, default_effort) = match role.rsplit_once(':') {
        Some((model, effort)) if LEVELS.contains(&effort) => (model, Some(effort)),
        _ => (role, None),
    };
    let entries = listing["models"].as_array().into_iter().flatten();
    let selectors: HashSet<&str> = entries
        .clone()
        .filter_map(|model| model["selector"].as_str())
        .collect();
    let mut seen = HashSet::new();
    let mut models: Vec<Value> = entries
        .filter(|model| model["kind"].as_str().is_none_or(|kind| kind == "chat"))
        .filter_map(|model| {
            let id = model["selector"].as_str()?;
            // A dated snapshot listed beside its alias is the same model
            // under the same name: show the alias once.
            if id != default && undated(id).is_some_and(|alias| selectors.contains(alias)) {
                return None;
            }
            if !seen.insert(id) {
                return None;
            }
            let efforts = levels(&model["thinking"]);
            let effort = default_effort
                .filter(|_| id == default)
                .or(thinking)
                .filter(|effort| efforts.contains(effort));
            Some(json!({
                "id": id,
                "label": model["name"].as_str().unwrap_or(id),
                "efforts": efforts,
                "effort": effort,
                "default": id == default,
            }))
        })
        .collect();
    name_providers(&mut models);
    json!(models)
}

/// The alias a dated snapshot selector belongs to:
/// `anthropic/claude-haiku-4-5-20251001` → `anthropic/claude-haiku-4-5`.
fn undated(id: &str) -> Option<&str> {
    let (alias, date) = id.rsplit_once('-')?;
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit())).then_some(alias)
}

/// The thinking levels in `list` the slider offers, lowest first.
fn levels(list: &Value) -> Vec<&'static str> {
    LEVELS
        .into_iter()
        .filter(|level| {
            list.as_array()
                .into_iter()
                .flatten()
                .any(|offered| offered == level)
        })
        .collect()
}

/// One model is often offered by several providers under the same name:
/// those labels say which (`Claude Opus 5.5 (openrouter)`).
fn name_providers(models: &mut [Value]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for model in models.iter() {
        *counts
            .entry(model["label"].as_str().unwrap_or_default().to_string())
            .or_default() += 1;
    }
    for model in models.iter_mut() {
        let label = model["label"].as_str().unwrap_or_default().to_string();
        if counts[&label] > 1
            && let Some((provider, _)) = model["id"].as_str().and_then(|id| id.split_once('/'))
        {
            model["label"] = json!(format!("{label} ({provider})"));
        }
    }
}

enum Request {
    Initialize,
    Session,
    Model(String),
    /// `session/set_config_option` for the model or thinking selects.
    Config,
    Mode(String),
    Prompt,
}

struct Approval {
    id: Value,
    options: Value,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Stream {
    kind: String,
    id: String,
    pending: String,
    text: String,
    index: u64,
}

struct Bridge {
    options: Options,
    dir: PathBuf,
    cwd: PathBuf,
    /// The agent process's extra environment, kept to start it again after
    /// its command line.
    env: Vec<(String, String)>,
    /// Sandhill's MCP server for ACP session requests (omp), or none.
    mcp: Value,
    /// Its address and token, for omp's command line.
    mcp_url: String,
    mcp_token: String,
    stdin: ChildStdin,
    host: OwnedWriteHalf,
    next: i64,
    pending: HashMap<i64, Request>,
    session: String,
    ready: bool,
    loading: bool,
    replay_skip: bool,
    replay_user: bool,
    replay_prompt: String,
    history: HashSet<String>,
    number: u64,
    turn: Option<String>,
    busy: bool,
    stopping: bool,
    queued: VecDeque<String>,
    stream: Stream,
    serial: u64,
    /// Every tool call seen, merged from its updates, so a later update
    /// (OpenScience sends some after completion) keeps its title.
    steps: HashMap<String, Value>,
    /// Tool calls already shown as a step, and those already finished: the
    /// chat adds a row per `step` entry, so each is sent once.
    shown_steps: HashSet<String>,
    finished_steps: HashSet<String>,
    approvals: HashMap<String, Approval>,
    answers: mpsc::UnboundedSender<(String, Option<Value>)>,
    models: Value,
    modes: Value,
    model: Option<String>,
    mode: Option<String>,
    selected_model: Option<String>,
    selected_mode: Option<String>,
    /// The session's model select (`configOptions`), when it has one.
    model_option: Option<String>,
    /// The session's thinking select, its levels and current one.
    effort_option: Option<String>,
    efforts: Vec<&'static str>,
    effort: Option<String>,
    selected_effort: Option<String>,
    /// Whether this session was told about Sandhill (omp has no system
    /// prompt over ACP, so its first message carries it).
    instructed: bool,
    view_mode: &'static str,
}

pub async fn run(options: Options) -> Result<i32> {
    if options.agent != OPENSCIENCE && options.agent != OMP {
        bail!("unsupported ACP agent {:?}", options.agent);
    }
    let dir = std::env::var_os("SANDHILL_TERMINAL")
        .map(PathBuf::from)
        .context("acp-chat runs in a Sandhill terminal")?;
    let cwd = std::env::current_dir()?;
    // Sandboxed local MCP servers could not reach the daemon's socket
    // (OpenScience starts them in a sandbox that denies every socket), so the
    // agent's own process connects to Sandhill's tools over loopback HTTP.
    let token = uuid::Uuid::new_v4().simple().to_string();
    let tools = crate::mcp::Tools::new(options.daemon.clone(), options.project.clone())
        .in_terminal(Some(dir.clone()));
    let port = crate::mcp::http::serve(std::sync::Arc::new(tools), token.clone()).await?;
    let mcp_url = format!("http://127.0.0.1:{port}/mcp");
    let mut env = Vec::new();
    let mut mcp = json!([]);
    if options.agent == OMP {
        // omp asks before steps that write or run (its default approves
        // everything); Sandhill answers for the modes that don't ask.
        let overlay = dir.join("omp-config.yml");
        std::fs::write(&overlay, "tools:\n  approvalMode: always-ask\n")?;
        env.push((
            "PI_CONFIG_FILES".to_string(),
            overlay.to_string_lossy().into_owned(),
        ));
        std::fs::write(dir.join("omp-sandhill.js"), OMP_EXTENSION)?;
        mcp = json!([{
            "type": "http", "name": "sandhill", "url": mcp_url,
            "headers": [{"name": "Authorization", "value": format!("Bearer {token}")}],
        }]);
    } else {
        let instructions = dir.join("openscience-instructions.txt");
        std::fs::write(&instructions, format!("{INSTRUCTIONS}\n"))?;
        let mut permission = json!({"python": "deny", "r": "deny"});
        permission["mcp"] = sandhill_tool_rules(options.options.permission.as_deref());
        // OpenScience offers MCP tools only from servers in its configuration
        // (`MCP.clients` filters out servers added through ACP session/new).
        let mut config = json!({
            "permission": permission,
            "instructions": [instructions],
            "mcp": {"sandhill": {
                "type": "remote",
                "url": mcp_url,
                "headers": {"Authorization": format!("Bearer {token}")},
                "oauth": false,
                "enabled": true,
            }},
        });
        if let Some(model) = &options.options.model {
            config["model"] = json!(model);
        }
        env.push(("OPENSCIENCE_CONFIG_CONTENT".to_string(), config.to_string()));
    }
    let mut child = spawn(&options.program, &cwd, &env)?;
    let socket = UnixStream::connect(Paths::new(&dir).socket()).await?;
    let (reader, host) = socket.into_split();
    let mut commands = BufReader::new(reader).lines();
    let mut messages = BufReader::new(child.stdout.take().context("no ACP stdout")?).lines();
    let (answers, mut answer_rx) = mpsc::unbounded_channel();
    let mut queued = VecDeque::new();
    if let Some(prompt) = options.prompt.clone().filter(|p| !p.trim().is_empty()) {
        queued.push_back(prompt);
    }
    let history = super::chat::read(&dir, 0)
        .into_iter()
        .filter_map(|entry| {
            (entry.event["kind"] == "stop")
                .then(|| entry.event["turn"].as_str().map(str::to_string))
                .flatten()
        })
        .collect();
    let mut bridge = Bridge {
        selected_model: options.options.model.clone(),
        selected_mode: None,
        selected_effort: options.options.effort.clone(),
        session: options.session.clone().unwrap_or_default(),
        instructed: options.session.is_some(),
        options,
        dir,
        cwd,
        env,
        mcp,
        mcp_url,
        mcp_token: token,
        stdin: child.stdin.take().context("no ACP stdin")?,
        host,
        next: 1,
        pending: HashMap::new(),
        ready: false,
        loading: false,
        replay_skip: false,
        replay_user: false,
        replay_prompt: String::new(),
        history,
        number: 0,
        turn: None,
        busy: false,
        stopping: false,
        queued,
        stream: Stream::default(),
        serial: 0,
        steps: HashMap::new(),
        shown_steps: HashSet::new(),
        finished_steps: HashSet::new(),
        approvals: HashMap::new(),
        answers,
        models: Value::Null,
        modes: Value::Null,
        model: None,
        mode: None,
        model_option: None,
        effort_option: None,
        efforts: Vec::new(),
        effort: None,
        view_mode: "chat",
    };
    bridge.send_host(ToHost::Listen).await?;
    bridge.initialize().await?;
    let mut tick = tokio::time::interval(FLUSH);
    let result = async {
        loop {
            tokio::select! {
                message = messages.next_line() => {
                    let Some(line) = message? else { bail!("ACP agent closed its output"); };
                    if let Ok(message) = serde_json::from_str::<Value>(&line) { bridge.message(message).await?; }
                }
                command = commands.next_line() => {
                    let Some(line) = command? else { bail!("Sandhill terminal closed"); };
                    if let Ok(FromHost::Command { command }) = serde_json::from_str(&line) {
                        if command["kind"] == "view" {
                            if command["mode"] == "terminal" && bridge.options.agent == OMP {
                                bridge.native(&mut child, &mut messages, &mut commands).await?;
                            }
                        } else {
                            bridge.command(command).await?;
                        }
                    }
                }
                Some((key, answer)) = answer_rx.recv() => bridge.answer(&key, answer).await?,
                _ = tick.tick() => bridge.flush(false).await?,
                status = child.wait() => return Ok(status?.code().unwrap_or(1)),
            }
            bridge.advance().await?;
        }
    }.await;
    if let Err(error) = &result {
        let name = display(&bridge.options.agent);
        bridge
            .chat(
                json!({"kind": "error", "turn": bridge.turn, "text": format!("{name}: {error:#}")}),
            )
            .await?;
        bridge.finish(true).await?;
    }
    for (_, approval) in bridge.approvals.drain() {
        approval.task.abort();
    }
    let _ = child.kill().await;
    result
}

/// The agent's ACP server, `<program> acp`.
fn spawn(program: &Path, cwd: &Path, env: &[(String, String)]) -> Result<Child> {
    tokio::process::Command::new(program)
        .arg("acp")
        .current_dir(cwd)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot start {}", program.display()))
}

/// OpenScience asks before every MCP call (`permission.mcp` patterns are the
/// `<server>_<tool>` names). Looking tools run freely; tools that run code or
/// change documents ask first when the conversation is supervised. Sandhill's
/// own guard still blocks changes in Ask mode.
fn sandhill_tool_rules(permission: Option<&str>) -> Value {
    let read_only = crate::mcp::read_only_tools();
    let supervised = matches!(permission, None | Some("supervised"));
    crate::mcp::tool_names()
        .into_iter()
        .map(|tool| {
            let action = if !supervised || read_only.contains(&tool) {
                "allow"
            } else {
                "ask"
            };
            (format!("sandhill_{tool}"), json!(action))
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// omp's approval mode for its own command line, from Sandhill's mode.
fn omp_approval(permission: Option<&str>) -> &'static str {
    match permission {
        Some("full") => "yolo",
        Some("edits") => "write",
        _ => "always-ask",
    }
}

impl Bridge {
    async fn send_host(&mut self, message: ToHost) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        Ok(self.host.write_all(&bytes).await?)
    }

    async fn chat(&mut self, mut event: Value) -> Result<()> {
        if self.loading && self.replay_skip {
            return Ok(());
        }
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, value| !value.is_null());
            map.entry("at_ms").or_insert_with(|| json!(now_ms()));
        }
        self.send_host(ToHost::Chat { event }).await
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
        let mut report = json!({
            "state": if !self.approvals.is_empty() { "waiting" } else if self.busy || !self.ready { "busy" } else { "idle" },
            "thread": self.session, "permission": self.options.options.permission.as_deref().unwrap_or("supervised"),
            "view_mode": self.view_mode, "models": self.models,
            "status": {"model": label, "model_id": self.model, "effort": self.effort},
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            report[key] = value.clone();
        }
        self.send_host(ToHost::Agent { value: report }).await
    }

    async fn write(&mut self, message: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await?;
        Ok(self.stdin.flush().await?)
    }

    async fn call(&mut self, request: Request, method: &str, params: Value) -> Result<()> {
        let id = self.next;
        self.next += 1;
        self.pending.insert(id, request);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await
    }

    async fn initialize(&mut self) -> Result<()> {
        self.call(Request::Initialize, "initialize", json!({
            "protocolVersion": 1,
            "clientInfo": {"name": "sandhill", "title": "Sandhill", "version": env!("CARGO_PKG_VERSION")},
            "clientCapabilities": {},
        })).await
    }

    async fn message(&mut self, message: Value) -> Result<()> {
        match message["method"].as_str() {
            Some("session/update") => {
                if message["params"]["sessionId"] == self.session {
                    self.update(&message["params"]["update"]).await?;
                }
            }
            Some("session/request_permission") => self.permission(&message).await?,
            Some(_) if message.get("id").is_some() => {
                self.write(json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Sandhill does not implement this ACP client method"}})).await?;
            }
            Some(_) => {}
            None => {
                let Some(request) = message["id"]
                    .as_i64()
                    .and_then(|id| self.pending.remove(&id))
                else {
                    return Ok(());
                };
                if let Some(error) = message.get("error") {
                    let text = error["message"].as_str().unwrap_or("ACP request failed");
                    match request {
                        Request::Initialize | Request::Session => bail!("{text}"),
                        Request::Prompt => {
                            self.chat(json!({"kind": "error", "turn": self.turn, "text": text}))
                                .await?;
                            self.finish(true).await?;
                        }
                        Request::Model(_) | Request::Config => {
                            if self.selected_model != self.model {
                                self.queued.clear();
                            }
                            self.selected_model = self.model.clone();
                            self.selected_effort = self.effort.clone();
                            self.chat(json!({"kind": "error", "text": text})).await?;
                        }
                        Request::Mode(_) => bail!("ACP could not apply the selected mode: {text}"),
                    }
                    return Ok(());
                }
                let result = &message["result"];
                match request {
                    Request::Initialize => {
                        let mut params = json!({"cwd": self.cwd, "mcpServers": self.mcp});
                        let method = if self.options.session.is_some() {
                            if result["agentCapabilities"]["loadSession"] != true {
                                bail!("this ACP agent cannot restore sessions");
                            }
                            self.loading = true;
                            params["sessionId"] = json!(self.session);
                            "session/load"
                        } else {
                            "session/new"
                        };
                        self.call(Request::Session, method, params).await?;
                    }
                    Request::Session => {
                        if !self.loading {
                            self.session = result["sessionId"]
                                .as_str()
                                .context("ACP omitted sessionId")?
                                .to_string();
                        }
                        self.finish_replay().await?;
                        self.loading = false;
                        self.ready = true;
                        if result["models"].is_object() {
                            self.models = acp_models(&result["models"]);
                            self.model = result["models"]["currentModelId"]
                                .as_str()
                                .map(str::to_string);
                        }
                        self.apply_options(&result["configOptions"]);
                        if self.model.is_none() {
                            self.model = self.options.options.model.clone();
                        }
                        self.modes = result["modes"]["availableModes"].clone();
                        self.mode = result["modes"]["currentModeId"]
                            .as_str()
                            .map(str::to_string);
                        self.chat(json!({"kind": "session", "source": if self.options.session.is_some() {"resume"} else {"startup"}, "model": self.model})).await?;
                        self.report(json!({})).await?;
                    }
                    Request::Model(model) => {
                        self.model = Some(model);
                        self.report(json!({})).await?;
                    }
                    Request::Config => {
                        self.apply_options(&result["configOptions"]);
                        self.report(json!({})).await?;
                    }
                    Request::Mode(mode) => {
                        self.mode = Some(mode);
                        self.report(json!({})).await?;
                    }
                    Request::Prompt => self.finish(result["stopReason"] == "cancelled").await?,
                }
            }
        }
        Ok(())
    }

    /// The session's model and thinking selects (`configOptions`), when it
    /// offers them: the models, each with the current model's levels.
    fn apply_options(&mut self, options: &Value) {
        for option in options.as_array().into_iter().flatten() {
            let Some(id) = option["id"].as_str() else {
                continue;
            };
            let current = option["currentValue"].as_str().map(str::to_string);
            match option["category"].as_str() {
                Some("model") => {
                    self.model_option = Some(id.to_string());
                    self.model = current;
                    let mut models: Vec<Value> = option["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|choice| {
                            let value = choice["value"].as_str()?;
                            Some(json!({"id": value, "label": choice["name"].as_str().unwrap_or(value), "description": choice["description"]}))
                        })
                        .collect();
                    name_providers(&mut models);
                    self.models = json!(models);
                }
                Some("thought_level") => {
                    self.effort_option = Some(id.to_string());
                    let values: Vec<Value> = option["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|choice| choice["value"].clone())
                        .collect();
                    self.efforts = levels(&json!(values));
                    self.effort = current.filter(|level| self.efforts.contains(&level.as_str()));
                }
                _ => {}
            }
        }
        if !self.efforts.is_empty()
            && let Some(models) = self.models.as_array_mut()
        {
            for model in models {
                model["efforts"] = json!(self.efforts);
            }
        }
    }

    async fn command(&mut self, command: Value) -> Result<()> {
        match command["kind"].as_str() {
            Some("say") => {
                if let Some(text) = command["text"].as_str().filter(|t| !t.trim().is_empty()) { self.queued.push_back(text.to_string()); }
            }
            Some("interrupt") => {
                self.queued.clear();
                self.stopping = true;
                self.cancel_approvals().await?;
                if self.busy { self.write(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": self.session}})).await?; }
            }
            Some("set") => {
                if command.get("model").is_some() { self.selected_model = command["model"].as_str().map(str::to_string); }
                if let Some(effort) = command["effort"].as_str() { self.selected_effort = Some(effort.to_string()); }
                if command.get("permission").is_some() { self.options.options.permission = command["permission"].as_str().map(str::to_string); }
                self.report(json!({})).await?;
            }
            Some("compact") => self.chat(json!({"kind": "error", "text": "This ACP agent does not offer conversation compaction. Start a new conversation instead."})).await?,
            _ => {}
        }
        Ok(())
    }

    async fn advance(&mut self) -> Result<()> {
        if !self.ready || self.busy || !self.pending.is_empty() {
            return Ok(());
        }
        if self.selected_model != self.model && self.selected_model.is_some() {
            let model = self.selected_model.clone().unwrap();
            if let Some(option) = self.model_option.clone() {
                self.call(
                    Request::Config,
                    "session/set_config_option",
                    json!({"sessionId": self.session, "configId": option, "value": model}),
                )
                .await?;
                return Ok(());
            }
            // ACP's unstable model API is advertised by the session's models.
            if self.models.is_array() {
                self.call(
                    Request::Model(model.clone()),
                    "session/set_model",
                    json!({"sessionId": self.session, "modelId": model}),
                )
                .await?;
                return Ok(());
            }
            self.selected_model = self.model.clone();
            self.queued.clear();
            self.chat(json!({"kind": "error", "text": "This ACP agent cannot switch models during a conversation. Start a new conversation with the selected model."})).await?;
        }
        if let Some(effort) = self.selected_effort.clone()
            && self.effort.as_deref() != Some(effort.as_str())
        {
            match self.effort_option.clone() {
                Some(option) if self.efforts.contains(&effort.as_str()) => {
                    self.call(
                        Request::Config,
                        "session/set_config_option",
                        json!({"sessionId": self.session, "configId": option, "value": effort}),
                    )
                    .await?;
                    return Ok(());
                }
                // The model has no such level (or the agent no levels).
                _ => self.selected_effort = self.effort.clone(),
            }
        }
        let planning = matches!(
            self.options.options.permission.as_deref(),
            Some("ask" | "plan")
        );
        let target = self
            .modes
            .as_array()
            .and_then(|modes| {
                modes.iter().find(|mode| {
                    let id = mode["id"].as_str().unwrap_or_default();
                    if planning {
                        id == "plan"
                    } else {
                        id == "build" || id == "default"
                    }
                })
            })
            .and_then(|mode| mode["id"].as_str())
            .map(str::to_string);
        if target.is_some() && target != self.mode {
            self.selected_mode = target;
            self.call(
                Request::Mode(self.selected_mode.clone().unwrap()),
                "session/set_mode",
                json!({"sessionId": self.session, "modeId": self.selected_mode}),
            )
            .await?;
            return Ok(());
        }
        if let Some(text) = self.queued.pop_front() {
            self.number += 1;
            self.turn = Some(format!("{}:turn:{}", self.session, self.number));
            self.busy = true;
            self.stopping = false;
            self.report(json!({})).await?;
            let tools =
                crate::mcp::Tools::new(self.options.daemon.clone(), self.options.project.clone())
                    .in_terminal(Some(self.dir.clone()));
            let prompt = tools.prompt(Duration::from_millis(1500)).await;
            if prompt
                .report
                .as_object()
                .is_some_and(|report| !report.is_empty())
            {
                self.report(prompt.report).await?;
            }
            self.chat(
                json!({"kind": "prompt", "turn": self.turn, "text": text, "place": prompt.place}),
            )
            .await?;
            println!("\n› {text}");
            let mut parts = vec![json!({"type": "text", "text": text})];
            if !self.instructed && self.options.agent == OMP {
                parts.push(json!({"type": "text", "text": crate::agents::instructions(&self.options.options)}));
            }
            self.instructed = true;
            if let Some(context) = prompt.context {
                parts.push(json!({"type": "text", "text": context}));
            }
            if planning {
                parts.push(json!({"type": "text", "text": guard::ASK_NOTE}));
            }
            self.call(
                Request::Prompt,
                "session/prompt",
                json!({"sessionId": self.session, "prompt": parts}),
            )
            .await?;
        }
        Ok(())
    }

    async fn update(&mut self, update: &Value) -> Result<()> {
        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
        if self.loading && kind == "user_message_chunk" {
            if !self.replay_user {
                self.finish_replay().await?;
                self.number += 1;
                let id = format!("{}:turn:{}", self.session, self.number);
                self.replay_skip = self.history.contains(&id);
                self.turn = Some(id);
                self.replay_user = true;
            }
            if let Some(text) = update["content"]["text"].as_str() {
                self.replay_prompt.push_str(text);
            }
            return Ok(());
        }
        if self.loading && self.replay_user {
            self.replay_user = false;
            let text = std::mem::take(&mut self.replay_prompt);
            self.chat(json!({"kind": "prompt", "turn": self.turn, "text": text}))
                .await?;
        }
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                let text = content_text(&update["content"]);
                if self.stream.kind != kind {
                    self.flush(true).await?;
                    self.serial += 1;
                    self.stream.kind = kind.to_string();
                    self.stream.id = format!(
                        "{}:message:{}",
                        self.turn.as_deref().unwrap_or("history"),
                        self.serial
                    );
                }
                self.stream.pending.push_str(&text);
            }
            "tool_call" | "tool_call_update" => {
                self.flush(true).await?;
                let Some(id) = update["toolCallId"].as_str() else {
                    return Ok(());
                };
                let mut tool = self.steps.remove(id).unwrap_or_else(|| json!({}));
                if let Some(fields) = update.as_object() {
                    for (key, value) in fields {
                        if !value.is_null() {
                            tool[key] = value.clone();
                        }
                    }
                }
                let name = tool["title"].as_str().unwrap_or("ACP tool").to_string();
                if self.shown_steps.insert(id.to_string()) {
                    self.chat(json!({"kind": "step", "turn": self.turn, "id": id, "tool": name, "input": shorten(&tool)})).await?;
                }
                match tool["status"].as_str() {
                    Some("completed") if self.finished_steps.insert(id.to_string()) => {
                        self.chat(json!({"kind": "step_done", "turn": self.turn, "id": id, "tool": name, "output": shorten(&tool)})).await?;
                    }
                    Some("failed") if self.finished_steps.insert(id.to_string()) => {
                        self.chat(json!({"kind": "step_failed", "turn": self.turn, "id": id, "tool": name, "error": shorten(&tool)})).await?;
                    }
                    _ => {}
                }
                self.steps.insert(id.to_string(), tool);
            }
            "plan" => {
                self.flush(true).await?;
                self.serial += 1;
                let id = format!(
                    "{}:plan:{}",
                    self.turn.as_deref().unwrap_or("history"),
                    self.serial
                );
                let todos: Vec<Value> = update["entries"].as_array().into_iter().flatten().map(|entry| json!({"content": entry["content"], "status": entry["status"], "priority": entry["priority"]})).collect();
                self.chat(json!({"kind": "step", "turn": self.turn, "id": id, "tool": "TodoWrite", "input": {"todos": todos}})).await?;
                self.chat(json!({"kind": "step_done", "turn": self.turn, "id": id, "tool": "TodoWrite", "output": {}})).await?;
            }
            "current_mode_update" => {
                self.mode = update["currentModeId"].as_str().map(str::to_string);
            }
            "config_option_update" => {
                self.apply_options(&update["configOptions"]);
                if !self.loading {
                    self.report(json!({})).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn flush(&mut self, final_piece: bool) -> Result<()> {
        if self.stream.kind.is_empty() {
            return Ok(());
        }
        if self.stream.kind == "agent_thought_chunk" {
            let changed = !self.stream.pending.is_empty();
            self.stream
                .text
                .push_str(&std::mem::take(&mut self.stream.pending));
            if (changed || final_piece) && !self.stream.text.is_empty() {
                self.chat(json!({"kind": "step", "turn": self.turn, "id": self.stream.id, "tool": "reasoning", "input": {"text": self.stream.text}})).await?;
                if final_piece {
                    self.chat(json!({"kind": "step_done", "turn": self.turn, "id": self.stream.id, "tool": "reasoning", "output": {"text": self.stream.text}})).await?;
                }
            }
        } else if !self.stream.pending.is_empty() || final_piece {
            let text = std::mem::take(&mut self.stream.pending);
            if !text.is_empty() {
                print!("{text}");
            }
            self.chat(json!({"kind": "text", "turn": self.turn, "message": self.stream.id, "index": self.stream.index, "text": text, "final": final_piece})).await?;
            self.stream.index += 1;
        }
        if final_piece {
            self.stream = Stream::default();
        }
        Ok(())
    }

    async fn finish_replay(&mut self) -> Result<()> {
        if self.loading && self.turn.is_some() {
            if self.replay_user {
                let text = std::mem::take(&mut self.replay_prompt);
                self.chat(json!({"kind": "prompt", "turn": self.turn, "text": text}))
                    .await?;
            }
            self.finish(false).await?;
        }
        self.replay_user = false;
        self.replay_skip = false;
        Ok(())
    }

    async fn finish(&mut self, interrupted: bool) -> Result<()> {
        self.flush(true).await?;
        for (id, tool) in std::mem::take(&mut self.steps) {
            if self.finished_steps.contains(&id) {
                continue;
            }
            self.chat(json!({"kind": "step_failed", "turn": self.turn, "id": id, "tool": tool["title"], "error": "The turn ended before the tool completed.", "interrupted": interrupted})).await?;
        }
        self.shown_steps.clear();
        self.finished_steps.clear();
        self.cancel_approvals().await?;
        if let Some(turn) = self.turn.take() {
            self.chat(json!({"kind": "stop", "turn": turn})).await?;
            self.history.insert(turn);
        }
        self.busy = false;
        self.stopping = false;
        if !self.loading {
            self.report(json!({})).await?;
        }
        Ok(())
    }

    /// The option Sandhill's mode picks without asking: everything in Full
    /// access, edits in Edit mode.
    fn approve_without_asking(&self, params: &Value) -> Option<Value> {
        let approve = match self.options.options.permission.as_deref() {
            Some("full") => true,
            Some("edits") => params["toolCall"]["kind"] == "edit",
            _ => false,
        };
        let options = params["options"].as_array()?;
        approve
            .then(|| {
                ["allow_once", "allow_always"].iter().find_map(|kind| {
                    options
                        .iter()
                        .find(|option| option["kind"] == *kind)
                        .map(|option| option["optionId"].clone())
                })
            })
            .flatten()
    }

    async fn permission(&mut self, request: &Value) -> Result<()> {
        let params = &request["params"];
        if params["sessionId"] != self.session || self.stopping || !self.busy {
            return self.write(json!({"jsonrpc": "2.0", "id": request["id"], "result": {"outcome": {"outcome": "cancelled"}}})).await;
        }
        if let Some(option) = self.approve_without_asking(params) {
            return self.write(json!({"jsonrpc": "2.0", "id": request["id"], "result": {"outcome": {"outcome": "selected", "optionId": option}}})).await;
        }
        let key = uuid::Uuid::new_v4().to_string();
        let options = params["options"].clone();
        let event = json!({"kind": "ask", "turn": self.turn, "id": key,
            "tool": params["toolCall"]["title"].as_str().unwrap_or("ACP tool"),
            "step": params["toolCall"]["toolCallId"], "input": shorten(&params["toolCall"]), "options": options, "at_ms": now_ms()});
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
        self.approvals.insert(
            key,
            Approval {
                id: request["id"].clone(),
                options,
                task,
            },
        );
        self.report(json!({})).await?;
        let _ = go.send(());
        Ok(())
    }

    async fn answer(&mut self, key: &str, answer: Option<Value>) -> Result<()> {
        let Some(approval) = self.approvals.remove(key) else {
            return Ok(());
        };
        let selected = answer
            .as_ref()
            .and_then(|answer| answer["option_id"].as_str())
            .filter(|id| {
                approval
                    .options
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|option| option["optionId"] == *id)
            });
        let outcome = match selected {
            Some(id) => json!({"outcome": "selected", "optionId": id}),
            None => json!({"outcome": "cancelled"}),
        };
        self.write(json!({"jsonrpc": "2.0", "id": approval.id, "result": {"outcome": outcome}}))
            .await?;
        self.report(json!({})).await
    }

    async fn cancel_approvals(&mut self) -> Result<()> {
        for (_, approval) in std::mem::take(&mut self.approvals) {
            approval.task.abort();
            self.write(json!({"jsonrpc": "2.0", "id": approval.id, "result": {"outcome": {"outcome": "cancelled"}}})).await?;
        }
        Ok(())
    }

    /// Hands the conversation to omp's own screen in this terminal and takes
    /// it back when the user returns to the chat: one process owns an omp
    /// session at a time, so the ACP server ends first and starts again
    /// after, loading what the command line added.
    async fn native(
        &mut self,
        child: &mut Child,
        messages: &mut Messages,
        commands: &mut Commands,
    ) -> Result<()> {
        if !self.ready || self.session.is_empty() {
            self.chat(json!({"kind": "error", "text": "Oh My Pi is still starting its conversation; try the command line again in a moment."})).await?;
            return Ok(());
        }
        self.view_mode = "switching_terminal";
        self.report(json!({})).await?;
        if self.busy {
            self.write(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": self.session}})).await?;
        }
        self.stopping = true;
        self.cancel_approvals().await?;
        if let Some(pid) = child.id() {
            // SAFETY: the child has not been reaped; omp saves and releases
            // its session on SIGTERM.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        if tokio::time::timeout(RELEASE_TIMEOUT, child.wait())
            .await
            .is_err()
        {
            child.kill().await?;
        }
        if self.busy {
            self.finish(true).await?;
        }
        self.pending.clear();
        self.ready = false;

        let mut command = self.native_command().await;
        let queued = super::codex::native::run(&mut command, &self.dir, commands).await;
        self.view_mode = "switching_chat";
        self.report(json!({})).await?;
        if let Err(error) = &queued {
            self.chat(json!({"kind": "error", "text": format!("Oh My Pi's command line failed: {error:#}")})).await?;
        }

        *child = spawn(&self.options.program, &self.cwd, &self.env)?;
        self.stdin = child.stdin.take().context("no ACP stdin")?;
        *messages = BufReader::new(child.stdout.take().context("no ACP stdout")?).lines();
        // Load the session again, importing the turns taken on the command
        // line; the chat already has the others.
        self.options.session = Some(self.session.clone());
        self.instructed = true;
        self.number = 0;
        self.stopping = false;
        self.view_mode = "chat";
        self.initialize().await?;
        self.report(json!({})).await?;
        for command in queued.unwrap_or_default() {
            self.command(command).await?;
        }
        Ok(())
    }

    /// omp's own screen on this conversation, with its model, effort and
    /// Sandhill's mode, instructions and tools.
    async fn native_command(&self) -> tokio::process::Command {
        let help = crate::agents::help_text(&self.options.program).await;
        let mut command = tokio::process::Command::new(&self.options.program);
        command
            .arg("--resume")
            .arg(&self.session)
            .arg("--append-system-prompt")
            .arg(crate::agents::instructions(&self.options.options))
            .arg("--extension")
            .arg(self.dir.join("omp-sandhill.js"))
            .current_dir(&self.cwd)
            .env("SANDHILL_MCP_URL", &self.mcp_url)
            .env("SANDHILL_MCP_TOKEN", &self.mcp_token);
        if help.contains("--approval-mode") {
            command
                .arg("--approval-mode")
                .arg(omp_approval(self.options.options.permission.as_deref()));
        }
        if help.contains("--allow-home") {
            command.arg("--allow-home");
        }
        if let Some(model) = &self.model {
            command.arg("--model").arg(model);
        }
        if let Some(effort) = &self.effort {
            command.arg("--thinking").arg(effort);
        }
        command
    }
}

fn acp_models(models: &Value) -> Value {
    models["availableModels"].as_array().map(|models| json!(models.iter().filter_map(|model| {
        Some(json!({"id": model["modelId"].as_str()?, "label": model["name"].as_str().or(model["modelId"].as_str())?, "description": model["description"]}))
    }).collect::<Vec<_>>())).unwrap_or_default()
}

fn content_text(content: &Value) -> String {
    match content["type"].as_str() {
        Some("text") => content["text"].as_str().unwrap_or_default().to_string(),
        Some("resource_link") => format!(
            "[{}]({})",
            content["name"].as_str().unwrap_or("resource"),
            content["uri"].as_str().unwrap_or_default()
        ),
        Some("resource") => content["resource"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        Some("image") => "[Image returned by the agent]".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_models_without_warnings_or_duplicates() {
        assert_eq!(
            parse_models(
                "Loading models…\nopenai-codex/gpt-5.6-luna\nopenrouter/a/b\nopenai-codex/gpt-5.6-luna\ninvalid\nfoo/\n/foo\n"
            ),
            json!([{"id": "openai-codex/gpt-5.6-luna", "label": "openai-codex/gpt-5.6-luna"}, {"id": "openrouter/a/b", "label": "openrouter/a/b"}])
        );
    }

    #[test]
    fn omp_models_carry_levels_default_and_providers() {
        let listing = json!({"models": [
            {"provider": "openai-codex", "kind": "chat", "selector": "openai-codex/gpt-6.1-sol",
             "name": "GPT-6.1 Sol", "thinking": ["low", "medium", "high", "xhigh", "max"]},
            {"provider": "anthropic", "kind": "chat", "selector": "anthropic/claude-opus-5-5",
             "name": "Claude Opus 5.5", "thinking": ["minimal", "low", "high"]},
            {"provider": "openrouter", "kind": "chat", "selector": "openrouter/claude-opus-5-5",
             "name": "Claude Opus 5.5", "thinking": []},
            {"provider": "anthropic", "kind": "chat", "selector": "anthropic/claude-haiku-4-5",
             "name": "Claude Haiku 4.5", "thinking": []},
            {"provider": "anthropic", "kind": "chat", "selector": "anthropic/claude-haiku-4-5-20251001",
             "name": "Claude Haiku 4.5", "thinking": []},
            {"provider": "openai-codex", "kind": "image", "selector": "openai-codex/gpt-image-2",
             "name": "GPT Image 2"},
        ]});
        let roles = json!({"default": "openai-codex/gpt-6.1-sol:xhigh"});
        assert_eq!(
            omp_models(&listing, &roles, Some("high")),
            json!([
                {"id": "openai-codex/gpt-6.1-sol", "label": "GPT-6.1 Sol",
                 "efforts": ["low", "medium", "high", "xhigh", "max"], "effort": "xhigh", "default": true},
                {"id": "anthropic/claude-opus-5-5", "label": "Claude Opus 5.5 (anthropic)",
                 "efforts": ["minimal", "low", "high"], "effort": "high", "default": false},
                {"id": "openrouter/claude-opus-5-5", "label": "Claude Opus 5.5 (openrouter)",
                 "efforts": [], "effort": null, "default": false},
                {"id": "anthropic/claude-haiku-4-5", "label": "Claude Haiku 4.5",
                 "efforts": [], "effort": null, "default": false},
            ])
        );
    }

    #[test]
    fn omp_approves_by_sandhills_mode() {
        assert_eq!(omp_approval(Some("full")), "yolo");
        assert_eq!(omp_approval(Some("edits")), "write");
        for mode in [
            None,
            Some("supervised"),
            Some("restricted"),
            Some("ask"),
            Some("plan"),
            Some("auto"),
        ] {
            assert_eq!(omp_approval(mode), "always-ask");
        }
    }
}
