//! What the message box offers, from the Tauri app's `model/agents.ts` and
//! `model/mentions.ts`: each agent's models and thinking effort, the modes
//! (Posit Assistant's names), and the @ menu (files, variables) and # menu
//! (cells of the notebooks named with @). Drawing is in `agents::view`.

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
    /// Actual returned IDs grouped by effort, only for Antigravity families.
    pub effort_families: Vec<Vec<(String, String)>>,
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Agents with the user's own configured models (OpenScience, Oh My Pi,
/// pi): effort levels only where a listed model has them.
pub fn configured_models(kind: &str) -> bool {
    matches!(kind, "openscience" | "omp" | "pi")
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
            effort_families: Vec::new(),
        },
        "codex" => Models {
            models: vec![ModelChoice {
                id: None,
                label: "Codex's default model".into(),
            }],
            start: 0,
            efforts: strings(&["low", "medium", "high", "xhigh"]),
            recommended: "medium".into(),
            effort_families: Vec::new(),
        },
        "openscience" | "omp" | "pi" => Models {
            models: vec![ModelChoice {
                id: None,
                label: match kind {
                    "omp" => "Oh My Pi's default model",
                    "pi" => "pi's default model",
                    _ => "OpenScience's configured model",
                }
                .into(),
            }],
            start: 0,
            efforts: Vec::new(),
            recommended: String::new(),
            effort_families: Vec::new(),
        },
        _ => Models {
            models: vec![ModelChoice {
                id: None,
                label: "Antigravity models not loaded".into(),
            }],
            start: 0,
            efforts: strings(&["low", "medium", "high"]),
            recommended: "medium".into(),
            effort_families: Vec::new(),
        },
    }
}

/// Only exact, returned sibling IDs form an effort family. A lone suffix
/// (or a similarly named model) is not evidence of interchangeable models.
fn variant(id: &str) -> Option<(&str, &str)> {
    let (base, effort) = id.rsplit_once('-')?;
    ["low", "medium", "high", "xhigh", "max"]
        .contains(&effort)
        .then_some((base, effort))
}

impl Models {
    fn family(&self, id: Option<&str>) -> Option<&Vec<(String, String)>> {
        let id = id?;
        self.effort_families
            .iter()
            .find(|family| family.iter().any(|(_, model)| model == id))
    }

    pub fn same_model(&self, a: Option<&str>, b: Option<&str>) -> bool {
        a == b
            || self
                .family(a)
                .is_some_and(|family| family.iter().any(|(_, model)| Some(model.as_str()) == b))
    }

    /// Keep a stored favourite's actual ID, with the family's display label.
    pub fn choice(&self, id: &str) -> ModelChoice {
        self.models
            .iter()
            .find(|m| self.same_model(m.id.as_deref(), Some(id)))
            .map(|m| ModelChoice {
                id: Some(id.into()),
                label: m.label.clone(),
            })
            .unwrap_or_else(|| ModelChoice {
                id: Some(id.into()),
                label: format!("{id} (unavailable)"),
            })
    }

    pub fn effort_model(&self, id: Option<&str>, effort: &str) -> Option<ModelChoice> {
        self.family(id)?
            .iter()
            .find(|(level, _)| level == effort)
            .map(|(_, model)| self.choice(model))
    }

    pub fn variant_effort(&self, id: Option<&str>) -> Option<&str> {
        self.family(id)?
            .iter()
            .find(|(_, model)| Some(model.as_str()) == id)
            .map(|(effort, _)| effort.as_str())
    }

    /// Whether `id` is already one of the rows, itself or an effort sibling.
    fn lists(&self, id: &str) -> bool {
        self.models
            .iter()
            .any(|m| self.same_model(m.id.as_deref(), Some(id)))
    }

    /// The effort a conversation starts with: exactly the one the box
    /// shows, picked or else the model's recommended level. Sent even when
    /// it is the recommended one, as a CLI left to itself takes its own saved
    /// default (Claude Code's per-model `effortLevel`, Codex's
    /// `model_reasoning_effort`), which may be another. None when the
    /// model's ID carries its effort, or it has no levels.
    pub fn start_effort(&self, model: Option<&str>, picked: Option<&str>) -> Option<String> {
        if self.variant_effort(model).is_some() {
            return None;
        }
        let offered = |e: &&str| self.efforts.iter().any(|level| level == e);
        picked
            .filter(offered)
            .or(Some(self.recommended.as_str()).filter(offered))
            .map(str::to_string)
    }
}

/// Keep listing order. Antigravity's first actual ID, not its CLI default
/// marker, is the new-conversation default.
pub fn listed_models(kind: &str, listed: &[Value], chosen: Option<&str>) -> Models {
    let mut models = known_models(kind);
    if listed.is_empty() {
        if let Some(id) = chosen
            && !models.lists(id)
        {
            models.models.push(models.choice(id));
        }
        return models;
    }
    let start = if kind == "antigravity" {
        0
    } else {
        listed
            .iter()
            .position(|m| m["default"] == true)
            .unwrap_or(0)
    };
    let selected = chosen.or_else(|| listed[start]["id"].as_str());
    models.models.clear();
    for m in listed {
        let id = m["id"].as_str();
        if models
            .models
            .iter()
            .any(|row| models.same_model(row.id.as_deref(), id))
        {
            continue;
        }
        let mut choice = ModelChoice {
            id: id.map(str::to_string),
            label: m["label"].as_str().or(id).unwrap_or_default().to_string(),
        };
        if kind == "antigravity"
            && let Some((base, _)) = id.and_then(variant)
        {
            let mut family: Vec<(String, String)> = listed
                .iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    let (stem, effort) = variant(id)?;
                    (stem == base).then(|| (effort.into(), id.into()))
                })
                .collect();
            family.sort_by_key(|(level, _)| {
                ["low", "medium", "high", "xhigh", "max"]
                    .iter()
                    .position(|e| e == level)
                    .unwrap()
            });
            family.dedup_by(|a, b| a.0 == b.0);
            if family.len() > 1 {
                let suffix = variant(id.unwrap()).unwrap().1;
                let title = effort_name(suffix);
                choice.label = choice
                    .label
                    .strip_suffix(&format!(" ({title})"))
                    .or_else(|| choice.label.strip_suffix(&format!(" {title}")))
                    .unwrap_or(&choice.label)
                    .to_string();
                models.effort_families.push(family);
            }
        }
        models.models.push(choice);
    }
    models.start = models
        .models
        .iter()
        .position(|m| models.same_model(m.id.as_deref(), listed[start]["id"].as_str()))
        .unwrap_or(0);
    if let Some(family) = models.family(selected) {
        let efforts = family.iter().map(|(level, _)| level.clone()).collect();
        let recommended = models.variant_effort(selected).unwrap().to_string();
        models.efforts = efforts;
        models.recommended = recommended;
    } else if let Some(model) = listed.iter().find(|m| m["id"].as_str() == selected) {
        if let Some(efforts) = model["efforts"].as_array().filter(|e| !e.is_empty()) {
            models.efforts = efforts
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect();
        }
        if let Some(effort) = model["effort"].as_str() {
            models.recommended = effort.into();
        }
    }
    if let Some(id) = chosen
        && !models.lists(id)
    {
        models.models.push(models.choice(id));
    }
    models
}

/// What a running agent offers: the models it reported, else the known ones.
pub fn running_models(t: &TerminalInfo, chosen: Option<&str>) -> Models {
    let listed = report(t, "models")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    listed_models(
        &t.kind,
        listed,
        chosen.or_else(|| {
            report(t, "status")
                .and_then(|s| s["model_id"].as_str())
                .or(t.options.model.as_deref())
        }),
    )
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

/// A mode: what the agent may do without asking, in the agent's own words
/// (Huan, 2026-11-16). `permission` is what the daemon starts the agent
/// with; [`DEFAULT`] is always the agent's own default.
#[derive(Debug, PartialEq)]
pub struct Mode {
    pub permission: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    /// Its icon in `docs/design/ui/icons/system`.
    pub icon: &'static str,
}

/// The permission every agent's own default mode has.
pub const DEFAULT: &str = "supervised";

const fn m(
    permission: &'static str,
    label: &'static str,
    icon: &'static str,
    detail: &'static str,
) -> Mode {
    Mode {
        permission,
        label,
        detail,
        icon,
    }
}

const CLAUDE_MODES: [Mode; 5] = [
    m(DEFAULT, "Manual", "ask", "Asks before edits and commands."),
    m(
        "edits",
        "Accept edits",
        "edit.rename",
        "Edits files without asking. Commands still ask.",
    ),
    m(
        "plan",
        "Plan mode",
        "plan",
        "Looks and plans, changing nothing, then asks before carrying the plan out.",
    ),
    m(
        "auto",
        "Auto mode",
        "sparkle.ai",
        "A second model approves safe steps. Risky ones still ask.",
    ),
    m(
        "full",
        "Bypass permissions",
        "auto",
        "Runs everything without asking. Only for folders you can afford to lose.",
    ),
];

const CODEX_MODES: [Mode; 5] = [
    m(
        "readonly",
        "Read Only",
        "context.show",
        "Reads the project. Asks before editing files or using the internet.",
    ),
    m(
        DEFAULT,
        "Default",
        "ask",
        "Reads and edits the project and runs commands. Asks before using the internet or editing other files.",
    ),
    m(
        "auto",
        "Approve for me",
        "sparkle.ai",
        "As Default, but a reviewer model answers its requests instead of you.",
    ),
    m(
        "full",
        "Full Access",
        "auto",
        "Edits anything and uses the internet without asking. Only for folders you can afford to lose.",
    ),
    m(
        "plan",
        "Plan mode",
        "plan",
        "Reads and plans, then asks before carrying the plan out.",
    ),
];

/// Headless Antigravity can't stop to ask: what it would ask about is
/// turned down.
const ANTIGRAVITY_MODES: [Mode; 5] = [
    m(
        DEFAULT,
        "Default",
        "ask",
        "Edits files in the project. Commands it would ask about are turned down.",
    ),
    m(
        "edits",
        "Accept edits",
        "edit.rename",
        "Accepts file edits. Other steps it would ask about are turned down.",
    ),
    m(
        "auto",
        "Auto",
        "sparkle.ai",
        "Antigravity's own reviewer decides which steps run.",
    ),
    m(
        "plan",
        "Plan",
        "plan",
        "Writes a plan first, then asks before carrying it out.",
    ),
    m(
        "full",
        "Skip permissions",
        "auto",
        "Runs everything without asking. Only for folders you can afford to lose.",
    ),
];

const OPENSCIENCE_MODES: [Mode; 2] = [
    m(
        DEFAULT,
        "Build",
        "ask",
        "Works on the project, with OpenScience's own permission settings.",
    ),
    m("plan", "Plan", "plan", "Looks and plans, changing nothing."),
];

const OMP_MODES: [Mode; 4] = [
    m(
        DEFAULT,
        "Always ask",
        "ask",
        "Asks before every edit and command.",
    ),
    m(
        "edits",
        "Write",
        "edit.rename",
        "Edits files without asking. Commands still ask.",
    ),
    m(
        "full",
        "YOLO",
        "auto",
        "Runs everything without asking. Only for folders you can afford to lose.",
    ),
    m(
        "plan",
        "Plan",
        "plan",
        "Looks and plans, then asks before carrying the plan out.",
    ),
];

/// The agent's own modes, in its order. pi has none (it runs everything),
/// and Posit Assistant keeps its own menu on its screen.
pub fn modes(kind: &str) -> &'static [Mode] {
    match kind {
        "claude" => &CLAUDE_MODES,
        "codex" => &CODEX_MODES,
        "antigravity" => &ANTIGRAVITY_MODES,
        "openscience" => &OPENSCIENCE_MODES,
        "omp" => &OMP_MODES,
        _ => &[],
    }
}

/// The agent's mode with `permission`, else its default; `None` when the
/// agent has no modes.
pub fn mode(kind: &str, permission: Option<&str>) -> Option<&'static Mode> {
    let modes = modes(kind);
    modes
        .iter()
        .find(|m| Some(m.permission) == permission)
        .or_else(|| modes.iter().find(|m| m.permission == DEFAULT))
}

/// The mode closest to read-only the agent has, for asking it only to
/// look (a notebook's Explain): Read Only, else Plan.
pub fn read_only_mode(kind: &str) -> Option<&'static str> {
    let modes = modes(kind);
    ["readonly", "plan"]
        .into_iter()
        .find(|p| modes.iter().any(|m| m.permission == *p))
}

/// The permission for a Claude Code permission mode.
pub fn claude_permission(claude: Option<&str>) -> Option<&'static str> {
    match claude? {
        // `manual` is what newer versions call `default`.
        "default" | "manual" => Some(DEFAULT),
        "acceptEdits" => Some("edits"),
        "plan" => Some("plan"),
        "auto" => Some("auto"),
        "bypassPermissions" => Some("full"),
        _ => None,
    }
}

/// The agent's mode now: as it last reported (Claude Code its own), else
/// the one it started in.
pub fn current_mode(t: &TerminalInfo) -> Option<&'static Mode> {
    let permission = claude_permission(report(t, "mode").and_then(Value::as_str))
        .or_else(|| report(t, "permission").and_then(Value::as_str))
        .or(t.options.permission.as_deref());
    mode(&t.kind, permission)
}

/// The permission of the mode Claude Code's terminal shows under its
/// prompt ("⏸ plan mode on"), from its last lines.
pub fn screen_mode(screen: &str) -> Option<&'static str> {
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
        return claude_permission(Some(claude));
    }
    // Older versions show nothing in the default mode.
    screen.contains("? for shortcuts").then_some(DEFAULT)
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

/// What the agent runs with now, as it last reported: what it starts again
/// with. Conversations are about the whole project, so no documents or pin.
pub fn live_options(t: &TerminalInfo) -> AgentOptions {
    let model = model_id(t);
    let encoded = running_models(t, model.as_deref())
        .variant_effort(model.as_deref())
        .is_some();
    AgentOptions {
        model,
        effort: effort_of(t).filter(|_| !encoded),
        permission: current_mode(t).map(|m| m.permission.to_string()),
        code: code_to_console(t).then(|| "console".to_string()),
        documents: Vec::new(),
        pin: None,
        refs: Vec::new(),
        account: t.options.account.clone(),
        fork: None,
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

/// The "/" list's groups, in order: Sandhill's own (Compact, its guides),
/// then the agent's commands and skills.
pub const SLASH_GROUPS: [(&str, &str); 3] = [
    ("sandhill", "Sandhill"),
    ("command", "Commands"),
    ("skill", "Skills"),
];

fn slash_group(kind: &str) -> &str {
    match kind {
        "compact" | "guide" => "sandhill",
        kind => kind,
    }
}

/// The "/" list's items matching `query` (any part of the name or
/// description, any case), grouped in list order.
pub fn filter_commands<'a>(all: &'a [Mention], query: &str) -> Vec<(String, Vec<&'a Mention>)> {
    let q = query.to_lowercase();
    SLASH_GROUPS
        .iter()
        .filter_map(|(group, label)| {
            let hits: Vec<&Mention> = all
                .iter()
                .filter(|m| {
                    slash_group(ref_kind(&m.item)) == *group
                        && (q.is_empty()
                            || ref_name(&m.item).to_lowercase().contains(&q)
                            || m.detail
                                .as_deref()
                                .is_some_and(|d| d.to_lowercase().contains(&q)))
                })
                .take(SHOWN)
                .collect();
            (!hits.is_empty()).then(|| (label.to_string(), hits))
        })
        .collect()
}

/// What a message with a "/" pick sends: its text, and for Codex's skills
/// the skill as a ref (Codex takes it as part of the message). Commands
/// and other agents' skills go as "/name text", as typed in the agent's
/// own screen; a Sandhill guide puts its instruction first.
pub fn slash_message(kind: &str, pick: &Value, text: &str) -> (String, Option<Value>) {
    let name = ref_name(pick);
    match ref_kind(pick) {
        "guide" => {
            let prompt = pick["prompt"].as_str().unwrap_or_default();
            let joined = if text.is_empty() {
                prompt.to_string()
            } else {
                format!("{prompt}\n\n{text}")
            };
            (joined, None)
        }
        "skill" if kind == "codex" => (
            text.to_string(),
            Some(json!({"kind": "skill", "name": name, "path": pick["path"]})),
        ),
        _ => (format!("/{name} {text}").trim_end().to_string(), None),
    }
}

/// Whether a chip is a "/" pick: one per message, first.
pub fn is_slash(item: &Value) -> bool {
    matches!(ref_kind(item), "command" | "skill" | "guide")
}

/// Agents Sandhill can ask to compact: Claude Code (its /compact), Codex
/// (app-server's compact) and pi (its RPC compact). Antigravity and the ACP
/// agents have no such request.
pub fn can_compact(kind: &str) -> bool {
    matches!(kind, "claude" | "codex" | "pi")
}

/// The commands a running OpenScience or Oh My Pi conversation reported.
pub fn reported_commands(t: &TerminalInfo) -> Vec<Value> {
    report(t, "commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The "@" (or "#", for cells) being typed before the caret (a byte
/// offset), and what follows it: `(where the sign is, query, sign)`. "/"
/// counts only at the start of the message, as agents read it there.
pub fn typed_mention(text: &str, caret: usize) -> Option<(usize, String, char)> {
    let before = text.get(..caret)?;
    if let Some(query) = before.strip_prefix('/') {
        return (!query.contains(char::is_whitespace)).then(|| (0, query.to_string(), '/'));
    }
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
    // Selections: the same code from the same place, whatever the name.
    if ref_kind(a) == "selection" && ref_kind(b) == "selection" {
        return a["path"] == b["path"] && a["cell"] == b["cell"] && a["text"] == b["text"];
    }
    ref_kind(a) == ref_kind(b) && ref_name(a) == ref_name(b)
}

/// Where attached files go, in the project (kept out of git).
pub const ATTACHMENTS: &str = ".sandhill/attachments";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openscience_models_preserve_configured_ids_labels_and_selection() {
        let known = known_models("openscience");
        assert_eq!(known.models[0].label, "OpenScience's configured model");
        assert!(known.efforts.is_empty());
        let listing = vec![
            json!({"id": "local/model-high", "label": "Local model High"}),
            json!({"id": "cloud/model", "label": "My configured model", "default": true}),
            json!({"id": "local/model-low", "label": "Local model Low"}),
        ];
        let listed = listed_models("openscience", &listing, Some("local/model-high"));
        assert_eq!(listed.models.len(), 3);
        assert_eq!(
            listed.models[listed.start].id.as_deref(),
            Some("cloud/model")
        );
        assert_eq!(listed.choice("local/model-high").label, "Local model High");
        assert!(listed.effort_families.is_empty());
        assert!(listed.efforts.is_empty());
        let terminal: TerminalInfo = serde_json::from_value(json!({
            "id": "os1", "kind": "openscience", "title": "OpenScience", "cwd": "/p",
            "created_ms": 0, "running": false, "end": 0,
            "options": {"model": "local/model-high", "permission": "supervised"},
            "agent": {
                "thread": "acp-session",
                "models": listing,
                "permission": "plan",
                "status": {"model_id": "cloud/model", "model": "My configured model"}
            }
        }))
        .unwrap();
        assert!(can_resume(&terminal));
        assert_eq!(model_id(&terminal).as_deref(), Some("cloud/model"));
        assert_eq!(model_label(&terminal), "My configured model");
        assert_eq!(current_mode(&terminal).map(|m| m.label), Some("Plan"));
        let options = live_options(&terminal);
        assert_eq!(options.model.as_deref(), Some("cloud/model"));
        assert_eq!(options.permission.as_deref(), Some("plan"));
        assert_eq!(options.effort, None);
        assert_eq!(
            running_models(&terminal, None).choice("cloud/model").label,
            "My configured model"
        );
    }

    fn antigravity_listing() -> Vec<Value> {
        vec![
            json!({"id": "gemini-3.8-flash-high", "label": "Gemini 3.8 Flash (High)"}),
            json!({"id": "gemini-3.8-flash-medium", "label": "Gemini 3.8 Flash (Medium)"}),
            json!({"id": "gemini-3.8-flash-low", "label": "Gemini 3.8 Flash (Low)"}),
            json!({"id": "claude-sonnet-4-6", "label": "Claude Sonnet 4.6", "default": true}),
        ]
    }

    #[test]
    fn antigravity_groups_actual_variants_and_routes_every_effort_to_a_returned_id() {
        let listed = antigravity_listing();
        let models = listed_models("antigravity", &listed, None);
        assert_eq!(models.models.len(), 2);
        assert_eq!(models.models[0].label, "Gemini 3.8 Flash");
        assert_eq!(
            models.models[0].id.as_deref(),
            Some("gemini-3.8-flash-high")
        );
        assert_eq!(models.efforts, ["low", "medium", "high"]);
        assert_eq!(models.recommended, "high");
        for level in ["low", "medium", "high"] {
            let choice = models
                .effort_model(models.models[0].id.as_deref(), level)
                .unwrap();
            assert_eq!(choice.id, Some(format!("gemini-3.8-flash-{level}")));
            assert!(
                listed
                    .iter()
                    .any(|m| m["id"].as_str() == choice.id.as_deref())
            );
            assert!(models.same_model(choice.id.as_deref(), models.models[0].id.as_deref()));
            let changed = listed_models("antigravity", &listed, choice.id.as_deref());
            assert_eq!(changed.recommended, level);
        }
        assert!(
            models
                .effort_model(models.models[0].id.as_deref(), "max")
                .is_none()
        );
    }

    #[test]
    fn only_antigravity_sibling_effort_ids_collapse_not_singletons_or_similar_names() {
        let listed = vec![
            json!({"id": "gemini-3.8-flash-high"}),
            json!({"id": "gemini-3.8-flash-lite-low"}),
            json!({"id": "gemini-3.8-flash-high-preview"}),
            json!({"id": "gpt-oss-120b-medium", "efforts": ["medium", "high"], "effort": "high"}),
        ];
        let models = listed_models("antigravity", &listed, Some("gpt-oss-120b-medium"));
        assert_eq!(models.models.len(), listed.len());
        assert!(models.effort_families.is_empty());
        assert_eq!(models.efforts, ["medium", "high"]);
        assert_eq!(models.recommended, "high");
        let codex = listed_models("codex", &antigravity_listing(), None);
        assert_eq!(codex.models.len(), 4);
        assert!(codex.effort_families.is_empty());
    }

    #[test]
    fn first_returned_antigravity_model_wins_but_other_providers_keep_cli_default() {
        let listed = antigravity_listing();
        let antigravity = listed_models("antigravity", &listed, None);
        assert_eq!(
            antigravity.models[antigravity.start].id,
            Some("gemini-3.8-flash-high".into())
        );
        let codex = listed_models("codex", &listed, None);
        assert_eq!(
            codex.models[codex.start].id,
            Some("claude-sonnet-4-6".into())
        );
        let low = listed_models("antigravity", &listed, Some("gemini-3.8-flash-low"));
        assert_eq!(low.recommended, "low");
        assert_eq!(
            low.choice("gemini-3.8-flash-low").id.as_deref(),
            Some("gemini-3.8-flash-low")
        );
        let stale = listed_models("antigravity", &listed, Some("removed-model"));
        assert_eq!(
            stale.models.last().unwrap().id.as_deref(),
            Some("removed-model")
        );
        assert!(stale.models.last().unwrap().label.contains("unavailable"));
    }

    #[test]
    fn live_options_preserve_actual_reported_variant_without_a_conflicting_effort() {
        let mut terminal: TerminalInfo = serde_json::from_value(json!({
            "id": "t1", "kind": "antigravity", "title": "Antigravity", "cwd": "/p",
            "created_ms": 0, "running": true, "end": 0,
            "options": {"model": "gemini-3.8-flash-high", "effort": "medium"},
            "agent": {
                "models": antigravity_listing(),
                "status": {"model_id": "gemini-3.8-flash-low", "effort": "low"}
            }
        }))
        .unwrap();
        let options = live_options(&terminal);
        assert_eq!(options.model.as_deref(), Some("gemini-3.8-flash-low"));
        assert_eq!(options.effort, None);
        let models = running_models(&terminal, Some("gemini-3.8-flash-medium"));
        assert_eq!(models.recommended, "medium");
        terminal.agent.as_mut().unwrap()["status"] =
            json!({"model_id": "claude-sonnet-4-6", "effort": "high"});
        let options = live_options(&terminal);
        assert_eq!(options.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(options.effort.as_deref(), Some("high"));
    }

    #[test]
    fn a_running_conversations_listed_model_is_its_row_not_a_second_one() {
        // Claude Code lists no models: its running model is one of the known rows.
        let terminal: TerminalInfo = serde_json::from_value(json!({
            "id": "t1", "kind": "claude", "title": "Claude", "cwd": "/p",
            "created_ms": 0, "running": true, "end": 0,
            "agent": {"status": {"model_id": "claude-haiku-5-5", "effort": "medium"}}
        }))
        .unwrap();
        let models = running_models(&terminal, None);
        assert_eq!(models.models, known_models("claude").models);
        let current = model_id(&terminal);
        let matching: Vec<_> = models
            .models
            .iter()
            .filter(|m| models.same_model(m.id.as_deref(), current.as_deref()))
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].label, "Haiku 5.5");
        // A model no row has is still offered, once.
        let other = listed_models("claude", &[], Some("claude-opus-4-1"));
        assert_eq!(other.models.len(), known_models("claude").models.len() + 1);
        // Listed providers: a returned ID or an effort sibling is never added again.
        let listed = antigravity_listing();
        for chosen in ["claude-sonnet-4-6", "gemini-3.8-flash-low"] {
            let models = listed_models("antigravity", &listed, Some(chosen));
            assert_eq!(models.models.len(), 2, "{chosen}");
        }
    }

    #[test]
    fn a_new_conversation_starts_with_exactly_the_effort_shown() {
        let claude = known_models("claude");
        let opus = Some("claude-opus-5-5");
        // Nothing picked: the recommended level the slider shows, sent so the
        // CLI's own saved default can't replace it.
        assert_eq!(claude.start_effort(opus, None).as_deref(), Some("medium"));
        assert_eq!(
            claude.start_effort(opus, Some("high")).as_deref(),
            Some("high")
        );
        // A level this model doesn't offer shows (and starts) as recommended.
        let codex = listed_models(
            "codex",
            &[json!({"id": "a", "efforts": ["low", "high"], "effort": "high", "default": true})],
            None,
        );
        assert_eq!(
            codex.start_effort(Some("a"), Some("medium")).as_deref(),
            Some("high")
        );
        assert_eq!(
            codex.start_effort(Some("a"), Some("low")).as_deref(),
            Some("low")
        );
        // An ID that carries its effort gets no second one; no levels, none.
        let antigravity = listed_models("antigravity", &antigravity_listing(), None);
        assert_eq!(
            antigravity.start_effort(Some("gemini-3.8-flash-high"), Some("low")),
            None
        );
        assert_eq!(known_models("openscience").start_effort(None, None), None);
    }

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
    fn slash_counts_only_at_the_start() {
        assert_eq!(typed_mention("/rev", 4), Some((0, "rev".into(), '/')));
        assert_eq!(typed_mention("/review now", 11), None);
        assert_eq!(typed_mention("see /tmp", 8), None);
        assert_eq!(typed_mention("@data/x", 7), Some((0, "data/x".into(), '@')));
    }

    #[test]
    fn slash_picks_go_as_each_agent_takes_them() {
        let skill = json!({"kind": "skill", "name": "scanpy", "path": "/s/SKILL.md"});
        assert_eq!(
            slash_message("claude", &skill, "on adata"),
            ("/scanpy on adata".into(), None)
        );
        let (text, sent) = slash_message("codex", &skill, "on adata");
        assert_eq!(text, "on adata");
        assert_eq!(sent.unwrap()["path"], "/s/SKILL.md");
        let command = json!({"kind": "command", "name": "review"});
        assert_eq!(slash_message("omp", &command, "").0, "/review");
        let guide = json!({"kind": "guide", "name": "g", "prompt": "Follow g."});
        assert_eq!(
            slash_message("pi", &guide, "Plan it").0,
            "Follow g.\n\nPlan it"
        );
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
        let id = screen_mode;
        assert_eq!(
            id("❯ \n⏸ manual mode on · ? for shortcuts"),
            Some("supervised")
        );
        assert_eq!(
            id("⏸ manual mode on · ? for shortcuts\n⏵⏵ accept edits on (shift+tab to cycle)"),
            Some("edits")
        );
        assert_eq!(id("⏸ plan mode on (shift+tab to cycle)"), Some("plan"));
        assert_eq!(id("⏵⏵ bypass permissions on"), Some("full"));
        assert_eq!(id("  ? for shortcuts"), Some("supervised"));
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

#[cfg(test)]
mod account_tests {
    use super::live_options;
    use sandhill_proto::TerminalInfo;
    use serde_json::json;

    #[test]
    fn resumed_options_keep_the_conversations_account() {
        let terminal: TerminalInfo = serde_json::from_value(json!({
            "id":"chat", "kind":"codex", "title":"Codex", "cwd":"/project",
            "created_ms":1, "running":true, "end":0,
            "options":{"account":"codex:work"}
        }))
        .unwrap();
        assert_eq!(
            live_options(&terminal).account.as_deref(),
            Some("codex:work")
        );
        let mut default = terminal;
        default.options.account = None;
        assert_eq!(live_options(&default).account, None);
    }
}
