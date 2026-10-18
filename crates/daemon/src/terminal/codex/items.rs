//! What Codex's app-server reports, as Sandhill's conversation entries
//! ([`super::super::chat`]), and the user's answers in the app as its
//! responses. Steps take the shapes the chat already shows for Claude Code
//! where there is one: a command is `Bash`, a Sandhill tool is
//! `mcp__sandhill__add_cell`. Codex's own are `apply_patch` (file edits)
//! and `reasoning` (its summary of what it is thinking).

use std::path::Path;

use serde_json::{Map, Value, json};

use super::super::chat::shorten;

/// The tool and input of the step an item shows as, for items that are
/// steps.
pub fn step(item: &Value, cwd: &Path) -> Option<(String, Value)> {
    let step = match item["type"].as_str()? {
        "commandExecution" => ("Bash".to_string(), command_input(item)),
        "fileChange" => (
            "apply_patch".to_string(),
            json!({"changes": changes(item, cwd)}),
        ),
        "mcpToolCall" => (
            format!(
                "mcp__{}__{}",
                item["server"].as_str()?,
                item["tool"].as_str()?
            ),
            shorten(&item["arguments"]),
        ),
        "dynamicToolCall" => (
            item["tool"].as_str()?.to_string(),
            shorten(&item["arguments"]),
        ),
        "webSearch" => ("WebSearch".to_string(), json!({"query": item["query"]})),
        "imageView" => ("view_image".to_string(), json!({"path": item["path"]})),
        "reasoning" => {
            let text = summary(item)?;
            ("reasoning".to_string(), json!({"text": text}))
        }
        _ => return None,
    };
    Some(step)
}

/// Whether an item shows as a step from its start; the others are shown
/// once they are complete, when what they did is known.
pub fn shown_at_start(item: &Value) -> bool {
    matches!(
        item["type"].as_str(),
        Some("commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall")
    )
}

/// How a step's item ended: `step_done` with its output, or `step_failed`.
/// Without the turn and the step's id, which the caller adds.
pub fn step_end(item: &Value, tool: &str) -> Value {
    let status = item["status"].as_str().unwrap_or("completed");
    let ms = &item["durationMs"];
    let failed =
        |error: Value| json!({"kind": "step_failed", "tool": tool, "error": error, "ms": ms});
    let done =
        |output: Value| json!({"kind": "step_done", "tool": tool, "output": output, "ms": ms});
    if status == "declined" {
        return failed(json!("Declined"));
    }
    match item["type"].as_str().unwrap_or("") {
        "commandExecution" => {
            let output = item["aggregatedOutput"].as_str().unwrap_or("");
            let code = item["exitCode"].as_i64();
            if status == "failed" || code.is_some_and(|c| c != 0) {
                let error = match code {
                    Some(code) if output.trim().is_empty() => format!("Exit code {code}"),
                    _ => output.to_string(),
                };
                failed(shorten(&json!(error)))
            } else {
                done(shorten(&json!({"stdout": output, "exit_code": code})))
            }
        }
        "fileChange" if status == "failed" => failed(json!("The edit could not be made")),
        "fileChange" => done(json!({"status": status})),
        // The tool's error, or what it said when it failed.
        "mcpToolCall" if status == "failed" => failed(match item["error"]["message"].as_str() {
            Some(message) => json!(message),
            None if !item["result"].is_null() => shorten(&item["result"]),
            None => json!("Failed"),
        }),
        "mcpToolCall" => done(shorten(&item["result"])),
        "dynamicToolCall" if item["success"] == false => {
            failed(shorten(&json!({"content": item["contentItems"]})))
        }
        "dynamicToolCall" => done(shorten(&json!({"content": item["contentItems"]}))),
        "webSearch" => done(json!({"query": item["query"], "action": item["action"]})),
        "reasoning" => done(json!({"text": summary(item)})),
        _ => done(json!({})),
    }
}

/// A command as the chat shows it: what Codex runs inside `bash -lc`, and
/// what it does, when Codex could tell (read a file, list files, search).
fn command_input(item: &Value) -> Value {
    let command = item["command"]
        .as_str()
        .map(unwrap_shell)
        .unwrap_or_default();
    let mut input = json!({"command": command});
    if let Some(description) = describe_actions(&item["commandActions"]) {
        input["description"] = json!(description);
    }
    input
}

/// "Read app.R", "List files in data", "Search for “umap” in R".
fn describe_actions(actions: &Value) -> Option<String> {
    let actions = actions.as_array()?;
    let parts: Vec<String> = actions
        .iter()
        .map(|action| {
            let text = |key: &str| action[key].as_str().filter(|s| !s.is_empty());
            match action["type"].as_str() {
                Some("read") => text("name")
                    .or(text("path"))
                    .map(|name| format!("Read {name}")),
                Some("listFiles") => Some(match text("path") {
                    Some(path) => format!("List files in {path}"),
                    None => "List files".to_string(),
                }),
                Some("search") => Some(match (text("query"), text("path")) {
                    (Some(query), Some(path)) => format!("Search for “{query}” in {path}"),
                    (Some(query), None) => format!("Search for “{query}”"),
                    _ => "Search".to_string(),
                }),
                _ => None,
            }
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// `echo hi` from `/bin/bash -lc 'echo hi'`, as Codex wraps commands.
pub fn unwrap_shell(command: &str) -> String {
    let rest = command
        .split_once(' ')
        .filter(|(shell, _)| {
            let name = shell.rsplit('/').next().unwrap_or(shell);
            matches!(name, "bash" | "zsh" | "sh")
        })
        .and_then(|(_, rest)| {
            rest.strip_prefix("-lc ")
                .or_else(|| rest.strip_prefix("-c "))
        });
    let Some(rest) = rest else {
        return command.to_string();
    };
    match rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        Some(quoted) => quoted.replace(r"'\''", "'"),
        None => rest.to_string(),
    }
}

/// A patch's files, relative to the folder when inside it.
fn changes(item: &Value, cwd: &Path) -> Value {
    let changes = item["changes"].as_array().cloned().unwrap_or_default();
    Value::Array(
        changes
            .into_iter()
            .map(|change| {
                let path = change["path"].as_str().unwrap_or("");
                let relative = Path::new(path)
                    .strip_prefix(cwd)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.to_string());
                let mut out = json!({
                    "path": relative,
                    "kind": change["kind"]["type"],
                    "diff": shorten(&change["diff"]),
                });
                if let Some(to) = change["kind"]["move_path"].as_str() {
                    out["move_path"] = json!(to);
                }
                out
            })
            .collect(),
    )
}

/// What Codex said it is thinking about, when it said.
fn summary(item: &Value) -> Option<String> {
    let parts: Vec<&str> = item["summary"]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Codex's plan of a turn (its `update_plan` tool), as the to-do list
/// Claude Code's TodoWrite shows.
pub fn todos(params: &Value) -> Value {
    let steps = params["plan"].as_array().cloned().unwrap_or_default();
    let todos: Vec<Value> = steps
        .iter()
        .map(|step| {
            let status = match step["status"].as_str() {
                Some("completed") => "completed",
                Some("inProgress") => "in_progress",
                _ => "pending",
            };
            json!({"content": step["step"], "status": status})
        })
        .collect();
    json!({"todos": todos, "explanation": params["explanation"]})
}

/// A request from Codex for the user, as the chat asks it.
#[derive(Debug, PartialEq)]
pub struct Ask {
    pub tool: String,
    pub input: Value,
    /// The step it is about.
    pub step: Option<String>,
    /// What allowing it for longer allows.
    pub remember: Vec<String>,
    /// For how long it can be allowed: `chat`, `project`.
    pub scopes: Vec<&'static str>,
}

/// The ask for a server request, given the steps in progress (by id: their
/// tool and input). `None` for requests the chat doesn't ask.
pub fn ask(method: &str, params: &Value, steps: &[(String, String, Value)]) -> Option<Ask> {
    let item = params["itemId"].as_str().map(str::to_string);
    let step_of = |id: &Option<String>| {
        steps
            .iter()
            .find(|(step, _, _)| Some(step) == id.as_ref())
            .map(|(_, tool, input)| (tool.clone(), input.clone()))
    };
    match method {
        "item/commandExecution/requestApproval" => {
            let mut input = command_input(params);
            if let Some(reason) = params["reason"].as_str() {
                input["description"] = json!(reason);
            }
            let mut scopes = Vec::new();
            if has_decision(params, "acceptForSession") {
                scopes.push("chat");
            }
            let amendment = params["proposedExecpolicyAmendment"]
                .as_array()
                .filter(|a| !a.is_empty() && has_decision(params, "acceptWithExecpolicyAmendment"));
            let remember = match amendment {
                Some(prefix) => {
                    scopes.push("project");
                    let words: Vec<&str> = prefix.iter().filter_map(Value::as_str).collect();
                    vec![format!(
                        "commands that start with “{}”",
                        unwrap_shell(&words.join(" "))
                    )]
                }
                None if !scopes.is_empty() => vec!["this command".to_string()],
                None => Vec::new(),
            };
            Some(Ask {
                tool: "Bash".into(),
                input,
                step: item,
                remember,
                scopes,
            })
        }
        "item/fileChange/requestApproval" => {
            let (_, mut input) =
                step_of(&item).unwrap_or(("apply_patch".into(), json!({"changes": []})));
            if let Some(reason) = params["reason"].as_str() {
                input["reason"] = json!(reason);
            }
            Some(Ask {
                tool: "apply_patch".into(),
                input,
                step: item,
                remember: vec!["file edits".into()],
                scopes: vec!["chat"],
            })
        }
        "mcpServer/elicitation/request" => {
            let server = params["serverName"].as_str().unwrap_or("mcp");
            let meta = &params["_meta"];
            if meta["codex_approval_kind"] != "mcp_tool_call" {
                // A server asking for something else: shown with its message.
                return Some(Ask {
                    tool: format!("mcp__{server}"),
                    input: json!({"message": params["message"], "url": params["url"]}),
                    step: None,
                    remember: Vec::new(),
                    scopes: Vec::new(),
                });
            }
            // It names the tool only in its message; the step in progress
            // with the same server and input says which.
            let prefix = format!("mcp__{server}__");
            let found = steps.iter().rev().find(|(_, tool, input)| {
                tool.starts_with(&prefix) && *input == shorten(&meta["tool_params"])
            });
            let (step, tool) = match found {
                Some((id, tool, _)) => (Some(id.clone()), tool.clone()),
                None => (
                    None,
                    format!("{prefix}{}", quoted_tool(params).unwrap_or("tool")),
                ),
            };
            let persist: Vec<&str> = meta["persist"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let scopes: Vec<&'static str> = [("session", "chat"), ("always", "project")]
                .into_iter()
                .filter(|(p, _)| persist.contains(p))
                .map(|(_, scope)| scope)
                .collect();
            let name = tool.rsplit("__").next().unwrap_or(&tool).to_string();
            Some(Ask {
                tool,
                input: shorten(&meta["tool_params"]),
                step,
                remember: if scopes.is_empty() {
                    Vec::new()
                } else {
                    vec![name]
                },
                scopes,
            })
        }
        "item/tool/requestUserInput" => {
            let questions: Vec<Value> = params["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|q| {
                    json!({
                        "question": q["question"],
                        "header": q["header"],
                        "multiSelect": false,
                        "options": q["options"].as_array().cloned().unwrap_or_default(),
                    })
                })
                .collect();
            Some(Ask {
                tool: "AskUserQuestion".into(),
                input: json!({"questions": questions}),
                step: item,
                remember: Vec::new(),
                scopes: Vec::new(),
            })
        }
        "item/permissions/requestApproval" => Some(Ask {
            tool: "permissions".into(),
            input: json!({"reason": params["reason"], "permissions": params["permissions"]}),
            step: item,
            remember: vec!["these permissions".into()],
            scopes: vec!["chat"],
        }),
        _ => None,
    }
}

fn has_decision(params: &Value, name: &str) -> bool {
    match params["availableDecisions"].as_array() {
        Some(decisions) => decisions.iter().any(|d| d == name || d.get(name).is_some()),
        // Older servers don't list them; these two are always there.
        None => name == "acceptForSession",
    }
}

/// `add_cell` from `Allow the sandhill MCP server to run tool "add_cell"?`.
fn quoted_tool(params: &Value) -> Option<&str> {
    let message = params["message"].as_str()?;
    let start = message.find("tool \"")? + 6;
    let end = start + message[start..].find('"')?;
    Some(&message[start..end])
}

/// Codex's response to a request, from the user's answer (see
/// `AnswerTerminal`): `{behavior: "allow" | "deny", scope?, answers?}`.
pub fn response(method: &str, params: &Value, answer: &Value) -> Value {
    let allow = answer["behavior"] == "allow";
    let scope = answer["scope"].as_str().unwrap_or("once");
    match method {
        "item/commandExecution/requestApproval" => {
            let decision = if !allow {
                if has_decision(params, "decline") || params["availableDecisions"].is_null() {
                    json!("decline")
                } else {
                    json!("cancel")
                }
            } else if scope == "project" && has_decision(params, "acceptWithExecpolicyAmendment") {
                json!({"acceptWithExecpolicyAmendment": {
                    "execpolicy_amendment": params["proposedExecpolicyAmendment"]
                }})
            } else if scope != "once" && has_decision(params, "acceptForSession") {
                json!("acceptForSession")
            } else {
                json!("accept")
            };
            json!({"decision": decision})
        }
        "item/fileChange/requestApproval" => {
            let decision = match (allow, scope) {
                (false, _) => "decline",
                (true, "once") => "accept",
                (true, _) => "acceptForSession",
            };
            json!({"decision": decision})
        }
        "mcpServer/elicitation/request" => {
            if !allow {
                return json!({"action": "decline", "content": null, "_meta": null});
            }
            let meta = match scope {
                "chat" => json!({"persist": "session"}),
                "project" => json!({"persist": "always"}),
                _ => Value::Null,
            };
            let content = if params["_meta"]["codex_approval_kind"] == "mcp_tool_call" {
                Value::Null
            } else {
                json!({})
            };
            json!({"action": "accept", "content": content, "_meta": meta})
        }
        "item/tool/requestUserInput" => {
            let given = &answer["answers"];
            let mut answers = Map::new();
            for q in params["questions"].as_array().into_iter().flatten() {
                let (Some(id), Some(question)) = (q["id"].as_str(), q["question"].as_str()) else {
                    continue;
                };
                if let Some(text) = given[question].as_str().filter(|_| allow) {
                    answers.insert(id.into(), json!({"answers": [text]}));
                }
            }
            json!({"answers": answers})
        }
        "item/permissions/requestApproval" => {
            if !allow {
                return json!({"permissions": {}, "scope": "turn"});
            }
            let mut permissions = params["permissions"].clone();
            if let Some(map) = permissions.as_object_mut() {
                map.retain(|_, v| !v.is_null());
            }
            let scope = if scope == "once" { "turn" } else { "session" };
            json!({"permissions": permissions, "scope": scope})
        }
        _ => Value::Null,
    }
}

/// What a mode lets Codex do without asking, as a turn's settings. The
/// modes are Sandhill's (`AgentOptions::permission`): Normal
/// (`supervised`) runs only what reads, Auto has Codex's reviewer decide
/// risky steps, YOLO (`full`) runs everything, and Plan only reads.
/// Restricted asks before anything Codex doesn't know to be harmless, and
/// Ask only reads and never asks (Sandhill's tools refuse to change
/// anything, see `guard`).
pub fn mode_settings(permission: Option<&str>) -> Value {
    let read_only = json!({"type": "readOnly", "networkAccess": false});
    let workspace = json!({
        "type": "workspaceWrite",
        "writableRoots": [],
        "networkAccess": false,
        "excludeTmpdirEnvVar": false,
        "excludeSlashTmp": false,
    });
    let (approval, sandbox, reviewer) = match permission {
        Some("edits") => ("on-request", workspace, "user"),
        Some("auto") => ("on-request", workspace, "auto_review"),
        Some("full") => ("never", json!({"type": "dangerFullAccess"}), "user"),
        Some("restricted") => ("untrusted", read_only, "user"),
        Some("ask") => ("never", read_only, "user"),
        _ => ("on-request", read_only, "user"),
    };
    json!({"approvalPolicy": approval, "sandboxPolicy": sandbox, "approvalsReviewer": reviewer})
}

/// How much of the model's context the conversation uses, from Codex's
/// token counts: `{context: percent, context_size}`.
pub fn context(usage: &Value) -> Option<Value> {
    let size = usage["modelContextWindow"].as_f64().filter(|s| *s > 0.0)?;
    let used = usage["last"]["totalTokens"].as_f64()?;
    let percent = (used * 100.0 / size).min(100.0);
    Some(json!({"context": (percent * 10.0).round() / 10.0, "context_size": size as u64}))
}

/// Codex's plan limits, in the shape Claude Code's status line gives them
/// (`five_hour`, `seven_day`), merged into what was known: updates leave
/// out what didn't change.
pub fn rate_limits(known: &Value, snapshot: &Value) -> Value {
    let mut limits = known.as_object().cloned().unwrap_or_default();
    for (window, key) in [("primary", "five_hour"), ("secondary", "seven_day")] {
        let window = &snapshot[window];
        if let Some(used) = window["usedPercent"].as_f64() {
            limits.insert(
                key.into(),
                json!({"used_percentage": used, "resets_at": window["resetsAt"]}),
            );
        }
    }
    Value::Object(limits)
}

/// The models to offer: `{id, label, efforts, effort, default}`.
pub fn models(list: &Value) -> Value {
    let models: Vec<Value> = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["hidden"] != true)
        .map(|m| {
            let efforts: Vec<&Value> = m["supportedReasoningEfforts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| &e["reasoningEffort"])
                .collect();
            json!({
                "id": m["model"],
                "label": m["displayName"],
                "efforts": efforts,
                "effort": m["defaultReasoningEffort"],
                "default": m["isDefault"],
            })
        })
        .collect();
    Value::Array(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codex 0.163's messages for one turn: a command it asked to run, a
    /// patch, a Sandhill tool it asked to use and one it didn't (read-only).
    fn recorded() -> Vec<Value> {
        let text = include_str!("../../../../../tests/fixtures/codex-app-server.jsonl");
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn find<'a>(messages: &'a [Value], method: &str, kind: &str) -> &'a Value {
        messages
            .iter()
            .find(|m| {
                m["method"] == method && (kind.is_empty() || m["params"]["item"]["type"] == kind)
            })
            .unwrap_or_else(|| panic!("no {method} {kind}"))
    }

    const CWD: &str = "/data/pbmc";

    #[test]
    fn commands_show_what_runs_and_what_it_does() {
        let messages = recorded();
        let started = &find(&messages, "item/started", "commandExecution")["params"]["item"];
        let (tool, input) = step(started, Path::new(CWD)).unwrap();
        assert_eq!(tool, "Bash");
        assert_eq!(input, json!({"command": "echo hi > made.txt"}));
        let done = &find(&messages, "item/completed", "commandExecution")["params"]["item"];
        let end = step_end(done, "Bash");
        assert_eq!(end["kind"], "step_done");
        assert_eq!(end["output"]["exit_code"], 0);

        let read = json!({"type": "commandExecution", "command": "/bin/zsh -lc 'sed -n 1,20p app.R'",
            "commandActions": [{"type": "read", "command": "sed -n 1,20p app.R", "name": "app.R", "path": "app.R"}]});
        assert_eq!(
            step(&read, Path::new(CWD)).unwrap().1,
            json!({"command": "sed -n 1,20p app.R", "description": "Read app.R"})
        );
        let failed = json!({"type": "commandExecution", "status": "failed", "exitCode": 2, "aggregatedOutput": "", "durationMs": 5});
        assert_eq!(step_end(&failed, "Bash")["error"], "Exit code 2");
    }

    #[test]
    fn shells_are_unwrapped() {
        assert_eq!(unwrap_shell("/bin/bash -lc 'echo it'\\''s'"), "echo it's");
        assert_eq!(unwrap_shell("bash -lc ls"), "ls");
        assert_eq!(unwrap_shell("python -c 'print(1)'"), "python -c 'print(1)'");
    }

    #[test]
    fn approvals_become_asks_and_answers_decisions() {
        let messages = recorded();
        let request = &find(&messages, "item/commandExecution/requestApproval", "")["params"];
        let ask = ask("item/commandExecution/requestApproval", request, &[]).unwrap();
        assert_eq!(ask.tool, "Bash");
        assert_eq!(ask.step.as_deref(), Some("call_1"));
        assert_eq!(ask.input["description"], "Write made.txt?");
        // This request can be allowed from now on (a rule Codex keeps), not
        // for the conversation.
        assert_eq!(ask.scopes, ["project"]);
        assert_eq!(
            ask.remember,
            ["commands that start with “echo hi > made.txt”"]
        );
        let method = "item/commandExecution/requestApproval";
        let answer = |a: Value| response(method, request, &a);
        assert_eq!(
            answer(json!({"behavior": "allow", "scope": "once"})),
            json!({"decision": "accept"})
        );
        assert_eq!(
            answer(json!({"behavior": "allow", "scope": "project"}))["decision"]["acceptWithExecpolicyAmendment"]
                ["execpolicy_amendment"],
            json!(["/bin/bash", "-lc", "echo hi > made.txt"])
        );
        // No declining here, only stopping.
        assert_eq!(
            answer(json!({"behavior": "deny"})),
            json!({"decision": "cancel"})
        );
    }

    #[test]
    fn patches_show_their_files_and_ask_for_the_conversation() {
        let messages = recorded();
        let started = &find(&messages, "item/started", "fileChange")["params"]["item"];
        let (tool, input) = step(started, Path::new(CWD)).unwrap();
        assert_eq!(tool, "apply_patch");
        assert_eq!(
            input,
            json!({"changes": [{"path": "hello.txt", "kind": "add", "diff": "hello\nworld\n"}]})
        );
        let steps = vec![("call_p".to_string(), tool, input.clone())];
        let method = "item/fileChange/requestApproval";
        let request = &find(&messages, method, "")["params"];
        let asked = ask(method, request, &steps).unwrap();
        assert_eq!(asked.input, input);
        assert_eq!(asked.step.as_deref(), Some("call_p"));
        assert_eq!(
            response(
                method,
                request,
                &json!({"behavior": "allow", "scope": "chat"})
            ),
            json!({"decision": "acceptForSession"})
        );
    }

    #[test]
    fn sandhill_tools_ask_through_codex_and_can_be_allowed_for_longer() {
        let messages = recorded();
        let started = &find(&messages, "item/started", "mcpToolCall")["params"]["item"];
        let (tool, input) = step(started, Path::new(CWD)).unwrap();
        assert_eq!(tool, "mcp__sandhill__add_cell");
        let steps = vec![("call_2".to_string(), tool.clone(), input)];
        let method = "mcpServer/elicitation/request";
        let request = &find(&messages, method, "")["params"];
        let asked = ask(method, request, &steps).unwrap();
        assert_eq!(asked.tool, tool);
        assert_eq!(asked.step.as_deref(), Some("call_2"));
        assert_eq!(asked.scopes, ["chat", "project"]);
        assert_eq!(
            response(
                method,
                request,
                &json!({"behavior": "allow", "scope": "chat"})
            ),
            json!({"action": "accept", "content": null, "_meta": {"persist": "session"}})
        );
        assert_eq!(
            response(method, request, &json!({"behavior": "deny"}))["action"],
            "decline"
        );
        // Without the step, the tool is read from the message.
        assert_eq!(ask(method, request, &[]).unwrap().tool, tool);

        let done = &find(&messages, "item/completed", "mcpToolCall")["params"]["item"];
        let end = step_end(done, &tool);
        assert_eq!(end["output"]["content"][0]["type"], "text");
    }

    #[test]
    fn questions_are_answered_by_their_ids() {
        let method = "item/tool/requestUserInput";
        let request = json!({"itemId": "call_4", "questions": [{"id": "q1", "header": "Plot", "question": "Which plot?", "isOther": true, "isSecret": false,
            "options": [{"label": "UMAP", "description": "2D embedding"}, {"label": "PCA", "description": "linear"}]}]});
        let asked = ask(method, &request, &[]).unwrap();
        assert_eq!(asked.tool, "AskUserQuestion");
        assert_eq!(asked.input["questions"][0]["options"][1]["label"], "PCA");
        let answer = json!({"behavior": "allow", "answers": {"Which plot?": "UMAP"}});
        assert_eq!(
            response(method, &request, &answer),
            json!({"answers": {"q1": {"answers": ["UMAP"]}}})
        );
    }

    #[test]
    fn modes_set_the_sandbox_and_who_approves() {
        assert_eq!(
            mode_settings(Some("supervised"))["sandboxPolicy"]["type"],
            "readOnly"
        );
        assert_eq!(
            mode_settings(Some("auto"))["approvalsReviewer"],
            "auto_review"
        );
        let yolo = mode_settings(Some("full"));
        assert_eq!(yolo["approvalPolicy"], "never");
        assert_eq!(yolo["sandboxPolicy"]["type"], "dangerFullAccess");
        assert_eq!(
            mode_settings(Some("plan"))["sandboxPolicy"]["type"],
            "readOnly"
        );
        assert_eq!(
            mode_settings(Some("restricted"))["approvalPolicy"],
            "untrusted"
        );
        let ask = mode_settings(Some("ask"));
        assert_eq!(ask["approvalPolicy"], "never");
        assert_eq!(ask["sandboxPolicy"]["type"], "readOnly");
    }

    #[test]
    fn usage_and_limits_take_claude_codes_shape() {
        let messages = recorded();
        let usage = &find(&messages, "thread/tokenUsage/updated", "")["params"]["tokenUsage"];
        assert_eq!(
            context(usage).unwrap(),
            json!({"context": 0.5, "context_size": 258400})
        );
        let limits = rate_limits(
            &Value::Null,
            &json!({"primary": {"usedPercent": 42.0, "windowDurationMins": 300, "resetsAt": 1792270000}, "secondary": null}),
        );
        assert_eq!(limits["five_hour"]["used_percentage"], 42.0);
        // A later update without the 5-hour window keeps it.
        let limits = rate_limits(
            &limits,
            &json!({"secondary": {"usedPercent": 7.0, "resetsAt": 1792900000}}),
        );
        assert_eq!(limits["five_hour"]["used_percentage"], 42.0);
        assert_eq!(limits["seven_day"]["used_percentage"], 7.0);
    }

    #[test]
    fn plans_become_todo_lists() {
        let params = json!({"explanation": null, "plan": [{"step": "Load", "status": "completed"}, {"step": "Plot", "status": "inProgress"}]});
        assert_eq!(
            todos(&params)["todos"],
            json!([{"content": "Load", "status": "completed"}, {"content": "Plot", "status": "in_progress"}])
        );
    }
}
