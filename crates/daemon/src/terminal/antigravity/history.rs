//! Read conversation records through agy's official ACP `session/load` replay.
//! No prompts, terminal parsing, private files, or provider credentials.

use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::{Bridge, COMMAND_TIMEOUT};

pub(super) async fn load(bridge: &Bridge, conversation: &str) -> Result<Vec<Value>> {
    let mut command = super::account_command(&bridge.agy);
    command
        .arg("acp")
        .current_dir(&bridge.cwd)
        .env("SANDHILL_SOCKET", &bridge.daemon)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    super::headless(&mut command);
    if let Some(project) = &bridge.project {
        command.env("SANDHILL_PROJECT", project);
    }
    let mut child = command
        .spawn()
        .context("cannot start agy ACP history reader")?;
    let mut input = child.stdin.take().context("no ACP stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no ACP stdout")?).lines();
    let result = tokio::time::timeout(COMMAND_TIMEOUT, async {
        let mut records = Vec::new();
        // ACP does not select the API-key method from --settings itself.
        // It reads the inherited GEMINI_API_KEY; never send the key in JSON.
        let authenticate = super::api_key_account()
            .then(|| (3, "authenticate", json!({"methodId":"gemini-api-key"})));
        for (id, method, params) in [
            (1, "initialize", json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"sandhill","version":env!("CARGO_PKG_VERSION")}})),
        ]
        .into_iter()
        .chain(authenticate)
        .chain([
            (2, "session/load", json!({"sessionId":conversation,"cwd":bridge.cwd,"mcpServers":[]})),
        ]) {
            let mut request = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
            request.push(b'\n');
            input.write_all(&request).await?;
            loop {
                let line = lines.next_line().await?.context("agy ACP closed during history loading")?;
                let message: Value = serde_json::from_str(&line).context("invalid agy ACP history response")?;
                if message["id"] == id && message.get("method").is_none() {
                    if let Some(error) = message.get("error") {
                        bail!("agy {method}: {error}");
                    }
                    if id == 1 {
                        let capabilities = &message["result"]["agentCapabilities"];
                        if capabilities["loadSession"] != true && capabilities["sessionCapabilities"].get("load").is_none() {
                            bail!("this agy does not support ACP conversation history loading");
                        }
                    }
                    break;
                }
                if message["method"] == "session/update" && message["params"]["sessionId"] == conversation {
                    let update = &message["params"]["update"];
                    if matches!(update["sessionUpdate"].as_str(), Some("user_message_chunk" | "agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update")) {
                        records.push(update.clone());
                    }
                } else if message.get("method").is_some() && message.get("id").is_some() {
                    // A history-only reader must never approve or execute work.
                    let mut reply = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Sandhill history reader cannot drive agent work"}}))?;
                    reply.push(b'\n');
                    input.write_all(&reply).await?;
                }
            }
        }
        Ok::<_, anyhow::Error>(records)
    }).await.context("agy history loading took too long").and_then(|result| result);
    // Reap the read-only ACP owner before native/headless may load this ID.
    // kill() also waits; kill_on_drop covers cancellation of this function.
    let released = child
        .kill()
        .await
        .context("cannot release agy ACP history reader");
    let records = result?;
    released?;
    Ok(records)
}

fn content_text(content: &Value) -> String {
    content["text"]
        .as_str()
        .or(content["resource"]["text"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| content.to_string())
}

pub(super) async fn import(
    bridge: &Bridge,
    conversation: &str,
    before: &[Value],
    after: &[Value],
) -> Result<()> {
    if !after.starts_with(before) {
        bail!("agy rewrote the conversation history; refusing to duplicate existing chat turns");
    }
    let mut turn: Option<String> = None;
    let mut user = String::new();
    let mut user_sent = false;
    let mut message = 0u64;
    let mut tools = std::collections::HashMap::new();
    for (offset, update) in after.iter().enumerate().skip(before.len()) {
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        if kind == "user_message_chunk" {
            if user_sent {
                if let Some(id) = turn.take() {
                    bridge.chat(json!({"kind":"stop","turn":id})).await;
                }
                user.clear();
                user_sent = false;
            }
            turn.get_or_insert_with(|| format!("agy-native-{conversation}-{offset}"));
            user.push_str(&content_text(&update["content"]));
            continue;
        }
        let id = turn.get_or_insert_with(|| format!("agy-native-{conversation}-{offset}"));
        if !user_sent && !user.is_empty() {
            bridge
                .chat(json!({"kind":"prompt","turn":id,"text":user}))
                .await;
        }
        user_sent = true;
        match kind {
            "agent_message_chunk" => {
                bridge.chat(json!({"kind":"text","turn":id,"message":format!("{id}-r{message}"),"index":0,"text":content_text(&update["content"]),"final":true})).await;
                message += 1;
            }
            "agent_thought_chunk" => {
                bridge.chat(json!({"kind":"text","turn":id,"message":format!("{id}-thought{message}"),"index":0,"text":content_text(&update["content"]),"final":true,"thinking":true})).await;
                message += 1;
            }
            "tool_call" | "tool_call_update" => {
                let call = update["toolCallId"]
                    .as_str()
                    .context("agy history tool has no ID")?;
                let tool = tools.entry(call.to_string()).or_insert_with(|| {
                    match update["kind"].as_str() {
                        Some("execute") => "Bash".to_string(),
                        Some("edit") => "Edit".to_string(),
                        Some("read") => "Read".to_string(),
                        _ => update["title"].as_str().unwrap_or("tool").to_string(),
                    }
                });
                let step = format!("{id}-{call}");
                if kind == "tool_call" {
                    bridge.chat(json!({"kind":"step","turn":id,"id":step,"tool":tool,"input":update["rawInput"]})).await;
                }
                if matches!(update["status"].as_str(), Some("completed" | "failed")) {
                    let failed = update["status"] == "failed";
                    let output = update
                        .get("rawOutput")
                        .or(update.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mut event = json!({"kind":if failed {"step_failed"} else {"step_done"},"turn":id,"id":step,"tool":tool});
                    event[if failed { "error" } else { "output" }] = output;
                    bridge.chat(event).await;
                }
            }
            _ => {}
        }
    }
    if let Some(id) = turn {
        if !user_sent && !user.is_empty() {
            bridge
                .chat(json!({"kind":"prompt","turn":id,"text":user}))
                .await;
        }
        bridge.chat(json!({"kind":"stop","turn":id})).await;
    }
    Ok(())
}
