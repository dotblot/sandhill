//! `sandhilld terminal-event`: what agents run from their hooks to tell
//! Sandhill how they are doing. The terminal they run in is named by
//! `SANDHILL_TERMINAL`, which the daemon sets; outside a Sandhill terminal
//! this does nothing.
//!
//! - `state busy|idle|waiting`: Claude Code's prompt, tool and stop hooks.
//! - `notification`: Claude Code's Notification hook (JSON on stdin).
//!   Asking for permission or input means waiting for the user; the
//!   reminder it sends after a while idle doesn't.
//! - `status`: Claude Code's status line (JSON on stdin): model, effort,
//!   context use and plan limits. Prints the user's own status line, if
//!   they set one up, since this one takes its place.
//! - `codex`: Codex's `notify` program, run when a turn ends.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const STDIN_TIMEOUT: Duration = Duration::from_secs(2);
const REPORT_TIMEOUT: Duration = Duration::from_secs(1);
const STATUS_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

pub async fn run(args: &[String]) -> Result<()> {
    let terminal = std::env::var_os("SANDHILL_TERMINAL").map(PathBuf::from);
    let what = args.first().map(String::as_str).unwrap_or("");
    match what {
        "state" => {
            let state = args.get(1).map(String::as_str).unwrap_or("");
            if !["busy", "idle", "waiting"].contains(&state) {
                bail!("unknown state {state:?}");
            }
            send(terminal.as_deref(), json!({"state": state})).await
        }
        "notification" => {
            let input = read_stdin().await;
            let state = match input["notification_type"].as_str() {
                Some("idle_prompt") => "idle",
                _ => "waiting",
            };
            send(
                terminal.as_deref(),
                json!({"state": state, "message": input["message"]}),
            )
            .await
        }
        "status" => {
            let raw = read_stdin_bytes().await;
            let input: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            let report = send(terminal.as_deref(), json!({"status": status(&input)}));
            let own = own_status_line(&input, &raw);
            let (reported, line) = tokio::join!(report, own);
            if let Some(line) = line {
                print!("{line}");
            }
            reported
        }
        "codex" => {
            // The JSON argument says what happened; a finished turn is the
            // only kind Codex sends.
            send(terminal.as_deref(), json!({"state": "idle"})).await
        }
        _ => bail!("unknown event {what:?}"),
    }
}

async fn send(terminal: Option<&Path>, value: Value) -> Result<()> {
    let Some(terminal) = terminal else {
        return Ok(());
    };
    tokio::time::timeout(REPORT_TIMEOUT, super::report(terminal, value))
        .await
        .unwrap_or(Ok(()))
}

/// What the app shows of Claude Code's status line input.
fn status(input: &Value) -> Value {
    let mut out = json!({
        "model": input["model"]["display_name"],
        "model_id": input["model"]["id"],
        "context": input["context_window"]["used_percentage"],
        "context_size": input["context_window"]["context_window_size"],
        "effort": input["effort"]["level"],
        "rate_limits": input["rate_limits"],
        "cost_usd": input["cost"]["total_cost_usd"],
        "version": input["version"],
    });
    if let Some(map) = out.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    out
}

async fn read_stdin_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = tokio::time::timeout(STDIN_TIMEOUT, tokio::io::stdin().read_to_end(&mut bytes)).await;
    bytes
}

async fn read_stdin() -> Value {
    serde_json::from_slice(&read_stdin_bytes().await).unwrap_or(Value::Null)
}

/// Runs the status line command the user configured for Claude Code, so
/// they keep it.
async fn own_status_line(input: &Value, raw: &[u8]) -> Option<String> {
    let command = own_status_command(input["workspace"]["project_dir"].as_str())?;
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let raw = raw.to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&raw).await;
    });
    let output = tokio::time::timeout(STATUS_COMMAND_TIMEOUT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `statusLine` command from the user's Claude Code settings, in the
/// order Claude Code reads them: the project's local settings, the
/// project's, then the user's.
fn own_status_command(project: Option<&str>) -> Option<String> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".claude")))?;
    let mut files = Vec::new();
    if let Some(project) = project {
        let dir = Path::new(project).join(".claude");
        files.push(dir.join("settings.local.json"));
        files.push(dir.join("settings.json"));
    }
    files.push(config.join("settings.json"));
    files.into_iter().find_map(|file| {
        let settings: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        let line = &settings["statusLine"];
        let command = line["command"].as_str()?;
        (line["type"] == "command" && !command.contains("terminal-event"))
            .then(|| command.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_keeps_what_the_app_shows() {
        let input = json!({
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "context_window": {"used_percentage": 38, "context_window_size": 200000},
            "rate_limits": {"five_hour": {"used_percentage": 42, "resets_at": "2026-10-12T05:00:00Z"}},
            "workspace": {"current_dir": "/data"},
        });
        assert_eq!(
            status(&input),
            json!({
                "model": "Opus 5.5",
                "model_id": "claude-opus-5-5",
                "context": 38,
                "context_size": 200000,
                "rate_limits": {"five_hour": {"used_percentage": 42, "resets_at": "2026-10-12T05:00:00Z"}},
            })
        );
    }
}
