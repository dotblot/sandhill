//! The message box's choices for a conversation (draft 30), kept in its
//! report (`agent.json`) from the options it started with and `set`
//! commands from the app:
//!
//! - `pin`: the file the conversation works in. Unpinned, each message
//!   works in the editor's front document, which the message records as
//!   `document`, so a working agent keeps the file it started in.
//! - `refs`: files, chunks, variables and plots the user added to the next
//!   message with the @ menu. That message takes them, and they go.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The report keys the app sets with `{kind: "set", ...}`; `refs` also
/// comes with a message (`{kind: "say", text, refs}`).
pub const KEYS: [&str; 2] = ["pin", "refs"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Choices {
    pub pin: Option<String>,
    pub refs: Vec<Ref>,
}

/// Something added to a message: `kind` is `file`, `chunk`, `variable`,
/// `plot`, `attach` (a file from the user's Mac), or `skill` (native Codex
/// input); `name` is how the app showed it.
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
    /// The code a `selection` chip carries, as the user selected it.
    #[serde(default)]
    pub text: Option<String>,
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
        refs: serde_json::from_value(report["refs"].clone()).unwrap_or_default(),
    }
}

/// What the agent is told of the things added to a message.
pub fn refs_text(refs: &[Ref]) -> Option<String> {
    if !refs.iter().any(|r| r.kind != "skill") {
        return None;
    }
    let mut text = String::from("Sandhill: the user added these to this message:");
    for r in refs.iter().filter(|r| r.kind != "skill") {
        let path = r.path.as_deref().unwrap_or(&r.name);
        let line = match r.kind.as_str() {
            "file" => format!("the file {path}"),
            "chunk" => match &r.cell {
                Some(cell) => {
                    format!("the cell `{cell}` of {path} (get_outputs reads what it showed)")
                }
                None => r.name.clone(),
            },
            "selection" => format!(
                "this code they selected, {}:\n```\n{}\n```",
                r.name,
                r.text.as_deref().unwrap_or_default().trim_end()
            ),
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
            "refs": [{"kind": "file", "name": "theme.R", "path": "R/theme.R"}],
        }));
        assert_eq!(choices.pin.as_deref(), Some("fig3.qmd"));
        assert_eq!(choices.refs[0].path.as_deref(), Some("R/theme.R"));
        // Cleared refs and an unpinned file read as none.
        let cleared = of(&json!({"pin": null, "refs": null}));
        assert_eq!(cleared, Choices::default());
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
        let skills = serde_json::from_value::<Vec<Ref>>(json!([
            {"kind": "skill", "name": "scan", "path": "/skills/scan/SKILL.md"}
        ]))
        .unwrap();
        assert_eq!(refs_text(&skills), None);
        let mut mixed = refs.clone();
        mixed.extend(skills);
        assert_eq!(refs_text(&mixed), Some(text));
    }
}
