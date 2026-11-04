//! What the message box offers, from the Tauri app's `model/agents.ts` and
//! `model/mentions.ts`: each agent's models and thinking effort, the modes
//! (Posit Assistant's names), and the @ menu (files, variables) and # menu
//! (cells of the notebooks named with @). Drawing is in `agents_view`.

use sandhill_daemon::qmd;
use sandhill_proto::{AgentOptions, TerminalInfo};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    /// What the CLI takes; `None` leaves the CLI's own choice.
    pub id: Option<String>,
    pub label: String,
}

/// An agent's models and efforts, as its model menu lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct Models {
    pub models: Vec<ModelChoice>,
    /// The model new conversations get until another is picked.
    pub start: usize,
    /// Thinking effort levels, lowest first.
    pub efforts: Vec<String>,
    /// The level a session gets when none is picked: the CLI's own.
    pub recommended: String,
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// What an agent offers before it lists its own models.
pub fn known_models(kind: &str) -> Models {
    match kind {
        "claude" => Models {
            models: [
                ("claude-fable-5-1", "Fable 5.1"),
                ("claude-opus-5-5", "Opus 5.5"),
                ("claude-sonnet-5-5", "Sonnet 5.5"),
                ("claude-haiku-5-5", "Haiku 5.5"),
            ]
            .iter()
            .map(|(id, label)| ModelChoice {
                id: Some(id.to_string()),
                label: label.to_string(),
            })
            .collect(),
            start: 1,
            efforts: strings(&["low", "medium", "high", "xhigh", "max"]),
            recommended: "medium".into(),
        },
        "codex" => Models {
            models: vec![ModelChoice {
                id: None,
                label: "Codex's default model".into(),
            }],
            start: 0,
            efforts: strings(&["low", "medium", "high", "xhigh"]),
            recommended: "medium".into(),
        },
        _ => Models {
            models: vec![ModelChoice {
                id: None,
                label: "Antigravity's default model".into(),
            }],
            start: 0,
            efforts: strings(&["low", "medium", "high"]),
            recommended: "medium".into(),
        },
    }
}

/// An agent's models as it listed them (`agent_models`, or a running
/// agent's report): `[{id, label, efforts?, effort?, default?}]`, with the
/// efforts of the one `chosen` (else its default).
pub fn listed_models(kind: &str, listed: &[Value], chosen: Option<&str>) -> Models {
    let mut models = known_models(kind);
    if listed.is_empty() {
        return models;
    }
    let start = listed
        .iter()
        .position(|m| m["default"] == true)
        .unwrap_or(0);
    let model = chosen
        .and_then(|id| listed.iter().find(|m| m["id"] == id))
        .unwrap_or(&listed[start]);
    models.models = listed
        .iter()
        .map(|m| ModelChoice {
            id: m["id"].as_str().map(str::to_string),
            label: m["label"]
                .as_str()
                .or(m["id"].as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    models.start = start;
    if let Some(efforts) = model["efforts"].as_array().filter(|e| !e.is_empty()) {
        models.efforts = efforts
            .iter()
            .filter_map(|e| e.as_str().map(str::to_string))
            .collect();
    }
    if let Some(effort) = model["effort"].as_str() {
        models.recommended = effort.to_string();
    }
    models
}

/// What a running agent offers: the models it reported, else the known ones.
pub fn running_models(t: &TerminalInfo) -> Models {
    let listed = report(t, "models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    listed_models(&t.kind, &listed, model_id(t).as_deref())
}

fn report<'a>(t: &'a TerminalInfo, key: &str) -> Option<&'a Value> {
    t.agent.as_ref().map(|a| &a[key]).filter(|v| !v.is_null())
}

/// The model the agent uses: as it last reported, else as it started.
pub fn model_id(t: &TerminalInfo) -> Option<String> {
    report(t, "status")
        .and_then(|s| s["model_id"].as_str())
        .map(str::to_string)
        .or_else(|| t.options.model.clone())
}

/// "Opus 5.5" once the agent says, else what it was started with.
pub fn model_label(t: &TerminalInfo) -> String {
    if let Some(model) = report(t, "status").and_then(|s| s["model"].as_str()) {
        return model.trim_start_matches("Claude ").to_string();
    }
    let id = t.options.model.as_deref();
    known_models(&t.kind)
        .models
        .into_iter()
        .find(|m| m.id.as_deref() == id && id.is_some())
        .map(|m| m.label)
        .or_else(|| id.map(str::to_string))
        .unwrap_or_else(|| {
            crate::agents::kind_of(&t.kind).map_or(t.title.clone(), |k| k.name.into())
        })
}

/// The effort the agent last reported, else the one it started with.
pub fn effort_of(t: &TerminalInfo) -> Option<String> {
    report(t, "status")
        .and_then(|s| s["effort"].as_str())
        .map(str::to_string)
        .or_else(|| t.options.effort.clone())
}

/// "Opus" for "Opus 5.5": the chip leaves the version out.
pub fn short_model(label: &str) -> String {
    match label.rsplit_once(' ') {
        Some((name, version))
            if version.starts_with(|c: char| c.is_ascii_digit())
                && version.chars().all(|c| c.is_ascii_digit() || c == '.') =>
        {
            name.to_string()
        }
        _ => label.to_string(),
    }
}

/// "High", "Extra high".
pub fn effort_name(level: &str) -> String {
    if level == "xhigh" {
        return "Extra high".into();
    }
    let mut chars = level.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// "12K", "200K", "1M".
pub fn tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let m = (n as f64 / 100_000.).round() / 10.;
        if m.fract() == 0. {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1000 {
        format!("{}K", (n as f64 / 1000.).round() as u64)
    } else {
        n.to_string()
    }
}

/// A mode: what the agent may do without asking. `permission` is what the
/// daemon starts the agent with, `claude` Claude Code's own mode.
#[derive(Debug, PartialEq)]
pub struct Mode {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub permission: &'static str,
    pub claude: &'static str,
    /// Its icon in `docs/design/ui/icons/system`.
    pub icon: &'static str,
    /// Offered in the mode menu.
    pub offered: bool,
}

pub const MODES: [Mode; 7] = [
    Mode {
        id: "normal",
        label: "Normal",
        detail: "Safe steps run. Risky ones ask first.",
        permission: "supervised",
        claude: "default",
        icon: "ask",
        offered: true,
    },
    Mode {
        id: "auto",
        label: "Auto",
        detail: "A second model reviews risky steps.",
        permission: "auto",
        claude: "auto",
        icon: "sparkle.ai",
        offered: true,
    },
    Mode {
        id: "yolo",
        label: "YOLO",
        detail: "Everything runs without asking. Only for folders you can afford to lose.",
        permission: "full",
        claude: "bypassPermissions",
        icon: "auto",
        offered: true,
    },
    // Sandhill's own: Claude Code runs in its default mode, and Sandhill's
    // hooks ask about or block each step.
    Mode {
        id: "restricted",
        label: "Restricted",
        detail: "Asks before every step.",
        permission: "restricted",
        claude: "default",
        icon: "lock",
        offered: true,
    },
    Mode {
        id: "ask",
        label: "Ask",
        detail: "Read-only. No edits and no code runs; suggestions stay in the chat.",
        permission: "ask",
        claude: "default",
        icon: "context.show",
        offered: true,
    },
    Mode {
        id: "plan",
        label: "Plan",
        detail: "Writes a plan first, then asks before carrying it out.",
        permission: "plan",
        claude: "plan",
        icon: "plan",
        offered: true,
    },
    // Claude Code's own, between Normal and Plan (⇧Tab in its terminal).
    Mode {
        id: "edits",
        label: "Edits",
        detail: "Edits files without asking. Other steps ask first.",
        permission: "edits",
        claude: "acceptEdits",
        icon: "edit.rename",
        offered: false,
    },
];

pub fn mode(id: &str) -> &'static Mode {
    MODES.iter().find(|m| m.id == id).unwrap_or(&MODES[0])
}

/// Whether the agent has the mode: headless Antigravity can't stop to ask.
pub fn has_mode(kind: &str, mode: &Mode) -> bool {
    kind != "antigravity" || matches!(mode.id, "normal" | "yolo" | "plan")
}

/// What a mode does with this agent.
pub fn mode_detail(kind: &str, mode: &Mode) -> &'static str {
    if kind == "antigravity" && mode.id == "normal" {
        return "Edits files in the project. Commands it would ask about are turned down.";
    }
    mode.detail
}

/// The mode a session started in, by its `permission`.
pub fn mode_by_permission(permission: Option<&str>) -> &'static Mode {
    MODES
        .iter()
        .find(|m| Some(m.permission) == permission)
        .unwrap_or(&MODES[0])
}

/// The mode for a Claude Code permission mode.
pub fn claude_mode(claude: Option<&str>) -> Option<&'static Mode> {
    // `manual` is what newer versions call `default`.
    let claude = if claude == Some("manual") {
        Some("default")
    } else {
        claude
    };
    MODES.iter().find(|m| Some(m.claude) == claude)
}

/// Sandhill's own mode when one is on (Restricted, Ask), else the mode the
/// agent last reported, else the one it started in.
pub fn current_mode(t: &TerminalInfo) -> &'static Mode {
    if let Some(guard) = report(t, "guard").and_then(Value::as_str) {
        return mode_by_permission(Some(guard));
    }
    claude_mode(report(t, "mode").and_then(Value::as_str)).unwrap_or_else(|| {
        mode_by_permission(
            report(t, "permission")
                .and_then(Value::as_str)
                .or(t.options.permission.as_deref()),
        )
    })
}

/// The mode Claude Code's terminal shows under its prompt ("⏸ plan mode
/// on"), from its last lines.
pub fn screen_mode(screen: &str) -> Option<&'static Mode> {
    const SHOWN: [(&str, &str); 5] = [
        ("manual mode on", "default"),
        ("accept edits on", "acceptEdits"),
        ("plan mode on", "plan"),
        ("auto mode on", "auto"),
        ("bypass permissions on", "bypassPermissions"),
    ];
    let last = SHOWN
        .iter()
        .filter_map(|(text, claude)| screen.rfind(text).map(|at| (at, *claude)))
        .max_by_key(|(at, _)| *at);
    if let Some((_, claude)) = last {
        return claude_mode(Some(claude));
    }
    // Older versions show nothing in the default mode.
    screen.contains("? for shortcuts").then_some(&MODES[0])
}

/// Whether Claude Code's screen asks if the user trusts the folder (it
/// does once for each folder, before anything else), and if so whether
/// its "Yes" is the one picked: `Some(true)` when Enter would trust it.
/// Its first choice is "No, exit", so a message typed at it ends the
/// conversation.
pub fn trust_question(screen: &str) -> Option<bool> {
    let asks = screen.contains("trust this folder")
        || screen.contains("Do you trust the files in this folder");
    if !asks {
        return None;
    }
    let picked = screen
        .lines()
        .find(|l| l.trim_start().starts_with('❯') || l.trim_start().starts_with('>'))
        .map(|l| l.trim_start().trim_start_matches(['❯', '>']).trim_start());
    Some(picked.is_some_and(|l| l.starts_with("Yes") || l.starts_with("1. Yes")))
}

/// Whether the agent can start again on its conversation: it said which.
pub fn can_resume(t: &TerminalInfo) -> bool {
    if t.kind == "claude" {
        return report(t, "session_id").is_some();
    }
    crate::agents::drives_chat(&t.kind) && report(t, "thread").is_some()
}

/// Where the agent's code goes: `true` for the Console.
pub fn code_to_console(t: &TerminalInfo) -> bool {
    match report(t, "code") {
        Some(code) => code == "console",
        None => t.options.code.as_deref() == Some("console"),
    }
}

/// Web search, or session context, on: unless the agent or its start said off.
pub fn switch_on(t: &TerminalInfo, key: &str) -> bool {
    let started = if key == "web" {
        t.options.web
    } else {
        t.options.context
    };
    report(t, key).and_then(Value::as_bool).or(started) != Some(false)
}

/// What the agent runs with now, as it last reported: what it starts again
/// with. Conversations are about the whole project, so no documents or pin.
pub fn live_options(t: &TerminalInfo) -> AgentOptions {
    AgentOptions {
        model: model_id(t),
        effort: effort_of(t),
        permission: Some(current_mode(t).permission.to_string()),
        code: code_to_console(t).then(|| "console".to_string()),
        documents: Vec::new(),
        pin: None,
        web: (!switch_on(t, "web")).then_some(false),
        context: (!switch_on(t, "context")).then_some(false),
        refs: Vec::new(),
    }
}

/// Something the @ or # menu offers: a message ref (`{kind, name, path?,
/// cell?, session?, language?}`, as the daemon takes them).
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    pub item: Value,
    /// What the menu shows, when not the ref's name: "cell 3".
    pub label: Option<String>,
    /// Grey text after it: a cell's first line of code.
    pub detail: Option<String>,
    /// What picking it saves first, as a plot's image is saved where the
    /// agent can see it.
    pub source: Option<Value>,
}

impl Mention {
    pub fn file(path: &str) -> Self {
        Mention {
            item: json!({"kind": "file", "name": path, "path": path}),
            label: None,
            detail: None,
            source: None,
        }
    }

    pub fn name(&self) -> &str {
        self.label
            .as_deref()
            .unwrap_or_else(|| ref_name(&self.item))
    }
}

pub fn ref_name(item: &Value) -> &str {
    item["name"].as_str().unwrap_or_default()
}

pub fn ref_kind(item: &Value) -> &str {
    item["kind"].as_str().unwrap_or_default()
}

/// The @ menu's groups, in order.
pub const GROUPS: [(&str, &str); 4] = [
    ("file", "Files"),
    ("chunk", "Cells"),
    ("variable", "Variables"),
    ("plot", "Plots"),
];

/// At most this many of each group are shown; typing narrows them.
pub const SHOWN: usize = 30;

/// The mentions matching `query` (any part of the name, any case), grouped
/// in menu order.
pub fn filter_mentions<'a>(all: &'a [Mention], query: &str) -> Vec<(String, Vec<&'a Mention>)> {
    let q = query.to_lowercase();
    GROUPS
        .iter()
        .filter_map(|(kind, label)| {
            let hits: Vec<&Mention> = all
                .iter()
                .filter(|m| {
                    ref_kind(&m.item) == *kind
                        && (q.is_empty() || ref_name(&m.item).to_lowercase().contains(&q))
                })
                .take(SHOWN)
                .collect();
            (!hits.is_empty()).then(|| (label.to_string(), hits))
        })
        .collect()
}

/// The "@" (or "#", for cells) being typed before the caret (a byte
/// offset), and what follows it: `(where the sign is, query, sign)`.
pub fn typed_mention(text: &str, caret: usize) -> Option<(usize, String, char)> {
    let before = text.get(..caret)?;
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || *c == '@' || *c == '#')?;
    let (at, sign) = start;
    if sign.is_whitespace() {
        return None;
    }
    // The sign starts the text or follows a space, so "me@x" isn't one.
    if before[..at]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    Some((at, before[at + 1..].to_string(), sign))
}

/// The documents a message names with @ whose cells # can pick.
pub fn cell_documents(refs: &[Value]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for r in refs {
        if ref_kind(r) != "file" {
            continue;
        }
        let Some(path) = r["path"].as_str() else {
            continue;
        };
        let lower = path.to_lowercase();
        if (lower.ends_with(".ipynb") || lower.ends_with(".qmd") || lower.ends_with(".rmd"))
            && !paths.iter().any(|p| p == path)
        {
            paths.push(path.to_string());
        }
    }
    paths
}

/// A code cell of a document, as the # menu names it.
#[derive(Debug, Clone, PartialEq)]
pub struct DocCell {
    pub id: String,
    /// "cell 3" in a notebook (its place among all cells), a chunk's label
    /// or "chunk 2" in Quarto.
    pub name: String,
    pub code: String,
}

/// A document's code cells, read from its saved text.
pub fn document_cells(path: &str, text: &str) -> Vec<DocCell> {
    if path.to_lowercase().ends_with(".ipynb") {
        let nb: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let cells = nb["cells"].as_array().cloned().unwrap_or_default();
        return cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c["cell_type"] == "code" && c["id"].is_string())
            .map(|(i, c)| DocCell {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                name: format!("cell {}", i + 1),
                code: match &c["source"] {
                    Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
                    other => other.as_str().unwrap_or_default().to_string(),
                },
            })
            .collect();
    }
    qmd::parse_chunks(text)
        .into_iter()
        .enumerate()
        .map(|(i, c)| DocCell {
            name: c.label.clone().unwrap_or(format!("chunk {}", i + 1)),
            id: c.cell_id,
            code: c.code,
        })
        .collect()
}

/// A document's cells as # menu items: "cell 3", with its first line.
pub fn cell_mentions(path: &str, name: &str, cells: &[DocCell]) -> Vec<Mention> {
    cells
        .iter()
        .map(|cell| Mention {
            item: json!({
                "kind": "chunk",
                "name": format!("{name} › {}", cell.name),
                "path": path,
                "cell": cell.id,
            }),
            source: None,
            label: Some(cell.name.clone()),
            detail: cell
                .code
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string),
        })
        .collect()
}

/// The # menu: the cells matching `query` (a number finds that cell, words
/// its name or code), under their document's name.
pub fn filter_cells<'a>(all: &'a [Mention], query: &str) -> Vec<(String, Vec<&'a Mention>)> {
    let q = query.to_lowercase();
    let numeric = !q.is_empty() && q.chars().all(|c| c.is_ascii_digit());
    let mut groups: Vec<(String, Vec<&Mention>)> = Vec::new();
    for m in all {
        let name = m.name();
        let number: String = {
            let tail: String = name
                .chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect();
            tail.chars().rev().collect()
        };
        let hit = q.is_empty()
            || (numeric && !number.is_empty() && number.starts_with(&q))
            || format!("{name} {}", m.detail.as_deref().unwrap_or_default())
                .to_lowercase()
                .contains(&q);
        if !hit {
            continue;
        }
        let doc = m.item["path"]
            .as_str()
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        match groups.iter_mut().find(|(d, _)| *d == doc) {
            Some((_, list)) if list.len() < SHOWN => list.push(m),
            Some(_) => {}
            None => groups.push((doc, vec![m])),
        }
    }
    groups
}

/// Whether a ref is already a chip: the same kind and name.
pub fn same_ref(a: &Value, b: &Value) -> bool {
    ref_kind(a) == ref_kind(b) && ref_name(a) == ref_name(b)
}

/// Where attached files go, in the project (kept out of git).
pub const ATTACHMENTS: &str = ".sandhill/attachments";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_sign_being_typed() {
        assert_eq!(
            typed_mention("look at @an", 11),
            Some((8, "an".into(), '@'))
        );
        assert_eq!(typed_mention("@", 1), Some((0, "".into(), '@')));
        assert_eq!(typed_mention("fix #3", 6), Some((4, "3".into(), '#')));
        assert_eq!(typed_mention("issue#3", 7), None);
        assert_eq!(typed_mention("mail me@x", 9), None);
        assert_eq!(typed_mention("@seu and more", 13), None);
    }

    #[test]
    fn groups_matches_in_menu_order() {
        let all = vec![
            Mention {
                item: json!({"kind": "variable", "name": "markers"}),
                source: None,
                label: None,
                detail: None,
            },
            Mention::file("R/markers.R"),
            Mention {
                item: json!({"kind": "plot", "name": "umap · analysis.qmd"}),
                source: None,
                label: None,
                detail: None,
            },
        ];
        let groups: Vec<(String, Vec<&str>)> = filter_mentions(&all, "MARK")
            .into_iter()
            .map(|(label, list)| (label, list.iter().map(|m| ref_name(&m.item)).collect()))
            .collect();
        assert_eq!(
            groups,
            vec![
                ("Files".to_string(), vec!["R/markers.R"]),
                ("Variables".to_string(), vec!["markers"]),
            ]
        );
    }

    #[test]
    fn lists_the_cells_of_named_notebooks_by_number() {
        let refs = vec![
            json!({"kind": "file", "name": "de.ipynb", "path": "de.ipynb"}),
            json!({"kind": "file", "name": "R/markers.R", "path": "R/markers.R"}),
            json!({"kind": "file", "name": "report.qmd", "path": "docs/report.qmd"}),
        ];
        assert_eq!(cell_documents(&refs), vec!["de.ipynb", "docs/report.qmd"]);
        let nb = json!({"cells": [
            {"cell_type": "markdown", "id": "t", "source": "# QC"},
            {"cell_type": "code", "id": "load", "source": ["\n", "adata = sc.read('x')\n"]},
            {"cell_type": "code", "id": "umap", "source": "sc.pl.umap(adata)"},
        ]});
        let cells = document_cells("de.ipynb", &nb.to_string());
        let names: Vec<&str> = cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["cell 2", "cell 3"]);
        let qmd = document_cells(
            "docs/report.qmd",
            "```{r setup}\nlibrary(x)\n```\n\n```{r}\nplot(1)\n```\n",
        );
        let names: Vec<&str> = qmd.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["setup", "chunk 2"]);
        let all = cell_mentions("de.ipynb", "de.ipynb", &cells);
        assert_eq!(all[0].item["name"], "de.ipynb › cell 2");
        assert_eq!(all[0].item["cell"], "load");
        assert_eq!(all[0].detail.as_deref(), Some("adata = sc.read('x')"));
        let by_number = filter_cells(&all, "3");
        assert_eq!(by_number.len(), 1);
        assert_eq!(by_number[0].0, "de.ipynb");
        assert_eq!(by_number[0].1[0].name(), "cell 3");
        assert_eq!(filter_cells(&all, "umap")[0].1[0].item["cell"], "umap");
    }

    #[test]
    fn reads_the_mode_from_claude_codes_screen() {
        let id = |s: &str| screen_mode(s).map(|m| m.id);
        assert_eq!(id("❯ \n⏸ manual mode on · ? for shortcuts"), Some("normal"));
        assert_eq!(
            id("⏸ manual mode on · ? for shortcuts\n⏵⏵ accept edits on (shift+tab to cycle)"),
            Some("edits")
        );
        assert_eq!(id("⏸ plan mode on (shift+tab to cycle)"), Some("plan"));
        assert_eq!(id("⏵⏵ bypass permissions on"), Some("yolo"));
        assert_eq!(id("  ? for shortcuts"), Some("normal"));
        assert_eq!(id("Do you trust the files in this folder?"), None);
    }

    #[test]
    fn sees_claude_codes_trust_question() {
        let screen = |picked_yes: bool| {
            format!(
                " Accessing workspace:\n /home/huan/proj\n Quick safety check: Is this a project you created or one you trust?\n {} No, exit\n {} Yes, I trust this folder\n Enter to confirm · Esc to cancel",
                if picked_yes { " " } else { "❯" },
                if picked_yes { "❯" } else { " " },
            )
        };
        assert_eq!(trust_question(&screen(false)), Some(false));
        assert_eq!(trust_question(&screen(true)), Some(true));
        assert_eq!(
            trust_question("❯ \n⏸ manual mode on · ? for shortcuts"),
            None
        );
    }

    #[test]
    fn names_models_efforts_and_sizes() {
        assert_eq!(short_model("Opus 5.5"), "Opus");
        assert_eq!(
            short_model("Codex's default model"),
            "Codex's default model"
        );
        assert_eq!(effort_name("xhigh"), "Extra high");
        assert_eq!(effort_name("high"), "High");
        assert_eq!(tokens(12_300), "12K");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_500_000), "1.5M");
        assert_eq!(tokens(900), "900");
        let listed = vec![
            json!({"id": "a", "label": "A", "efforts": ["low", "high"], "effort": "high"}),
            json!({"id": "b", "label": "B", "default": true}),
        ];
        let models = listed_models("codex", &listed, Some("a"));
        assert_eq!(models.start, 1);
        assert_eq!(models.efforts, vec!["low", "high"]);
        assert_eq!(models.recommended, "high");
        assert_eq!(listed_models("codex", &listed, None).efforts.len(), 4);
    }
}
