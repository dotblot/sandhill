//! The coding agents Sandhill runs: Claude Code and Codex as their own,
//! unmodified CLIs, each in a terminal the daemon keeps (see `terminal`).
//! Sandhill only configures them the way a user could, with command-line
//! flags:
//!
//! - its MCP server (`sandhilld mcp`), so they can reach the project's live
//!   R and Python sessions;
//! - for Claude Code, hooks and a status line that report what it is doing
//!   (busy, waiting for the user, model, context use) to its terminal
//!   through `sandhilld terminal-event`;
//! - for Codex, nothing: Sandhill drives its conversation through `codex
//!   app-server` (see `terminal::codex`), and tells it the same things.
//!
//! Sandhill never reads their logins: each signs in through its own
//! `/login` or sign-in (or an API key the user set up for it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use sandhill_proto::{AgentInfo, AgentOptions, TERMINAL_SHELL};
use serde_json::json;

use crate::terminal::Launch;

pub const CLAUDE: &str = "claude";
pub const CODEX: &str = "codex";

struct Agent {
    kind: &'static str,
    name: &'static str,
    program: &'static str,
    install: &'static str,
}

const AGENTS: [Agent; 2] = [
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
];

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
/// session (from its hooks) and Codex's thread (from Sandhill's bridge).
pub const RESUME_KEYS: [&str; 2] = ["session_id", "thread"];

/// The conversation an agent of `kind` would resume, from its report.
pub fn resume_id(kind: &str, report: Option<&serde_json::Value>) -> Option<String> {
    let key = match kind {
        CLAUDE => RESUME_KEYS[0],
        CODEX => RESUME_KEYS[1],
        _ => return None,
    };
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
    launch.title = agent.name.into();
    match kind {
        CLAUDE => {
            launch.args = claude_args(options, context, &help);
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

fn claude_args(options: &AgentOptions, context: &Context<'_>, help: &str) -> Vec<String> {
    let exe = context.executable.to_string_lossy();
    let mut mcp_args = vec![
        "--socket".to_string(),
        context.socket.to_string_lossy().into_owned(),
        "mcp".into(),
    ];
    if let Some(project) = context.project {
        mcp_args.extend(["--project".into(), project.into()]);
    }
    let mcp =
        json!({"mcpServers": {"sandhill": {"type": "stdio", "command": exe, "args": mcp_args}}});
    // One command for every hook: it reports the agent's state, keeps its
    // conversation for the chat, adds where the user is to each prompt, and
    // asks the user in the app for permissions.
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
    let mut mcp_args = vec![
        "--socket".to_string(),
        context.socket.to_string_lossy().into_owned(),
        "mcp".into(),
    ];
    if let Some(project) = context.project {
        mcp_args.extend(["--project".into(), project.into()]);
    }
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
         for quick looks they don't need to keep. get_focus says where the user is. Files you \
         edit show up in the user's open editor.",
    );
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
    }
    text
}

/// The user's login shell.
fn shell() -> PathBuf {
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

async fn version(program: &Path) -> Option<String> {
    let output = run(program, "--version").await?;
    // "2.1.296 (Claude Code)", "codex-cli 0.162.1"
    output
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
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

/// Quotes a word for `sh`, which runs hook commands.
fn shell_quote(word: &str) -> String {
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
}
