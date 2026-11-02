//! An agent's conversation as the AI pane shows it, from the entries the
//! daemon keeps of what the agent's hooks report: the user's messages, the
//! parts of each reply, each step with its result, and what the agent asks
//! the user. The same rules as the Tauri app's `app/src/model/chat.ts`.

use std::collections::HashMap;

use serde_json::Value;

/// One entry of the conversation: `{seq, event}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub seq: u64,
    pub event: Value,
}

/// A request waiting for the user, or answered. `answer` is `None` while
/// it waits, `Some(Value::Null)` when the agent stopped waiting.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub id: String,
    pub tool: String,
    pub input: Value,
    pub answer: Option<Value>,
    pub remember: Vec<String>,
    /// For how long it can be allowed (`chat`, `project`); both when `None`.
    pub scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Run,
    Wait,
    Done,
    Fail,
    Deny,
    Cancel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: String,
    pub tool: String,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub ms: Option<u64>,
    pub status: StepStatus,
    pub ask: Option<Ask>,
}

/// Steps are most of a chat, so they aren't boxed.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        key: String,
        text: String,
        place: Value,
    },
    Reply {
        key: String,
        text: String,
    },
    Step {
        key: String,
        step: Step,
    },
    /// A request with no step to show it on.
    Ask {
        key: String,
        ask: Ask,
    },
    Footer {
        key: String,
        at_ms: Option<i64>,
        ms: Option<i64>,
        text: String,
    },
    Note {
        key: String,
        text: String,
    },
    /// Something went wrong: a turn couldn't start, or failed.
    Error {
        key: String,
        text: String,
    },
    Thinking {
        key: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chat {
    pub items: Vec<Item>,
    /// Requests waiting for the user, oldest first.
    pub waiting: Vec<Ask>,
    /// The first message, as a title.
    pub title: Option<String>,
}

fn str_of<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event[key].as_str()
}

struct Turn {
    id: String,
    prompt: Option<Value>,
    events: Vec<Entry>,
    stop: Option<Value>,
}

enum Part {
    Turn(usize),
    Loose(Entry),
}

/// The chat from the conversation's entries. `working` is whether the
/// agent says it is busy: a turn the agent was stopped in has no `stop`.
pub fn build(entries: &[Entry], working: bool) -> Chat {
    let mut answers: HashMap<String, Value> = HashMap::new();
    for entry in entries {
        if str_of(&entry.event, "kind") == Some("answered")
            && let Some(id) = str_of(&entry.event, "id")
        {
            answers.insert(id.to_string(), entry.event["answer"].clone());
        }
    }
    // Turns in the order they started, with the notes between them.
    let mut order: Vec<Part> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let event = &entry.event;
        let kind = str_of(event, "kind").unwrap_or_default();
        if kind == "answered" {
            continue;
        }
        let Some(turn_id) = str_of(event, "turn") else {
            if kind == "session" || kind == "error" {
                order.push(Part::Loose(entry.clone()));
            }
            continue;
        };
        let ix = *by_id.entry(turn_id.to_string()).or_insert_with(|| {
            turns.push(Turn {
                id: turn_id.to_string(),
                prompt: None,
                events: Vec::new(),
                stop: None,
            });
            order.push(Part::Turn(turns.len() - 1));
            turns.len() - 1
        });
        let turn = &mut turns[ix];
        match kind {
            "prompt" => turn.prompt = Some(event.clone()),
            "stop" => turn.stop = Some(event.clone()),
            _ => turn.events.push(entry.clone()),
        }
    }

    let mut chat = Chat::default();
    let last_turn = turns.len().checked_sub(1);
    for part in order {
        let ix = match part {
            Part::Loose(entry) => {
                if str_of(&entry.event, "kind") == Some("error") {
                    chat.items.push(Item::Error {
                        key: format!("e{}", entry.seq),
                        text: str_of(&entry.event, "text").unwrap_or_default().into(),
                    });
                } else if let Some(text) = session_note(&entry.event) {
                    chat.items.push(Item::Note {
                        key: format!("n{}", entry.seq),
                        text: text.into(),
                    });
                }
                continue;
            }
            Part::Turn(ix) => ix,
        };
        let turn = &turns[ix];
        // A turn has ended when the agent said so, another began, or the
        // agent is idle.
        let ended = turn.stop.is_some() || Some(ix) != last_turn || !working;
        if let Some(text) = turn.prompt.as_ref().and_then(|p| str_of(p, "text")) {
            chat.items.push(Item::User {
                key: format!("u{}", turn.id),
                text: text.into(),
                place: turn
                    .prompt
                    .as_ref()
                    .map_or(Value::Null, |p| p["place"].clone()),
            });
            if chat.title.is_none() {
                chat.title = Some(title_of(text));
            }
        }
        // Where each step and reply sits in `chat.items`.
        let mut steps: Vec<(String, usize)> = Vec::new();
        // (reply id, where, its pieces by seq)
        type Replies = Vec<(String, usize, Vec<(u64, String)>)>;
        let mut replies: Replies = Vec::new();
        // Hooks run side by side: put things in the order they happened.
        let mut events = turn.events.clone();
        events.sort_by_key(|e| (e.event["at_ms"].as_i64().unwrap_or(0), e.seq));
        let mut waiting_here = false;
        for Entry { seq, event } in events {
            let kind = str_of(&event, "kind").unwrap_or_default();
            match kind {
                "step" => {
                    let id = str_of(&event, "id").map_or(format!("s{seq}"), str::to_string);
                    steps.push((id.clone(), chat.items.len()));
                    chat.items.push(Item::Step {
                        key: format!("s{id}"),
                        step: Step {
                            id,
                            tool: str_of(&event, "tool").unwrap_or_default().into(),
                            input: object_or_empty(&event["input"]),
                            output: None,
                            error: None,
                            ms: None,
                            status: StepStatus::Run,
                            ask: None,
                        },
                    });
                }
                "step_done" | "step_failed" => {
                    let id = str_of(&event, "id").map_or(format!("s{seq}"), str::to_string);
                    let at = match steps.iter().find(|(s, _)| *s == id) {
                        Some((_, at)) => *at,
                        None => {
                            steps.push((id.clone(), chat.items.len()));
                            chat.items.push(Item::Step {
                                key: format!("s{id}"),
                                step: Step {
                                    id,
                                    tool: str_of(&event, "tool").unwrap_or_default().into(),
                                    input: Value::Object(Default::default()),
                                    output: None,
                                    error: None,
                                    ms: None,
                                    status: StepStatus::Run,
                                    ask: None,
                                },
                            });
                            chat.items.len() - 1
                        }
                    };
                    if let Item::Step { step, .. } = &mut chat.items[at] {
                        if kind == "step_done" {
                            step.output = Some(event["output"].clone());
                        } else {
                            let text = error_text(&event["error"]);
                            step.error = Some(if !text.is_empty() {
                                text
                            } else if event["interrupted"].as_bool() == Some(true) {
                                "Stopped".into()
                            } else {
                                "Failed".into()
                            });
                        }
                        step.ms = event["ms"].as_u64();
                    }
                }
                "ask" => {
                    let id = str_of(&event, "id").map_or(format!("a{seq}"), str::to_string);
                    let ask = Ask {
                        answer: answers.get(&id).cloned(),
                        id,
                        tool: str_of(&event, "tool").unwrap_or_default().into(),
                        input: object_or_empty(&event["input"]),
                        remember: strings(&event["remember"]).unwrap_or_default(),
                        scopes: strings(&event["scopes"]),
                    };
                    if ask.answer.is_none() && !ended {
                        chat.waiting.push(ask.clone());
                        waiting_here = true;
                    }
                    match asked_step(&chat.items, &steps, &ask, str_of(&event, "step")) {
                        Some(at) => {
                            if let Item::Step { step, .. } = &mut chat.items[at] {
                                step.ask = Some(ask);
                            }
                        }
                        None => chat.items.push(Item::Ask {
                            key: format!("a{}", ask.id),
                            ask,
                        }),
                    }
                }
                "error" => chat.items.push(Item::Error {
                    key: format!("e{seq}"),
                    text: str_of(&event, "text").unwrap_or_default().into(),
                }),
                "text" => {
                    let id = str_of(&event, "message").map_or(format!("m{seq}"), str::to_string);
                    let at = match replies.iter().position(|(r, _, _)| *r == id) {
                        Some(at) => at,
                        None => {
                            replies.push((id.clone(), chat.items.len(), Vec::new()));
                            chat.items.push(Item::Reply {
                                key: format!("r{id}"),
                                text: String::new(),
                            });
                            replies.len() - 1
                        }
                    };
                    let (_, item, parts) = &mut replies[at];
                    let index = event["index"].as_u64().unwrap_or(parts.len() as u64);
                    let text = str_of(&event, "text").unwrap_or_default().to_string();
                    match parts.iter_mut().find(|(i, _)| *i == index) {
                        Some(part) => part.1 = text,
                        None => parts.push((index, text)),
                    }
                    parts.sort_by_key(|(i, _)| *i);
                    let joined: String = parts.iter().map(|(_, t)| t.as_str()).collect();
                    if let Item::Reply { text, .. } = &mut chat.items[*item] {
                        *text = joined;
                    }
                }
                _ => {}
            }
        }
        for (_, at) in &steps {
            if let Item::Step { step, .. } = &mut chat.items[*at] {
                step.status = step_status(step, ended);
            }
        }
        if ended {
            let start = turn.prompt.as_ref().and_then(|p| p["at_ms"].as_i64());
            let end = turn.stop.as_ref().and_then(|s| s["at_ms"].as_i64());
            let text = replies
                .iter()
                .filter_map(|(_, at, _)| match &chat.items[*at] {
                    Item::Reply { text, .. } if !text.trim().is_empty() => {
                        Some(text.trim().to_string())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            chat.items.push(Item::Footer {
                key: format!("f{}", turn.id),
                at_ms: end,
                ms: start.zip(end).map(|(s, e)| e - s),
                text,
            });
        } else if !waiting_here && chat.waiting.is_empty() {
            chat.items.push(Item::Thinking {
                key: format!("t{}", turn.id),
            });
        }
    }
    chat
}

fn object_or_empty(value: &Value) -> Value {
    if value.is_null() {
        Value::Object(Default::default())
    } else {
        value.clone()
    }
}

fn strings(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    })
}

/// The step a request is about: the one it names (Codex), else the latest
/// of that tool with the same input (Claude Code's hooks don't say).
fn asked_step(
    items: &[Item],
    steps: &[(String, usize)],
    ask: &Ask,
    named: Option<&str>,
) -> Option<usize> {
    if let Some(named) = named
        && let Some((_, at)) = steps.iter().find(|(id, _)| id == named)
    {
        return Some(*at);
    }
    steps.iter().rev().map(|(_, at)| *at).find(|at| {
        matches!(&items[*at], Item::Step { step, .. }
            if step.tool == ask.tool && step.ask.is_none() && step.output.is_none() && step.input == ask.input)
    })
}

fn step_status(step: &Step, ended: bool) -> StepStatus {
    if step.output.is_some() {
        return StepStatus::Done;
    }
    if step.error.is_some() {
        return StepStatus::Fail;
    }
    if let Some(ask) = &step.ask {
        if ask.answer.as_ref().and_then(|a| a["behavior"].as_str()) == Some("deny") {
            return StepStatus::Deny;
        }
        if ended {
            return StepStatus::Cancel;
        }
        if ask.answer.is_none() {
            return StepStatus::Wait;
        }
    }
    if ended {
        StepStatus::Cancel
    } else {
        StepStatus::Run
    }
}

fn session_note(event: &Value) -> Option<&'static str> {
    match str_of(event, "source") {
        Some("compact") => Some("Conversation compacted"),
        Some("clear") => Some("Conversation cleared"),
        Some("resume") => Some("Conversation resumed"),
        _ => None,
    }
}

/// When the conversation last changed, if it has.
pub fn last_active(entries: &[Entry]) -> Option<i64> {
    entries
        .iter()
        .rev()
        .find_map(|e| e.event["at_ms"].as_i64().filter(|&at| at > 0))
}

/// A title from a first message: its first few words, without file names,
/// @ mentions, "Please" or "Can you", or a trailing "in" or "of".
pub fn title_of(text: &str) -> String {
    let text = text.trim();
    // The first sentence or line.
    let mut end = text.len();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(at, c)) in chars.iter().enumerate() {
        let next_space = chars.get(i + 1).is_none_or(|&(_, n)| n.is_whitespace());
        if c == '\n' || (matches!(c, '.' | '?' | '!') && next_space) {
            end = at;
            break;
        }
    }
    let first: String = text[..end]
        .split_whitespace()
        .filter(|w| !w.starts_with('@'))
        .collect::<Vec<_>>()
        .join(" ");
    let lower = first.to_lowercase();
    let mut first = first.as_str();
    for lead in ["please ", "can you ", "could you ", "would you "] {
        if lower.starts_with(lead) {
            first = &first[lead.len()..];
            break;
        }
    }
    let mut words: Vec<String> = Vec::new();
    for raw in first.split_whitespace() {
        let w = raw
            .trim_start_matches(['`', '"', '\'', '“', '‘', '('])
            .trim_end_matches(['`', '"', '\'', '”', '’', ')', ',', ':', ';']);
        if w.is_empty() {
            continue;
        }
        // A file name ends the title once it has a few words, else it is
        // left out.
        if w.contains('/') || looks_like_file(w) {
            if words.len() >= 3 {
                break;
            }
            continue;
        }
        words.push(w.to_string());
        if words.len() == 6 {
            break;
        }
    }
    const SMALL: [&str; 13] = [
        "in", "of", "for", "to", "the", "a", "an", "and", "with", "on", "from", "at", "by",
    ];
    while words.len() > 2
        && words
            .last()
            .is_some_and(|w| SMALL.contains(&w.to_lowercase().as_str()))
    {
        words.pop();
    }
    let title = words.join(" ");
    let mut chars = title.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => "New conversation".into(),
    }
}

/// Ends in `.ext`, the extension starting with a letter.
fn looks_like_file(word: &str) -> bool {
    word.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && ext.starts_with(|c: char| c.is_ascii_alphabetic())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

fn error_text(error: &Value) -> String {
    match error {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => {
            let text = content_text(other);
            if text.is_empty() {
                other.to_string()
            } else {
                text
            }
        }
    }
}

/// The text of an MCP tool's result: `[{type: "text", text}, …]`.
pub fn content_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            // Claude Code adds where it saved each image.
            .filter(|t| !(t.starts_with("[Image: source: ") && t.ends_with(']')))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) if map.get("content").is_some_and(Value::is_array) => {
            content_text(&map["content"])
        }
        _ => String::new(),
    }
}

/// How a step's row and card show it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepView {
    /// In bold code font: `run_code`, `bash`, `read`.
    pub name: String,
    /// A short plain description.
    pub desc: String,
    /// A file the description names.
    pub file: Option<String>,
    /// On the right: "R · 0.5 s", "lines 1–40 of 120", "+7 −0".
    pub detail: Option<String>,
    pub added: Option<usize>,
    pub removed: Option<usize>,
    pub code: Option<String>,
    pub language: Option<String>,
    /// Where the code ran, for the card's header.
    pub place: Option<String>,
    pub output: Option<String>,
    /// A diff: lines with "+", "-" or " " in front.
    pub diff: Option<Vec<String>>,
}

const SANDHILL: &str = "mcp__sandhill__";

fn s(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn first_line(text: &str) -> String {
    text.trim().lines().next().unwrap_or_default().to_string()
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn line_count(text: Option<&str>) -> usize {
    text.filter(|t| !t.is_empty())
        .map_or(0, |t| t.split('\n').count())
}

fn language_name(language: &str) -> String {
    if language.eq_ignore_ascii_case("r") {
        return "R".into();
    }
    let mut chars = language.chars();
    chars
        .next()
        .map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// What a step did, for its row and its card.
pub fn step_view(
    tool: &str,
    input: &Value,
    output: Option<&Value>,
    linked: Option<&str>,
) -> StepView {
    let mut view = StepView {
        name: display_name(tool),
        ..Default::default()
    };
    let null = Value::Null;
    let out = output.unwrap_or(&null);
    if tool.starts_with(SANDHILL) {
        let text = content_text(out);
        let (head, rest) = text.split_once('\n').unwrap_or((&text, ""));
        let ran = sandhill_run(head);
        match view.name.as_str() {
            "run_code" => {
                view.desc = s(&input["description"])
                    .unwrap_or_else(|| first_line(input["code"].as_str().unwrap_or_default()));
                view.code = s(&input["code"]);
                view.language = s(&input["language"]).or(ran.language);
                view.place = Some(head.to_string());
                view.output = Some(rest.trim_end().to_string());
                view.detail = ran.detail;
            }
            "add_cell" | "edit_cell" => {
                let file = s(&input["file"])
                    .or(ran.file)
                    .or(linked.map(str::to_string));
                let in_file = file.clone().unwrap_or("the document".into());
                view.desc = if view.name == "add_cell" {
                    format!("Add a cell to {in_file}")
                } else {
                    format!(
                        "Change {} in {in_file}",
                        input["cell"].as_str().unwrap_or("a cell")
                    )
                };
                view.file = file;
                view.code = s(&input["code"]);
                view.language = s(&input["language"]).or(ran.language);
                view.place = Some(head.to_string());
                view.output = Some(rest.trim_end().to_string());
                let code = input["code"].as_str();
                if view.name == "add_cell" {
                    view.added = Some(line_count(code));
                    view.removed = Some(0);
                    view.diff = Some(
                        code.unwrap_or_default()
                            .split('\n')
                            .map(|l| format!("+{l}"))
                            .collect(),
                    );
                } else {
                    let n = line_count(code);
                    view.detail = Some(plural(n as u64, "line", "lines"));
                }
            }
            "list_variables" => {
                view.desc = match input["language"].as_str() {
                    Some(l) => format!("List the {} session's variables", language_name(l)),
                    None => "List the session's variables".into(),
                };
                view.output = Some(text.clone());
            }
            "list_sessions" => {
                view.desc = "List the running sessions".into();
                view.output = Some(text.clone());
            }
            "get_focus" => {
                view.desc = "See where you are".into();
                view.output = Some(text.clone());
            }
            "get_outputs" => {
                view.desc = match input["cell"].as_str() {
                    Some(c) => format!("Read the outputs of {c}"),
                    None => "Read the outputs".into(),
                };
                view.file = s(&input["file"]);
                view.output = Some(text.clone());
            }
            "list_jobs" => {
                view.desc = "List the jobs".into();
                view.output = Some(text.clone());
            }
            "read_job_log" => {
                view.desc = match input["job"].as_str() {
                    Some(j) => format!("Read the log of job {j}"),
                    None => "Read the latest job's log".into(),
                };
                view.output = Some(text.clone());
            }
            _ => view.output = Some(text.clone()),
        }
        return view;
    }
    match tool {
        "Bash" => {
            view.desc = s(&input["description"])
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| first_line(input["command"].as_str().unwrap_or_default()));
            view.code = s(&input["command"]);
            view.language = Some("shell".into());
            let parts: Vec<&str> = [&out["stdout"], &out["stderr"]]
                .iter()
                .filter_map(|v| v.as_str().filter(|s| !s.is_empty()))
                .collect();
            view.output = Some(parts.join("\n"));
        }
        "Read" => {
            view.file = s(&input["file_path"]);
            view.desc = base_name(input["file_path"].as_str().unwrap_or_default());
            let file = &out["file"];
            if let Some(content) = file["content"].as_str() {
                let start = file["startLine"].as_u64().unwrap_or(1);
                view.output = Some(
                    content
                        .split('\n')
                        .enumerate()
                        .map(|(i, line)| format!("{:>4}  {line}", start + i as u64))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                if let Some(n) = file["numLines"].as_u64() {
                    let last = start + n.saturating_sub(1);
                    view.detail = Some(match file["totalLines"].as_u64() {
                        Some(total) if total > n => format!("lines {start}–{last} of {total}"),
                        _ => format!("{n} lines"),
                    });
                }
            }
        }
        "Edit" | "MultiEdit" => {
            view.file = s(&input["file_path"]);
            view.desc = format!(
                "Edit {}",
                base_name(input["file_path"].as_str().unwrap_or_default())
            );
            let edits: Vec<&Value> = if tool == "Edit" {
                vec![input]
            } else {
                input["edits"]
                    .as_array()
                    .map_or(vec![], |e| e.iter().collect())
            };
            let diff: Vec<String> = edits
                .iter()
                .flat_map(|e| {
                    let old = e["old_string"].as_str().unwrap_or_default();
                    let new = e["new_string"].as_str().unwrap_or_default();
                    old.split('\n')
                        .map(|l| format!("-{l}"))
                        .chain(new.split('\n').map(|l| format!("+{l}")))
                        .collect::<Vec<_>>()
                })
                .collect();
            set_diff(&mut view, diff);
            view.detail = Some(format!(
                "+{} −{}",
                view.added.unwrap_or(0),
                view.removed.unwrap_or(0)
            ));
        }
        "Write" => {
            view.file = s(&input["file_path"]);
            view.desc = format!(
                "Write {}",
                base_name(input["file_path"].as_str().unwrap_or_default())
            );
            let content = input["content"].as_str();
            view.diff = Some(
                content
                    .unwrap_or_default()
                    .split('\n')
                    .map(|l| format!("+{l}"))
                    .collect(),
            );
            view.added = Some(line_count(content));
            view.removed = Some(0);
            view.detail = Some(format!("+{}", line_count(content)));
        }
        "NotebookEdit" => {
            view.file = s(&input["notebook_path"]);
            view.desc = format!(
                "Edit a cell in {}",
                base_name(input["notebook_path"].as_str().unwrap_or_default())
            );
            view.code = s(&input["new_source"]);
        }
        "Grep" | "Glob" => {
            view.desc = if tool == "Grep" {
                let mut d = format!(
                    "Search for “{}”",
                    input["pattern"].as_str().unwrap_or_default()
                );
                if let Some(path) = input["path"].as_str() {
                    d.push_str(&format!(" in {}", base_name(path)));
                }
                d
            } else {
                format!("Find {}", input["pattern"].as_str().unwrap_or("files"))
            };
            let names = || {
                out["filenames"].as_array().map_or(String::new(), |f| {
                    f.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            };
            view.output = Some(match out["content"].as_str() {
                Some(content) if tool == "Grep" => content.to_string(),
                _ => names(),
            });
            if let Some(n) = out["numFiles"].as_u64() {
                view.detail = Some(plural(n, "file", "files"));
            }
        }
        "WebSearch" => {
            view.desc = s(&input["query"])
                .filter(|q| !q.is_empty())
                .or(s(&out["query"]))
                .unwrap_or_default();
        }
        "WebFetch" => view.desc = s(&input["url"]).unwrap_or_default(),
        "Task" | "Agent" => {
            view.desc = s(&input["description"]).unwrap_or_default();
            let text = content_text(out);
            view.output = Some(if text.is_empty() {
                out.as_str().unwrap_or_default().to_string()
            } else {
                text
            });
        }
        "ToolSearch" => {
            let query = input["query"].as_str().unwrap_or_default();
            view.desc = match query.strip_prefix("select:") {
                Some(names) => {
                    let names: Vec<String> = names
                        .split(',')
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(|n| {
                            if n.starts_with("mcp__") {
                                display_name(n)
                            } else {
                                n.to_string()
                            }
                        })
                        .collect();
                    format!("Load {}", names.join(", "))
                }
                None => format!("Find tools for “{query}”"),
            };
            if let Some(matches) = out["matches"].as_array() {
                view.output = Some(
                    matches
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
        }
        "AskUserQuestion" => {
            let questions = input["questions"].as_array().cloned().unwrap_or_default();
            let answers = &out["answers"];
            view.desc = if questions.len() == 1 {
                s(&questions[0]["question"]).unwrap_or_default()
            } else {
                format!("Ask {} questions", questions.len())
            };
            let given: Vec<&str> = questions
                .iter()
                .filter_map(|q| answers[q["question"].as_str().unwrap_or_default()].as_str())
                .collect();
            if !given.is_empty() {
                view.detail = Some(given.join(", "));
            }
            view.output = Some(
                questions
                    .iter()
                    .map(|q| {
                        let question = q["question"].as_str().unwrap_or_default();
                        format!(
                            "{question}\n› {}",
                            answers[question].as_str().unwrap_or("no answer")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
        }
        "ExitPlanMode" => {
            view.desc = "Share a plan".into();
            view.output = s(&input["plan"]);
        }
        "apply_patch" => {
            let changes = input["changes"].as_array().cloned().unwrap_or_default();
            let one = (changes.len() == 1).then(|| &changes[0]);
            view.file = one.and_then(|c| s(&c["path"]));
            view.desc = match one {
                Some(change) => patch_title(change),
                None => format!("Edit {} files", changes.len()),
            };
            let diff = changes
                .iter()
                .flat_map(|change| {
                    let mut lines = Vec::new();
                    if one.is_none() {
                        lines.push(format!(" {}", change["path"].as_str().unwrap_or_default()));
                    }
                    lines.extend(patch_lines(change));
                    lines
                })
                .collect();
            set_diff(&mut view, diff);
        }
        "reasoning" => {
            let text = s(&input["text"]).or(s(&out["text"])).unwrap_or_default();
            view.desc = first_line(&text).replace("**", "");
            view.output = Some(text);
        }
        "view_image" => {
            view.file = s(&input["path"]);
            view.desc = format!(
                "Look at {}",
                base_name(input["path"].as_str().unwrap_or_default())
            );
        }
        "permissions" => {
            view.desc = s(&input["reason"]).unwrap_or_default();
            view.output = Some(permission_lines(&input["permissions"]).join("\n"));
        }
        "TodoWrite" => {
            view.desc = "Update the to-do list".into();
            view.output = Some(
                input["todos"]
                    .as_array()
                    .map_or(vec![], |todos| {
                        todos
                            .iter()
                            .map(|t| {
                                let mark = match t["status"].as_str() {
                                    Some("completed") => "✓",
                                    Some("in_progress") => "›",
                                    _ => "○",
                                };
                                format!("{mark} {}", t["content"].as_str().unwrap_or_default())
                            })
                            .collect()
                    })
                    .join("\n"),
            );
        }
        _ => {
            if output.is_some() {
                let text = content_text(out);
                view.output = Some(if text.is_empty() {
                    serde_json::to_string_pretty(out).unwrap_or_default()
                } else {
                    text
                });
            } else if input.as_object().is_some_and(|o| !o.is_empty()) {
                view.output = serde_json::to_string_pretty(input).ok();
            }
        }
    }
    view
}

fn set_diff(view: &mut StepView, diff: Vec<String>) {
    view.added = Some(diff.iter().filter(|l| l.starts_with('+')).count());
    view.removed = Some(diff.iter().filter(|l| l.starts_with('-')).count());
    view.diff = Some(diff);
}

/// "run_code" for Sandhill's tools, "bash" or "read" for the agent's own,
/// "github · create_issue" for other servers'.
pub fn display_name(tool: &str) -> String {
    if let Some(name) = tool.strip_prefix(SANDHILL) {
        return name.into();
    }
    if let Some(rest) = tool.strip_prefix("mcp__") {
        return match rest.split_once("__") {
            Some((server, name)) => format!("{server} · {name}"),
            None => rest.into(),
        };
    }
    match tool {
        "WebSearch" => "web_search",
        "WebFetch" => "web_fetch",
        "TodoWrite" => "todo",
        "MultiEdit" => "edit",
        "NotebookEdit" => "edit_notebook",
        "ToolSearch" => "tools",
        "AskUserQuestion" => "question",
        "ExitPlanMode" => "plan",
        "apply_patch" => "edit",
        "reasoning" => "thinking",
        other => return other.to_lowercase(),
    }
    .into()
}

struct Ran {
    language: Option<String>,
    detail: Option<String>,
    file: Option<String>,
}

/// What Sandhill's run tools say first: "Ran in R session 1a2b (R 4.4.1,
/// kernel ark) · idle · in /data · ok · 0.5 s".
fn sandhill_run(head: &str) -> Ran {
    let words: Vec<&str> = head
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let language = words
        .iter()
        .find(|w| **w == "R" || **w == "Python")
        .map(|w| w.to_string());
    let seconds = head
        .rsplit_once("· ")
        .map(|(_, last)| last.trim())
        .filter(|last| {
            last.strip_suffix(" s")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
        })
        .map(str::to_string);
    let failed = head.contains("· error") || head.contains("· failed");
    let file = file_in(head);
    let parts: Vec<String> = [
        language.clone(),
        if failed {
            Some("error".into())
        } else {
            seconds
        },
    ]
    .into_iter()
    .flatten()
    .collect();
    Ran {
        language: language.map(|l| l.to_lowercase()),
        detail: (!parts.is_empty()).then(|| parts.join(" · ")),
        file,
    }
}

/// "to FILE below|after", else "of NAME.ext".
fn file_in(head: &str) -> Option<String> {
    let words: Vec<&str> = head.split_whitespace().collect();
    for w in words.windows(3) {
        if w[0] == "to" && (w[2] == "below" || w[2] == "after") {
            return Some(w[1].to_string());
        }
    }
    words
        .windows(2)
        .find(|w| w[0] == "of" && looks_like_file(w[1].trim_end_matches([',', ')'])))
        .map(|w| w[1].trim_end_matches([',', ')']).to_string())
}

fn patch_title(change: &Value) -> String {
    let name = base_name(change["path"].as_str().unwrap_or_default());
    if let Some(to) = change["move_path"].as_str() {
        return format!("Move {name} to {}", base_name(to));
    }
    match change["kind"].as_str() {
        Some("add") => format!("Add {name}"),
        Some("delete") => format!("Delete {name}"),
        _ => format!("Edit {name}"),
    }
}

/// A patch's lines with "+", "-" or " " in front; hunks after the first
/// start with " ⋯".
fn patch_lines(change: &Value) -> Vec<String> {
    let diff = change["diff"].as_str().unwrap_or_default();
    let lines = diff.strip_suffix('\n').unwrap_or(diff).split('\n');
    match change["kind"].as_str() {
        Some("add") => return lines.map(|l| format!("+{l}")).collect(),
        Some("delete") => return lines.map(|l| format!("-{l}")).collect(),
        _ => {}
    }
    let mut out = Vec::new();
    for line in lines {
        if line.starts_with("@@") {
            if !out.is_empty() {
                out.push(" ⋯".into());
            }
        } else if !line.starts_with("--- ") && !line.starts_with("+++ ") && !line.starts_with('\\')
        {
            out.push(if line.starts_with(['+', '-', ' ']) {
                line.to_string()
            } else {
                format!(" {line}")
            });
        }
    }
    out
}

/// What more access Codex asks for.
fn permission_lines(permissions: &Value) -> Vec<String> {
    if !permissions.is_object() {
        return vec![];
    }
    let mut lines = Vec::new();
    if permissions["network"]["enabled"].as_bool() == Some(true) {
        lines.push("Use the network".to_string());
    }
    let files = if permissions["fileSystem"].is_object() {
        &permissions["fileSystem"]
    } else {
        &permissions["file_system"]
    };
    for path in files["write"].as_array().into_iter().flatten() {
        lines.push(format!("Write to {}", path.as_str().unwrap_or_default()));
    }
    for path in files["read"].as_array().into_iter().flatten() {
        lines.push(format!("Read {}", path.as_str().unwrap_or_default()));
    }
    if lines.is_empty() {
        lines.push(serde_json::to_string_pretty(permissions).unwrap_or_default());
    }
    lines
}

/// How a request is put to the user.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AskView {
    pub title: String,
    /// Under the title: where it goes.
    pub detail: Option<String>,
    pub file: Option<String>,
    pub view: StepView,
    pub questions: Vec<Question>,
    /// A plan to approve (Claude's plan mode).
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Question {
    pub question: String,
    pub header: Option<String>,
    pub multi_select: bool,
    pub options: Vec<(String, Option<String>)>,
}

pub fn ask_view(ask: &Ask, linked: Option<&str>, agent: &str) -> AskView {
    let view = step_view(&ask.tool, &ask.input, None, linked);
    let input = &ask.input;
    let plain = |title: String, detail: Option<String>, view: StepView| AskView {
        title,
        detail,
        file: view.file.clone(),
        view,
        ..Default::default()
    };
    if ask.tool == "AskUserQuestion" {
        let questions = input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|q| Question {
                question: s(&q["question"]).unwrap_or_default(),
                header: s(&q["header"]),
                multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                options: q["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|o| (s(&o["label"]).unwrap_or_default(), s(&o["description"])))
                    .collect(),
            })
            .collect();
        return AskView {
            title: format!("{agent} has a question"),
            view,
            questions,
            ..Default::default()
        };
    }
    if ask.tool == "ExitPlanMode" {
        return AskView {
            title: "Carry out this plan?".into(),
            plan: Some(s(&input["plan"]).unwrap_or_default()),
            view,
            ..Default::default()
        };
    }
    if let Some(rest) = ask.tool.strip_prefix("mcp__")
        && !rest.contains("__")
    {
        // A server asking for something other than a tool call (Codex).
        let title = s(&input["message"])
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("{} asks for your answer", view.name));
        let mut view = view;
        view.output = None;
        return plain(title, s(&input["url"]), view);
    }
    let file_name = || base_name(view.file.as_deref().unwrap_or_default());
    match view.name.as_str() {
        "add_cell" => {
            let mut detail = match input["after"].as_str() {
                Some(after) => format!("Below {after}"),
                None => "Below your active cell".into(),
            };
            if input["run"].as_bool() != Some(false) {
                detail.push_str(", then run it");
            }
            let title = format!(
                "Add a cell to {}?",
                view.file.as_deref().unwrap_or("the document")
            );
            plain(title, Some(detail), view)
        }
        "edit_cell" => {
            let title = format!(
                "Change {} in {}?",
                input["cell"].as_str().unwrap_or("a cell"),
                view.file.as_deref().unwrap_or("the document")
            );
            let detail = (input["run"].as_bool() != Some(false)).then(|| "Then run it".into());
            plain(title, detail, view)
        }
        "run_code" => {
            let language = view
                .language
                .as_deref()
                .map(|l| format!("{} ", language_name(l)))
                .unwrap_or_default();
            plain(
                format!("Run this {language}code in your session?"),
                None,
                view,
            )
        }
        "bash" => plain("Run this command?".into(), s(&input["description"]), view),
        "edit" => {
            if ask.tool == "apply_patch" {
                let title = format!("{}?", view.desc);
                plain(title, s(&input["reason"]), view)
            } else {
                plain(format!("Edit {}?", file_name()), None, view)
            }
        }
        "write" => plain(format!("Write {}?", file_name()), None, view),
        "permissions" => {
            let mut view = view;
            view.desc = String::new();
            plain(
                format!("Give {agent} more access?"),
                s(&input["reason"]),
                view,
            )
        }
        "web_fetch" => plain(
            format!("Open {}?", input["url"].as_str().unwrap_or("a web page")),
            None,
            view,
        ),
        "web_search" => plain("Search the web?".into(), s(&input["query"]), view),
        "read" => {
            let name = file_name();
            let name = if name.is_empty() {
                "a file".into()
            } else {
                name
            };
            plain(format!("Read {name}?"), None, view)
        }
        "grep" | "glob" | "list_variables" | "list_sessions" | "get_focus" | "get_outputs"
        | "list_jobs" | "read_job_log" => {
            let title = if view.desc.is_empty() {
                format!("Allow {}?", view.name)
            } else {
                format!("{}?", view.desc)
            };
            plain(title, None, view)
        }
        _ => {
            let detail = (!view.desc.is_empty()).then(|| view.desc.clone());
            plain(format!("Allow {}?", view.name), detail, view)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entries(events: Vec<Value>) -> Vec<Entry> {
        events
            .into_iter()
            .enumerate()
            .map(|(seq, event)| Entry {
                seq: seq as u64,
                event,
            })
            .collect()
    }

    fn recorded() -> Vec<Entry> {
        let all: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/chat.recorded.json")).unwrap();
        all.into_iter()
            .map(|e| Entry {
                seq: e["seq"].as_u64().unwrap(),
                event: e["event"].clone(),
            })
            .collect()
    }

    fn kinds(chat: &Chat) -> Vec<&'static str> {
        chat.items
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Reply { .. } => "reply",
                Item::Step { .. } => "step",
                Item::Ask { .. } => "ask",
                Item::Footer { .. } => "footer",
                Item::Note { .. } => "note",
                Item::Error { .. } => "error",
                Item::Thinking { .. } => "thinking",
            })
            .collect()
    }

    fn steps(chat: &Chat) -> Vec<&Step> {
        chat.items
            .iter()
            .filter_map(|i| match i {
                Item::Step { step, .. } => Some(step),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn shows_a_recorded_conversation() {
        let chat = build(&recorded(), false);
        assert_eq!(
            kinds(&chat),
            ["user", "step", "step", "step", "step", "reply", "footer"]
        );
        assert!(
            chat.title
                .as_deref()
                .unwrap()
                .starts_with("Use add_cell to add a cell")
        );
        assert!(chat.waiting.is_empty());
        let steps = steps(&chat);
        assert_eq!(
            steps
                .iter()
                .map(|s| display_name(&s.tool))
                .collect::<Vec<_>>(),
            ["get_focus", "list_sessions", "bash", "add_cell"]
        );
        assert!(steps.iter().all(|s| s.status == StepStatus::Done));
        assert_eq!(
            steps[3].ask.as_ref().unwrap().answer,
            Some(json!({"behavior": "allow"}))
        );
        assert!(steps[2].ask.is_none());
        let Item::Footer { text, ms, .. } = &chat.items[6] else {
            panic!()
        };
        assert!(text.starts_with("I added an R cell"));
        assert!(ms.unwrap() > 0);
    }

    #[test]
    fn waits_for_an_answer_while_the_turn_goes_on() {
        let unanswered: Vec<Entry> = recorded()
            .into_iter()
            .filter(|e| e.event["kind"] != "stop" && e.event["kind"] != "text" && e.seq < 14)
            .collect();
        let chat = build(&unanswered, true);
        assert_eq!(chat.waiting.len(), 1);
        assert_eq!(display_name(&chat.waiting[0].tool), "add_cell");
        let add = steps(&chat)
            .into_iter()
            .find(|s| s.tool.ends_with("add_cell"))
            .unwrap();
        assert_eq!(add.status, StepStatus::Wait);
        assert_eq!(kinds(&chat).last(), Some(&"step"));
    }

    #[test]
    fn puts_parts_together_and_marks_stopped_and_denied_steps() {
        let chat = build(
            &entries(vec![
                json!({"turn": "t1", "kind": "prompt", "text": "hi", "at_ms": 1}),
                json!({"turn": "t1", "kind": "text", "message": "m", "index": 1, "text": "world", "at_ms": 3}),
                json!({"turn": "t1", "kind": "text", "message": "m", "index": 0, "text": "Hello ", "at_ms": 2}),
                json!({"turn": "t1", "kind": "stop", "at_ms": 4}),
            ]),
            false,
        );
        assert_eq!(
            chat.items[2],
            Item::Footer {
                key: "ft1".into(),
                at_ms: Some(4),
                ms: Some(3),
                text: "Hello world".into()
            }
        );
        let chat = build(
            &entries(vec![
                json!({"turn": "t1", "kind": "prompt", "text": "one", "at_ms": 1}),
                json!({"turn": "t1", "kind": "step", "id": "a", "tool": "Bash", "input": {"command": "rm x"}, "at_ms": 2}),
                json!({"turn": "t1", "kind": "ask", "id": "q1", "tool": "Bash", "input": {"command": "rm x"}, "at_ms": 3}),
                json!({"kind": "answered", "id": "q1", "answer": {"behavior": "deny"}, "at_ms": 4}),
                json!({"turn": "t2", "kind": "prompt", "text": "two", "at_ms": 5}),
                json!({"turn": "t2", "kind": "step", "id": "b", "tool": "Bash", "input": {"command": "sleep 100"}, "at_ms": 6}),
            ]),
            false,
        );
        assert_eq!(
            steps(&chat).iter().map(|s| s.status).collect::<Vec<_>>(),
            [StepStatus::Deny, StepStatus::Cancel]
        );
        assert!(chat.waiting.is_empty());
    }

    #[test]
    fn shows_questions_codex_requests_and_errors() {
        let chat = build(
            &entries(vec![
                json!({"turn": "t1", "kind": "prompt", "text": "plan it", "at_ms": 1}),
                json!({"turn": "t1", "kind": "ask", "id": "q", "tool": "AskUserQuestion", "input": {"questions": [{"question": "Which?", "options": [{"label": "A"}]}]}, "at_ms": 2}),
            ]),
            true,
        );
        assert_eq!(kinds(&chat), ["user", "ask"]);
        let view = ask_view(&chat.waiting[0], None, "Claude");
        assert_eq!(view.questions[0].question, "Which?");

        let chat = build(
            &entries(vec![
                json!({"turn": "t1", "kind": "prompt", "text": "make a file", "at_ms": 1}),
                json!({"turn": "t1", "kind": "step", "id": "call_1", "tool": "Bash", "input": {"command": "echo hi"}, "at_ms": 2}),
                json!({"turn": "t1", "kind": "ask", "id": "q1", "step": "call_1", "tool": "Bash", "input": {"command": "echo hi", "description": "Say hi"}, "scopes": ["chat"], "at_ms": 3}),
            ]),
            true,
        );
        assert_eq!(kinds(&chat), ["user", "step"]);
        assert_eq!(steps(&chat)[0].status, StepStatus::Wait);

        let chat = build(
            &entries(vec![
                json!({"kind": "session", "source": "startup"}),
                json!({"kind": "error", "text": "Codex couldn't compact"}),
                json!({"turn": "t1", "kind": "prompt", "text": "hi", "at_ms": 1}),
                json!({"turn": "t1", "kind": "error", "text": "Usage limit", "at_ms": 2}),
                json!({"turn": "t1", "kind": "stop", "at_ms": 3}),
                json!({"kind": "session", "source": "compact"}),
            ]),
            false,
        );
        assert_eq!(kinds(&chat), ["error", "user", "error", "footer", "note"]);
    }

    #[test]
    fn describes_steps() {
        let edit = step_view(
            "Edit",
            &json!({"file_path": "/p/a.R", "old_string": "x <- 1", "new_string": "x <- 2\ny <- 3"}),
            None,
            None,
        );
        assert_eq!(edit.desc, "Edit a.R");
        assert_eq!(edit.detail.as_deref(), Some("+2 −1"));
        let run = step_view(
            "mcp__sandhill__run_code",
            &json!({"code": "dim(seu)"}),
            Some(
                &json!([{"type": "text", "text": "Ran in R session 1a2b (R 4.4.1) · idle · ok · 0.5 s\n[1] 10 20"}]),
            ),
            None,
        );
        assert_eq!(run.desc, "dim(seu)");
        assert_eq!(run.detail.as_deref(), Some("R · 0.5 s"));
        assert_eq!(run.output.as_deref(), Some("[1] 10 20"));
        let patch = step_view(
            "apply_patch",
            &json!({"changes": [{"path": "/p/x.py", "kind": "update", "diff": "@@ -1 +1 @@\n-a\n+b\n@@ -9 +9 @@\n c"}]}),
            None,
            None,
        );
        assert_eq!(patch.desc, "Edit x.py");
        assert_eq!(patch.diff.unwrap(), ["-a", "+b", " ⋯", " c"]);
        assert_eq!(
            display_name("mcp__github__create_issue"),
            "github · create_issue"
        );
    }

    #[test]
    fn titles_from_the_first_message() {
        assert_eq!(
            title_of(
                "The `markers` chunk is stale. Re-run it and tell me which cell type each cluster is."
            ),
            "The markers chunk is stale"
        );
        assert_eq!(
            title_of("Please check the QC thresholds in 01_qc.R against this dataset"),
            "Check the QC thresholds"
        );
        assert_eq!(
            title_of("can you plot @analysis.qmd the UMAP of"),
            "Plot the UMAP"
        );
        assert_eq!(
            title_of("Write results/cd14_stim_vs_ctrl.csv from the DE notebook"),
            "Write from the DE notebook"
        );
        assert_eq!(title_of("data.csv"), "New conversation");
    }
}
