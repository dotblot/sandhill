//! The Help pane, as the Tauri app has it (`HelpPane.tsx`,
//! `model/help.ts`): search the active session's help topics (R's installed
//! packages, or Python's imported modules) and read a page, its links
//! followed here, with Back and Forward and the function's source a click
//! away. Help runs in the session, quietly, so it fails while the session
//! runs other code. Help asked for at the Console (`?lm`, `np.mean?`) opens
//! here instead of running. R's vignettes a search finds open in the
//! Viewer, as their package built them (HTML or PDF), with their R code a
//! click away.

use std::time::Duration;

use gpui_kit::component::input::{EditorState, Input, InputEvent, InputState};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, FileText, HelpPage, HelpSource, HelpTopics, HelpVignette};

use crate::look::{self, Palette, RADIUS, icon, icon_button};
use crate::workbench::{TopTab, Workbench};

/// How long typing pauses before the help is searched.
const WAIT: Duration = Duration::from_millis(150);
/// Topics a search reads, and how many show before "more".
const LIMIT: u32 = 50;
const FEW: usize = 6;
/// What the daemon says when the session runs other code.
const BUSY: &str = "the session is busy running code";

/// A page to show: a topic, in a package (R) or module (Python).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HelpRequest {
    /// The function or topic; empty to list a package's topics.
    pub topic: String,
    pub package: Option<String>,
    /// Search for it rather than open its page (R's `??`).
    pub search: bool,
}

pub(crate) struct Help {
    pub query: Entity<InputState>,
    pub package: Entity<InputState>,
    found: Option<HelpTopics>,
    search_error: Option<String>,
    all: bool,
    /// Folded to one row once a topic is opened from them, so the page
    /// starts near the top; a new search unfolds them.
    results_folded: bool,
    page: Option<HelpPage>,
    page_error: Option<String>,
    loading: bool,
    source: Option<Result<HelpSource, String>>,
    show_source: bool,
    /// Pages visited, for Back and Forward, and the one shown.
    trail: Vec<HelpRequest>,
    at: usize,
    generation: u64,
}

/// Makes the Help pane's state, with its search boxes.
pub(crate) fn new_help(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Help, Vec<Subscription>) {
    let query = cx.new(|cx| InputState::new(window, cx).placeholder("Function or topic"));
    let package = cx.new(|cx| InputState::new(window, cx).placeholder("All packages"));
    let subscriptions = vec![
        cx.subscribe(&query, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.search_help_soon(cx),
            InputEvent::PressEnter { .. } => this.open_typed_help(cx),
            _ => {}
        }),
        cx.subscribe(&package, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.search_help_soon(cx);
            }
        }),
    ];
    let help = Help {
        query,
        package,
        found: None,
        search_error: None,
        all: false,
        results_folded: false,
        page: None,
        page_error: None,
        loading: false,
        source: None,
        show_source: false,
        trail: Vec::new(),
        at: 0,
        generation: 0,
    };
    (help, subscriptions)
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '.' || c == '_'
}

fn unquote(name: &str) -> &str {
    let b = name.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'"' | b'\'' | b'`') && b[b.len() - 1] == b[0] {
        &name[1..name.len() - 1]
    } else {
        name
    }
}

fn is_r_name(name: &str) -> bool {
    let name = unquote(name);
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '.')
        && name.chars().all(is_name_char)
        || (name != unquote(name))
}

fn is_py_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(is_name_char)
}

/// The name under the cursor, for F1: `stats::lm` is lm in stats (R);
/// Python names may be dotted, `np.mean`.
pub(crate) fn name_at(line: &str, column: usize, language: &str) -> Option<HelpRequest> {
    let python = language.to_ascii_lowercase().starts_with("python");
    let chars: Vec<char> = line.chars().collect();
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || (!python && c == ':');
    let column = column.min(chars.len());
    let mut from = column;
    let mut to = column;
    while from > 0 && part(chars[from - 1]) {
        from -= 1;
    }
    while to < chars.len() && part(chars[to]) {
        to += 1;
    }
    let name: String = chars[from..to].iter().collect();
    let name = name.trim_matches(|c| c == '.' || c == ':');
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let ask = |topic: &str, package: Option<&str>| HelpRequest {
        topic: topic.to_string(),
        package: package.map(str::to_string),
        search: false,
    };
    if !python
        && let Some((package, topic)) = name.split_once(":::").or_else(|| name.split_once("::"))
        && package.starts_with(|c: char| c.is_ascii_alphabetic())
        && !topic.is_empty()
        && !topic.contains(':')
    {
        return Some(ask(topic, Some(package)));
    }
    (!name.contains(':')).then(|| ask(name, None))
}

/// Help asked for at the Console, which opens in the Help pane rather than
/// running: R's `?lm`, `?stats::lm`, `??pca`, `help(lm)`,
/// `help(package = "stats")`; Python's `?np.mean`, `np.mean?`, `help(np.mean)`.
pub(crate) fn console_help(code: &str, language: &str) -> Option<HelpRequest> {
    let text = code.trim();
    if text.is_empty() || text.contains('\n') {
        return None;
    }
    let ask = |topic: &str, package: Option<&str>, search| HelpRequest {
        topic: unquote(topic).to_string(),
        package: package.map(|p| unquote(p).to_string()),
        search,
    };
    if language.to_ascii_lowercase().starts_with("python") {
        let name = if let Some(rest) = text.strip_prefix('?') {
            rest.trim_start_matches('?').trim()
        } else if let Some(rest) = text.strip_suffix('?') {
            rest.trim_end_matches('?').trim()
        } else {
            let inner = text
                .strip_prefix("help(")
                .and_then(|t| t.strip_suffix(')'))?;
            unquote(inner.trim())
        };
        return is_py_name(name).then(|| ask(name, None, false));
    }
    if let Some(rest) = text.strip_prefix("??") {
        let topic = rest.trim();
        return (!topic.is_empty()).then(|| ask(topic, None, true));
    }
    if let Some(rest) = text.strip_prefix('?') {
        let rest = rest.trim();
        let (package, topic) = match rest.split_once("::") {
            Some((p, t)) => (Some(p), t.trim_start_matches(':')),
            None => (None, rest),
        };
        return (is_r_name(topic) && package.is_none_or(is_r_name))
            .then(|| ask(topic, package, false));
    }
    let inner = text.strip_prefix("help(")?.strip_suffix(')')?;
    let mut topic = None;
    let mut package = None;
    for (i, arg) in inner.split(',').map(str::trim).enumerate() {
        let (field, value) = match arg.split_once('=') {
            Some((name, value)) => (name.trim(), value.trim()),
            None if i == 0 => ("topic", arg),
            None if i == 1 => ("package", arg),
            None => return None,
        };
        if !is_r_name(value) {
            return None;
        }
        match field {
            "topic" => topic = Some(value),
            "package" => package = Some(value),
            _ => return None,
        }
    }
    if topic.is_none() && package.is_none() {
        return None;
    }
    Some(ask(topic.unwrap_or(""), package, false))
}

/// Where a link in an R help page goes: another topic (`../../stats/help/lm`,
/// `../help/lm`, `../../stats/html/lm.html`), the package's index, or the web.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HelpLink {
    Web(String),
    Topic {
        topic: String,
        package: Option<String>,
    },
    Index(Option<String>),
    Example {
        topic: String,
        package: Option<String>,
    },
}

fn decode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn help_link(href: &str, from: Option<&str>) -> Option<HelpLink> {
    let lower = href.to_ascii_lowercase();
    if ["http:", "https:", "mailto:", "ftp:"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return Some(HelpLink::Web(href.to_string()));
    }
    let path = href.split(['?', '#']).next().unwrap_or("");
    let from = from.map(str::to_string);
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["..", "..", package, "html", "00Index.html"] => {
            Some(HelpLink::Index(Some(package.to_string())))
        }
        [.., "00Index.html"] if parts.iter().rev().skip(1).all(|p| *p == "..") => {
            Some(HelpLink::Index(from))
        }
        ["..", "..", package, kind @ ("help" | "html"), rest @ ..] if !rest.is_empty() => {
            let topic = decode(&rest.join("/"));
            let topic = if *kind == "html" {
                topic
                    .strip_suffix(".html")
                    .map(str::to_string)
                    .unwrap_or(topic)
            } else {
                topic
            };
            Some(HelpLink::Topic {
                topic,
                package: Some(package.to_string()),
            })
        }
        ["..", "help", rest @ ..] if !rest.is_empty() => Some(HelpLink::Topic {
            topic: decode(&rest.join("/")),
            package: from,
        }),
        ["..", "Example", rest @ ..] if !rest.is_empty() => Some(HelpLink::Example {
            topic: decode(&rest.join("/")),
            package: from,
        }),
        [name] if name.ends_with(".html") => Some(HelpLink::Topic {
            topic: decode(name.trim_end_matches(".html")),
            package: from,
        }),
        _ => None,
    }
}

/// The R code that runs a topic's examples.
fn example_code(topic: &str, package: Option<&str>) -> String {
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_default();
    match package {
        Some(p) => format!("example({}, package = {})", quote(topic), quote(p)),
        None => format!("example({})", quote(topic)),
    }
}

/// A call's signature, one argument to a line when it is long.
pub(crate) fn signature_lines(name: &str, signature: &str, width: usize) -> String {
    let short = name.rsplit('.').next().unwrap_or(name);
    let flat = format!("{short}{signature}");
    if flat.chars().count() <= width {
        return flat;
    }
    let close = signature.rfind(')').unwrap_or(signature.len());
    let inner = signature.get(1..close).unwrap_or("");
    let tail = signature.get(close + 1..).unwrap_or("");
    let mut args = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0usize);
    let mut chars = inner.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match quote {
            Some(q) if c == '\\' => {
                chars.next();
                let _ = q;
            }
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => {
                    args.push(inner[start..i].trim().to_string());
                    start = i + 1;
                }
                _ => {}
            },
        }
    }
    if !inner[start..].trim().is_empty() {
        args.push(inner[start..].trim().to_string());
    }
    let body: Vec<String> = args.iter().map(|a| format!("    {a},")).collect();
    format!("{short}(\n{}\n){tail}", body.join("\n"))
}

impl Workbench {
    /// The kernel language of the active session, in lower case.
    pub(crate) fn active_language(&self) -> Option<String> {
        self.active_kernel()
            .and_then(|k| self.kernels.iter().find(|s| s.name == k))
            .map(|s| s.language.to_ascii_lowercase())
    }

    fn python(&self) -> bool {
        self.active_language()
            .is_some_and(|l| l.starts_with("python"))
    }

    fn search_help_soon(&mut self, cx: &mut Context<Self>) {
        // Typing is a new search: its results show in full.
        self.help.results_folded = false;
        self.help.generation += 1;
        let generation = self.help.generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WAIT).await;
            this.update(cx, |this, cx| {
                if this.help.generation == generation {
                    this.search_help(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Searches the session's help for what is typed.
    pub(crate) fn search_help(&mut self, cx: &mut Context<Self>) {
        let query = self.help.query.read(cx).value().trim().to_string();
        let package = self.help.package.read(cx).value().trim().to_string();
        self.help.all = false;
        if query.is_empty() && package.is_empty() {
            self.help.found = None;
            self.help.search_error = None;
            cx.notify();
            return;
        }
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        let found = daemon.call::<HelpTopics>(Call::HelpTopics {
            session,
            query: query.clone(),
            package: (!package.is_empty()).then_some(package.clone()),
            limit: Some(LIMIT),
        });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                // A later search answers for itself.
                if this.help.query.read(cx).value().trim() != query
                    || this.help.package.read(cx).value().trim() != package
                {
                    return;
                }
                match found {
                    Ok(found) => {
                        this.help.found = Some(found);
                        this.help.search_error = None;
                    }
                    Err(err) => {
                        let err = format!("{err:#}");
                        this.help.search_error = Some(if err.contains(BUSY) {
                            "The session is running code; search again when it finishes.".into()
                        } else {
                            format!("Could not search the help: {err}")
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Enter in the search box: help typed as at the Console, else the
    /// first topic found, else the name typed.
    fn open_typed_help(&mut self, cx: &mut Context<Self>) {
        self.help.results_folded = true;
        let typed = self.help.query.read(cx).value().trim().to_string();
        let language = self.active_language().unwrap_or_default();
        if let Some(asked) =
            console_help(&typed, &language).filter(|a| !a.search && !a.topic.is_empty())
        {
            self.open_help(asked, true, cx);
        } else if let Some(first) = self.help.found.as_ref().and_then(|f| f.topics.first()) {
            let asked = HelpRequest {
                topic: first.topic.clone(),
                package: Some(first.package.clone()),
                search: false,
            };
            self.open_help(asked, true, cx);
        } else if !typed.is_empty() {
            let package = self.help.package.read(cx).value().trim().to_string();
            let asked = HelpRequest {
                topic: typed,
                package: (!package.is_empty()).then_some(package),
                search: false,
            };
            self.open_help(asked, true, cx);
        }
    }

    /// F1 (fn+F1 on a Mac keyboard): help for the name under the cursor (or
    /// the selected name) in a script, a cell or the Console, read from a
    /// live session of the code's language: the file's own (the Console
    /// moves to it, as running the file does), else the Console's.
    pub(crate) fn help_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = |editor: &Entity<EditorState>, cx: &App| {
            editor.read(cx).focus_handle(cx).is_focused(window)
        };
        let console = focused(&self.console_input, cx);
        let editor = if console {
            Some(self.console_input.clone())
        } else {
            self.open
                .as_ref()
                .and_then(|open| open.editors.iter().find(|e| focused(e, cx)).cloned())
        };
        let Some(editor) = editor else {
            return;
        };
        let (language, selected, text, at) = {
            let e = editor.read(cx);
            (
                e.language_name().to_ascii_lowercase(),
                e.selected_value().to_string(),
                e.value().to_string(),
                e.cursor_position(),
            )
        };
        if !matches!(language.as_str(), "r" | "python") {
            return;
        }
        let asked = if !selected.trim().is_empty() {
            name_at(selected.trim(), 0, &language)
        } else {
            let line = text.split('\n').nth(at.line as usize).unwrap_or("");
            name_at(line, at.character as usize, &language)
        };
        let Some(asked) = asked else {
            return;
        };
        if !console
            && let Some(path) = self
                .open
                .as_ref()
                .filter(|open| open.session.id().is_some())
                .map(|open| open.notebook.path.clone())
            && self.console_bound_file() != Some(path.as_str())
        {
            self.bind_console_document(&path, cx);
        }
        let live = self
            .variables_session()
            .and_then(|session| self.session_language(session))
            .is_some_and(|l| l.to_ascii_lowercase().starts_with(&language));
        if !live {
            self.top_tab = TopTab::Help;
            self.show_files = true;
            self.help.page = None;
            self.help.page_error = Some(format!(
                "Help for {} comes from a running {} session. Run some code first, then press F1 again.",
                asked.topic,
                if language == "r" { "R" } else { "Python" }
            ));
            cx.notify();
            return;
        }
        self.ask_help(asked, window, cx);
    }

    /// Help asked for elsewhere (the Console, Packages): the Help tab comes
    /// to the front with it.
    pub(crate) fn ask_help(
        &mut self,
        asked: HelpRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.top_tab = TopTab::Help;
        self.show_files = true;
        if asked.search || asked.topic.is_empty() {
            let package = asked.package.clone().unwrap_or_default();
            let topic = asked.topic.clone();
            self.help
                .query
                .update(cx, |i, cx| i.set_value(topic, window, cx));
            self.help
                .package
                .update(cx, |i, cx| i.set_value(package, window, cx));
            self.search_help(cx);
        } else {
            // The page shows on its own, not under an old search.
            self.help
                .query
                .update(cx, |i, cx| i.set_value("", window, cx));
            self.help
                .package
                .update(cx, |i, cx| i.set_value("", window, cx));
            self.help.found = None;
            self.open_help(asked, true, cx);
        }
        cx.notify();
    }

    /// Reads a page; `push` adds it to the trail (not for Back and Forward).
    fn open_help(&mut self, asked: HelpRequest, push: bool, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        if push {
            let keep = if self.help.trail.is_empty() {
                0
            } else {
                self.help.at + 1
            };
            self.help.trail.truncate(keep);
            if self.help.trail.last() != Some(&asked) {
                self.help.trail.push(asked.clone());
            }
            self.help.at = self.help.trail.len() - 1;
        }
        self.help.loading = true;
        self.help.show_source = false;
        self.help.source = None;
        let page = daemon.call::<HelpPage>(Call::HelpPage {
            session,
            topic: asked.topic.clone(),
            package: asked.package.clone(),
        });
        cx.spawn(async move |this, cx| {
            let page = page.await;
            this.update(cx, |this, cx| {
                this.help.loading = false;
                match page {
                    Ok(page) => {
                        this.help.page = Some(page);
                        this.help.page_error = None;
                    }
                    Err(err) => {
                        let err = format!("{err:#}");
                        this.help.page_error = Some(if err.contains(BUSY) {
                            "The session is running code; open the page again when it finishes."
                                .into()
                        } else {
                            format!("No help for {}: {err}", asked.topic)
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn help_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let to = self.help.at as isize + by;
        if to < 0 || to as usize >= self.help.trail.len() {
            return;
        }
        self.help.at = to as usize;
        let asked = self.help.trail[self.help.at].clone();
        self.open_help(asked, false, cx);
    }

    fn toggle_source(&mut self, cx: &mut Context<Self>) {
        self.help.show_source = !self.help.show_source;
        if self.help.show_source
            && self.help.source.is_none()
            && let (Some(daemon), Some(session), Some(page)) = (
                self.daemon(),
                self.variables_session().map(str::to_string),
                self.help.page.clone(),
            )
        {
            let read = daemon.call::<HelpSource>(Call::HelpSource {
                session,
                topic: page.topic.clone(),
                package: Some(page.package.clone()).filter(|p| !p.is_empty()),
            });
            cx.spawn(async move |this, cx| {
                let read = read.await.map_err(|err| format!("{err:#}"));
                this.update(cx, |this, cx| {
                    if this.help.page.as_ref().map(|p| &p.topic) == Some(&page.topic) {
                        this.help.source = Some(read);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// A link clicked in an R help page.
    fn follow_help_link(&mut self, href: &str, cx: &mut Context<Self>) {
        let from = self.help.page.as_ref().map(|p| p.package.clone());
        match help_link(href, from.as_deref()) {
            Some(HelpLink::Web(url)) => cx.open_url(&url),
            Some(HelpLink::Topic { topic, package }) => self.open_help(
                HelpRequest {
                    topic,
                    package,
                    search: false,
                },
                true,
                cx,
            ),
            Some(HelpLink::Index(package)) => self.open_help(
                HelpRequest {
                    topic: String::new(),
                    package,
                    search: false,
                },
                true,
                cx,
            ),
            Some(HelpLink::Example { topic, package }) => {
                self.run_in_console(example_code(&topic, package.as_deref()), cx)
            }
            None => {}
        }
    }

    /// Runs code in the active session's Console, where its output shows.
    pub(crate) fn run_in_console(&mut self, code: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        self.console_tab = crate::workbench::ConsoleTab::Console;
        self.show_console = true;
        let started = daemon.call_in_order::<sandhill_proto::RunStarted>(Call::Execute {
            session,
            code,
            cell: None,
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = started.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Running the code failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// The Help tab's body.
    pub(crate) fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let python = self.python();
        let search = h_flex()
            .flex_none()
            .h(px(37.))
            .gap(px(6.))
            .px(px(8.))
            .py(px(6.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(
                div().flex_1().child(
                    Input::new(&self.help.query)
                        .small()
                        .prefix(icon("search").size(px(13.))),
                ),
            )
            .child(
                div().w(px(130.)).child(
                    Input::new(&self.help.package)
                        .small()
                        .prefix(icon("package").size(px(13.))),
                ),
            )
            .map(|row| look::floating_header(&p, look::STRIP, false, row));
        if self.variables_session().is_none() {
            return v_flex()
                .relative()
                .size_full()
                .child(look::PaneScroll::new(
                    "help-scroll",
                    look::STRIP + 37.,
                    8.,
                    div()
                        .id("help-body")
                        .px(px(14.))
                        .pb(px(8.))
                        .text_color(p.muted)
                        .child("Start R or Python in the Console, and its help shows here."),
                ))
                .child(search)
                .into_any_element();
        }
        let typed = !self.help.query.read(cx).value().trim().is_empty()
            || !self.help.package.read(cx).value().trim().is_empty();
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(err) = &self.help.search_error {
            body.push(note(err.clone(), p.err, &p));
        }
        if typed && let Some(found) = &self.help.found {
            if self.help.results_folded && self.help.page.is_some() {
                body.push(self.render_folded_topics(found, cx));
            } else {
                body.push(self.render_topics(found, python, cx));
            }
        } else if self.help.page.is_none() && self.help.page_error.is_none() {
            body.push(note(
                if python {
                    "Type a function name. Leave the module empty to search every imported module."
                } else {
                    "Type a function name. Leave the package empty to search every installed package."
                }
                .into(),
                p.muted,
                &p,
            ));
        }
        if let Some(err) = &self.help.page_error {
            body.push(note(err.clone(), p.err, &p));
        }
        if let Some(page) = &self.help.page {
            body.push(self.render_page(page, cx));
        }
        v_flex()
            .relative()
            .size_full()
            .child(look::PaneScroll::new(
                "help-scroll",
                look::STRIP + 37.,
                0.,
                div().id("help-body").child(v_flex().children(body)),
            ))
            .child(search)
            .into_any_element()
    }

    fn render_topics(
        &self,
        found: &HelpTopics,
        python: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let total = found.topics.len() as u64 + found.more;
        let shown = if self.help.all {
            found.topics.len()
        } else {
            FEW.min(found.topics.len())
        };
        let hidden = found.topics.len() - shown;
        let open_page = self
            .help
            .page
            .as_ref()
            .map(|pg| (pg.topic.clone(), pg.package.clone()));
        let hover = p.ink(7.);
        let rows = found.topics.iter().take(shown).enumerate().map(|(ix, t)| {
            let on = open_page.as_ref() == Some(&(t.topic.clone(), t.package.clone()));
            let asked = HelpRequest {
                topic: t.topic.clone(),
                package: Some(t.package.clone()),
                search: false,
            };
            h_flex()
                .id(("help-topic", ix))
                .gap(px(8.))
                .px(px(10.))
                .py(px(3.))
                .mx(px(4.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(p.accent_soft))
                .hover(move |d| d.bg(hover))
                .child(
                    div()
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(4.))
                        .text_size(px(look::SMALL - 2.))
                        .bg(p.ink(8.))
                        .text_color(p.muted)
                        .child(if python { "Py" } else { "R" }),
                )
                .child(div().font_family(look::MONO).child(t.topic.clone()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(p.faint)
                        .text_size(px(look::SMALL))
                        .child(t.package.clone()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.help.results_folded = true;
                    this.open_help(asked.clone(), true, cx)
                }))
        });
        v_flex()
            .pb(px(6.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(note(
                format!("{total} topic{}", if total == 1 { "" } else { "s" }),
                p.faint,
                &p,
            ))
            .children(rows)
            .when(hidden > 0, |d| {
                d.child(
                    div()
                        .id("help-more")
                        .px(px(14.))
                        .py(px(2.))
                        .text_size(px(look::SMALL))
                        .text_color(p.accent)
                        .cursor_pointer()
                        .child(format!("{} more", hidden as u64 + found.more))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.help.all = true;
                            cx.notify();
                        })),
                )
            })
            .when(found.topics.is_empty() && found.vignettes.is_empty(), |d| {
                d.child(note("No help topics match.".into(), p.muted, &p))
            })
            .when(!found.vignettes.is_empty(), |d| {
                d.child(note("Vignettes".into(), p.faint, &p))
                    .children(self.render_vignettes(&found.vignettes, cx))
            })
            .into_any_element()
    }

    /// The search results as one row above the page they led to.
    fn render_folded_topics(&self, found: &HelpTopics, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let total = found.topics.len() as u64 + found.more + found.vignettes.len() as u64;
        h_flex()
            .id("help-results-folded")
            .gap(px(6.))
            .px(px(14.))
            .py(px(4.))
            .text_size(px(look::SMALL))
            .text_color(p.accent)
            .cursor_pointer()
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(icon("chevron.right").size(px(12.)))
            .child(format!(
                "Show {total} search result{}",
                if total == 1 { "" } else { "s" }
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.help.results_folded = false;
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_vignettes(
        &self,
        vignettes: &[HelpVignette],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let hover = p.ink(7.);
        vignettes
            .iter()
            .enumerate()
            .map(|(ix, v)| {
                let open = v.clone();
                let code = v.clone();
                let title = if v.title.is_empty() {
                    &v.name
                } else {
                    &v.title
                };
                h_flex()
                    .id(("help-vignette", ix))
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(3.))
                    .mx(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .tooltip({
                        let tip = format!("{title} · vignette(\"{}\")", v.name);
                        move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        }
                    })
                    .child(div().flex_none().child(icon("help").size(px(13.))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(p.faint)
                            .text_size(px(look::SMALL))
                            .child(v.package.clone()),
                    )
                    .when(v.code.is_some(), |d| {
                        d.child(
                            div()
                                .id(("help-vignette-code", ix))
                                .flex_none()
                                .px(px(4.))
                                .rounded(px(4.))
                                .text_size(px(look::SMALL))
                                .text_color(p.accent)
                                .hover(|d| d.underline())
                                .child("R code")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_vignette_code(&code, cx);
                                })),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let title = if open.title.is_empty() {
                            &open.name
                        } else {
                            &open.title
                        };
                        this.open_html(open.file.clone(), format!("{title} · vignette"), cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }

    /// Shows a vignette's R code in the Viewer, as text.
    fn open_vignette_code(&mut self, vignette: &HelpVignette, cx: &mut Context<Self>) {
        let (Some(daemon), Some(path)) = (self.daemon(), vignette.code.clone()) else {
            return;
        };
        let title = format!("{} · R code", vignette.name);
        let key = format!("code:{path}");
        cx.spawn(async move |this, cx| {
            let read = daemon.call::<FileText>(Call::ReadFile { path }).await;
            this.update(cx, |this, cx| match read {
                Ok(file) => this.show_text(key, title, &file.text, cx),
                Err(err) => {
                    this.problem = Some(format!("Could not read the vignette's code: {err:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_page(&self, page: &HelpPage, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let back = self.help.at > 0;
        let on = self.help.at + 1 < self.help.trail.len();
        let what = [
            page.package.as_str(),
            page.version.as_str(),
            page.kind.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let head = h_flex()
            .gap(px(4.))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(look::CHROME + 2.))
                            .child(if page.topic.is_empty() {
                                page.package.clone()
                            } else {
                                page.topic.clone()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.faint)
                            .child(what),
                    ),
            )
            .child(
                icon_button("help-back", "previous", &p, false)
                    .when(!back, |d| d.opacity(0.35))
                    .on_click(cx.listener(|this, _, _, cx| this.help_step(-1, cx))),
            )
            .child(
                icon_button("help-forward", "next", &p, false)
                    .when(!on, |d| d.opacity(0.35))
                    .on_click(cx.listener(|this, _, _, cx| this.help_step(1, cx))),
            )
            .child(
                div()
                    .id("help-source")
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .text_size(px(look::SMALL))
                    .cursor_pointer()
                    .when(self.help.show_source, |d| d.bg(p.accent_soft))
                    .child(if self.help.show_source {
                        "Help"
                    } else {
                        "Source"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_source(cx))),
            );
        let also = (!page.alternatives.is_empty()).then(|| {
            h_flex()
                .flex_wrap()
                .gap(px(4.))
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .child("Also in")
                .children(page.alternatives.iter().enumerate().map(|(ix, alt)| {
                    let asked = HelpRequest {
                        topic: alt.topic.clone(),
                        package: Some(alt.package.clone()),
                        search: false,
                    };
                    div()
                        .id(("help-alt", ix))
                        .text_color(p.accent)
                        .cursor_pointer()
                        .child(alt.package.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_help(asked.clone(), true, cx)
                        }))
                }))
        });
        let content: AnyElement = if self.help.show_source {
            match &self.help.source {
                None => note("Reading the source…".into(), p.faint, &p),
                Some(Err(err)) => note(format!("No source: {err}"), p.err, &p),
                Some(Ok(source)) => code_view(
                    format!("help-source-{}-{}", page.package, page.topic),
                    if page.html.is_some() {
                        crate::doc_text::r_code_markdown(&source.text)
                    } else {
                        crate::doc_text::python_code_markdown(&source.text)
                    },
                ),
            }
        } else if let Some(html) = &page.html {
            // R help: its HTML, with the code in its `<pre>` blocks shown as
            // R code.
            let key = format!("help-{}-{}", page.package, page.topic);
            v_flex()
                .gap(px(6.))
                .children(
                    crate::doc_text::split_pre(html)
                        .into_iter()
                        .enumerate()
                        .map(|(ix, part)| match part {
                            crate::doc_text::HelpPart::Html(html) => {
                                let weak = cx.entity().downgrade();
                                TextView::html(SharedString::from(format!("{key}-{ix}")), html)
                                    .style(help_style())
                                    .selectable(true)
                                    .on_link_click(move |url, _, _, cx| {
                                        let url = url.to_string();
                                        weak.update(cx, |this, cx| this.follow_help_link(&url, cx))
                                            .ok();
                                    })
                                    .into_any_element()
                            }
                            crate::doc_text::HelpPart::Code(code) => code_view(
                                format!("{key}-{ix}"),
                                crate::doc_text::r_code_markdown(&code),
                            ),
                        }),
                )
                .into_any_element()
        } else {
            self.render_python_doc(page, cx)
        };
        v_flex()
            .gap(px(8.))
            .px(px(12.))
            .py(px(10.))
            .when(self.help.loading, |d| d.opacity(0.6))
            .child(head)
            .children(also)
            .child(div().text_size(px(look::CHROME)).child(content))
            .into_any_element()
    }

    fn render_python_doc(&self, page: &HelpPage, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let hover = p.ink(7.);
        let topic = page.topic.clone();
        v_flex()
            .gap(px(8.))
            .when_some(page.signature.clone(), |d, sig| {
                d.child(code_view(
                    format!("help-sig-{}", page.topic),
                    crate::doc_text::python_code_markdown(&signature_lines(&page.topic, &sig, 60)),
                ))
            })
            .when_some(
                page.text.clone().filter(|t| !t.trim().is_empty()),
                |d, text| {
                    d.child(
                        TextView::markdown(
                            SharedString::from(format!("help-doc-{}", page.topic)),
                            crate::doc_text::docstring_markdown(&text),
                        )
                        .style(help_style())
                        .selectable(true),
                    )
                },
            )
            .when(!page.members.is_empty(), |d| {
                d.child(div().font_weight(FontWeight::SEMIBOLD).child("Members"))
                    .children(page.members.iter().enumerate().map(|(ix, m)| {
                        let asked = HelpRequest {
                            topic: format!("{topic}.{}", m.name),
                            package: None,
                            search: false,
                        };
                        h_flex()
                            .id(("help-member", ix))
                            .gap(px(8.))
                            .px(px(6.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .child(
                                div()
                                    .font_family(look::MONO)
                                    .text_color(p.accent)
                                    .child(m.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(m.kind.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.muted)
                                    .child(m.title.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_help(asked.clone(), true, cx)
                            }))
                    }))
            })
            .into_any_element()
    }
}

fn note(text: String, color: Hsla, _p: &Palette) -> AnyElement {
    div()
        .px(px(14.))
        .py(px(6.))
        .text_size(px(look::SMALL + 1.))
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// Help's prose and code: the code font is wider than the interface font,
/// so code a pixel smaller looks the same size as the prose beside it.
fn help_style() -> TextViewStyle {
    TextViewStyle::default()
        .paragraph_gap(rems(0.5))
        .code_block(StyleRefinement::default().text_size(px(look::CHROME - 1.)))
}

/// Code as the AI pane shows it: the code font, its language's colours.
fn code_view(id: String, markdown: String) -> AnyElement {
    TextView::markdown(SharedString::from(id), markdown)
        .style(help_style())
        .selectable(true)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{HelpLink, HelpRequest, console_help, help_link, signature_lines};

    fn ask(topic: &str, package: Option<&str>, search: bool) -> Option<HelpRequest> {
        Some(HelpRequest {
            topic: topic.into(),
            package: package.map(str::to_string),
            search,
        })
    }

    #[test]
    fn finds_the_name_under_the_cursor() {
        let r = super::name_at("fit <- stats::lm(y ~ x)", 14, "r").unwrap();
        assert_eq!(
            (r.topic.as_str(), r.package.as_deref()),
            ("lm", Some("stats"))
        );
        let py = super::name_at("np.mean(x)", 4, "python").unwrap();
        assert_eq!(py.topic, "np.mean");
        assert!(super::name_at("x <- 42", 6, "r").is_none());
    }

    #[test]
    fn finds_help_asked_at_the_console() {
        assert_eq!(console_help("?lm", "R"), ask("lm", None, false));
        assert_eq!(
            console_help("?stats::lm", "R"),
            ask("lm", Some("stats"), false)
        );
        assert_eq!(console_help("??pca", "R"), ask("pca", None, true));
        assert_eq!(console_help("help(lm)", "R"), ask("lm", None, false));
        assert_eq!(
            console_help("help(package = \"stats\")", "R"),
            ask("", Some("stats"), false)
        );
        assert_eq!(
            console_help("help(\"lm\", \"stats\")", "R"),
            ask("lm", Some("stats"), false)
        );
        assert_eq!(console_help("lm(y ~ x)", "R"), None);
        assert_eq!(
            console_help("np.mean?", "python"),
            ask("np.mean", None, false)
        );
        assert_eq!(
            console_help("?sc.pp.neighbors", "python"),
            ask("sc.pp.neighbors", None, false)
        );
        assert_eq!(
            console_help("help(np.mean)", "python"),
            ask("np.mean", None, false)
        );
        assert_eq!(console_help("x = 1", "python"), None);
    }

    #[test]
    fn follows_links_in_r_help_pages() {
        assert_eq!(
            help_link("../../stats/help/lm", Some("base")),
            Some(HelpLink::Topic {
                topic: "lm".into(),
                package: Some("stats".into())
            })
        );
        assert_eq!(
            help_link("../../stats/html/lm.html", None),
            Some(HelpLink::Topic {
                topic: "lm".into(),
                package: Some("stats".into())
            })
        );
        assert_eq!(
            help_link("../help/glm", Some("stats")),
            Some(HelpLink::Topic {
                topic: "glm".into(),
                package: Some("stats".into())
            })
        );
        assert_eq!(
            help_link("../Example/lm", Some("stats")),
            Some(HelpLink::Example {
                topic: "lm".into(),
                package: Some("stats".into())
            })
        );
        assert_eq!(
            help_link("00Index.html", Some("stats")),
            Some(HelpLink::Index(Some("stats".into())))
        );
        assert_eq!(
            help_link("https://r-project.org", None),
            Some(HelpLink::Web("https://r-project.org".into()))
        );
        assert_eq!(
            help_link("../../base/help/%5B", None),
            Some(HelpLink::Topic {
                topic: "[".into(),
                package: Some("base".into())
            })
        );
    }

    #[test]
    fn breaks_long_signatures_one_argument_a_line() {
        assert_eq!(
            signature_lines("np.mean", "(a, axis=None)", 60),
            "mean(a, axis=None)"
        );
        assert_eq!(
            signature_lines(
                "f",
                "(adata, target_sum=None, key=('a', 'b'), x={'k': 1}) -> None",
                20
            ),
            "f(\n    adata,\n    target_sum=None,\n    key=('a', 'b'),\n    x={'k': 1},\n) -> None"
        );
    }
}
