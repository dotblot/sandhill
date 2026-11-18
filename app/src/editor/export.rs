//! A conversation exported from the chat, as the Tauri app's
//! `app/src/model/export.ts` makes it: Markdown to read or keep with the
//! project, a standalone HTML page that also has the plots, or JSON with
//! every entry the daemon kept (what an import would read back).

use gpui_kit::*;
use sandhill_proto::{Call, EXISTS_ERROR};
use serde_json::{Value, json};

use crate::agents::chat::{self, Entry, Item, Step, StepStatus};
use crate::agents::{Agents, kind_of};
use crate::look;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Markdown,
    Html,
    Json,
}

impl Format {
    pub(crate) const ALL: [Format; 3] = [Format::Markdown, Format::Html, Format::Json];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Format::Markdown => "Markdown",
            Format::Html => "HTML",
            Format::Json => "JSON",
        }
    }

    pub(crate) fn ext(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Json => "json",
        }
    }
}

/// What the export says about the conversation.
pub(crate) struct Info {
    pub title: String,
    /// "Claude", "Codex".
    pub agent: String,
    /// The agent's kind, as the daemon names it: `claude`, `codex`.
    pub kind: String,
    pub model: String,
    pub documents: Vec<String>,
    /// When, for people: "2 Nov 2026, 10:41".
    pub at: String,
    /// When, for programs: RFC 3339.
    pub at_iso: String,
}

/// One part of the conversation: Markdown, and the plots that go with it.
struct Block {
    md: String,
    images: Vec<String>,
}

impl Block {
    fn text(md: String) -> Self {
        Block { md, images: vec![] }
    }
}

pub(crate) fn export(format: Format, entries: &[Entry], info: &Info) -> String {
    match format {
        Format::Markdown => markdown(entries, info),
        Format::Html => html(entries, info),
        Format::Json => json(entries, info),
    }
}

fn markdown(entries: &[Entry], info: &Info) -> String {
    let parts = blocks(entries, info)
        .into_iter()
        .map(|b| match b.images.len() {
            0 => b.md,
            1 => format!("{}\n\n*1 plot, in the HTML export*", b.md),
            n => format!("{}\n\n*{n} plots, in the HTML export*", b.md),
        });
    let all: Vec<String> = std::iter::once(heading(info)).chain(parts).collect();
    format!("{}\n", all.join("\n\n"))
}

fn html(entries: &[Entry], info: &Info) -> String {
    let body = std::iter::once(Block::text(heading(info)))
        .chain(blocks(entries, info))
        .map(|b| {
            let images: String = b
                .images
                .iter()
                .map(|src| format!("<img src=\"{}\" alt=\"Plot\">", escape_attr(src)))
                .collect::<Vec<_>>()
                .join("\n");
            format!("<section>{}{images}</section>", to_html(&b.md))
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>\n\
         :root {{ --body-size: {}px; --title-size: {}px; --small-size: {}px; --code-size: {}px; --code-inset: {}px; }}\n\
         {STYLE}</style>\n</head>\n<body>\n<main>\n{body}\n</main>\n\
         </body>\n</html>\n",
        escape_text(&info.title),
        look::CHROME,
        look::TITLE,
        look::SMALL,
        look::DOC_CODE,
        look::DOC_CODE_INSET,
    )
}

fn json(entries: &[Entry], info: &Info) -> String {
    let entries: Vec<Value> = entries
        .iter()
        .map(|e| json!({"seq": e.seq, "event": e.event}))
        .collect();
    let conversation = json!({
        "sandhill": "conversation",
        "version": 1,
        "title": info.title,
        "agent": info.kind,
        "model": info.model,
        "documents": info.documents,
        "exported": info.at_iso,
        "entries": entries,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&conversation).unwrap_or_default()
    )
}

/// A file name for the export, from the conversation's title.
pub(crate) fn file_name(title: &str, format: Format) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        match fold(c) {
            Some(c) if c.is_ascii_alphanumeric() => slug.push(c),
            _ if !slug.ends_with('-') => slug.push('-'),
            _ => {}
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(50).collect();
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() {
        "conversation"
    } else {
        slug
    };
    format!("{slug}.{}", format.ext())
}

/// A letter without its accent, for the common Latin ones (what NFKD and
/// dropping the marks does in the Tauri app).
fn fold(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c);
    }
    const FROM: &str = "àáâãäåāçćčèéêëēěìíîïīñńňòóôõöøōùúûüūůýÿšśžźżłđ";
    const TO: &str = "aaaaaaaccceeeeeeiiiiinnnooooooouuuuuuyysszzzld";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|i| TO.chars().nth(i))
}

fn heading(info: &Info) -> String {
    let about: Vec<&str> = [info.agent.as_str(), info.model.as_str()]
        .into_iter()
        .chain(info.documents.iter().map(String::as_str))
        .filter(|s| !s.is_empty())
        .collect();
    format!(
        "# {}\n\n{}  \nExported from Sandhill on {}",
        one_line(&info.title),
        about.join(" · "),
        info.at
    )
}

fn blocks(entries: &[Entry], info: &Info) -> Vec<Block> {
    let mut out = Vec::new();
    let mut speaker: Option<bool> = None;
    for item in chat::build(entries, false).items {
        let Some(block) = item_block(&item, &info.agent) else {
            continue;
        };
        let user = match item {
            Item::User { .. } => Some(true),
            Item::Note { .. } | Item::Account { .. } => None,
            _ => Some(false),
        };
        if let Some(user) = user {
            if speaker != Some(user) {
                let who = if user { "You" } else { info.agent.as_str() };
                out.push(Block::text(format!("## {who}")));
            }
            speaker = Some(user);
        }
        out.push(block);
    }
    out
}

fn item_block(item: &Item, agent: &str) -> Option<Block> {
    match item {
        Item::User { text, .. } | Item::Reply { text, .. } => {
            let text = text.trim();
            (!text.is_empty()).then(|| Block::text(text.to_string()))
        }
        Item::Step { step, .. } => Some(step_block(step)),
        Item::Ask { ask, .. } => {
            let title = chat::ask_view(ask, None, agent).title;
            Some(Block::text(format!(
                "**{agent} asked:** {} · {}",
                one_line(&title),
                answer_text(ask)
            )))
        }
        Item::Note { text, .. } => Some(Block::text(format!("*{text}*"))),
        Item::Account { name, .. } => {
            Some(Block::text(format!("---\n\nSwitched to {name}\n\n---")))
        }
        Item::Error { text, .. } => Some(Block::text(format!("> **Error:** {text}"))),
        Item::Footer { .. } | Item::Thinking { .. } => None,
    }
}

fn step_block(step: &Step) -> Block {
    let view = chat::step_details::step_view(&step.tool, &step.input, step.output.as_ref(), None);
    let images = step.output.as_ref().map(content_images).unwrap_or_default();
    if view.name == "thinking" {
        let text = view.output.as_deref().map(str::trim).unwrap_or_default();
        let md = if text.is_empty() {
            "*Thought*".to_string()
        } else {
            quote(text)
        };
        return Block { md, images };
    }
    let outcome = match step.status {
        StepStatus::Fail => Some("failed"),
        StepStatus::Deny => Some("you said no"),
        StepStatus::Cancel => Some("stopped"),
        _ => None,
    };
    let line: Vec<String> = [Some(view.desc.as_str()), view.detail.as_deref(), outcome]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(one_line)
        .collect();
    let mut parts = vec![if line.is_empty() {
        format!("**`{}`**", view.name)
    } else {
        format!("**`{}`** {}", view.name, line.join(" · "))
    }];
    if let Some(code) = &view.code {
        let language = match view.language.as_deref() {
            Some("shell") => "sh",
            Some(language) => language,
            None => "",
        };
        parts.push(fence(code, language));
    }
    if let Some(diff) = view.diff.as_ref().filter(|d| !d.is_empty()) {
        parts.push(fence(&diff.join("\n"), "diff"));
    }
    if let Some(output) = view.output.as_ref().filter(|o| !o.is_empty()) {
        parts.push(fence(output, ""));
    }
    if let (Some(error), StepStatus::Fail) = (&step.error, step.status) {
        parts.push(fence(error, ""));
    }
    Block {
        md: parts.join("\n\n"),
        images,
    }
}

/// The images in an MCP tool's result, as `data:` addresses.
fn content_images(value: &Value) -> Vec<String> {
    let items = match value {
        Value::Array(items) => items,
        Value::Object(map) => match map.get("content") {
            Some(Value::Array(items)) => items,
            _ => return vec![],
        },
        _ => return vec![],
    };
    items
        .iter()
        .filter(|item| item["type"] == "image")
        .filter_map(|item| {
            let data = item["data"].as_str().or(item["source"]["data"].as_str())?;
            let mime = item["mimeType"]
                .as_str()
                .or(item["source"]["media_type"].as_str())
                .unwrap_or("image/png");
            Some(format!("data:{mime};base64,{data}"))
        })
        .collect()
}

fn answer_text(ask: &chat::Ask) -> &str {
    if let Some(option) = ask.answered_option() {
        return &option.name;
    }
    match ask.answer.as_ref() {
        None => "no answer",
        Some(Value::Null) => "it stopped waiting",
        Some(answer) if answer["behavior"] == "allow" => "you allowed it",
        Some(_) => "you said no",
    }
}

/// A fenced code block, with a fence longer than any run of backticks in it.
fn fence(text: &str, language: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
        .max(2);
    let ticks = "`".repeat(longest + 1);
    format!(
        "{ticks}{language}\n{}\n{ticks}",
        text.trim_end_matches('\n')
    )
}

fn quote(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Markdown as HTML. Raw HTML in it is shown as text, never run.
fn to_html(text: &str) -> String {
    markdown::to_html_with_options(text, &markdown::Options::gfm())
        .unwrap_or_else(|_| format!("<pre>{}</pre>", escape_text(text)))
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(text: &str) -> String {
    escape_text(text).replace('"', "&quot;")
}

const STYLE: &str = r#"
:root { --bg: #fbfaf7; --fg: #24221f; --muted: #6f6a62; --line: #e3dfd7; --code: #f1eee8; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #1d1c1a; --fg: #e8e4dc; --muted: #a39d93; --line: #3a3733; --code: #282623; }
}
body { margin: 0; background: var(--bg); color: var(--fg); font: var(--body-size)/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
main { max-width: 780px; margin: 0 auto; padding: 32px 16px 64px; }
h1 { font-size: var(--title-size); margin: 0 0 4px; }
h1 + p { color: var(--muted); font-size: var(--small-size); margin-top: 0; }
h2 { font-size: var(--body-size); margin: 28px 0 0; padding-top: 12px; border-top: 1px solid var(--line); }
section > :first-child { margin-top: 12px; }
pre { background: var(--code); border-radius: 6px; padding: 10px 12px; white-space: pre-wrap; overflow-wrap: anywhere; font-size: var(--code-size); line-height: 1.45; }
code { font-family: ui-monospace, Menlo, monospace; font-size: calc(1em - var(--code-inset)); }
pre code { font-size: inherit; }
blockquote { margin: 12px 0; padding-left: 12px; border-left: 3px solid var(--line); color: var(--muted); }
img { display: block; max-width: 100%; margin: 8px 0; border-radius: 4px; background: #fff; }
table { border-collapse: collapse; } td, th { border: 1px solid var(--line); padding: 3px 8px; }
"#;

/// The project's folder, with `name` in it, or "name 2.ext" and so on.
fn numbered(project: &str, name: &str, n: usize) -> String {
    let folder = project.trim_end_matches('/');
    if n == 1 {
        return format!("{folder}/{name}");
    }
    match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => format!("{folder}/{} {n}{}", &name[..dot], &name[dot..]),
        None => format!("{folder}/{name} {n}"),
    }
}

impl Agents {
    /// Saves the conversation shown as a file in the project's folder,
    /// named after its title; it never replaces a file that is there.
    pub(crate) fn export_chat(&mut self, format: Format, cx: &mut Context<Self>) {
        self.gear = false;
        let Some(t) = self.current().cloned() else {
            return;
        };
        let now = chrono::Local::now();
        let info = Info {
            title: self.title(&t),
            agent: kind_of(&t.kind).map_or(t.title.clone(), |k| k.name.to_string()),
            kind: t.kind.clone(),
            model: crate::agents::model_label(&t).unwrap_or_default(),
            documents: t.options.documents.clone(),
            at: now.format("%-d %b %Y, %H:%M").to_string(),
            at_iso: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        };
        let text = export(format, self.entries(&t.id), &info);
        let name = file_name(&info.title, format);
        let project = self.project.clone();
        let daemon = self.daemon.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut result = Err(anyhow::anyhow!("no free name"));
            for n in 1..=50 {
                let path = numbered(&project, &name, n);
                let written = daemon
                    .call::<Value>(Call::WriteFile {
                        path: path.clone(),
                        text: text.clone(),
                        expected_mtime_ms: None,
                        base64: false,
                        create: true,
                    })
                    .await;
                match written {
                    Err(err) if format!("{err:#}").contains(EXISTS_ERROR) => continue,
                    Err(err) => result = Err(err),
                    Ok(_) => result = Ok(path),
                }
                break;
            }
            this.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        let shown = path
                            .strip_prefix(&format!("{}/", project.trim_end_matches('/')))
                            .unwrap_or(&path)
                            .to_string();
                        this.workbench
                            .update(cx, |w, cx| {
                                w.notice = Some(format!("Exported the conversation to {shown}."));
                                cx.notify();
                            })
                            .ok();
                    }
                    Err(err) => {
                        this.problem = Some(format!("Could not export the conversation: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Format, Info, export, fence, file_name};
    use crate::agents::chat::Entry;

    fn info() -> Info {
        Info {
            title: "Plot the <QC> metrics".into(),
            agent: "Claude".into(),
            kind: "claude".into(),
            model: "Opus".into(),
            documents: vec!["qc.qmd".into()],
            at: "2 Nov 2026, 10:41".into(),
            at_iso: "2026-11-02T10:41:00Z".into(),
        }
    }

    #[test]
    fn account_switches_remain_in_exported_history() {
        let entries = vec![Entry {
            seq: 1,
            event: json!({"kind":"account","account":"claude:work","name":"Work"}),
        }];
        let markdown = export(Format::Markdown, &entries, &info());
        assert!(markdown.contains("---\n\nSwitched to Work\n\n---"));
    }

    #[test]
    fn exported_acp_approval_uses_the_selected_option_label() {
        let entries = vec![
            Entry {
                seq: 1,
                event: json!({"kind": "prompt", "text": "Inspect data", "turn": "t"}),
            },
            Entry {
                seq: 2,
                event: json!({"kind": "ask", "id": "permission", "turn": "t",
                    "tool": "inspect", "input": {"title": "Inspect data"},
                    "options": [{"optionId": "session", "name": "Allow in this session",
                        "kind": "allow_always"}]}),
            },
            Entry {
                seq: 3,
                event: json!({"kind": "answered", "id": "permission",
                    "answer": {"option_id": "session"}}),
            },
        ];
        let mut info = info();
        info.agent = "OpenScience".into();
        info.kind = "openscience".into();
        let md = export(Format::Markdown, &entries, &info);
        assert!(md.contains("**OpenScience asked:** Inspect data · Allow in this session"));
        assert!(!md.contains("you said no"));
    }

    #[test]
    fn names_come_from_the_title() {
        assert_eq!(
            file_name("Plot the QC metrics!", Format::Markdown),
            "plot-the-qc-metrics.md"
        );
        assert_eq!(file_name("Café résumé", Format::Html), "cafe-resume.html");
        assert_eq!(file_name("???", Format::Json), "conversation.json");
        assert_eq!(file_name(&"ab ".repeat(40), Format::Json).len(), 50 + 5);
    }

    #[test]
    fn fences_outlast_the_code() {
        assert_eq!(fence("a\n", ""), "```\na\n```");
        assert_eq!(fence("x ```` y", "r"), "`````r\nx ```` y\n`````");
    }

    #[test]
    fn exports_say_who_said_what() {
        let entries = vec![
            Entry {
                seq: 1,
                event: json!({"kind": "prompt", "text": "Plot it", "turn": "t1"}),
            },
            Entry {
                seq: 2,
                event: json!({"kind": "text", "message": "m", "index": 0, "text": "Done <b>now</b>.", "turn": "t1"}),
            },
            Entry {
                seq: 3,
                event: json!({"kind": "stop", "turn": "t1"}),
            },
        ];
        let md = export(Format::Markdown, &entries, &info());
        assert!(md.starts_with("# Plot the <QC> metrics\n\nClaude · Opus · qc.qmd  \n"));
        assert!(
            md.contains("## You\n\nPlot it\n\n## Claude\n\nDone"),
            "{md}"
        );
        let page = export(Format::Html, &entries, &info());
        assert!(page.contains("<title>Plot the &lt;QC&gt; metrics</title>"));
        assert!(!page.contains("<b>now</b>"), "raw HTML stays text");
        let data: serde_json::Value =
            serde_json::from_str(&export(Format::Json, &entries, &info())).unwrap();
        assert_eq!(data["agent"], "claude");
        assert_eq!(data["entries"][1]["seq"], 2);
    }
}
