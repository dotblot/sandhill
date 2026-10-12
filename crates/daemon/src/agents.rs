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
//! - for Codex, its `notify` program, which says when a turn ends.
//!
//! Sandhill never reads their logins: each signs in through its own
//! `/login` (or an API key the user set up for it).

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
    launch.args = match kind {
        CLAUDE => claude_args(options, context, &help),
        _ => codex_args(options, context, &help),
    };
    // An npm-installed CLI runs under `node` from the same folder, which may
    // not be on the daemon's PATH (as on a server reached over ssh).
    if let Some(dir) = program.parent() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        if !std::env::split_paths(&path).any(|p| p == dir) {
            let joined = std::env::join_paths(
                std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path)),
            )?;
            launch
                .env
                .push(("PATH".into(), joined.to_string_lossy().into_owned()));
        }
    }
    launch.program = program;
    Ok(launch)
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
    let hook = |what: &str| json!([{"hooks": [{"type": "command", "command": format!("{} terminal-event {what}", shell_quote(&exe))}]}]);
    // Adds where the user is to each prompt.
    let prompt = format!(
        "{} --socket {} terminal-event prompt{}",
        shell_quote(&exe),
        shell_quote(&context.socket.to_string_lossy()),
        context
            .project
            .map(|p| format!(" {}", shell_quote(p)))
            .unwrap_or_default()
    );
    let tool_hook = json!([{"matcher": "*", "hooks": [{"type": "command", "command": format!("{} terminal-event state busy", shell_quote(&exe))}]}]);
    let settings = json!({
        "hooks": {
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": prompt}]}],
            "PostToolUse": tool_hook,
            "Stop": hook("state idle"),
            "Notification": hook("notification"),
        },
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
    // user's own default, which may be one that asks less.
    let mode = match options.permission.as_deref() {
        Some("supervised") => Some("default"),
        Some("edits") => Some("acceptEdits"),
        Some("auto") if help.contains("\"auto\"") => Some("auto"),
        Some("full") => Some("bypassPermissions"),
        _ => None,
    };
    if let Some(mode) = mode
        && help.contains("--permission-mode")
    {
        args.extend(["--permission-mode".into(), mode.into()]);
    }
    args
}

fn codex_args(options: &AgentOptions, context: &Context<'_>, help: &str) -> Vec<String> {
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
        "-c".to_string(),
        format!("mcp_servers.sandhill.command={}", toml_string(&exe)),
        "-c".into(),
        format!("mcp_servers.sandhill.args={}", toml_array(&mcp_args)),
        "-c".into(),
        format!(
            "notify={}",
            toml_array(&[exe.into_owned(), "terminal-event".into(), "codex".into()])
        ),
        "-c".into(),
        format!(
            "developer_instructions={}",
            toml_string(&instructions(options))
        ),
    ];
    if let Some(model) = &options.model {
        args.extend(["-m".into(), model.clone()]);
    }
    if let Some(effort) = &options.effort {
        args.extend([
            "-c".into(),
            format!("model_reasoning_effort={}", toml_string(effort)),
        ]);
    }
    match options.permission.as_deref() {
        Some("supervised") => {
            args.extend(["-s", "read-only", "-a", "on-request"].map(String::from))
        }
        Some("edits") => {
            args.extend(["-s", "workspace-write", "-a", "on-request"].map(String::from))
        }
        Some("auto") if help.contains("--approve-for-me") => args.push("--approve-for-me".into()),
        Some("auto") => {
            args.extend(["-s", "workspace-write", "-a", "on-request"].map(String::from))
        }
        Some("full") => args.push("--dangerously-bypass-approvals-and-sandbox".into()),
        _ => {}
    }
    args
}

/// What an agent is told about Sandhill when it starts.
fn instructions(options: &AgentOptions) -> String {
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
    if !options.documents.is_empty() {
        text.push_str(&format!(
            " The user started this session about: {}; add_cell writes there.",
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
async fn help_text(program: &Path) -> String {
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
        }
    }

    #[test]
    fn claude_gets_the_mcp_server_hooks_and_only_flags_it_knows() {
        let options = AgentOptions {
            model: Some("opus".into()),
            effort: Some("high".into()),
            permission: Some("edits".into()),
            documents: vec!["analysis.qmd".into()],
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
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            r"'/opt/it'\''s/sandhilld' terminal-event state idle"
        );
        assert!(args[5].contains("analysis.qmd"));
        assert!(args.windows(2).any(|w| w == ["--model", "opus"]));
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-mode", "acceptEdits"])
        );
        // This CLI's help doesn't list --effort.
        assert!(!args.contains(&"--effort".to_string()));
    }

    #[test]
    fn supervised_claude_asks_whatever_the_users_default() {
        let options = AgentOptions {
            permission: Some("supervised".into()),
            ..Default::default()
        };
        let args = claude_args(&options, &context(), "--permission-mode");
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-mode", "default"])
        );
    }

    #[test]
    fn codex_gets_its_settings_as_toml() {
        let options = AgentOptions {
            permission: Some("full".into()),
            ..Default::default()
        };
        let args = codex_args(&options, &context(), "");
        assert!(
            args.contains(&r#"mcp_servers.sandhill.command="/opt/it's/sandhilld""#.to_string())
        );
        assert!(args.contains(
            &r#"mcp_servers.sandhill.args=["--socket", "/state/sandhilld.sock", "mcp", "--project", "/data/pbmc"]"#
                .to_string()
        ));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
    }
}
