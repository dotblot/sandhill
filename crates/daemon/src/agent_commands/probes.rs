use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REPLY_BYTES: u64 = 4 * 1024 * 1024;

pub(super) async fn list(kind: &str, cwd: &Path) -> Option<Vec<Value>> {
    let program = crate::agents::find(kind)?;
    let codex = kind == "codex";
    let mut command = Command::new(&program);
    if codex {
        if std::env::var("SANDHILL_CODEX_FILE_AUTH").as_deref() == Ok("1") {
            command.args(["-c", "cli_auth_credentials_store=\"file\""]);
        }
        command.arg("app-server");
    } else {
        command.args(["--mode", "rpc", "--no-session"]);
    }
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(&program).ok()? {
        command.env("PATH", path);
    }
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let mut lines = BufReader::new(child.stdout.take()?.take(MAX_REPLY_BYTES)).lines();
    let ask = async {
        if codex {
            send(&mut stdin, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "sandhill", "title": "Sandhill", "version": env!("CARGO_PKG_VERSION")}}})).await?;
        } else {
            send(&mut stdin, json!({"type": "get_commands"})).await?;
        }
        while let Some(line) = lines.next_line().await.ok()? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if codex {
                match message["id"].as_u64() {
                    Some(1) => {
                        if message.get("error").is_some() {
                            return None;
                        }
                        send(
                            &mut stdin,
                            json!({"jsonrpc": "2.0", "method": "initialized"}),
                        )
                        .await?;
                        send(&mut stdin, json!({"jsonrpc": "2.0", "id": 2, "method": "skills/list", "params": {"cwds": [cwd]}})).await?;
                    }
                    Some(2) => {
                        if message.get("error").is_some() {
                            return None;
                        }
                        return Some(codex_skills(&message["result"]));
                    }
                    _ => {}
                }
            } else if message["type"] == "response" && message["command"] == "get_commands" {
                return (message["success"] == true).then(|| pi_commands(&message["data"]));
            } else if message["type"] == "extension_ui_request" {
                // Startup extensions may ask a question; discovery never answers it.
                send(&mut stdin, json!({"type": "extension_ui_response", "id": message["id"], "cancelled": true})).await?;
            }
        }
        None
    };
    let result = tokio::time::timeout(TIMEOUT, ask).await.ok().flatten();
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

async fn send(stdin: &mut ChildStdin, message: Value) -> Option<()> {
    let mut bytes = serde_json::to_vec(&message).ok()?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.ok()?;
    stdin.flush().await.ok()
}

fn codex_skills(result: &Value) -> Vec<Value> {
    result["data"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["skills"].as_array().into_iter().flatten())
        .filter(|skill| skill["enabled"] != false)
        .filter_map(|skill| {
            let name = skill["name"].as_str()?;
            let path = skill["path"].as_str()?;
            let interface = &skill["interface"];
            let description = interface["short_description"]
                .as_str()
                .or_else(|| interface["shortDescription"].as_str())
                .or_else(|| skill["description"].as_str())
                .unwrap_or("");
            let mut item = super::item("skill", name, description);
            item["path"] = json!(path);
            Some(item)
        })
        .collect()
}

fn pi_commands(data: &Value) -> Vec<Value> {
    data["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|command| {
            let mut item = super::item(
                if command["source"] == "skill" {
                    "skill"
                } else {
                    "command"
                },
                command["name"].as_str()?,
                command["description"].as_str().unwrap_or(""),
            );
            if let Some(path) = command["sourceInfo"]["path"].as_str() {
                item["path"] = json!(path);
            }
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_returns_enabled_skills_and_prefers_interface_description() {
        assert_eq!(codex_skills(&json!({"data": [{"skills": [
            {"name": "a", "description": "long", "interface": {"shortDescription": " Short\n summary "}, "path": "/a/SKILL.md", "enabled": true},
            {"name": "b", "description": "long", "interface": {"short_description": "snake"}, "path": "/b/SKILL.md"},
            {"name": "c", "description": "fallback", "path": "/c/SKILL.md"},
            {"name": "disabled", "path": "/d/SKILL.md", "enabled": false}
        ]}]})), json!([
            {"kind": "skill", "name": "a", "description": "Short summary", "path": "/a/SKILL.md"},
            {"kind": "skill", "name": "b", "description": "snake", "path": "/b/SKILL.md"},
            {"kind": "skill", "name": "c", "description": "fallback", "path": "/c/SKILL.md"}
        ]).as_array().unwrap().to_vec());
    }

    #[test]
    fn pi_preserves_skill_prefix_and_maps_other_sources_to_commands() {
        assert_eq!(
            pi_commands(&json!({"commands": [
                {"source": "skill", "name": "skill:scan", "description": "Scan\n data"},
                {"source": "extension", "name": "extension"},
                {"source": "prompt", "name": "template", "sourceInfo": {"path": "/prompt.md"}}
            ]})),
            json!([
                {"kind": "skill", "name": "skill:scan", "description": "Scan data"},
                {"kind": "command", "name": "extension", "description": ""},
                {"kind": "command", "name": "template", "description": "", "path": "/prompt.md"}
            ])
            .as_array()
            .unwrap()
            .to_vec()
        );
    }
}
