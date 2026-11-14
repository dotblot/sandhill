//! pi's RPC events (`message_update`, `tool_execution_*`, `agent_settled`,
//! ...) and its saved session entries, as the chat entries Claude Code's
//! hooks make ([`super::super::chat`]). Pure: the bridge feeds events in and
//! sends what comes out, so recorded lines test it.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::super::chat::shorten;

/// One reply or reasoning block being streamed.
#[derive(Default)]
struct Stream {
    /// Reasoning (a `reasoning` step) rather than reply text.
    thinking: bool,
    id: String,
    pending: String,
    text: String,
    index: u64,
}

/// What a step was before it ended, for its `step_done`.
struct Step {
    tool: String,
    input: Value,
}

/// Turns one live conversation's events into chat entries, without
/// `at_ms`.
#[derive(Default)]
pub struct Translator {
    /// The Sandhill turn the events belong to.
    pub turn: Option<String>,
    serial: u64,
    stream: Option<Stream>,
    steps: HashMap<String, Step>,
    /// Steps Sandhill refused (Ask mode), shown as such in the chat.
    blocked: HashMap<String, String>,
    /// The last reply's error, said when the turn ends unless pi retried.
    error: Option<String>,
}

impl Translator {
    /// Begins the turn `id`, ending any open one first.
    pub fn begin(&mut self, id: String) -> Vec<Value> {
        let mut out = if self.turn.is_some() {
            self.finish(false)
        } else {
            Vec::new()
        };
        out.extend(self.flush(true));
        self.turn = Some(id);
        out
    }

    /// The chat says `reason` for a step Sandhill would not run.
    pub fn block(&mut self, step: &str, reason: &str) {
        self.blocked.insert(step.to_string(), reason.to_string());
    }

    /// Whether a step is open, by pi's id.
    pub fn step(&self, id: &str) -> Option<(&str, &Value)> {
        self.steps
            .get(id)
            .map(|step| (step.tool.as_str(), &step.input))
    }

    /// One event from pi's output.
    pub fn event(&mut self, event: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        match event["type"].as_str().unwrap_or_default() {
            "message_update" => {
                let update = &event["assistantMessageEvent"];
                match update["type"].as_str().unwrap_or_default() {
                    "text_delta" => out.extend(self.append(false, &update["delta"])),
                    "thinking_delta" => out.extend(self.append(true, &update["delta"])),
                    "text_end" | "thinking_end" => out.extend(self.flush(true)),
                    _ => {}
                }
            }
            "message_end" if event["message"]["role"] == "assistant" => {
                out.extend(self.flush(true));
                let message = &event["message"];
                match message["stopReason"].as_str() {
                    Some("error") => {
                        self.error = Some(
                            message["errorMessage"]
                                .as_str()
                                .unwrap_or("pi's model request failed")
                                .to_string(),
                        );
                    }
                    Some("aborted") => {}
                    _ => self.error = None,
                }
            }
            "tool_execution_start" => {
                out.extend(self.flush(true));
                let Some(id) = event["toolCallId"].as_str() else {
                    return out;
                };
                let (tool, input) =
                    tool_step(event["toolName"].as_str().unwrap_or("tool"), &event["args"]);
                out.push(json!({"kind": "step", "turn": self.turn, "id": id, "tool": tool, "input": input}));
                self.steps.insert(id.to_string(), Step { tool, input });
            }
            "tool_execution_end" => {
                let Some(id) = event["toolCallId"].as_str() else {
                    return out;
                };
                let step = self.steps.remove(id).unwrap_or_else(|| {
                    let (tool, input) =
                        tool_step(event["toolName"].as_str().unwrap_or("tool"), &event["args"]);
                    Step { tool, input }
                });
                let mut end = step_end(
                    &step.tool,
                    &step.input,
                    &event["result"],
                    event["isError"] == true,
                );
                if let Some(reason) = self.blocked.remove(id) {
                    end = json!({"kind": "step_failed", "tool": step.tool, "error": reason});
                }
                end["turn"] = json!(self.turn);
                end["id"] = json!(id);
                if let Some(ms) = event["durationMs"].as_f64() {
                    end["ms"] = json!(ms.round() as u64);
                }
                out.push(end);
            }
            "compaction_end" if event["result"].is_object() => {
                out.extend(self.flush(true));
                out.push(json!({"kind": "session", "source": "compact"}));
            }
            "compaction_end" if event["errorMessage"].is_string() => {
                out.push(json!({"kind": "error", "turn": self.turn,
                    "text": format!("pi couldn't compact the conversation: {}", event["errorMessage"].as_str().unwrap_or_default())}));
            }
            "auto_retry_end" if event["success"] == false => {
                if let Some(error) = event["finalError"].as_str() {
                    self.error = Some(error.to_string());
                }
            }
            "auto_retry_end" => self.error = None,
            _ => {}
        }
        out
    }

    fn append(&mut self, thinking: bool, delta: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        if self.stream.as_ref().is_some_and(|s| s.thinking != thinking) {
            out.extend(self.flush(true));
        }
        if self.stream.is_none() {
            self.serial += 1;
            self.stream = Some(Stream {
                thinking,
                id: format!(
                    "{}:message:{}",
                    self.turn.as_deref().unwrap_or("pi"),
                    self.serial
                ),
                ..Stream::default()
            });
        }
        if let (Some(stream), Some(delta)) = (self.stream.as_mut(), delta.as_str()) {
            stream.pending.push_str(delta);
        }
        out
    }

    /// What has streamed since the last flush: a piece of the reply, or the
    /// reasoning so far. `final_piece` ends the block.
    pub fn flush(&mut self, final_piece: bool) -> Vec<Value> {
        let mut out = Vec::new();
        let Some(stream) = self.stream.as_mut() else {
            return out;
        };
        if stream.thinking {
            let changed = !stream.pending.is_empty();
            stream.text.push_str(&std::mem::take(&mut stream.pending));
            if (changed || final_piece) && !stream.text.trim().is_empty() {
                out.push(json!({"kind": "step", "turn": self.turn, "id": stream.id, "tool": "reasoning", "input": {"text": stream.text}}));
                if final_piece {
                    out.push(json!({"kind": "step_done", "turn": self.turn, "id": stream.id, "tool": "reasoning", "output": {"text": stream.text}}));
                }
            }
        } else if !stream.pending.is_empty() || final_piece {
            let text = std::mem::take(&mut stream.pending);
            out.push(json!({"kind": "text", "turn": self.turn, "message": stream.id, "index": stream.index, "text": text, "final": final_piece}));
            stream.index += 1;
        }
        if final_piece {
            self.stream = None;
        }
        out
    }

    /// Ends the turn: the reply so far, steps that never ended, pi's last
    /// error, and `stop`.
    pub fn finish(&mut self, interrupted: bool) -> Vec<Value> {
        let mut out = self.flush(true);
        let mut open: Vec<(String, Step)> = self.steps.drain().collect();
        open.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, step) in open {
            out.push(
                json!({"kind": "step_failed", "turn": self.turn, "id": id, "tool": step.tool,
                "error": "The turn ended before the step completed.", "interrupted": interrupted}),
            );
        }
        self.blocked.clear();
        if let Some(error) = self.error.take().filter(|_| !interrupted) {
            out.push(json!({"kind": "error", "turn": self.turn, "text": error}));
        }
        if let Some(turn) = self.turn.take() {
            out.push(json!({"kind": "stop", "turn": turn}));
        }
        out
    }
}

/// A pi tool call as the chat shows it: its built-in tools as Claude Code's
/// are, MCP tools (`mcp__sandhill__run_code`) and the rest as pi names them.
pub fn tool_step(name: &str, args: &Value) -> (String, Value) {
    let path = &args["path"];
    match name {
        "bash" => ("Bash".into(), json!({"command": args["command"]})),
        "read" => (
            "Read".into(),
            strip_nulls(
                json!({"file_path": path, "offset": args["offset"], "limit": args["limit"]}),
            ),
        ),
        "write" => (
            "Write".into(),
            shorten(&json!({"file_path": path, "content": args["content"]})),
        ),
        "edit" => {
            let edits: Vec<Value> = args["edits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|edit| json!({"old_string": edit["oldText"], "new_string": edit["newText"]}))
                .collect();
            if let [edit] = edits.as_slice() {
                let mut input = edit.clone();
                input["file_path"] = path.clone();
                ("Edit".into(), shorten(&input))
            } else {
                (
                    "MultiEdit".into(),
                    shorten(&json!({"file_path": path, "edits": edits})),
                )
            }
        }
        "grep" => (
            "Grep".into(),
            strip_nulls(json!({"pattern": args["pattern"], "path": path, "glob": args["glob"]})),
        ),
        "find" => (
            "Glob".into(),
            strip_nulls(json!({"pattern": args["pattern"], "path": path})),
        ),
        _ => (name.to_string(), shorten(args)),
    }
}

/// How a step ended, without its turn and id: `step_done` with its output
/// in the form the chat reads for the tool, or `step_failed`.
pub fn step_end(tool: &str, input: &Value, result: &Value, is_error: bool) -> Value {
    let text = result_text(result);
    if is_error {
        let error = if text.is_empty() {
            "The step failed.".to_string()
        } else {
            text
        };
        return json!({"kind": "step_failed", "tool": tool, "error": shorten(&json!(error))});
    }
    let output = match tool {
        "Bash" => json!({"stdout": text}),
        "Read" => {
            json!({"file": {"content": text, "startLine": input["offset"].as_u64().unwrap_or(1)}})
        }
        "Grep" => json!({"content": text}),
        "Glob" => json!({"filenames": text.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>()}),
        _ => json!({"content": result["content"]}),
    };
    json!({"kind": "step_done", "tool": tool, "output": shorten(&output)})
}

/// The text parts of a tool result or message content.
pub fn result_text(result: &Value) -> String {
    content_text(&result["content"])
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| match part["type"].as_str() {
                Some("text") => part["text"].as_str().map(str::to_string),
                Some("image") => Some("[Image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn strip_nulls(mut value: Value) -> Value {
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, value| !value.is_null());
    }
    value
}

/// The id of the `number`th turn the user started in `session`. Live turns
/// and saved ones count the same way, so a turn is never shown twice.
pub fn turn_id(session: &str, number: u64) -> String {
    format!("{session}:turn:{number}")
}

/// Ends a replayed turn: steps it never finished, and `stop`.
fn close_turn(
    out: &mut Vec<Value>,
    turn: &mut Option<String>,
    open: &mut Vec<(String, Step)>,
    skip: bool,
    at_ms: &Value,
) {
    if let Some(id) = turn.take().filter(|_| !skip) {
        for (step, Step { tool, .. }) in open.drain(..) {
            out.push(json!({"kind": "step_failed", "turn": id, "id": step, "tool": tool,
                "error": "The turn ended before the step completed.", "interrupted": true, "at_ms": at_ms}));
        }
        out.push(json!({"kind": "stop", "turn": id, "at_ms": at_ms}));
    }
    open.clear();
}

/// A saved session's turns as chat entries (with `at_ms`), except those in
/// `known`, and how many turns the user started. Every user message in the
/// session's entries, in the order pi saved them, starts a turn.
pub fn history(entries: &[Value], session: &str, known: &HashSet<String>) -> (Vec<Value>, u64) {
    let mut out = Vec::new();
    let mut number = 0;
    let mut turn: Option<String> = None;
    let mut skip = false;
    let mut serial = 0;
    let mut open: Vec<(String, Step)> = Vec::new();
    let mut last_ms = Value::Null;
    for entry in entries {
        if entry["type"] != "message" {
            continue;
        }
        let message = &entry["message"];
        let at_ms = message["timestamp"].clone();
        match message["role"].as_str().unwrap_or_default() {
            "user" => {
                close_turn(&mut out, &mut turn, &mut open, skip, &last_ms);
                number += 1;
                let id = turn_id(session, number);
                skip = known.contains(&id);
                if !skip {
                    out.push(json!({"kind": "prompt", "turn": id, "text": content_text(&message["content"]), "at_ms": at_ms}));
                }
                turn = Some(id);
            }
            "assistant" if turn.is_some() && !skip => {
                let id = turn.clone().unwrap_or_default();
                for part in message["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str() {
                        Some("text") => {
                            let text = part["text"].as_str().unwrap_or_default();
                            if text.is_empty() {
                                continue;
                            }
                            serial += 1;
                            out.push(json!({"kind": "text", "turn": id, "message": format!("{id}:message:{serial}"),
                                "index": 0, "text": text, "final": true, "at_ms": at_ms}));
                        }
                        Some("thinking") => {
                            let text = part["thinking"].as_str().unwrap_or_default();
                            if text.trim().is_empty() {
                                continue;
                            }
                            serial += 1;
                            let step = format!("{id}:message:{serial}");
                            out.push(json!({"kind": "step", "turn": id, "id": step, "tool": "reasoning", "input": {"text": text}, "at_ms": at_ms}));
                            out.push(json!({"kind": "step_done", "turn": id, "id": step, "tool": "reasoning", "output": {"text": text}, "at_ms": at_ms}));
                        }
                        Some("toolCall") => {
                            let Some(step) = part["id"].as_str() else {
                                continue;
                            };
                            let (tool, input) = tool_step(
                                part["name"].as_str().unwrap_or("tool"),
                                &part["arguments"],
                            );
                            out.push(json!({"kind": "step", "turn": id, "id": step, "tool": tool, "input": input, "at_ms": at_ms}));
                            open.push((step.to_string(), Step { tool, input }));
                        }
                        _ => {}
                    }
                }
                if message["stopReason"] == "error" {
                    out.push(json!({"kind": "error", "turn": id, "at_ms": at_ms,
                        "text": message["errorMessage"].as_str().unwrap_or("pi's model request failed")}));
                }
            }
            "toolResult" if turn.is_some() && !skip => {
                let Some(step) = message["toolCallId"].as_str() else {
                    continue;
                };
                let Some(at) = open.iter().position(|(id, _)| id == step) else {
                    continue;
                };
                let (_, Step { tool, input }) = open.remove(at);
                let mut end = step_end(&tool, &input, message, message["isError"] == true);
                end["turn"] = json!(turn);
                end["id"] = json!(step);
                end["at_ms"] = at_ms.clone();
                out.push(end);
            }
            _ => {}
        }
        if !at_ms.is_null() {
            last_ms = at_ms;
        }
    }
    close_turn(&mut out, &mut turn, &mut open, skip, &last_ms);
    (out, number)
}

/// The thinking levels pi offers for `model` (its `getSupportedThinkingLevels`):
/// `off` alone without reasoning; `xhigh` and `max` only when the model maps
/// them; any level it maps to null is left out.
pub fn thinking_levels(model: &Value) -> Vec<&'static str> {
    const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if model["reasoning"] != true {
        return vec!["off"];
    }
    let map = &model["thinkingLevelMap"];
    LEVELS
        .into_iter()
        .filter(|level| match map.get(*level) {
            Some(Value::Null) => false,
            Some(_) => true,
            None => !matches!(*level, "xhigh" | "max"),
        })
        .collect()
}

/// `level` if the model offers it, else the nearest it does, higher first
/// (pi's `clampThinkingLevel`).
pub fn clamp_level(levels: &[&'static str], level: &str) -> Option<&'static str> {
    const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    if let Some(found) = levels.iter().find(|l| **l == level) {
        return Some(*found);
    }
    let at = LEVELS.iter().position(|l| *l == level)?;
    LEVELS[at..]
        .iter()
        .chain(LEVELS[..at].iter().rev())
        .find_map(|candidate| levels.iter().find(|l| *l == candidate).copied())
}

/// pi's id for a model: `provider/id`.
pub fn model_id(model: &Value) -> Option<String> {
    let provider = model["provider"].as_str()?;
    let id = model["id"].as_str()?;
    (provider != "unknown" && !provider.is_empty() && !id.is_empty())
        .then(|| format!("{provider}/{id}"))
}

/// The models to offer, `{id, label, description, efforts, effort,
/// default}`, from `get_available_models`; `current` is the model pi starts
/// with and `level` its thinking level. That level is pi's default effort
/// only when the current model reasons (otherwise pi reports `off`), so
/// only then is it each model's recommended `effort`.
pub fn models(available: &Value, current: Option<&str>, level: Option<&str>) -> Value {
    let list = available.as_array().map(Vec::as_slice).unwrap_or_default();
    let reasons = list
        .iter()
        .any(|m| model_id(m).as_deref() == current && thinking_levels(m).len() > 1);
    let level = level.filter(|_| reasons);
    let mut seen = HashSet::new();
    json!(
        list.iter()
            .filter_map(|model| {
                let id = model_id(model)?;
                if !seen.insert(id.clone()) {
                    return None;
                }
                let levels = thinking_levels(model);
                let mut entry = json!({
                    "id": id,
                    "label": model["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&id),
                    "description": model["provider"],
                });
                if levels.len() > 1 {
                    entry["efforts"] = json!(levels);
                    if let Some(level) = level.and_then(|level| clamp_level(&levels, level)) {
                        entry["effort"] = json!(level);
                    }
                }
                if current == Some(id.as_str()) {
                    entry["default"] = json!(true);
                }
                Some(entry)
            })
            .collect::<Vec<_>>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as `pi --mode rpc` 1.5.2 writes them for a turn that reasons,
    /// runs a command, reads a file through Sandhill and replies.
    const TURN: &str = r#"
{"type":"agent_start"}
{"type":"turn_start"}
{"type":"message_start","message":{"role":"user","content":"Count the rows","timestamp":1733234401000}}
{"type":"message_end","message":{"role":"user","content":"Count the rows","timestamp":1733234401000}}
{"type":"message_start","message":{"role":"assistant","content":[],"stopReason":"pending"}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"Look at "}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"the data."}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"thinking_end","contentIndex":0,"content":"Look at the data."}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"call_1","toolName":"bash"}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"toolcall_end","contentIndex":1,"toolCall":{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"wc -l data.csv"}}}}
{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"toolUse"}}
{"type":"tool_execution_start","toolCallId":"call_1","toolName":"bash","args":{"command":"wc -l data.csv"}}
{"type":"tool_execution_update","toolCallId":"call_1","toolName":"bash","args":{"command":"wc -l data.csv"},"partialResult":{"content":[{"type":"text","text":"4"}],"details":{}}}
{"type":"tool_execution_end","toolCallId":"call_1","toolName":"bash","result":{"content":[{"type":"text","text":"4 data.csv"}],"details":{}},"isError":false,"durationMs":12.4}
{"type":"tool_execution_start","toolCallId":"call_2","toolName":"mcp__sandhill__list_variables","args":{"session":"py"}}
{"type":"tool_execution_end","toolCallId":"call_2","toolName":"mcp__sandhill__list_variables","result":{"content":[{"type":"text","text":"df: DataFrame"}]},"isError":false}
{"type":"tool_execution_start","toolCallId":"call_3","toolName":"edit","args":{"path":"a.py","edits":[{"oldText":"x = 1","newText":"x = 2"}]}}
{"type":"tool_execution_end","toolCallId":"call_3","toolName":"edit","result":{"content":[{"type":"text","text":"Blocked: Ask mode"}]},"isError":true}
{"type":"turn_end","message":{"role":"assistant"},"toolResults":[]}
{"type":"message_start","message":{"role":"assistant","content":[],"stopReason":"pending"}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"There are "}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"3 rows."}}
{"type":"message_update","usage":{},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"There are 3 rows."}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"There are 3 rows."}],"stopReason":"stop"}}
{"type":"agent_end","messages":[],"willRetry":false}
{"type":"agent_settled","aborted":false}
"#;

    fn lines(text: &str) -> Vec<Value> {
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn run(events: &[Value], translator: &mut Translator) -> Vec<Value> {
        let mut out = Vec::new();
        for event in events {
            if event["type"] == "agent_settled" {
                out.extend(translator.finish(event["aborted"] == true));
            } else {
                out.extend(translator.event(event));
            }
        }
        out
    }

    #[test]
    fn a_turn_becomes_reasoning_steps_and_a_reply() {
        let mut translator = Translator::default();
        assert!(translator.begin("s:turn:1".into()).is_empty());
        translator.block("call_3", super::super::super::guard::BLOCKED_STEP);
        let out = run(&lines(TURN), &mut translator);
        let kinds: Vec<&str> = out.iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "step",
                "step_done",
                "step",
                "step_done",
                "step",
                "step_done",
                "step",
                "step_failed",
                "text",
                "stop"
            ]
        );
        assert!(out.iter().all(|e| e["turn"] == "s:turn:1"));
        assert_eq!(out[0]["tool"], "reasoning");
        assert_eq!(out[1]["output"]["text"], "Look at the data.");
        assert_eq!(out[2]["tool"], "Bash");
        assert_eq!(out[2]["input"], json!({"command": "wc -l data.csv"}));
        assert_eq!(out[3]["output"], json!({"stdout": "4 data.csv"}));
        assert_eq!(out[3]["ms"], 12);
        assert_eq!(out[4]["tool"], "mcp__sandhill__list_variables");
        assert_eq!(out[5]["output"]["content"][0]["text"], "df: DataFrame");
        assert_eq!(out[6]["tool"], "Edit");
        assert_eq!(
            out[6]["input"],
            json!({"file_path": "a.py", "old_string": "x = 1", "new_string": "x = 2"})
        );
        assert_eq!(out[7]["error"], "Not run: Ask mode is read-only.");
        assert_eq!(out[8]["text"], "There are 3 rows.");
        assert_eq!(out[8]["final"], true);
        assert_eq!(out[9]["turn"], "s:turn:1");
        assert!(translator.turn.is_none());
    }

    #[test]
    fn a_reply_streams_in_pieces_of_one_message() {
        let mut translator = Translator::default();
        translator.begin("t".into());
        let delta = |text: &str| json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text}});
        let mut out = translator.event(&delta("There are "));
        out.extend(translator.flush(false));
        out.extend(translator.flush(false));
        out.extend(translator.event(&delta("3 rows.")));
        out.extend(translator.event(&json!({"type": "message_update", "assistantMessageEvent": {"type": "text_end", "contentIndex": 0}})));
        let pieces: Vec<(&str, bool, u64)> = out
            .iter()
            .map(|e| {
                (
                    e["text"].as_str().unwrap(),
                    e["final"] == true,
                    e["index"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pieces, [("There are ", false, 0), ("3 rows.", true, 1)]);
        assert_eq!(out[0]["message"], out[1]["message"]);
        assert_eq!(out[0]["message"], "t:message:1");
    }

    #[test]
    fn errors_are_said_once_the_turn_ends_and_unfinished_steps_fail() {
        let events = lines(
            r#"
{"type":"tool_execution_start","toolCallId":"c","toolName":"read","args":{"path":"x.txt","offset":5}}
{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"No API key for openai/gpt"}}
{"type":"agent_settled","aborted":false}
"#,
        );
        let mut translator = Translator::default();
        translator.begin("t".into());
        let out = run(&events, &mut translator);
        assert_eq!(out[0]["tool"], "Read");
        assert_eq!(out[0]["input"], json!({"file_path": "x.txt", "offset": 5}));
        assert_eq!(out[1]["kind"], "step_failed");
        assert_eq!(
            out[2],
            json!({"kind": "error", "turn": "t", "text": "No API key for openai/gpt"})
        );
        assert_eq!(out[3]["kind"], "stop");
        // A retry that succeeds clears the error; an abort says nothing.
        let events = lines(
            r#"
{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"529 overloaded"}}
{"type":"auto_retry_end","success":true,"attempt":2}
{"type":"agent_settled","aborted":false}
"#,
        );
        translator.begin("t2".into());
        let out = run(&events, &mut translator);
        assert_eq!(out, [json!({"kind": "stop", "turn": "t2"})]);
        translator.begin("t3".into());
        let out = run(
            &lines(
                r#"
{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"aborted"}}
{"type":"agent_settled","aborted":true}
"#,
            ),
            &mut translator,
        );
        assert_eq!(out, [json!({"kind": "stop", "turn": "t3"})]);
    }

    #[test]
    fn compaction_is_a_session_entry() {
        let mut translator = Translator::default();
        let out = translator.event(&json!({"type": "compaction_end", "reason": "manual",
            "result": {"summary": "s"}, "aborted": false, "willRetry": false}));
        assert_eq!(out, [json!({"kind": "session", "source": "compact"})]);
    }

    #[test]
    fn tools_read_as_claude_codes_where_the_chat_knows_them() {
        let (tool, input) = tool_step(
            "edit",
            &json!({"path": "a", "edits": [{"oldText": "1", "newText": "2"}, {"oldText": "3", "newText": "4"}]}),
        );
        assert_eq!(tool, "MultiEdit");
        assert_eq!(
            input["edits"][1],
            json!({"old_string": "3", "new_string": "4"})
        );
        assert_eq!(
            tool_step("write", &json!({"path": "b", "content": "x"})).0,
            "Write"
        );
        assert_eq!(tool_step("find", &json!({"pattern": "*.py"})).0, "Glob");
        assert_eq!(tool_step("ls", &json!({})).0, "ls");
        let result = json!({"content": [{"type": "text", "text": "a.py\nb.py\n"}]});
        assert_eq!(
            step_end("Glob", &json!({}), &result, false)["output"],
            json!({"filenames": ["a.py", "b.py"]})
        );
        assert_eq!(
            step_end("Read", &json!({"offset": 3}), &result, false)["output"]["file"]["startLine"],
            3
        );
    }

    /// `get_entries` for a session with one turn already in the chat and one
    /// made in pi's own interface.
    const ENTRIES: &str = r#"
{"type":"model_change","id":"m","parentId":null,"provider":"openai","modelId":"gpt"}
{"type":"message","id":"a","parentId":"m","message":{"role":"user","content":"First","timestamp":1}}
{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","content":[{"type":"text","text":"One"}],"stopReason":"stop","timestamp":2}}
{"type":"message","id":"c","parentId":"b","message":{"role":"user","content":[{"type":"text","text":"Second"}],"timestamp":3}}
{"type":"message","id":"d","parentId":"c","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Hmm"},{"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"ls"}},{"type":"toolCall","id":"t2","name":"read","arguments":{"path":"x"}}],"stopReason":"toolUse","timestamp":4}}
{"type":"message","id":"e","parentId":"d","message":{"role":"toolResult","toolCallId":"t1","toolName":"bash","content":[{"type":"text","text":"x"}],"isError":false,"timestamp":5}}
{"type":"message","id":"f","parentId":"e","message":{"role":"assistant","content":[{"type":"text","text":"Two"}],"stopReason":"stop","timestamp":6}}
"#;

    #[test]
    fn saved_turns_not_in_the_chat_are_replayed() {
        let known = HashSet::from(["s:turn:1".to_string()]);
        let (out, count) = history(&lines(ENTRIES), "s", &known);
        assert_eq!(count, 2);
        let kinds: Vec<&str> = out.iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "prompt",
                "step",
                "step_done",
                "step",
                "step",
                "step_done",
                "text",
                "step_failed",
                "stop"
            ]
        );
        assert!(out.iter().all(|e| e["turn"] == "s:turn:2"));
        assert_eq!(out[0]["text"], "Second");
        assert_eq!(out[0]["at_ms"], 3);
        assert_eq!(out[5]["output"], json!({"stdout": "x"}));
        assert_eq!(out[6]["text"], "Two");
        assert_eq!(out[7]["id"], "t2");
        assert_eq!(out[8]["at_ms"], 6);
        let (all, _) = history(&lines(ENTRIES), "s", &HashSet::new());
        assert_eq!(all[0]["text"], "First");
        assert_eq!(
            all[2],
            json!({"kind": "stop", "turn": "s:turn:1", "at_ms": 2})
        );
    }

    #[test]
    fn models_carry_the_thinking_levels_pi_offers() {
        let available = json!([
            {"id": "gpt-6", "name": "GPT 6", "provider": "openai-codex", "reasoning": true,
             "thinkingLevelMap": {"xhigh": "xhigh", "minimal": null}},
            {"id": "small", "name": "", "provider": "local", "reasoning": false},
            {"id": "gpt-6", "name": "GPT 6", "provider": "openai-codex", "reasoning": true},
        ]);
        let listed = models(&available, Some("openai-codex/gpt-6"), Some("max"));
        assert_eq!(listed.as_array().unwrap().len(), 2);
        assert_eq!(listed[0]["id"], "openai-codex/gpt-6");
        assert_eq!(listed[0]["label"], "GPT 6");
        assert_eq!(
            listed[0]["efforts"],
            json!(["off", "low", "medium", "high", "xhigh"])
        );
        assert_eq!(listed[0]["effort"], "xhigh");
        assert_eq!(listed[0]["default"], true);
        assert_eq!(listed[1]["label"], "local/small");
        assert!(listed[1].get("efforts").is_none());
        assert!(listed[1].get("default").is_none());
        // pi says `off` for a current model that doesn't reason: no default.
        let listed = models(&available, Some("local/small"), Some("off"));
        assert!(listed[0].get("effort").is_none());
        assert_eq!(
            model_id(&json!({"provider": "unknown", "id": "unknown"})),
            None
        );
    }
}
