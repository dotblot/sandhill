//! Drawing the AI pane, in the Tauri app's look (`AgentsPane.tsx`,
//! `AgentChat.tsx`, the `.chat` rules of `styles.css`), text 2px larger:
//! the conversation's tab and buttons, then the chat, the terminal, the
//! history or a new conversation, and the message box.

use chrono::{Local, TimeZone};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::TerminalInfo;
use serde_json::{Value, json};

use crate::agents::{Agents, KINDS, Show, kind_of, model_label};
use crate::chat::{self, Ask, Item, Step, StepStatus};
use crate::look::{self, Palette, RADIUS, icon, icon_button, pane, picture};
use crate::workbench::Workbench;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

impl Render for Agents {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let hint = self.placeholder();
        if self.hint != hint {
            self.hint = hint;
            self.composer
                .update(cx, |c, cx| c.set_placeholder(hint, window, cx));
        }
        let current = self.current().cloned();
        let body: AnyElement = match (self.show, &current) {
            (Show::History, _) => self.render_history(cx),
            (Show::New, _) => self.render_new(cx),
            _ if self.starting.is_some() => self.render_note(
                &format!("Starting {}…", self.starting.unwrap_or_default()),
                &p,
            ),
            (Show::Terminal, Some(t)) => self.render_terminal(t, cx),
            (Show::Chat, Some(t)) => self.render_chat(t, cx),
            (_, None) => self.render_empty(cx),
        };
        let composer = match (self.show, &current) {
            (Show::New, _) => true,
            (Show::Chat, Some(t)) => t.running || crate::composer::can_resume(t),
            _ => false,
        };
        pane(&p)
            .size_full()
            .child(self.render_head(current.as_ref(), cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .when_some(self.problem.clone(), |d, problem| {
                d.child(
                    div()
                        .flex_none()
                        .mx(px(8.))
                        .mb(px(4.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(RADIUS))
                        .bg(p.err_soft)
                        .text_size(px(look::SMALL + 1.))
                        .child(problem),
                )
            })
            .when(composer, |d| {
                d.child(self.render_composer(current.as_ref(), window, cx))
            })
    }
}

impl Agents {
    /// The tab with the conversation's name, then New, History and the
    /// terminal.
    fn render_head(
        &self,
        current: Option<&TerminalInfo>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        let (leading, label): (AnyElement, String) = match (self.show, current) {
            (Show::History, _) => (
                icon("history").size(px(14.)).into_any_element(),
                "History".into(),
            ),
            (Show::New, _) => (
                icon("add").size(px(14.)).into_any_element(),
                "New conversation".into(),
            ),
            (_, Some(t)) => (provider(&t.kind).into_any_element(), self.title(t)),
            (_, None) => (
                icon("chat").size(px(14.)).into_any_element(),
                "Agents".into(),
            ),
        };
        let waiting_elsewhere = self
            .terminals
            .iter()
            .any(|t| self.waiting(t) && Some(&t.id) != self.selected.as_ref());
        let on = p.ink(11.);
        Workbench::strip(&p)
            .child(
                Workbench::tab("agent-title", Some(leading), label, true, &p)
                    .flex_shrink(1.)
                    .min_w(px(0.))
                    .overflow_hidden(),
            )
            .child(div().flex_1())
            .child(
                icon_button("agent-new", "add", &p, false)
                    .on_click(cx.listener(|this, _, window, cx| this.show(Show::New, window, cx))),
            )
            .child(
                icon_button("agent-history", "history", &p, false)
                    .relative()
                    .when(self.show == Show::History, |d| d.bg(on).text_color(p.text))
                    .when(waiting_elsewhere, |d| {
                        d.child(
                            look::dot(p.accent)
                                .size(px(6.))
                                .absolute()
                                .top(px(2.))
                                .right(px(2.)),
                        )
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        let show = if this.show == Show::History {
                            Show::Chat
                        } else {
                            Show::History
                        };
                        this.show(show, window, cx)
                    })),
            )
            .when(current.is_some(), |d| {
                let terminal = self.show == Show::Terminal;
                d.child(
                    icon_button(
                        "agent-terminal",
                        if terminal { "chat" } else { "terminal" },
                        &p,
                        false,
                    )
                    .when(terminal, |d| d.bg(on).text_color(p.text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let show = if terminal { Show::Chat } else { Show::Terminal };
                        this.show(show, window, cx)
                    })),
                )
            })
    }

    fn render_note(&self, text: &str, p: &Palette) -> AnyElement {
        div()
            .p(px(16.))
            .text_color(p.muted)
            .child(text.to_string())
            .into_any_element()
    }

    /// No conversation yet.
    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        v_flex()
            .items_center()
            .gap(px(8.))
            .px(px(28.))
            .py(px(70.))
            .text_color(p.muted)
            .child(
                div()
                    .size(px(44.))
                    .rounded(px(RADIUS))
                    .bg(p.accent_soft)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("chat").size(px(22.)).text_color(p.accent)),
            )
            .child(div().text_color(p.text).font_weight(FontWeight::SEMIBOLD).child("No conversations"))
            .child(
                div().text_center().line_height(rems(1.45)).child(
                    "Chat with Claude Code, Codex or Antigravity about this project. They run with your own plan, can run code in your live R and Python sessions, and add cells to your files.",
                ),
            )
            .child(
                primary_button("agent-empty-new", "New conversation", &p)
                    .on_click(cx.listener(|this, _, window, cx| this.show(Show::New, window, cx))),
            )
            .into_any_element()
    }

    /// Picking the agent; the first message starts it.
    fn render_new(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let project = self
            .project
            .rsplit('/')
            .next()
            .unwrap_or(&self.project)
            .to_string();
        let chosen = kind_of(self.new_kind).unwrap_or(&KINDS[0]);
        let missing = self
            .installed
            .iter()
            .find(|a| a.kind == chosen.kind)
            .filter(|a| a.path.is_none())
            .map(|a| a.install.clone());
        let chips = KINDS.iter().map(|k| {
            let on = k.kind == self.new_kind;
            let hover = p.ink(7.);
            let kind = k.kind;
            h_flex()
                .id(SharedString::from(format!("new-{}", k.kind)))
                .gap(px(6.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(RADIUS))
                .border_1()
                .cursor_pointer()
                .map(|d| {
                    if on {
                        d.border_color(p.accent).bg(p.accent_soft)
                    } else {
                        d.border_color(p.line).hover(move |d| d.bg(hover))
                    }
                })
                .child(provider(k.kind))
                .child(k.name)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_new_kind(kind, cx)))
        });
        div()
            .id("agent-new-body")
            .flex_1()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .gap(px(10.))
                    .p(px(16.))
                    .line_height(rems(1.45))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("New conversation"),
                    )
                    .child(div().text_color(p.muted).child(format!(
                        "It works on the whole {project} project and sees your live sessions. To have it work on a file, name the file in your message."
                    )))
                    .child(h_flex().flex_wrap().gap(px(6.)).children(chips))
                    .child(
                        div()
                            .text_size(px(look::SMALL + 1.))
                            .text_color(p.faint)
                            .child(chosen.plan),
                    )
                    .when_some(missing, |d, install| {
                        d.child(
                            v_flex()
                                .gap(px(6.))
                                .px(px(10.))
                                .py(px(8.))
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
                                ),
                        )
                    })
                    .child(div().text_color(p.faint).child(format!(
                        "Runs in this project's folder, signed in with your own {} account. Type your first message below.",
                        chosen.name
                    ))),
            )
            .into_any_element()
    }

    /// The project's conversations: those waiting for you, those running,
    /// then by day.
    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let mut all: Vec<&TerminalInfo> = self.terminals.iter().collect();
        all.sort_by_key(|t| std::cmp::Reverse(self.last_active(t)));
        let today = Local::now().date_naive();
        let group = |t: &TerminalInfo| -> &'static str {
            if self.waiting(t) {
                return "Needs you";
            }
            if self.working(t) {
                return "Running";
            }
            let day = Local
                .timestamp_millis_opt(self.last_active(t))
                .single()
                .map(|d| d.date_naive());
            match day {
                Some(d) if d == today => "Today",
                Some(d) if Some(d) == today.pred_opt() => "Yesterday",
                _ => "Earlier",
            }
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for name in ["Needs you", "Running", "Today", "Yesterday", "Earlier"] {
            let members: Vec<&TerminalInfo> =
                all.iter().copied().filter(|t| group(t) == name).collect();
            if members.is_empty() {
                continue;
            }
            rows.push(
                div()
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(2.))
                    .text_size(px(look::SMALL + 1.))
                    .text_color(p.faint)
                    .child(name)
                    .into_any_element(),
            );
            for t in members {
                rows.push(self.history_row(t, cx));
            }
        }
        if rows.is_empty() {
            return self.render_note("No conversations yet.", &p);
        }
        div()
            .id("agent-history")
            .flex_1()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .p(px(8.))
                    .children(rows)
                    .child(
                        div()
                            .px(px(10.))
                            .pt(px(14.))
                            .text_size(px(look::SMALL + 1.))
                            .text_color(p.faint)
                            .child("Conversations are kept by sandhilld. Reopening one resumes the agent's own conversation."),
                    ),
            )
            .into_any_element()
    }

    fn history_row(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let id = t.id.clone();
        let selected = self.selected.as_ref() == Some(&t.id);
        let state = if self.waiting(t) {
            "waiting for you".to_string()
        } else if self.working(t) {
            "running".to_string()
        } else {
            time_label(self.last_active(t))
        };
        let sub = [model_label(t), Some(state)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let hover = p.ink(6.);
        h_flex()
            .id(SharedString::from(format!("hist-{}", t.id)))
            .items_start()
            .gap(px(10.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(RADIUS))
            .cursor_pointer()
            .when(selected, |d| d.bg(p.ink(8.)))
            .hover(move |d| d.bg(hover))
            .child(div().pt(px(3.)).child(provider(&t.kind)))
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(self.title(t)),
                    )
                    .child(
                        div()
                            .text_size(px(look::SMALL + 1.))
                            .text_color(if self.waiting(t) { p.accent } else { p.faint })
                            .child(sub),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.select(id.clone(), window, cx)))
            .into_any_element()
    }

    fn render_terminal(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let program = match t.kind.as_str() {
            "claude" => "claude",
            "codex" => "codex",
            "antigravity" => "agy",
            other => other,
        };
        let label = match model_label(t) {
            Some(model) => format!("{program} in a terminal · {model}"),
            None => format!("{program} in a terminal"),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(3.))
                    .border_b_1()
                    .border_color(p.line)
                    .text_color(p.muted)
                    .text_size(px(look::SMALL + 1.))
                    .child(icon("terminal").size(px(14.)))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(label),
                    )
                    .child(
                        look::bar_button("back-to-chat", "Back to chat", Some("chat"), &p)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.show(Show::Chat, window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(p.term)
                    .children(self.view(&t.id)),
            )
            .into_any_element()
    }

    fn render_chat(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let chat = self.chat(t);
        let agent = kind_of(&t.kind).map_or("The agent", |k| k.short);
        let mut items: Vec<AnyElement> = chat
            .items
            .iter()
            .map(|item| self.render_item(item, agent, cx))
            .collect();
        if items.is_empty() {
            items.push(
                div()
                    .text_color(p.faint)
                    .child(if t.running {
                        format!("{agent} is ready. Type a message below.")
                    } else {
                        "This conversation has ended.".to_string()
                    })
                    .into_any_element(),
            );
        }
        if !t.running {
            items.push(
                div()
                    .self_center()
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child("The agent has stopped running.")
                    .into_any_element(),
            );
        }
        div()
            .id("chat")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.chat_scroll)
            .child(
                v_flex()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .gap(px(8.))
                    .children(items),
            )
            .into_any_element()
    }

    fn render_item(&self, item: &Item, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        match item {
            Item::User { text, place, .. } => {
                let refs: Vec<String> = place["refs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect();
                let context = place["file"]
                    .as_str()
                    .map(|f| f.rsplit('/').next().unwrap_or(f).to_string());
                v_flex()
                    .items_end()
                    .gap(px(3.))
                    .mt(px(6.))
                    .child(
                        div()
                            .max_w(relative(0.88))
                            .px(px(11.))
                            .py(px(8.))
                            .rounded(px(RADIUS))
                            .bg(p.ink(8.))
                            .line_height(rems(1.45))
                            .child(text.clone()),
                    )
                    .when(!refs.is_empty() || context.is_some(), |d| {
                        let mut words = refs;
                        if let Some(file) = context {
                            words.push(format!("in {file}"));
                        }
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(words.join(" · ")),
                        )
                    })
                    .into_any_element()
            }
            Item::Reply { key, text } => div()
                .line_height(rems(1.5))
                .child(
                    TextView::markdown(SharedString::from(format!("md-{key}")), text.clone())
                        .selectable(true)
                        .style(
                            TextViewStyle::default()
                                .paragraph_gap(rems(0.4))
                                .heading_font_size(|_, _| px(look::CHROME)),
                        ),
                )
                .into_any_element(),
            Item::Step { key, step } => self.render_step(key, step, agent, cx),
            Item::Ask { ask, .. } => self.render_ask(ask, agent, cx),
            Item::Footer { at_ms, ms, .. } => {
                let mut parts = Vec::new();
                if let Some(ms) = ms {
                    parts.push(duration_label(*ms));
                }
                if let Some(at) = at_ms {
                    parts.push(time_label(*at));
                }
                h_flex()
                    .gap(px(6.))
                    .mb(px(4.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(icon("check").size(px(12.)))
                    .child(parts.join(" · "))
                    .into_any_element()
            }
            Item::Note { text, .. } => div()
                .self_center()
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(text.clone())
                .into_any_element(),
            Item::Error { text, .. } => h_flex()
                .items_start()
                .gap(px(7.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(RADIUS))
                .bg(p.err_soft)
                .child(icon("warning").size(px(15.)).text_color(p.err))
                .child(div().flex_1().child(text.clone()))
                .into_any_element(),
            Item::Thinking { .. } => h_flex()
                .gap(px(7.))
                .text_size(px(look::SMALL + 1.))
                .text_color(p.faint)
                .child(icon("think").size(px(14.)))
                .child(format!("{agent} is working…"))
                .into_any_element(),
        }
    }

    /// A step's row, which opens to its card; a request waiting on it shows
    /// under it.
    fn render_step(
        &self,
        key: &str,
        step: &Step,
        agent: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let view = chat::step_view(&step.tool, &step.input, step.output.as_ref(), None);
        let open = self.open_steps.contains(key);
        let (icon_name, color) = match step.status {
            StepStatus::Fail => ("step.failed", p.err),
            StepStatus::Deny | StepStatus::Cancel => (tool_icon(&view.name), p.faint),
            StepStatus::Wait => ("ask", p.accent),
            StepStatus::Run => (tool_icon(&view.name), p.busy),
            StepStatus::Done => (tool_icon(&view.name), p.muted),
        };
        let detail = match (&step.error, step.status) {
            (Some(err), _) => Some(err.lines().next().unwrap_or_default().to_string()),
            (None, StepStatus::Deny) => Some("denied".into()),
            (None, StepStatus::Cancel) => Some("stopped".into()),
            _ => view.detail.clone(),
        };
        let hover = p.ink(6.);
        let key_owned = key.to_string();
        let row = h_flex()
            .id(SharedString::from(format!("step-{key}")))
            .gap(px(7.))
            .min_h(px(26.))
            .px(px(6.))
            .rounded(px(RADIUS))
            .cursor_pointer()
            .text_size(px(look::SMALL + 1.))
            .hover(move |d| d.bg(hover))
            .child(icon(icon_name).size(px(15.)).text_color(color))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .font_family(look::MONO)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(view.name.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.muted)
                            .when(step.status == StepStatus::Deny, |d| d.line_through())
                            .child(view.desc.clone()),
                    ),
            )
            .when_some(detail, |d, detail| {
                d.child(
                    div()
                        .flex_none()
                        .max_w(px(200.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(if step.error.is_some() { p.err } else { p.faint })
                        .child(detail),
                )
            })
            .child(
                icon(if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                })
                .size(px(11.))
                .text_color(p.faint),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_step(&key_owned, cx)));
        let waiting = step
            .ask
            .as_ref()
            .filter(|a| a.answer.is_none() && step.status == StepStatus::Wait);
        v_flex()
            .mx(px(-6.))
            .rounded(px(RADIUS))
            .when(open, |d| d.bg(p.ink(6.)))
            .child(row)
            .when(open, |d| {
                d.child(
                    div()
                        .pl(px(29.))
                        .pr(px(6.))
                        .pb(px(8.))
                        .child(step_card(&view, &p)),
                )
            })
            .when_some(waiting.cloned(), |d, ask| {
                d.child(
                    div()
                        .px(px(6.))
                        .pb(px(4.))
                        .child(self.render_ask(&ask, agent, cx)),
                )
            })
            .into_any_element()
    }

    /// What waits on the user: a permission, a question or a plan.
    fn render_ask(&self, ask: &Ask, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let view = chat::ask_view(ask, None, agent);
        let pending = ask.answer.is_none();
        let sent = self.sent.contains(&ask.id);
        let mut card = v_flex()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.accent.opacity(0.55))
            .bg(p.accent.opacity(0.06))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        icon(if view.questions.is_empty() {
                            "ask"
                        } else {
                            "help"
                        })
                        .size(px(18.))
                        .text_color(p.accent),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(view.title.clone()),
                            )
                            .when_some(view.detail.clone(), |d, detail| {
                                d.child(
                                    div()
                                        .text_size(px(look::SMALL + 1.))
                                        .text_color(p.muted)
                                        .child(detail),
                                )
                            }),
                    )
                    .when_some(view.view.added.filter(|n| *n > 0), |d, n| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL + 1.))
                                .text_color(p.ok)
                                .child(format!("+{n}")),
                        )
                    }),
            );
        if let Some(plan) = &view.plan {
            card = card.child(
                div()
                    .max_h(px(320.))
                    .p(px(8.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.bg)
                    .child(TextView::markdown(
                        SharedString::from(format!("plan-{}", ask.id)),
                        plan.clone(),
                    )),
            );
        } else if view.questions.is_empty() {
            card = card.child(step_card(&view.view, &p));
        }
        if !pending {
            let answer = ask.answer.as_ref().and_then(|a| a["behavior"].as_str());
            let text = match answer {
                Some("allow") => "Allowed",
                Some("deny") => "Denied",
                _ => "No longer waiting",
            };
            return card
                .child(
                    div()
                        .text_size(px(look::SMALL + 1.))
                        .text_color(p.faint)
                        .child(text),
                )
                .into_any_element();
        }
        for (qi, question) in view.questions.iter().enumerate() {
            let options = question
                .options
                .iter()
                .enumerate()
                .map(|(oi, (label, desc))| {
                    let id = ask.id.clone();
                    let q = question.question.clone();
                    let label_owned = label.clone();
                    let hover = p.faint;
                    v_flex()
                        .id(SharedString::from(format!("q-{}-{qi}-{oi}", ask.id)))
                        .px(px(9.))
                        .py(px(5.))
                        .rounded(px(RADIUS))
                        .border_1()
                        .border_color(p.line)
                        .bg(p.bg)
                        .cursor_pointer()
                        .hover(move |d| d.border_color(hover))
                        .child(div().font_weight(FontWeight::MEDIUM).child(label.clone()))
                        .when_some(desc.clone(), |d, desc| {
                            d.child(
                                div()
                                    .text_size(px(look::SMALL + 1.))
                                    .text_color(p.muted)
                                    .child(desc),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut answers = serde_json::Map::new();
                            answers.insert(q.clone(), Value::String(label_owned.clone()));
                            this.answer(&id, json!({"behavior": "allow", "answers": answers}), cx);
                        }))
                });
            card = card.child(
                v_flex()
                    .gap(px(6.))
                    .child(div().child(question.question.clone()))
                    .child(v_flex().gap(px(4.)).pl(px(26.)).children(options)),
            );
        }
        if view.questions.is_empty() {
            let scopes = ask.scopes.clone();
            let can = |scope: &str| scopes.as_ref().is_none_or(|s| s.iter().any(|x| x == scope));
            let plan = view.plan.is_some();
            let mut buttons = h_flex().gap(px(6.)).flex_wrap();
            let button = |id: &str, label: &str, primary: bool, deny: bool| {
                let hover = p.hover;
                div()
                    .id(SharedString::from(format!("{id}-{}", ask.id)))
                    .h(px(28.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS))
                    .border_1()
                    .whitespace_nowrap()
                    .text_size(px(look::SMALL + 1.))
                    .cursor_pointer()
                    .when(sent, |d| d.opacity(0.5))
                    .map(|d| {
                        if primary {
                            d.bg(p.accent)
                                .border_color(p.accent)
                                .text_color(p.accent_text)
                                .font_weight(FontWeight::SEMIBOLD)
                        } else {
                            d.bg(p.bg)
                                .border_color(p.line)
                                .when(deny, |d| d.text_color(p.err))
                                .hover(move |d| d.bg(hover))
                        }
                    })
                    .child(label.to_string())
            };
            let id = &ask.id;
            buttons = buttons.child(
                button(
                    "allow",
                    if plan { "Carry it out" } else { "Allow once" },
                    true,
                    false,
                )
                .on_click(Self::reply(
                    id.clone(),
                    json!({"behavior": "allow", "scope": "once"}),
                    cx,
                )),
            );
            if !plan && can("chat") {
                buttons = buttons.child(
                    button("allow-chat", "For this chat", false, false).on_click(Self::reply(
                        id.clone(),
                        json!({"behavior": "allow", "scope": "chat"}),
                        cx,
                    )),
                );
            }
            if !plan && can("project") {
                buttons = buttons.child(
                    button("allow-project", "For this project", false, false).on_click(
                        Self::reply(
                            id.clone(),
                            json!({"behavior": "allow", "scope": "project"}),
                            cx,
                        ),
                    ),
                );
            }
            buttons = buttons.child(
                button(
                    "deny",
                    if plan { "Keep planning" } else { "Deny" },
                    false,
                    true,
                )
                .on_click(Self::reply(id.clone(), json!({"behavior": "deny"}), cx)),
            );
            card = card.child(buttons);
            if !ask.remember.is_empty() && !plan {
                card = card.child(div().text_size(px(look::SMALL)).text_color(p.faint).child(
                    format!(
                        "Allowing it for the chat or project also allows: {}",
                        ask.remember.join(", ")
                    ),
                ));
            }
        }
        card.into_any_element()
    }

    fn reply(id: String, answer: Value, cx: &mut Context<Self>) -> ClickHandler {
        let entity = cx.entity().downgrade();
        Box::new(move |_, _, cx| {
            entity
                .update(cx, |this, cx| {
                    if !this.sent.contains(&id) {
                        this.answer(&id, answer.clone(), cx)
                    }
                })
                .ok();
        })
    }
}

/// The agent's own mark: Claude Code's, Codex's, Antigravity's.
fn provider(kind: &str) -> Img {
    let name = match kind {
        "claude" | "codex" | "antigravity" => kind,
        _ => "generic",
    };
    picture(&format!("agents/{name}")).size(px(14.))
}

/// A step's icon by what it does.
fn tool_icon(name: &str) -> &'static str {
    match name {
        "bash" => "terminal",
        "read" | "grep" | "glob" | "tools" => "search",
        "edit" | "write" | "edit_notebook" => "edit.compose",
        "run_code" => "run",
        "add_cell" | "edit_cell" => "add",
        "web_search" | "web_fetch" => "globe",
        "thinking" => "think",
        "todo" => "check",
        "plan" => "plan",
        "question" => "help",
        "list_variables" | "get_outputs" => "table",
        "list_sessions" => "sessions",
        "get_focus" => "context.show",
        "list_jobs" | "read_job_log" => "cpu",
        "task" | "agent" => "agent",
        _ => "skill",
    }
}

/// A step's card: its code, its diff, its output.
fn step_card(view: &chat::StepView, p: &Palette) -> AnyElement {
    let block = |text: String, bg: Hsla, color: Hsla| {
        div()
            .max_h(px(360.))
            .overflow_hidden()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.line)
            .bg(bg)
            .font_family(look::MONO)
            .text_size(px(look::SMALL + 1.))
            .line_height(rems(1.5))
            .text_color(color)
            .whitespace_nowrap()
            .child(text)
    };
    let mut card = v_flex().gap(px(6.));
    if let Some(diff) = &view.diff {
        let lines = diff.iter().take(400).map(|line| {
            let (bg, color) = if line.starts_with('+') {
                (p.ok.opacity(0.12), p.text)
            } else if line.starts_with('-') {
                (p.err.opacity(0.10), p.text)
            } else {
                (gpui_kit::transparent_black(), p.muted)
            };
            div()
                .px(px(10.))
                .bg(bg)
                .text_color(color)
                .whitespace_nowrap()
                .child(if line.is_empty() {
                    " ".to_string()
                } else {
                    line.clone()
                })
        });
        card = card.child(
            v_flex()
                .max_h(px(360.))
                .overflow_hidden()
                .py(px(6.))
                .rounded(px(RADIUS))
                .border_1()
                .border_color(p.line)
                .bg(p.chunk)
                .font_family(look::MONO)
                .text_size(px(look::SMALL + 1.))
                .line_height(rems(1.5))
                .children(lines),
        );
    } else if let Some(code) = view.code.as_ref().filter(|c| !c.is_empty()) {
        card = card.child(block(code.clone(), p.chunk, p.text));
    }
    if let Some(output) = view.output.as_ref().filter(|o| !o.trim().is_empty()) {
        let text: String = output.lines().take(200).collect::<Vec<_>>().join("\n");
        card = card.child(block(text, p.bg, p.text));
    }
    card.into_any_element()
}

fn primary_button(id: &'static str, label: &'static str, p: &Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap(px(5.))
        .px(px(12.))
        .py(px(4.))
        .rounded(px(RADIUS))
        .bg(p.accent)
        .text_color(p.accent_text)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(icon("add").size(px(14.)))
        .child(label)
}

/// "16:42", or "Oct 9" for another day.
fn time_label(ms: i64) -> String {
    let Some(at) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    if at.date_naive() == Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d").to_string()
    }
}

/// "3.1 s", "2 min 5 s".
fn duration_label(ms: i64) -> String {
    let s = ms as f64 / 1000.;
    if s < 60. {
        format!("{s:.1} s")
    } else {
        format!("{} min {} s", (s / 60.).floor(), (s % 60.).round())
    }
}
