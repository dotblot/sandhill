//! An agent's conversation, as its hooks report it: the user's messages,
//! the agent's replies, each step it takes and its result, and what it asks
//! the user. The terminal host keeps it in `chat.jsonl`, one [`ChatEntry`]
//! per line, so the app can show the conversation as a chat and replay it
//! after reconnecting, while the agent runs unmodified in its terminal.
//!
//! Each entry's `event` has a `kind`, and `at_ms`, when the hook started
//! (hooks run side by side, so entries can arrive a little out of order):
//!
//! - `prompt {turn, text, place?}`: a message from the user, with where
//!   they were in their document.
//! - `text {turn, message, index, text, final}`: part of a reply. The parts
//!   of one message come in `index` order; the last can follow `stop`.
//! - `step {turn, id, tool, input}`, then `step_done {turn, id, output, ms}`
//!   or `step_failed {turn, id, error, interrupted}`.
//! - `ask {turn, id, tool, input, remember?}`: the agent waits for the
//!   user's permission (or answers, for a question) until an `answered {id,
//!   answer}` entry. `answer` is null when the agent stopped waiting.
//!   `remember` lists what allowing it for the chat or the project would
//!   allow from then on (`Bash(npm test:*)`, `files in /data`).
//! - `stop {turn}`: the agent finished its turn.
//! - `session {source, model?}`: the agent's session started, resumed,
//!   was cleared or compacted.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use sandhill_proto::ChatEntry;
use serde_json::{Map, Value, json};

/// Longer text in a step's input or output is shortened, keeping its start
/// and end. Images are kept up to [`MAX_IMAGE`].
const MAX_TEXT: usize = 32 * 1024;
const MAX_IMAGE: usize = 4 * 1024 * 1024;

pub fn path(dir: &Path) -> PathBuf {
    dir.join("chat.jsonl")
}

/// The conversation file, being written by the terminal host.
pub struct ChatLog {
    file: File,
    next: u64,
}

impl ChatLog {
    pub fn open(dir: &Path) -> Result<ChatLog> {
        let next = read(dir, 0).last().map_or(0, |e| e.seq + 1);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path(dir))?;
        Ok(ChatLog { file, next })
    }

    /// The `seq` the next entry gets.
    pub fn next(&self) -> u64 {
        self.next
    }

    pub fn append(&mut self, event: Value) -> Result<ChatEntry> {
        let entry = ChatEntry {
            seq: self.next,
            event,
        };
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.next += 1;
        Ok(entry)
    }
}

/// The entries from `from` on.
pub fn read(dir: &Path, from: u64) -> Vec<ChatEntry> {
    let Ok(file) = File::open(path(dir)) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<ChatEntry>(&line).ok())
        .filter(|entry| entry.seq >= from)
        .collect()
}

/// What a Claude Code hook's input adds to the conversation, if anything.
/// `place` is where the user was, for a prompt.
pub fn from_claude(input: &Value, at_ms: i64, place: Option<Value>) -> Option<Value> {
    let turn = &input["prompt_id"];
    let mut event = match input["hook_event_name"].as_str()? {
        "UserPromptSubmit" => json!({
            "kind": "prompt",
            "turn": turn,
            "text": input["prompt"],
            "place": place,
        }),
        "MessageDisplay" => json!({
            "kind": "text",
            "turn": turn,
            "message": input["message_id"],
            "index": input["index"],
            "text": input["delta"],
            "final": input["final"],
        }),
        "PreToolUse" => json!({
            "kind": "step",
            "turn": turn,
            "id": input["tool_use_id"],
            "tool": input["tool_name"],
            "input": shorten(&input["tool_input"]),
        }),
        "PostToolUse" => json!({
            "kind": "step_done",
            "turn": turn,
            "id": input["tool_use_id"],
            "tool": input["tool_name"],
            "output": shorten(&input["tool_response"]),
            "ms": input["duration_ms"],
        }),
        "PostToolUseFailure" => json!({
            "kind": "step_failed",
            "turn": turn,
            "id": input["tool_use_id"],
            "tool": input["tool_name"],
            "error": shorten(&input["error"]),
            "interrupted": input["is_interrupt"],
        }),
        "Stop" => json!({"kind": "stop", "turn": turn}),
        "SessionStart" => json!({
            "kind": "session",
            "source": input["source"],
            "model": input["model"],
        }),
        _ => return None,
    };
    let map = event.as_object_mut()?;
    map.retain(|_, v| !v.is_null());
    map.insert("at_ms".into(), json!(at_ms));
    Some(event)
}

/// A permission request (or a question) from Claude Code, as an `ask`.
pub fn claude_ask(input: &Value, id: &str, at_ms: i64) -> Value {
    let mut event = json!({
        "kind": "ask",
        "turn": input["prompt_id"],
        "id": id,
        "tool": input["tool_name"],
        "input": shorten(&input["tool_input"]),
        "at_ms": at_ms,
    });
    let remember: Vec<String> = input["permission_suggestions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(suggestion_text)
        .collect();
    if !remember.is_empty() {
        event["remember"] = json!(remember);
    }
    if let Some(map) = event.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }
    event
}

/// What a permission rule Claude Code suggests allows, in words.
fn suggestion_text(suggestion: &Value) -> Vec<String> {
    let list =
        |key: &str| -> Vec<&Value> { suggestion[key].as_array().into_iter().flatten().collect() };
    match suggestion["type"].as_str() {
        Some("addRules") | Some("replaceRules") => list("rules")
            .into_iter()
            .filter_map(|rule| {
                let tool = rule["toolName"].as_str()?;
                Some(match rule["ruleContent"].as_str() {
                    Some(content) => format!("{tool}({content})"),
                    None => tool.to_string(),
                })
            })
            .collect(),
        Some("addDirectories") => list("directories")
            .into_iter()
            .filter_map(|d| d.as_str().map(|d| format!("files in {d}")))
            .collect(),
        Some("setMode") => suggestion["mode"]
            .as_str()
            .map(|m| vec![format!("the {m} mode")])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Claude Code's answer to its `PermissionRequest` hook, from the user's
/// `answer` (see `AnswerTerminal`). Allowing for the chat or the project
/// takes the rules Claude Code suggested, kept for that long.
pub fn claude_decision(input: &Value, answer: &Value) -> Value {
    let decision = if answer["behavior"] == "allow" {
        let mut decision = json!({"behavior": "allow"});
        // Claude Code takes a plan as approved only with `updatedInput`;
        // otherwise it still asks in the terminal.
        if input["tool_name"] == "ExitPlanMode" {
            decision["updatedInput"] = input["tool_input"].clone();
        }
        if let Some(answers) = answer["answers"].as_object() {
            // A question: its answers go with the tool's input.
            let mut updated = input["tool_input"].clone();
            if let Some(map) = updated.as_object_mut() {
                map.insert("answers".into(), Value::Object(answers.clone()));
            }
            decision["updatedInput"] = updated;
        }
        let destination = match answer["scope"].as_str() {
            Some("chat") => Some("session"),
            Some("project") => Some("localSettings"),
            _ => None,
        };
        if let (Some(destination), Some(suggestions)) =
            (destination, input["permission_suggestions"].as_array())
        {
            let rules: Vec<Value> = suggestions
                .iter()
                .cloned()
                .map(|mut s| {
                    s["destination"] = json!(destination);
                    s
                })
                .collect();
            if !rules.is_empty() {
                decision["updatedPermissions"] = Value::Array(rules);
            }
        }
        decision
    } else {
        let message = answer["message"]
            .as_str()
            .unwrap_or("The user said no in Sandhill. Ask them what to do instead.");
        json!({"behavior": "deny", "message": message})
    };
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}})
}

/// `value` with long strings shortened. Image data is kept, within reason.
fn shorten(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() > MAX_TEXT => Value::String(shorten_text(text)),
        Value::Array(items) => Value::Array(items.iter().map(shorten).collect()),
        Value::Object(map) => {
            // MCP's `{type: "image", data}`, or the API's `{type: "image",
            // source: {type: "base64", media_type, data}}` that hooks get.
            let image = match map.get("type").and_then(Value::as_str) {
                Some("image") => true,
                Some("base64") => map
                    .get("media_type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.starts_with("image/")),
                _ => false,
            };
            Value::Object(
                map.iter()
                    .map(|(key, value)| {
                        let keep = image
                            && key == "data"
                            && value.as_str().is_some_and(|d| d.len() <= MAX_IMAGE);
                        let value = if keep { value.clone() } else { shorten(value) };
                        (key.clone(), value)
                    })
                    .collect::<Map<_, _>>(),
            )
        }
        other => other.clone(),
    }
}

fn shorten_text(text: &str) -> String {
    let half = MAX_TEXT / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let left = text[head..tail].lines().count();
    format!(
        "{}\n… {left} more lines …\n{}",
        &text[..head],
        &text[tail..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hook inputs Claude Code 2.1 sent for one turn: a prompt, a command
    /// that needed permission, and the reply.
    fn recorded() -> Vec<Value> {
        let text = include_str!("../../../../tests/fixtures/claude-hooks.jsonl");
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn a_recorded_turn_becomes_a_conversation() {
        let events: Vec<Value> = recorded()
            .iter()
            .filter_map(|input| from_claude(input, 1, None))
            .collect();
        let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "session",
                "prompt",
                "step",
                "step_done",
                "text",
                "text",
                "text",
                "stop"
            ]
        );
        let turn = &events[1]["turn"];
        assert!(turn.is_string());
        assert!(events[1..].iter().all(|e| &e["turn"] == turn));
        assert!(
            events[1]["text"]
                .as_str()
                .unwrap()
                .starts_with("Run the shell command")
        );
        assert_eq!(events[2]["tool"], "Bash");
        assert_eq!(events[2]["id"], events[3]["id"]);
        assert!(events[2]["input"]["command"].is_string());
        assert!(events[3]["output"]["stdout"].is_string());
        assert_eq!(events[6]["final"], true);
        assert_eq!(events[6]["index"], 2);
        assert!(events.iter().all(|e| e["at_ms"] == 1));
        // Paths and session ids stay out.
        assert!(events.iter().all(|e| e.get("transcript_path").is_none()));
    }

    #[test]
    fn permission_answers_become_claude_decisions() {
        let request = recorded()
            .into_iter()
            .find(|e| e["hook_event_name"] == "PermissionRequest")
            .unwrap();
        let ask = claude_ask(&request, "a1", 5);
        assert_eq!(ask["kind"], "ask");
        assert_eq!(ask["tool"], "Bash");
        assert_eq!(ask["id"], "a1");
        assert_eq!(ask["remember"], json!(["files in /data/pbmc-atlas"]));
        let rules = json!({"permission_suggestions": [{"type": "addRules", "rules": [
            {"toolName": "Bash", "ruleContent": "npm test:*"}, {"toolName": "mcp__sandhill__add_cell"}
        ]}]});
        assert_eq!(
            claude_ask(&rules, "a2", 5)["remember"],
            json!(["Bash(npm test:*)", "mcp__sandhill__add_cell"])
        );

        let decision = |answer: Value| {
            claude_decision(&request, &answer)["hookSpecificOutput"]["decision"].clone()
        };
        assert_eq!(
            decision(json!({"behavior": "allow"})),
            json!({"behavior": "allow"})
        );
        let chat = decision(json!({"behavior": "allow", "scope": "chat"}));
        assert_eq!(chat["updatedPermissions"][0]["destination"], "session");
        let project = decision(json!({"behavior": "allow", "scope": "project"}));
        assert_eq!(
            project["updatedPermissions"][0]["destination"],
            "localSettings"
        );
        let denied = decision(json!({"behavior": "deny"}));
        assert_eq!(denied["behavior"], "deny");
        assert!(denied["message"].as_str().unwrap().contains("Sandhill"));
    }

    #[test]
    fn questions_are_answered_with_the_input() {
        let request = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"question": "Which colour?", "options": [{"label": "Red"}, {"label": "Blue"}]}]},
        });
        let answer = json!({"behavior": "allow", "answers": {"Which colour?": "Blue"}});
        let decision = &claude_decision(&request, &answer)["hookSpecificOutput"]["decision"];
        assert_eq!(decision["updatedInput"]["answers"]["Which colour?"], "Blue");
        assert_eq!(
            decision["updatedInput"]["questions"][0]["question"],
            "Which colour?"
        );
    }

    #[test]
    fn plans_are_approved_with_the_input() {
        let request = json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "ExitPlanMode",
            "tool_input": {"plan": "1. Add the cell."},
        });
        let allow = json!({"behavior": "allow", "scope": "once"});
        let decision = &claude_decision(&request, &allow)["hookSpecificOutput"]["decision"];
        assert_eq!(decision["updatedInput"]["plan"], "1. Add the cell.");
        // Other tools are allowed as they are.
        let request = json!({"tool_name": "Bash", "tool_input": {"command": "ls"}});
        let decision = &claude_decision(&request, &allow)["hookSpecificOutput"]["decision"];
        assert!(decision.get("updatedInput").is_none());
    }

    #[test]
    fn long_output_is_shortened_but_images_are_kept() {
        let long = "line\n".repeat(20_000);
        let image = "A".repeat(100_000);
        let value = json!({
            "stdout": long,
            "content": [
                {"type": "image", "data": image, "mimeType": "image/png"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": image}},
            ],
        });
        let short = shorten(&value);
        let stdout = short["stdout"].as_str().unwrap();
        assert!(stdout.len() < MAX_TEXT + 100);
        assert!(stdout.contains("more lines"));
        assert_eq!(short["content"][0]["data"].as_str().unwrap().len(), 100_000);
        let source = &short["content"][1]["source"]["data"];
        assert_eq!(source.as_str().unwrap().len(), 100_000);
    }

    #[test]
    fn the_log_counts_on_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = ChatLog::open(dir.path()).unwrap();
        assert_eq!(log.append(json!({"kind": "stop"})).unwrap().seq, 0);
        assert_eq!(log.append(json!({"kind": "stop"})).unwrap().seq, 1);
        drop(log);
        let mut log = ChatLog::open(dir.path()).unwrap();
        assert_eq!(log.next(), 2);
        log.append(json!({"kind": "stop"})).unwrap();
        assert_eq!(read(dir.path(), 1).len(), 2);
    }
}
