//! The message box's choices for a conversation (draft 30), kept in its
//! report (`agent.json`) from the options it started with and `set`
//! commands from the app:
//!
//! - `pin`: the file the conversation works in. Unpinned, each message
//!   works in the editor's front document, which the message records as
//!   `document`, so a working agent keeps the file it started in.
//! - `web: false`: no web search. Claude Code's `PreToolUse` hook blocks
//!   its web tools, and both agents are told with each message.
//! - `context: false`: where the user is in the editor stays out of their
//!   messages.
//! - `refs`: files, chunks, variables and plots the user added to the next
//!   message with the @ menu. That message takes them, and they go.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The report keys the app sets with `{kind: "set", ...}`; `refs` also
/// comes with a message (`{kind: "say", text, refs}`).
pub const KEYS: [&str; 4] = ["pin", "web", "context", "refs"];

/// Claude Code's tools that reach the web.
const WEB_TOOLS: [&str; 2] = ["WebSearch", "WebFetch"];

/// What the agent is told with each message while web search is off.
pub const WEB_OFF_NOTE: &str = "Sandhill: the user turned web search off for this conversation. \
    Don't search or fetch the web; work from the project, the sessions and what you know.";

/// Why a web step was blocked, for the agent and the chat.
pub const WEB_BLOCKED: &str = "Not run: web search is off for this conversation in Sandhill.";

#[derive(Debug, Clone, PartialEq)]
pub struct Choices {
    pub pin: Option<String>,
    pub web: bool,
    pub context: bool,
    pub refs: Vec<Ref>,
}

impl Default for Choices {
    fn default() -> Self {
        Choices {
            pin: None,
            web: true,
            context: true,
            refs: Vec::new(),
        }
    }
}

/// Something added to a message: `kind` is `file`, `chunk`, `variable`,
/// `plot` or `attach` (a file from the user's Mac), `name` how the app
/// showed it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Ref {
    pub kind: String,
    pub name: String,
    /// A file's path, as the agent should use it: a project file, or where
    /// a plot or an attached file was saved.
    #[serde(default)]
    pub path: Option<String>,
    /// A chunk's label or a notebook cell's id, in `path`.
    #[serde(default)]
    pub cell: Option<String>,
    /// A variable's session, and its language.
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

/// The choices in the terminal in `dir` now.
pub fn read(dir: &Path) -> Choices {
    of(&report(dir))
}

/// The report of the terminal in `dir`, null when it has none.
pub fn report(dir: &Path) -> Value {
    super::read_json(&super::Paths::new(dir).agent()).unwrap_or_default()
}

pub fn of(report: &Value) -> Choices {
    Choices {
        pin: report["pin"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(str::to_string),
        web: report["web"] != false,
        context: report["context"] != false,
        refs: serde_json::from_value(report["refs"].clone()).unwrap_or_default(),
    }
}

/// Whether Sandhill stops a step of Claude Code's: a web tool while web
/// search is off.
pub fn blocks(choices: &Choices, tool: &str) -> bool {
    !choices.web && WEB_TOOLS.contains(&tool)
}

/// What the agent is told of the things added to a message.
pub fn refs_text(refs: &[Ref]) -> Option<String> {
    if refs.is_empty() {
        return None;
    }
    let mut text = String::from("Sandhill: the user added these to this message:");
    for r in refs {
        let path = r.path.as_deref().unwrap_or(&r.name);
        let line = match r.kind.as_str() {
            "file" => format!("the file {path}"),
            "chunk" => match &r.cell {
                Some(cell) => {
                    format!("the cell `{cell}` of {path} (get_outputs reads what it showed)")
                }
                None => r.name.clone(),
            },
            "variable" => {
                let language = r.language.as_deref().map(language_name).unwrap_or("");
                let session = r
                    .session
                    .as_deref()
                    .map(|s| format!(" in session {s}"))
                    .unwrap_or_default();
                format!(
                    "the {language}{}variable `{}`{session} (list_variables looks inside it)",
                    if language.is_empty() { "" } else { " " },
                    r.name
                )
            }
            "plot" => format!(
                "the plot {}, saved as {path}: read the image to see it",
                r.name
            ),
            _ => format!("{}, a file from their Mac, saved as {path}", r.name),
        };
        text.push_str(&format!("\n- {line}"));
    }
    Some(text)
}

fn language_name(language: &str) -> &'static str {
    match crate::cells::normalize_language(language).as_str() {
        "r" => "R",
        "python" => "Python",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_choices_come_from_the_report() {
        assert_eq!(of(&json!({})), Choices::default());
        let choices = of(&json!({
            "pin": "fig3.qmd",
            "web": false,
            "context": true,
            "refs": [{"kind": "file", "name": "theme.R", "path": "R/theme.R"}],
        }));
        assert_eq!(choices.pin.as_deref(), Some("fig3.qmd"));
        assert!(!choices.web && choices.context);
        assert_eq!(choices.refs[0].path.as_deref(), Some("R/theme.R"));
        // Cleared refs and an unpinned file read as none.
        let cleared = of(&json!({"pin": null, "refs": null}));
        assert_eq!(cleared, Choices::default());
        assert!(blocks(&choices, "WebSearch") && !blocks(&choices, "Read"));
        assert!(!blocks(&cleared, "WebFetch"));
    }

    #[test]
    fn the_agent_is_told_what_came_with_a_message() {
        let refs: Vec<Ref> = serde_json::from_value(json!([
            {"kind": "file", "name": "data/raw/s1.csv", "path": "data/raw/s1.csv"},
            {"kind": "chunk", "name": "analysis.qmd › markers", "path": "analysis.qmd", "cell": "markers"},
            {"kind": "variable", "name": "seu", "session": "r-7f3a", "language": "R"},
            {"kind": "plot", "name": "umap · analysis.qmd", "path": ".sandhill/attachments/umap.png"},
            {"kind": "attach", "name": "gel.png", "path": ".sandhill/attachments/gel.png"},
        ]))
        .unwrap();
        let text = refs_text(&refs).unwrap();
        assert_eq!(
            text,
            "Sandhill: the user added these to this message:\n\
             - the file data/raw/s1.csv\n\
             - the cell `markers` of analysis.qmd (get_outputs reads what it showed)\n\
             - the R variable `seu` in session r-7f3a (list_variables looks inside it)\n\
             - the plot umap · analysis.qmd, saved as .sandhill/attachments/umap.png: read the image to see it\n\
             - gel.png, a file from their Mac, saved as .sandhill/attachments/gel.png"
        );
        assert_eq!(refs_text(&[]), None);
    }
}
