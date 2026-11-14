//! The coding agents Sandhill runs: Claude Code, Codex, Antigravity,
//! OpenScience, Oh My Pi, pi and Posit Assistant as their own, unmodified
//! CLIs, each in a terminal the daemon keeps (see `terminal`).
//! Sandhill only configures them the way a user could, with command-line
//! flags or environment variables:
//!
//! - its MCP server (`sandhilld mcp`), so they can reach the project's live
//!   R and Python sessions;
//! - for Claude Code, hooks and a status line that report what it is doing
//!   (busy, waiting for the user, model, context use) to its terminal
//!   through `sandhilld terminal-event`;
//! - for Codex, nothing: Sandhill drives its conversation through `codex
//!   app-server` (see `terminal::codex`), and tells it the same things;
//! - for Antigravity, nothing either: Sandhill runs `agy` headless for each
//!   message (see `terminal::antigravity`), which adds Sandhill's MCP server
//!   to agy's settings, the only place agy reads them from;
//! - for OpenScience and Oh My Pi, nothing either: Sandhill drives them
//!   over ACP (see `terminal::acp`), passing its tools per run;
//! - for pi, nothing either: Sandhill drives `pi --mode rpc` (see
//!   `terminal::pi`), passing its tools in a per-run extension;
//! - for Posit Assistant, nothing either: `pa` runs its own screen (the
//!   app shows that terminal), with the MCP server added through its
//!   administrator defaults, which leave the user's settings file alone.
//!
//! Sandhill never reads their logins: each signs in through its own
//! `/login` or sign-in (or an API key the user set up for it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use sandhill_proto::{AgentInfo, AgentOptions, TERMINAL_SHELL};
use serde_json::{Value, json};

use crate::terminal::Launch;

pub const CLAUDE: &str = "claude";
pub const CODEX: &str = "codex";
pub const ANTIGRAVITY: &str = "antigravity";
pub const OPENSCIENCE: &str = "openscience";
pub const OMP: &str = "omp";
pub const PI: &str = "pi";
pub const POSIT: &str = "posit";

struct Agent {
    kind: &'static str,
    name: &'static str,
    program: &'static str,
    install: &'static str,
}

const AGENTS: [Agent; 7] = [
    Agent {
        kind: CLAUDE,
        name: "Claude Code",
        program: "claude",
        install: "curl -fsSL https://claude.ai/install.sh | bash",
    },
    Agent {
        kind: CODEX,
        name: "Codex",
        program: "codex",
        install: "npm install -g @openai/codex",
    },
    Agent {
        kind: ANTIGRAVITY,
        name: "Antigravity",
        program: "agy",
        install: "curl -fsSL https://antigravity.google/install.sh | bash",
    },
    Agent {
        kind: OPENSCIENCE,
        name: "OpenScience",
        program: "openscience",
        install: "npm install -g @synsci/openscience",
    },
    Agent {
        kind: OMP,
        name: "Oh My Pi",
        program: "omp",
        install: "curl -fsSL https://omp.sh/install.sh | bash",
    },
    Agent {
        kind: PI,
        name: "pi",
        program: "pi",
        install: "curl -fsSL https://pi.dev/install.sh | sh",
    },
    Agent {
        kind: POSIT,
        name: "Posit Assistant",
        program: "pa",
        install: "curl -fsSL https://assistant.posit.co/builds/install-tui.sh | bash",
    },
];

/// Where `pa` reads administrator defaults: settings under the user's own.
const POSIT_DEFAULTS: &str = "POSIT_ASSISTANT_SETTINGS_DEFAULT";

/// What an agent's terminal needs to know about this daemon.
pub struct Context<'a> {
    /// `sandhilld`, for the MCP server and the hooks.
    pub executable: &'a Path,
    /// The daemon's socket, which the MCP server connects to.
    pub socket: &'a Path,
    pub project: Option<&'a str>,
    pub cwd: &'a Path,
    /// The first message, given to the CLI as its prompt.
    pub prompt: Option<&'a str>,
    /// The conversation to go on with, when the agent is started again:
    /// what [`RESUME_KEYS`] names in its report.
    pub resume: Option<&'a str>,
}

/// What an agent reports that finds its conversation again: Claude Code's
/// session (from its hooks), and Codex's thread or Antigravity's
/// conversation (from Sandhill's bridges).
pub const RESUME_KEYS: [&str; 2] = ["session_id", "thread"];

/// Which of [`RESUME_KEYS`] an agent of `kind` reports.
pub fn resume_key(kind: &str) -> Option<&'static str> {
    match kind {
        CLAUDE => Some(RESUME_KEYS[0]),
        CODEX | ANTIGRAVITY | OPENSCIENCE | OMP | PI => Some(RESUME_KEYS[1]),
        _ => None,
    }
}

/// The conversation an agent of `kind` would resume, from its report.
pub fn resume_id(kind: &str, report: Option<&serde_json::Value>) -> Option<String> {
    let key = resume_key(kind)?;
    report?[key]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// The agents and whether this machine has their CLIs.
pub async fn list() -> Vec<AgentInfo> {
    let mut list = Vec::new();
    for agent in &AGENTS {
        let path = find(agent.program);
        let version = match &path {
            Some(path) => version(path).await,
            None => None,
        };
        list.push(AgentInfo {
            kind: agent.kind.into(),
            name: agent.name.into(),
            path: path.map(|p| p.to_string_lossy().into_owned()),
            version,
            install: agent.install.into(),
        });
    }
    list
}

/// The models `kind` offers before any conversation, for a new one's model
/// menu.
pub async fn models(kind: &str) -> Result<Value> {
    let agent = AGENTS
        .iter()
        .find(|a| a.kind == kind)
        .with_context(|| format!("no agent {kind:?}"))?;
    let program = find(agent.program)
        .with_context(|| format!("{} is not installed: {}", agent.name, agent.install))?;
    match kind {
        CODEX => crate::terminal::codex::list_models(&program).await,
        ANTIGRAVITY => crate::terminal::antigravity::list_models(&program).await,
        OPENSCIENCE | OMP => crate::terminal::acp::list_models(kind, &program).await,
        PI => crate::terminal::pi::list_models(&program).await,
        _ => bail!("{} lists its models itself", agent.name),
    }
}

/// One cache per provider, shared by every monitor connected to this daemon.
/// The lock spans discovery. A forced waiter may share a query completed after
/// it arrived, but can never receive a snapshot cached before its request.
struct AccountCache {
    snapshot: tokio::sync::Mutex<Option<(std::time::Instant, Option<Value>)>>,
}

impl AccountCache {
    const fn new() -> Self {
        Self {
            snapshot: tokio::sync::Mutex::const_new(None),
        }
    }

    async fn query<F, Fut>(&self, force: bool, query: F) -> Option<Value>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Option<Value>>,
    {
        let requested = std::time::Instant::now();
        let mut cache = self.snapshot.lock().await;
        if let Some((at, snapshot)) = cache.as_ref()
            && ((!force && at.elapsed() < Duration::from_secs(30)) || *at >= requested)
        {
            return snapshot.clone();
        }
        let snapshot = query().await;
        *cache = Some((std::time::Instant::now(), snapshot.clone()));
        snapshot
    }
}

/// Account identity and quotas from official CLI interfaces only. Failure is not
/// sign-out: identity queries are omitted; anonymous usage reports carry an error.
/// None selects every supported provider for the unchanged CLI request; an
/// explicit empty selection performs no discovery at all.
pub async fn accounts(kinds: Option<&[String]>, force: bool) -> Result<Value> {
    static CODEX_CACHE: AccountCache = AccountCache::new();
    static CLAUDE_CACHE: AccountCache = AccountCache::new();
    static ANTIGRAVITY_CACHE: AccountCache = AccountCache::new();
    Ok(json!(
        account_snapshots(
            kinds,
            force,
            &[
                (CODEX, &CODEX_CACHE),
                (CLAUDE, &CLAUDE_CACHE),
                (ANTIGRAVITY, &ANTIGRAVITY_CACHE),
            ],
            query_account
        )
        .await
    ))
}

async fn account_snapshots<F, Fut>(
    kinds: Option<&[String]>,
    force: bool,
    providers: &[(&'static str, &AccountCache)],
    query: F,
) -> Vec<Value>
where
    F: Fn(&'static str) -> Fut,
    Fut: Future<Output = Option<Value>>,
{
    let mut accounts = Vec::with_capacity(providers.len());
    for &(kind, cache) in providers {
        if kinds.is_some_and(|kinds| !kinds.iter().any(|selected| selected == kind)) {
            continue;
        }
        if let Some(snapshot) = cache.query(force, || query(kind)).await {
            accounts.push(snapshot);
        }
    }
    accounts
}

async fn query_account(kind: &str) -> Option<Value> {
    match kind {
        CODEX => {
            let codex = find("codex")?;
            let mut snapshot = crate::terminal::codex::read_account(&codex).await.ok()?;
            snapshot["kind"] = json!(CODEX);
            Some(snapshot)
        }
        CLAUDE => {
            let claude = find("claude")?;
            let account = claude_account(&claude).await.ok()?;
            Some(json!({"kind": CLAUDE, "account": account,
                "observed_at_ms": crate::journal::now_ms()}))
        }
        ANTIGRAVITY => {
            let agy = find("agy")?;
            match crate::terminal::antigravity::usage::read_usage(&agy, None).await {
                Ok(usage) => Some(usage.report),
                Err(error) => Some(json!({"kind": ANTIGRAVITY,
                    "quota_source": "cli_snapshot", "observed_at_ms": crate::journal::now_ms(),
                    "quota_error": format!("{error:#}"), "status": {"rate_limits": {}}})),
            }
        }
        _ => None,
    }
}

/// Official read-only identity query shared by discovery and quota status hooks.
pub(crate) async fn claude_account(claude: &Path) -> Result<Value> {
    claude_account_in(claude, None).await
}

/// Queries the account selected for this run, never the daemon's other accounts.
pub(crate) async fn claude_account_in(claude: &Path, config: Option<&Path>) -> Result<Value> {
    let mut command = tokio::process::Command::new(claude);
    command
        .args(["auth", "status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = path_for(claude)? {
        command.env("PATH", path);
    }
    if let Some(config) = config {
        command.env("CLAUDE_CONFIG_DIR", config);
    }
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
    let status: Value = serde_json::from_slice(&output.stdout)?;
    match status["loggedIn"].as_bool() {
        Some(true) => Ok(
            json!({"type": status["authMethod"], "email": status["email"],
            "plan": status["subscriptionType"], "organization": status["orgId"]}),
        ),
        Some(false) => Ok(Value::Null),
        None => bail!("Claude did not report its authentication state"),
    }
}

/// What to run for a new terminal of `kind`.
pub async fn launch(kind: &str, options: &AgentOptions, context: &Context<'_>) -> Result<Launch> {
    let mut launch = Launch {
        kind: kind.to_string(),
        title: String::new(),
        program: PathBuf::new(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: context.cwd.to_path_buf(),
        project: context.project.map(str::to_string),
        options: options.clone(),
        size: None,
        job: None,
    };
    if kind == TERMINAL_SHELL {
        launch.title = "Terminal".into();
        launch.program = shell();
        launch.args = vec!["-l".into()];
        return Ok(launch);
    }
    let Some(agent) = AGENTS.iter().find(|a| a.kind == kind) else {
        bail!("unknown kind of terminal: {kind}");
    };
    let Some(program) = find(agent.program) else {
        bail!(
            "{} is not installed on this machine; install it with `{}`",
            agent.name,
            agent.install
        );
    };
    let help = help_text(&program).await;
    launch.env =
        crate::accounts::Registry::from_env().environment(kind, options.account.as_deref())?;
    launch.title = agent.name.into();
    match kind {
        CLAUDE => {
            launch.args = claude_args(options, context, &help);
            launch.program = program.clone();
        }
        ANTIGRAVITY => {
            // The Antigravity app installs an `agy` that only opens it.
            if !help.contains("--output-format") {
                bail!(
                    "this `agy` has no headless mode (it may be the Antigravity app's launcher); install Antigravity's CLI with `{}`",
                    agent.install
                );
            }
            if !help.contains("--input-format") {
                bail!(
                    "this `agy` has no structured headless input; update Antigravity's CLI with `agy update`"
                );
            }
            // Sandhill runs agy for each message (see
            // `terminal::antigravity`).
            launch.args = agy_args(options, context, &program);
            launch.program = context.executable.to_path_buf();
        }
        OPENSCIENCE | OMP | PI => {
            launch.args = acp_args(kind, options, context, &program);
            launch.program = context.executable.to_path_buf();
        }
        POSIT => {
            // No arguments: a prompt would run pa once without its screen,
            // so the app types the first message once pa is ready.
            let defaults = std::env::var(POSIT_DEFAULTS).ok();
            launch.env.push((
                POSIT_DEFAULTS.into(),
                posit_defaults(context, defaults.as_deref()),
            ));
            launch.program = program.clone();
        }
        _ => {
            if !help.contains("app-server") {
                bail!(
                    "this Codex is too old for Sandhill's chat; update it with `{}`",
                    agent.install
                );
            }
            // Sandhill drives Codex's conversation itself (see
            // `terminal::codex`), with Codex underneath.
            launch.args = codex_args(options, context, &program);
            launch.program = context.executable.to_path_buf();
        }
    }

    if let Some(path) = path_for(&program)? {
        launch.env.push(("PATH".into(), path));
    }
    Ok(launch)
}

/// `PATH` with the CLI's folder first, when it isn't on it already: an
/// npm-installed CLI runs under `node` from the same folder, which may not be
/// on the daemon's `PATH` (as on a server reached over ssh).
pub(crate) fn path_for(program: &Path) -> Result<Option<String>> {
    let Some(dir) = program.parent() else {
        return Ok(None);
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&path).any(|p| p == dir) {
        return Ok(None);
    }
    let joined = std::env::join_paths(
        std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path)),
    )?;
    Ok(Some(joined.to_string_lossy().into_owned()))
}

/// `sandhilld`'s arguments for its MCP server for the project.
fn mcp_args(context: &Context<'_>) -> Vec<String> {
    let mut args = vec![
        "--socket".to_string(),
        context.socket.to_string_lossy().into_owned(),
        "mcp".into(),
    ];
    if let Some(project) = context.project {
        args.extend(["--project".into(), project.into()]);
    }
    args
}

/// pa's administrator defaults, `existing` (what the daemon's environment
/// sets, if anything) with Sandhill's MCP server added.
fn posit_defaults(context: &Context<'_>, existing: Option<&str>) -> String {
    let mut defaults = existing
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if !defaults["mcpServers"].is_object() {
        defaults["mcpServers"] = json!({});
    }
    let mut command = vec![context.executable.to_string_lossy().into_owned()];
    command.extend(mcp_args(context));
    defaults["mcpServers"]["sandhill"] = json!({"type": "local", "command": command});
    defaults.to_string()
}

fn claude_args(options: &AgentOptions, context: &Context<'_>, help: &str) -> Vec<String> {
    let exe = context.executable.to_string_lossy();
    let mcp = json!({"mcpServers": {"sandhill": {"type": "stdio", "command": exe, "args": mcp_args(context)}}});
    // One command for every hook: it reports the agent's state, keeps its
    // conversation for the chat, adds what the user added with @ and # to
    // each prompt, and asks the user in the app for permissions.
    let command = format!(
        "{} --socket {} terminal-event hook{}",
        shell_quote(&exe),
        shell_quote(&context.socket.to_string_lossy()),
        context
            .project
            .map(|p| format!(" {}", shell_quote(p)))
            .unwrap_or_default()
    );
    let hook = json!([{"hooks": [{"type": "command", "command": command}]}]);
    let tool_hook = json!([{"matcher": "*", "hooks": [{"type": "command", "command": command}]}]);
    // A permission waits for the user, who may be away for a while.
    let ask_hook = json!([{"matcher": "*", "hooks": [{"type": "command", "command": command, "timeout": ASK_TIMEOUT_S}]}]);
    let settings = json!({
        "hooks": {
            "SessionStart": hook,
            "UserPromptSubmit": hook,
            "MessageDisplay": hook,
            // In Restricted mode, it waits for the user too.
            "PreToolUse": ask_hook,
            "PostToolUse": tool_hook,
            "PostToolUseFailure": tool_hook,
            "PermissionRequest": ask_hook,
            "Notification": hook,
            "Stop": hook,
        },
        // Sandhill's tools that only look, like Claude Code's own Read and
        // Grep, don't ask first.
        "permissions": {"allow": crate::mcp::read_only_tools().iter().map(|t| format!("mcp__sandhill__{t}")).collect::<Vec<_>>()},
        "statusLine": {"type": "command", "command": format!("{} terminal-event status", shell_quote(&exe)), "padding": 0},
    });
    let mut args = vec!["--mcp-config".to_string(), mcp.to_string()];
    if help.contains("--settings") {
        args.extend(["--settings".into(), settings.to_string()]);
    }
    if help.contains("--append-system-prompt") {
        args.extend(["--append-system-prompt".into(), instructions(options)]);
    }
    if let Some(model) = &options.model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(effort) = &options.effort
        && help.contains("--effort")
    {
        args.extend(["--effort".into(), effort.clone()]);
    }
    // Supervised is named too: without a mode, Claude Code takes the
    // user's own default, which may be one that asks less. Restricted and
    // Ask are Sandhill's own, on top of the default (see `guard`).
    let mode = match options.permission.as_deref() {
        Some("supervised" | "restricted" | "ask") => Some("default"),
        Some("edits") => Some("acceptEdits"),
        Some("auto") if help.contains("\"auto\"") => Some("auto"),
        Some("full") => Some("bypassPermissions"),
        Some("plan") => Some("plan"),
        _ => None,
    };
    if let Some(mode) = mode
        && help.contains("--permission-mode")
    {
        args.extend(["--permission-mode".into(), mode.into()]);
    }
    if let Some(session) = context.resume
        && help.contains("--resume")
    {
        args.extend(["--resume".into(), session.into()]);
    }
    push_prompt(&mut args, context);
    args
}

/// `sandhilld codex-chat`'s arguments: the conversation's settings, and
/// Sandhill's MCP server for `codex app-server`.
fn codex_args(options: &AgentOptions, context: &Context<'_>, codex: &Path) -> Vec<String> {
    let exe = context.executable.to_string_lossy();
    let mcp_args = mcp_args(context);
    let mut args = vec![
        "--socket".to_string(),
        context.socket.to_string_lossy().into_owned(),
        "codex-chat".into(),
        "--codex".into(),
        codex.to_string_lossy().into_owned(),
        "--options".into(),
        serde_json::to_string(options).unwrap_or_else(|_| "{}".into()),
    ];
    if let Some(project) = context.project {
        args.extend(["--project".into(), project.into()]);
    }
    if let Some(prompt) = context.prompt.filter(|p| !p.trim().is_empty()) {
        args.extend(["--prompt".into(), prompt.into()]);
    }
    if let Some(thread) = context.resume {
        args.extend(["--thread".into(), thread.into()]);
    }
    args.extend([
        "--".into(),
        "-c".into(),
        format!("mcp_servers.sandhill.command={}", toml_string(&exe)),
        "-c".into(),
        format!("mcp_servers.sandhill.args={}", toml_array(&mcp_args)),
    ]);
    args
}

/// `sandhilld agy-chat`'s arguments: the conversation's settings.
fn agy_args(options: &AgentOptions, context: &Context<'_>, agy: &Path) -> Vec<String> {
    let mut args = vec![
        "--socket".to_string(),
        context.socket.to_string_lossy().into_owned(),
        "agy-chat".into(),
        "--agy".into(),
        agy.to_string_lossy().into_owned(),
        "--options".into(),
        serde_json::to_string(options).unwrap_or_else(|_| "{}".into()),
    ];
    if let Some(project) = context.project {
        args.extend(["--project".into(), project.into()]);
    }
    if let Some(prompt) = context.prompt.filter(|p| !p.trim().is_empty()) {
        args.extend(["--prompt".into(), prompt.into()]);
    }
    if let Some(conversation) = context.resume {
        args.extend(["--conversation".into(), conversation.into()]);
    }
    args
}

/// A bridge that drives the provider's unmodified executable over its
/// machine interface: `acp-chat` for ACP agents, `pi-chat` for pi's RPC mode.
fn acp_args(
    kind: &str,
    options: &AgentOptions,
    context: &Context<'_>,
    program: &Path,
) -> Vec<String> {
    let mut args = vec![
        "--socket".into(),
        context.socket.to_string_lossy().into_owned(),
    ];
    if kind == PI {
        args.push("pi-chat".into());
    } else {
        args.extend(["acp-chat".into(), "--agent".into(), kind.into()]);
    }
    args.extend([
        "--program".into(),
        program.to_string_lossy().into_owned(),
        "--options".into(),
        serde_json::to_string(options).unwrap_or_else(|_| "{}".into()),
    ]);
    if let Some(project) = context.project {
        args.extend(["--project".into(), project.into()]);
    }
    if let Some(prompt) = context.prompt.filter(|p| !p.trim().is_empty()) {
        args.extend(["--prompt".into(), prompt.into()]);
    }
    if let Some(session) = context.resume {
        args.extend(["--session".into(), session.into()]);
    }
    args
}

/// The first message, after `--` so one starting with a dash isn't taken for a flag.
fn push_prompt(args: &mut Vec<String>, context: &Context<'_>) {
    if let Some(prompt) = context.prompt.filter(|p| !p.trim().is_empty()) {
        args.extend(["--".into(), prompt.into()]);
    }
}

/// How long, in seconds, Claude Code waits for the user to answer a
/// permission request in the app before asking in its terminal.
const ASK_TIMEOUT_S: u64 = 24 * 60 * 60;

/// What an agent is told about Sandhill when it starts.
pub fn instructions(options: &AgentOptions) -> String {
    let mut text = String::from(
        "You are running inside Sandhill, an IDE for R and Python data analysis, next to the \
         user's editor. The user's live R and Python sessions are reachable through the \
         `sandhill` MCP tools: list_sessions, list_variables, run_code and get_outputs. Use \
         them to look at the objects and results the user already has in memory instead of \
         recomputing them. When the user asks you to compute, load, plot or make something, \
         write the code into their document with add_cell: it goes below the cell they are in \
         and runs in their session, so they see the code and the result together. If a cell \
         you added fails, fix it with edit_cell instead of adding another. Use run_code only \
         for quick looks they don't need to keep. get_focus says where the user is. \
         get_console shows what they ran lately and its errors, get_plots their plots, \
         view_table a data frame's first rows, get_help a function's help page as installed, \
         and list_packages what is installed; delete_cell removes a notebook cell. \
         list_jobs and read_job_log show the scripts and pipelines running in the background \
         and what they printed. Files you edit show up in the user's open editor.",
    );
    text.push(' ');
    text.push_str(crate::mcp::bio::GUIDANCE);
    if let Some(pin) = &options.pin {
        text.push_str(&format!(
            " The user pinned this session to {pin}; add_cell writes there."
        ));
    } else if !options.documents.is_empty() {
        text.push_str(&format!(
            " The user started this session in {}. add_cell writes into the document in \
             front of them when they send a message, unless they pin one.",
            options.documents.join(", ")
        ));
    } else {
        text.push_str(
            " This conversation is about the whole project, not one file. Files the user \
             names with @ (and notebook cells, with #) come with their message: work on a \
             file only when the user names it. add_cell writes into the document the \
             message names; when it names none, add_cell runs the code in the Console.",
        );
    }
    text
}

/// The user's login shell.
pub fn shell() -> PathBuf {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
        .or_else(|| {
            ["/bin/zsh", "/bin/bash", "/bin/sh"]
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// Whether `kind` is a coding agent's terminal, rather than a shell's or a job's.
pub fn is_agent(kind: &str) -> bool {
    AGENTS.iter().any(|a| a.kind == kind)
}

/// Finds a CLI on `PATH`, or where its installers put it.
pub fn find(program: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = &home {
        for dir in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            ".cargo/bin",
        ] {
            dirs.push(home.join(dir));
        }
        // nvm keeps one folder per Node version; prefer the newest.
        let mut nvm: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("bin"))
            .collect();
        nvm.sort();
        dirs.extend(nvm.into_iter().rev());
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter()
        .map(|dir| dir.join(program))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub(crate) async fn version(program: &Path) -> Option<String> {
    version_in(&run(program, "--version").await?)
}

/// The version a CLI's `--version` gives: "2.1.296 (Claude Code)",
/// "codex-cli 0.162.1", "agy version 2.0.0", "Posit Assistant TUI v0.1.0"
/// (then its build and commit).
fn version_in(output: &str) -> Option<String> {
    let starts_with_digit = |word: &str| word.starts_with(|c: char| c.is_ascii_digit());
    output
        .split_whitespace()
        .map(|word| {
            word.strip_prefix('v')
                .filter(|rest| starts_with_digit(rest))
                .unwrap_or(word)
        })
        .find(|word| starts_with_digit(word))
        .map(str::to_string)
}

/// The CLI's `--help`, to know which flags it takes: an older version
/// rejects a flag it doesn't know. Cached per executable and its mtime.
pub(crate) async fn help_text(program: &Path) -> String {
    /// Per executable: its mtime and its help.
    type Cache = HashMap<PathBuf, (Option<SystemTime>, String)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let mtime = std::fs::metadata(program).and_then(|m| m.modified()).ok();
    if let Some((when, text)) = CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(program)
        && *when == mtime
    {
        return text.clone();
    }
    let text = run(program, "--help").await.unwrap_or_default();
    CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(program.to_path_buf(), (mtime, text.clone()));
    text
}

async fn run(program: &Path, arg: &str) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(program)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(text)
}

/// Quotes a word for `sh`, which runs hook commands and jobs.
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn toml_array(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|i| toml_string(i)).collect();
    format!("[{}]", items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn provider_selection_excludes_disabled_discovery() {
        let codex = AccountCache::new();
        let claude = AccountCache::new();
        let antigravity = AccountCache::new();
        let providers = [
            (CODEX, &codex),
            (CLAUDE, &claude),
            (ANTIGRAVITY, &antigravity),
        ];
        let calls = std::sync::Mutex::new(Vec::new());
        let query = |kind| {
            calls.lock().unwrap().push(kind);
            std::future::ready(Some(json!({"kind": kind})))
        };
        let only_codex = vec![CODEX.to_string()];
        assert_eq!(
            account_snapshots(Some(&only_codex), false, &providers, query).await,
            vec![json!({"kind": CODEX})]
        );
        assert_eq!(*calls.lock().unwrap(), vec![CODEX]);
        assert!(
            account_snapshots(Some(&[]), false, &providers, query)
                .await
                .is_empty()
        );
        assert_eq!(*calls.lock().unwrap(), vec![CODEX]);
        // Explicit refresh selects the disabled provider without polling the
        // enabled one or changing the caller's persistent selection.
        let only_claude = vec![CLAUDE.to_string()];
        assert_eq!(
            account_snapshots(Some(&only_claude), true, &providers, query).await,
            vec![json!({"kind": CLAUDE})]
        );
        assert_eq!(*calls.lock().unwrap(), vec![CODEX, CLAUDE]);
        account_snapshots(Some(&only_codex), false, &providers, query).await;
        assert_eq!(*calls.lock().unwrap(), vec![CODEX, CLAUDE]);
        let only_antigravity = vec![ANTIGRAVITY.to_string()];
        assert_eq!(
            account_snapshots(Some(&only_antigravity), true, &providers, query).await,
            vec![json!({"kind": ANTIGRAVITY})]
        );
        assert_eq!(*calls.lock().unwrap(), vec![CODEX, CLAUDE, ANTIGRAVITY]);
        account_snapshots(Some(&only_antigravity), false, &providers, query).await;
        assert_eq!(*calls.lock().unwrap(), vec![CODEX, CLAUDE, ANTIGRAVITY]);
    }

    #[tokio::test]
    async fn forced_provider_refresh_bypasses_cache_and_preserves_failure() {
        let cache = AccountCache::new();
        let old = json!({"observed_at_ms": 1});
        assert_eq!(
            cache.query(false, || async { Some(old.clone()) }).await,
            Some(old.clone())
        );
        assert_eq!(
            cache
                .query(false, || async { panic!("cached provider queried") })
                .await,
            Some(old)
        );
        let fresh = json!({"observed_at_ms": 2});
        assert_eq!(
            cache.query(true, || async { Some(fresh.clone()) }).await,
            Some(fresh)
        );
        assert_eq!(cache.query(true, || async { None }).await, None);
        assert_eq!(
            cache
                .query(false, || async { panic!("failed discovery queried again") })
                .await,
            None
        );
    }

    #[tokio::test]
    async fn concurrent_forced_monitors_share_provider_work() {
        let cache = AccountCache::new();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let query = || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tokio::task::yield_now().await;
            Some(json!({"observed_at_ms": 42}))
        };
        let (first, second, third) = tokio::join!(
            cache.query(true, query),
            cache.query(true, query),
            cache.query(false, query)
        );
        assert_eq!(first, second);
        assert_eq!(second, third);
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    fn context() -> Context<'static> {
        Context {
            executable: Path::new("/opt/it's/sandhilld"),
            socket: Path::new("/state/sandhilld.sock"),
            project: Some("/data/pbmc"),
            cwd: Path::new("/data/pbmc"),
            prompt: None,
            resume: None,
        }
    }

    #[test]
    fn the_first_message_comes_last() {
        let context = Context {
            prompt: Some("-v is what?"),
            ..context()
        };
        let help = "--mcp-config --settings";
        let args = claude_args(&AgentOptions::default(), &context, help);
        assert_eq!(args[args.len() - 2..], ["--", "-v is what?"]);
        // Codex's goes to Sandhill's bridge, which sends it as a message.
        let args = codex_args(&AgentOptions::default(), &context, Path::new("codex"));
        assert!(args.windows(2).any(|w| w == ["--prompt", "-v is what?"]));
    }

    #[test]
    fn claude_gets_the_mcp_server_hooks_and_only_flags_it_knows() {
        let options = AgentOptions {
            model: Some("opus".into()),
            effort: Some("high".into()),
            permission: Some("edits".into()),
            documents: vec!["analysis.qmd".into()],
            ..Default::default()
        };
        let help = "--mcp-config --settings --append-system-prompt --permission-mode";
        let args = claude_args(&options, &context(), help);
        let mcp: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
        assert_eq!(
            mcp["mcpServers"]["sandhill"]["command"],
            "/opt/it's/sandhilld"
        );
        assert_eq!(
            mcp["mcpServers"]["sandhill"]["args"],
            json!([
                "--socket",
                "/state/sandhilld.sock",
                "mcp",
                "--project",
                "/data/pbmc"
            ])
        );
        let settings: serde_json::Value = serde_json::from_str(&args[3]).unwrap();
        // Every hook runs the same command, which can ask the daemon where
        // the user is in the project.
        let command = r"'/opt/it'\''s/sandhilld' --socket '/state/sandhilld.sock' terminal-event hook '/data/pbmc'";
        for event in ["UserPromptSubmit", "Stop", "MessageDisplay", "PreToolUse"] {
            assert_eq!(settings["hooks"][event][0]["hooks"][0]["command"], command);
        }
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert_eq!(
            settings["permissions"]["allow"][2],
            "mcp__sandhill__get_focus"
        );
        // Both wait for the user.
        for event in ["PermissionRequest", "PreToolUse"] {
            assert_eq!(
                settings["hooks"][event][0]["hooks"][0]["timeout"],
                ASK_TIMEOUT_S
            );
        }
        assert!(args[5].contains("analysis.qmd"));
        assert!(args[5].contains("add_cell"));
        assert!(args.windows(2).any(|w| w == ["--model", "opus"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-mode", "acceptEdits"])
        );
        // This CLI's help doesn't list --effort.
        assert!(!args.contains(&"--effort".to_string()));
    }

    #[test]
    fn a_conversation_started_again_resumes() {
        let context = Context {
            prompt: Some("go on"),
            resume: Some("5f3c-session"),
            ..context()
        };
        let options = AgentOptions {
            permission: Some("full".into()),
            ..Default::default()
        };
        let args = claude_args(&options, &context, "--resume --permission-mode");
        assert_eq!(
            args[args.len() - 6..],
            [
                "--permission-mode",
                "bypassPermissions",
                "--resume",
                "5f3c-session",
                "--",
                "go on"
            ]
        );
        let args = codex_args(&options, &context, Path::new("codex"));
        assert!(args.windows(2).any(|w| w == ["--thread", "5f3c-session"]));

        let report = json!({"session_id": "s1", "thread": "t1", "state": "idle"});
        assert_eq!(resume_id(CLAUDE, Some(&report)).as_deref(), Some("s1"));
        assert_eq!(resume_id(CODEX, Some(&report)).as_deref(), Some("t1"));
        assert_eq!(resume_id(CLAUDE, Some(&json!({"session_id": ""}))), None);
        assert_eq!(resume_id(TERMINAL_SHELL, Some(&report)), None);
        assert_eq!(resume_id(CLAUDE, None), None);
    }

    #[test]
    fn supervised_claude_asks_whatever_the_users_default() {
        // Restricted and Ask are Sandhill's hooks on top of the default.
        for permission in ["supervised", "restricted", "ask"] {
            let options = AgentOptions {
                permission: Some(permission.into()),
                ..Default::default()
            };
            let args = claude_args(&options, &context(), "--permission-mode");
            assert!(
                args.windows(2)
                    .any(|w| w == ["--permission-mode", "default"]),
                "{permission}"
            );
        }
    }

    #[test]
    fn codex_runs_under_sandhills_bridge_with_the_mcp_server() {
        let options = AgentOptions {
            permission: Some("full".into()),
            ..Default::default()
        };
        let args = codex_args(&options, &context(), Path::new("/usr/bin/codex"));
        assert_eq!(
            args[..5],
            [
                "--socket",
                "/state/sandhilld.sock",
                "codex-chat",
                "--codex",
                "/usr/bin/codex"
            ]
        );
        let settings: AgentOptions =
            serde_json::from_str(&args[args.iter().position(|a| a == "--options").unwrap() + 1])
                .unwrap();
        assert_eq!(settings, options);
        // After `--`: for `codex app-server`.
        let rest = &args[args.iter().position(|a| a == "--").unwrap() + 1..];
        assert!(
            rest.contains(&r#"mcp_servers.sandhill.command="/opt/it's/sandhilld""#.to_string())
        );
        assert!(rest.contains(
            &r#"mcp_servers.sandhill.args=["--socket", "/state/sandhilld.sock", "mcp", "--project", "/data/pbmc"]"#
                .to_string()
        ));
    }

    #[test]
    fn oh_my_pi_and_pi_run_under_their_bridges() {
        let context = Context {
            resume: Some("01a1-session"),
            ..context()
        };
        let omp = acp_args(
            OMP,
            &AgentOptions::default(),
            &context,
            Path::new("/bin/omp"),
        );
        assert_eq!(
            omp[..7],
            [
                "--socket",
                "/state/sandhilld.sock",
                "acp-chat",
                "--agent",
                "omp",
                "--program",
                "/bin/omp"
            ]
        );
        let pi = acp_args(PI, &AgentOptions::default(), &context, Path::new("/bin/pi"));
        assert_eq!(
            pi[..5],
            [
                "--socket",
                "/state/sandhilld.sock",
                "pi-chat",
                "--program",
                "/bin/pi"
            ]
        );
        for args in [omp, pi] {
            assert_eq!(args[args.len() - 2..], ["--session", "01a1-session"]);
        }
        assert_eq!(resume_key(OMP), Some("thread"));
        assert_eq!(resume_key(PI), Some("thread"));
    }

    #[test]
    fn posit_assistant_gets_the_mcp_server_beside_the_admins_defaults() {
        let defaults: Value = serde_json::from_str(&posit_defaults(&context(), None)).unwrap();
        assert_eq!(
            defaults,
            json!({"mcpServers": {"sandhill": {"type": "local", "command": [
                "/opt/it's/sandhilld", "--socket", "/state/sandhilld.sock", "mcp", "--project", "/data/pbmc"
            ]}}})
        );
        // What the daemon's environment already sets stays.
        let existing =
            r#"{"defaultModel": "posit/x", "mcpServers": {"db": {"url": "https://db"}}}"#;
        let defaults: Value =
            serde_json::from_str(&posit_defaults(&context(), Some(existing))).unwrap();
        assert_eq!(defaults["defaultModel"], "posit/x");
        assert_eq!(defaults["mcpServers"]["db"]["url"], "https://db");
        assert_eq!(defaults["mcpServers"]["sandhill"]["type"], "local");
        // Something pa couldn't read either is replaced.
        for broken in ["not json", "[1]", r#"{"mcpServers": 3}"#] {
            let defaults: Value =
                serde_json::from_str(&posit_defaults(&context(), Some(broken))).unwrap();
            assert_eq!(
                defaults["mcpServers"]["sandhill"]["type"], "local",
                "{broken}"
            );
        }
    }

    #[test]
    fn versions_are_read_from_each_cli() {
        assert_eq!(
            version_in("2.1.296 (Claude Code)").as_deref(),
            Some("2.1.296")
        );
        assert_eq!(version_in("codex-cli 0.162.1").as_deref(), Some("0.162.1"));
        assert_eq!(version_in("agy version 2.0.0").as_deref(), Some("2.0.0"));
        let pa = "Posit Assistant TUI v0.1.0\nBuild:  dev\nCommit: 8c1abe7\n";
        assert_eq!(version_in(pa).as_deref(), Some("0.1.0"));
        assert_eq!(version_in("no version here"), None);
    }
}
