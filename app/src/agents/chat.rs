//! An agent's conversation as the AI pane shows it, from the entries the
//! daemon keeps of what the agent's hooks report: the user's messages, the
//! parts of each reply, each step with its result, and what the agent asks
//! the user. The same rules as the Tauri app's `app/src/model/chat.ts`.

use std::collections::HashMap;

use serde_json::Value;

pub(crate) mod step_details;
use step_details::{base_name, language_name, s, step_view};

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
    /// Exact approval choices offered by ACP, not Sandhill permission scopes.
    pub options: Option<Vec<ApprovalOption>>,
}

impl Ask {
    pub fn answered_option(&self) -> Option<&ApprovalOption> {
        let id = self.answer.as_ref()?["option_id"].as_str()?;
        self.options
            .as_ref()?
            .iter()
            .find(|option| option.option_id == id)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ApprovalOption {
    #[serde(rename = "optionId")]
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

impl ApprovalOption {
    pub fn answer(&self) -> Value {
        serde_json::json!({"option_id": self.option_id})
    }

    pub fn rejects(&self) -> bool {
        matches!(self.kind.as_str(), "reject_once" | "reject_always")
    }
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
    Account {
        key: String,
        name: String,
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
    // Exchanges undone (Codex's Undo last exchange): their entries go.
    let mut undone: Vec<&str> = Vec::new();
    for entry in entries {
        match str_of(&entry.event, "kind") {
            Some("answered") => {
                if let Some(id) = str_of(&entry.event, "id") {
                    answers.insert(id.to_string(), entry.event["answer"].clone());
                }
            }
            Some("revert") => undone.extend(str_of(&entry.event, "turn")),
            _ => {}
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
        if kind == "revert" {
            order.push(Part::Loose(entry.clone()));
            continue;
        }
        let Some(turn_id) = str_of(event, "turn") else {
            if matches!(kind, "session" | "error" | "account") {
                order.push(Part::Loose(entry.clone()));
            }
            continue;
        };
        if undone.contains(&turn_id) {
            continue;
        }
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
                } else if str_of(&entry.event, "kind") == Some("account") {
                    chat.items.push(Item::Account {
                        key: format!("account{}", entry.seq),
                        name: str_of(&entry.event, "name").unwrap_or("account").into(),
                    });
                } else if str_of(&entry.event, "kind") == Some("revert") {
                    chat.items.push(Item::Note {
                        key: format!("n{}", entry.seq),
                        text: "The last exchange was undone.".into(),
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
                    // Streamed steps (ACP and pi reasoning) repeat the same id
                    // with more text: update that row rather than add another.
                    if let Some(&(_, at)) = steps.iter().find(|(s, _)| *s == id)
                        && let Item::Step { step, .. } = &mut chat.items[at]
                    {
                        step.input = object_or_empty(&event["input"]);
                        continue;
                    }
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
                        options: event
                            .get("options")
                            .and_then(|options| serde_json::from_value(options.clone()).ok()),
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

/// Only the latest reply/error turn can offer continuation after a usage limit.
pub fn latest_usage_limit(chat: &Chat) -> bool {
    chat.items
        .iter()
        .rev()
        .take_while(|item| !matches!(item, Item::User { .. } | Item::Account { .. }))
        .any(|item| match item {
            Item::Reply { text, .. } | Item::Error { text, .. } => usage_limit_text(text),
            _ => false,
        })
}

fn usage_limit_text(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "usage limit reached",
        "usage limit exceeded",
        "you've hit your usage limit",
        "you’ve hit your usage limit",
        "you've reached your usage limit",
        "you have reached your usage limit",
        "you have exceeded your usage limit",
        "usage_limit_reached",
        "you've hit your limit",
        "you’ve hit your limit",
        "you've reached your limit",
        "you have reached your limit",
        "rate limit reached",
        "rate limit exceeded",
        "quota exceeded",
        "quota has been exceeded",
        "you have reached the quota limit",
        "quota exhausted",
        "resource_exhausted",
        "exhausted your capacity",
        "out of extra usage",
    ]
    .iter()
    .any(|wording| text.contains(wording))
        || text.trim() == "usage limit"
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
        if ask.answer.as_ref().and_then(|a| a["behavior"].as_str()) == Some("deny")
            || ask.answered_option().is_some_and(ApprovalOption::rejects)
        {
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
            .filter_map(|c| match c["type"].as_str() {
                Some("text") => c["text"].as_str(),
                Some("content") if c["content"]["type"] == "text" => c["content"]["text"].as_str(),
                _ => None,
            })
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
    if ask.options.is_some() {
        let title = s(&input["title"])
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("Allow {}?", view.name));
        return plain(title, None, view);
    }
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
        | "list_jobs" | "read_job_log" | "get_console" | "get_plots" | "view_table"
        | "get_help" | "list_packages" => {
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
mod tests;

#[cfg(test)]
mod account_tests {
    use super::{Chat, Entry, Item, build, latest_usage_limit};
    use serde_json::{Value, json};

    fn conversation(events: Vec<Value>) -> Chat {
        build(
            &events
                .into_iter()
                .enumerate()
                .map(|(seq, event)| Entry {
                    seq: seq as u64,
                    event,
                })
                .collect::<Vec<_>>(),
            false,
        )
    }

    #[test]
    fn account_divider_preserves_order_and_ends_old_limit_offer() {
        let chat = conversation(vec![
            json!({"kind":"prompt","turn":"one","text":"Hello"}),
            json!({"kind":"error","turn":"one","text":"Usage limit reached"}),
            json!({"kind":"stop","turn":"one"}),
            json!({"kind":"account","account":"claude:work","name":"Work"}),
        ]);
        assert!(matches!(chat.items.last(), Some(Item::Account { name, .. }) if name == "Work"));
        assert!(!latest_usage_limit(&chat));
    }

    #[test]
    fn latest_turn_only_matches_agents_limit_messages() {
        for text in [
            "You've hit your limit · resets 5pm",
            "You've reached your usage limit",
            "You have exhausted your capacity on this model",
            "RESOURCE_EXHAUSTED: quota exceeded",
        ] {
            let mut events = vec![
                json!({"kind":"prompt","turn":"one","text":"Hello"}),
                json!({"kind":"text","turn":"one","text":text}),
                json!({"kind":"stop","turn":"one"}),
            ];
            assert!(latest_usage_limit(&conversation(events.clone())), "{text}");
            events.extend([
                json!({"kind":"prompt","turn":"two","text":"Try again"}),
                json!({"kind":"text","turn":"two","text":"Done"}),
                json!({"kind":"stop","turn":"two"}),
            ]);
            assert!(!latest_usage_limit(&conversation(events)));
        }
        assert!(!latest_usage_limit(&conversation(vec![
            json!({"kind":"error","text":"Connection refused"}),
        ])));
    }
}
