//! A new conversation's screen (`docs/design/done/new-conversation-plan.md`): the
//! agent it starts with, the project's live sessions, ways to start that
//! fill the message box, and the latest conversations to go on with.

use std::time::{Duration, Instant};

use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, SessionInfo, SessionStatus, TerminalInfo, Variables};
use serde_json::{Value, json};

use crate::agents::composer::Mention;
use crate::agents::view::{provider, time_label};
use crate::agents::{Agents, KINDS, Kind, kind_of, terminal_only};
use crate::look::{self, Palette, RADIUS, dot};

/// The context is read again once it is this old.
const REREAD: Duration = Duration::from_secs(15);
/// A kernel busy with a run doesn't hold the screen up.
const ASK_LIMIT: Duration = Duration::from_secs(2);
/// Conversations offered to go on with.
const CONTINUE: usize = 3;

/// What the screen knows of the project, read in the background.
#[derive(Default)]
pub(crate) struct StartContext {
    sessions: Vec<LiveSession>,
    /// The document open in the editor, in the project.
    file: Option<String>,
    /// A data frame or matrix in a live session, as an @ variable ref.
    table: Option<Value>,
    read_at: Option<Instant>,
    reading: bool,
}

struct LiveSession {
    label: String,
    busy: bool,
}

/// A way to start: words around the chip of what it attaches (the first of
/// `refs`), and what it puts in the message box.
struct Starter {
    before: String,
    after: String,
    text: String,
    refs: Vec<Value>,
}

impl Agents {
    /// Reads the project's sessions, a table in one and the open document,
    /// unless that was done a moment ago.
    pub(crate) fn refresh_start(&mut self, cx: &mut Context<Self>) {
        let fresh = self.start.read_at.is_some_and(|at| at.elapsed() < REREAD);
        if self.start.reading || fresh {
            return;
        }
        self.start.reading = true;
        self.start.file = self.open_document(cx);
        let daemon = self.daemon.clone();
        let project = self.project.clone();
        let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let sessions: Vec<SessionInfo> = sessions
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|s| {
                    s.status == SessionStatus::Running && s.project.as_deref() == Some(&project)
                })
                .collect();
            let mut table = None;
            for s in &sessions {
                let listed = daemon
                    .call_within::<Variables>(
                        Call::ListVariables {
                            session: s.id.clone(),
                            path: Vec::new(),
                        },
                        ASK_LIMIT,
                    )
                    .await;
                if let Some(v) = listed
                    .ok()
                    .and_then(|l| l.variables.into_iter().find(|v| v.table))
                {
                    table = Some(json!({
                        "kind": "variable",
                        "name": v.name,
                        "session": s.id,
                        "language": s.language,
                    }));
                    break;
                }
            }
            let live = sessions
                .iter()
                .map(|s| LiveSession {
                    label: session_label(s),
                    busy: s.pending_runs > 0,
                })
                .collect();
            this.update(cx, |this, cx| {
                this.start.sessions = live;
                this.start.table = table;
                this.start.read_at = Some(Instant::now());
                this.start.reading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The document shown in the editor, else the last one opened, in the
    /// project.
    fn open_document(&self, cx: &App) -> Option<String> {
        let workbench = self.workbench.upgrade()?;
        let wb = workbench.read(cx);
        let path = wb
            .active_document_path()
            .map(str::to_string)
            .or_else(|| wb.tabs.last().cloned())?;
        let root = format!("{}/", self.project.trim_end_matches('/'));
        path.strip_prefix(&root).map(str::to_string)
    }

    fn starters(&self) -> Vec<Starter> {
        let mut starters = vec![Starter {
            before: "Explain what this project does".into(),
            after: String::new(),
            text: "Explain what this project does: its data, its analysis and where it stands."
                .into(),
            refs: Vec::new(),
        }];
        if let Some(file) = &self.start.file {
            let name = crate::panes::files::base_name(file).to_string();
            starters.push(Starter {
                before: "Look over".into(),
                after: "for problems".into(),
                text: format!("Look over {name} for problems."),
                refs: vec![Mention::file(file).item],
            });
        }
        if let Some(table) = &self.start.table {
            let name = table["name"].as_str().unwrap_or_default().to_string();
            starters.push(Starter {
                before: "Summarise".into(),
                after: String::new(),
                text: format!("Summarise {name}: its columns, missing values and anything odd."),
                refs: vec![table.clone()],
            });
        }
        if starters.len() == 1 {
            starters.push(Starter {
                before: "Suggest what to do next in this analysis".into(),
                after: String::new(),
                text: "Suggest what to do next in this analysis.".into(),
                refs: Vec::new(),
            });
        }
        starters
    }

    /// Puts a starter in the message box, chips and all; nothing is sent.
    fn use_starter(
        &mut self,
        text: String,
        refs: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refs.clear();
        for item in refs {
            self.add_ref(item);
        }
        self.composer.update(cx, |c, cx| {
            c.set_value(text, window, cx);
            // Ready to add to it.
            let end = c.text().offset_to_position(c.text().len());
            c.set_cursor_position(end, window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    /// The account a new conversation of `kind` uses, as a few words.
    fn account_words(&self, kind: &Kind) -> String {
        let added = crate::prefs::agent_account(&self.host, kind.kind)
            .and_then(|id| self.accounts.iter().find(|a| a.id == id))
            .filter(|a| !a.default);
        match added {
            Some(account) => format!("{} account", account.name),
            None => kind
                .plan
                .rsplit(" · ")
                .next()
                .unwrap_or(kind.plan)
                .to_string(),
        }
    }

    /// The new conversation's screen; agent and model are chosen in the
    /// message box below it.
    pub(crate) fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let project = crate::panes::files::base_name(&self.project).to_string();
        let chosen = kind_of(self.new_kind).unwrap_or(&KINDS[0]);
        let missing = self
            .installed
            .iter()
            .find(|a| a.kind == chosen.kind)
            .filter(|a| a.path.is_none())
            .map(|a| a.install.clone());
        let model = self.box_model();
        let subtitle = [model, self.account_words(chosen)]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let works = if self.start.sessions.is_empty() {
            format!("Works on all of {project}.")
        } else {
            format!("Works on all of {project} and its live sessions.")
        };
        let sessions = h_flex().flex_wrap().justify_center().gap(px(6.)).children(
            self.start.sessions.iter().map(|s| {
                h_flex()
                    .h(px(22.))
                    .gap(px(6.))
                    .px(px(9.))
                    .rounded_full()
                    .bg(p.cap_bg)
                    .text_size(px(look::SMALL))
                    .text_color(p.text)
                    .child(dot(if s.busy { p.busy } else { p.ok }).size(px(7.)))
                    .child(s.label.clone())
            }),
        );
        let header = v_flex()
            .items_center()
            .text_center()
            .gap(px(4.))
            .pt(px(28.))
            .child(
                div()
                    .size(px(44.))
                    .mb(px(6.))
                    .rounded(px(RADIUS))
                    .bg(p.accent_soft)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(provider(chosen.kind).size(px(24.))),
            )
            .child(
                div()
                    .text_size(px(look::CHROME + 1.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .child(chosen.name),
            )
            .child(
                div()
                    .text_size(px(look::SMALL + 1.))
                    .text_color(p.muted)
                    .child(subtitle),
            )
            .child(div().pt(px(6.)).text_color(p.muted).child(works))
            .when(!self.start.sessions.is_empty(), |d| {
                d.child(div().pt(px(4.)).child(sessions))
            })
            .when(terminal_only(chosen.kind), |d| {
                d.child(
                    div()
                        .pt(px(6.))
                        .text_size(px(look::SMALL + 1.))
                        .text_color(p.faint)
                        .child(format!(
                            "Opens {}'s own screen here. Enter starts it without a message.",
                            chosen.name
                        )),
                )
            });
        let install = missing.map(|install| {
            v_flex()
                .gap(px(6.))
                .px(px(12.))
                .py(px(9.))
                .rounded(px(RADIUS))
                .bg(p.accent_soft)
                .child(format!(
                    "{} isn't installed on this machine. Install it with:",
                    chosen.name
                ))
                .child(
                    div()
                        .font_family(look::MONO)
                        .text_size(px(look::SMALL + 1.))
                        .child(install),
                )
        });
        let starters = self
            .starters()
            .into_iter()
            .enumerate()
            .map(|(ix, s)| self.starter_row(ix, s, &p, cx))
            .collect::<Vec<_>>();
        let mut recent: Vec<&TerminalInfo> = self.terminals.iter().collect();
        recent.sort_by_key(|t| std::cmp::Reverse(self.last_active(t)));
        let recent: Vec<AnyElement> = recent
            .into_iter()
            .take(CONTINUE)
            .map(|t| self.continue_row(t, &p, cx))
            .collect();
        let key = |text: &'static str| {
            div()
                .px(px(5.))
                .h(px(17.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(p.ink(14.))
                .text_size(px(look::SMALL - 1.))
                .text_color(p.muted)
                .child(text)
        };
        let hint =
            |k: &'static str, words: &'static str| h_flex().gap(px(5.)).child(key(k)).child(words);
        // The agent sits in the middle of the free space; the ways to start
        // sit just over the message box, where the hand goes next.
        div()
            .id("agent-new-body")
            .flex_1()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .min_h_full()
                    .gap(px(18.))
                    .px(px(16.))
                    .pb(px(14.))
                    .line_height(rems(1.4))
                    .child(div().flex_1().min_h(px(4.)))
                    .child(header)
                    .children(install)
                    .child(div().flex_1().min_h(px(8.)))
                    .child(section("Start with", starters, &p))
                    .when(!recent.is_empty(), |d| {
                        d.child(section("Continue", recent, &p))
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(14.))
                            .px(px(4.))
                            .text_size(px(look::SMALL))
                            .text_color(p.muted)
                            .child(hint("@", "attach a file"))
                            .child(hint("#", "attach a cell"))
                            .child(hint("↩", "send")),
                    ),
            )
            .into_any_element()
    }

    fn starter_row(
        &self,
        ix: usize,
        starter: Starter,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Starter {
            before,
            after,
            text,
            refs,
        } = starter;
        let chip = refs.first().cloned();
        let (rest, hover, press) = (p.ink(4.), p.ink(8.), p.ink(11.));
        h_flex()
            .id(("agent-starter", ix))
            .min_h(px(34.))
            .flex_wrap()
            .gap(px(6.))
            .px(px(12.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(rest)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .active(move |d| d.bg(press))
            .text_color(p.text)
            .child(before)
            .children(chip.map(|item| {
                // As the message box shows what @ attached.
                h_flex()
                    .flex_none()
                    .max_w(px(180.))
                    .h(px(22.))
                    .gap(px(4.))
                    .px(px(6.))
                    .rounded(px(7.))
                    .bg(p.ink(8.))
                    .text_size(px(look::SMALL))
                    .child(crate::agents::composer_view::ref_icon(&item))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(crate::agents::composer::ref_name(&item).to_string()),
                    )
            }))
            .when(!after.is_empty(), |d| d.child(after))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.use_starter(text.clone(), refs.clone(), window, cx)
            }))
            .into_any_element()
    }

    fn continue_row(&self, t: &TerminalInfo, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let id = t.id.clone();
        let (hover, press) = (p.ink(6.), p.ink(10.));
        h_flex()
            .id(SharedString::from(format!("agent-continue-{}", t.id)))
            .gap(px(10.))
            .px(px(12.))
            .h(px(32.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .active(move |d| d.bg(press))
            .child(provider(&t.kind))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(p.text)
                    .child(self.title(t)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(time_label(self.last_active(t))),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.select(id.clone(), window, cx)))
            .into_any_element()
    }
}

/// A heading over its rows.
fn section(title: &'static str, rows: Vec<AnyElement>, p: &Palette) -> Div {
    v_flex()
        .gap(px(4.))
        .child(
            div()
                .px(px(4.))
                .pb(px(2.))
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .child(title),
        )
        .children(rows)
}

/// "Python 3.12", "R 4.6": the language and its version to the minor one.
fn session_label(s: &SessionInfo) -> String {
    let language = match s.language.to_lowercase().as_str() {
        "python" => "Python".to_string(),
        "r" => "R".to_string(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    let version = s
        .language_version
        .as_deref()
        .map(|v| v.split('.').take(2).collect::<Vec<_>>().join("."));
    match version.filter(|v| !v.is_empty()) {
        Some(v) => format!("{language} {v}"),
        None => language,
    }
}

#[cfg(test)]
mod tests {
    use super::session_label;
    use sandhill_proto::{SessionInfo, SessionStatus};

    #[test]
    fn sessions_are_named_by_language_and_minor_version() {
        let session = |language: &str, version: Option<&str>| SessionInfo {
            id: "s".into(),
            kernel: "k".into(),
            language: language.into(),
            cwd: "/".into(),
            pid: None,
            status: SessionStatus::Running,
            created_ms: 0,
            ended_ms: None,
            last_seq: 0,
            project: None,
            pending_runs: 0,
            language_version: version.map(str::to_string),
        };
        assert_eq!(
            session_label(&session("python", Some("3.12.4"))),
            "Python 3.12"
        );
        assert_eq!(session_label(&session("R", Some("4.6.2"))), "R 4.6");
        assert_eq!(session_label(&session("julia", None)), "Julia");
    }
}
