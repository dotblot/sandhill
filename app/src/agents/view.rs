//! Drawing the AI pane, laid out as in the Tauri app (`AgentsPane.tsx`,
//! `AgentChat.tsx`, the `.chat` rules of `styles.css`), text 2px larger, in
//! a translucent pane on the window's glass: the conversation's tab and
//! buttons, then the chat, the terminal, the history or a new conversation,
//! and the message box.

use chrono::{Local, TimeZone};
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, TerminalInfo};
use serde_json::{Value, json};

use crate::agents::chat::{self, Ask, Item, Step, StepStatus};
use crate::agents::{
    Agents, Show, kind_of, model_label, provider_show, question_key, supports_terminal,
    terminal_only,
};
use crate::look::{self, Palette, RADIUS, icon, icon_button, picture, side_pane};
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
        self.question_fields(window, cx);
        if self.show == Show::New {
            self.refresh_start(cx);
        }
        let current = self.current().cloned();
        if let Some(t) = &current {
            self.show = provider_show(&t.kind, self.show);
        }
        let switching = current
            .as_ref()
            .and_then(|t| self.view_switches.get(&t.id))
            .map(|switch| match switch.target {
                Show::Terminal => "Opening command-line mode…",
                _ => "Returning to chat…",
            });
        let body: AnyElement = match (self.show, &current) {
            (Show::History, _) => self.render_history(cx),
            (Show::New, _) => self.render_new(cx),
            _ if self.starting.is_some() => self.render_note(
                &format!("Starting {}…", self.starting.unwrap_or_default()),
                &p,
            ),
            _ if switching.is_some() => self.render_note(switching.unwrap_or_default(), &p),
            (Show::Terminal, Some(t)) => self.render_terminal(t, cx),
            (Show::Chat, Some(t)) if terminal_only(&t.kind) => self.render_terminal(t, cx),
            (Show::Chat, Some(t)) => self.render_chat(t, cx),
            (_, None) => self.render_empty(cx),
        };
        let composer = match (self.show, &current) {
            (Show::New, _) => true,
            (Show::Chat, Some(t)) => {
                switching.is_none()
                    && !terminal_only(&t.kind)
                    && (t.running || crate::agents::composer::can_resume(t))
            }
            _ => false,
        };
        let floating = self.show == Show::History
            || (self.show == Show::Chat
                && current.as_ref().is_some_and(|t| !terminal_only(&t.kind))
                && self.starting.is_none()
                && switching.is_none());
        let head = self.render_head(current.as_ref(), cx);
        let head = if floating {
            look::floating_header(&p, 0., false, head)
        } else {
            head.into_any_element()
        };
        let mut head = Some(head);
        side_pane(&p)
            .relative()
            .size_full()
            .when(!floating, |d| d.children(head.take()))
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .when_some(self.problem.clone(), |d, problem| {
                d.child(
                    h_flex()
                        .flex_none()
                        .items_start()
                        .gap(px(6.))
                        .mx(px(8.))
                        .mb(px(4.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(RADIUS))
                        .bg(p.err_soft)
                        .text_size(px(look::SMALL + 1.))
                        .child(div().flex_1().min_w_0().child(problem))
                        .child(
                            icon_button("agent-error-close", "close", &p, false)
                                .size(px(20.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.problem = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(composer, |d| {
                d.when_some(
                    current.as_ref().filter(|t| {
                        self.show == Show::Chat && t.running && self.trust_asked.contains(&t.id)
                    }),
                    |d, t| {
                        d.child(
                            div()
                                .flex_none()
                                .mx(px(8.))
                                .mb(px(4.))
                                .child(self.render_trust(t, cx)),
                        )
                    },
                )
                .child(self.render_composer(current.as_ref(), window, cx))
            })
            .children(head)
    }
}

impl Agents {
    /// The conversation's name as plain text, then New, History and the
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
            // The + button beside it starts one; a second + would read as
            // another button.
            (Show::New, _) => (
                icon("chat").size(px(14.)).into_any_element(),
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
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(id, _)| {
                matches!(self.show, Show::Chat | Show::Terminal)
                    && Some(id) == current.map(|t| &t.id)
            })
            .map(|(_, input)| input.clone());
        let title: AnyElement = match renaming {
            Some(input) => h_flex()
                .w(px(240.))
                .flex_shrink(1.)
                .min_w(px(0.))
                .gap(px(6.))
                .px(px(6.))
                .children(current.map(|t| provider(&t.kind)))
                .child(Input::new(&input).small())
                .into_any_element(),
            // Plain text, not a tab: there is nothing to switch to here.
            None => h_flex()
                .id("agent-title")
                .gap(px(6.))
                .px(px(6.))
                .flex_shrink(1.)
                .min_w(px(0.))
                .text_size(px(look::TAB_TEXT))
                .text_color(p.text)
                .child(div().flex_none().child(leading))
                .child(div().min_w_0().truncate().child(label))
                .overflow_hidden()
                .when(
                    current.is_some() && matches!(self.show, Show::Chat | Show::Terminal),
                    |d| {
                        d.on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                            if event.click_count() == 2 {
                                this.start_rename(window, cx);
                            }
                        }))
                    },
                )
                .into_any_element(),
        };
        let gear_on = current.is_some() && matches!(self.show, Show::Chat | Show::Terminal);
        // The same backdrop and icon spacing as the other panes' bars.
        let strip = Workbench::strip(&p)
            .gap(px(0.))
            .child(title)
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
            .when(
                current.is_some_and(|t| !terminal_only(&t.kind) && supports_terminal(&t.kind)),
                |d| {
                    let terminal = self.show == Show::Terminal;
                    d.child(
                        icon_button(
                            "agent-terminal",
                            if terminal { "chat" } else { "terminal" },
                            &p,
                            false,
                        )
                        .when(terminal, |d| d.bg(on).text_color(p.text))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let show = if terminal { Show::Chat } else { Show::Terminal };
                                this.show(show, window, cx)
                            },
                        )),
                    )
                },
            )
            .child(
                div()
                    .relative()
                    .child(
                        icon_button("agent-gear", "settings", &p, false)
                            .on_prepaint(self.popup_triggers.track("agent-gear"))
                            .when(self.gear && gear_on, |d| d.bg(on).text_color(p.text))
                            .when(!gear_on, |d| d.opacity(0.4))
                            .when(gear_on, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.toggle_gear(cx)))
                            }),
                    )
                    .when(self.gear && gear_on, |d| {
                        d.children(current.map(|t| {
                            div().absolute().right_0().bottom_0().child(
                                deferred(
                                    anchored()
                                        .anchor(Anchor::TopRight)
                                        .snap_to_window_with_margin(px(8.))
                                        .child(self.render_gear(t, &p, cx)),
                                )
                                .with_priority(3),
                            )
                        }))
                    }),
            )
            .child({
                let panel = crate::panes::Panel::Agents;
                let maximized = self
                    .workbench
                    .upgrade()
                    .is_some_and(|w| w.read(cx).maximized_panel == Some(panel));
                icon_button(
                    "agent-maximize",
                    if maximized {
                        "pane.restore"
                    } else {
                        "pane.maximize"
                    },
                    &p,
                    false,
                )
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if maximized {
                        "Restore"
                    } else {
                        "Maximize to the window"
                    })
                    .build(w, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let _ = this
                        .workbench
                        .update(cx, |w, cx| w.maximize_panel(panel, cx));
                    cx.notify();
                }))
            });
        look::stacked_header(&p, strip)
    }

    /// The conversation's menu: compact, rename, end.
    fn render_gear(&self, t: &TerminalInfo, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let hover = p.ink(7.);
        let row = |id: &'static str, name: &'static str, label: String| {
            h_flex()
                .id(id)
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child(icon(name).size(px(14.)))
                .child(label)
        };
        let compact = t.running && (t.kind == "claude" || crate::agents::drives_chat(&t.kind));
        let end = if !t.running {
            "Remove from history".to_string()
        } else if self.confirm_end {
            "Click again to end".to_string()
        } else {
            "End conversation".to_string()
        };
        crate::agents::composer_view::popup(p)
            .mt(px(4.))
            .w(px(220.))
            .occlude()
            .when(compact, |d| {
                d.child(
                    row("gear-compact", "compact", "Compact conversation".into()).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.gear = false;
                            this.compact(cx);
                        }),
                    ),
                )
            })
            .child(
                row("gear-rename", "edit.rename", "Rename".into())
                    .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx))),
            )
            .when(!self.entries(&t.id).is_empty(), |d| {
                d.child(div().h(px(1.)).mx(px(4.)).my(px(3.)).bg(p.line))
                    .children(crate::editor::export::Format::ALL.map(|format| {
                        row(
                            format.ext(),
                            "share",
                            format!("Export as {}", format.label()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.export_chat(format, cx)))
                    }))
            })
            .child(div().h(px(1.)).mx(px(4.)).my(px(3.)).bg(p.line))
            .child(
                row("gear-end", "delete", end)
                    .when(self.confirm_end, |d| d.text_color(p.err))
                    .on_click(cx.listener(|this, _, window, cx| this.end_conversation(window, cx))),
            )
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("agent-gear", event.position) {
                    return;
                }
                this.gear = false;
                this.confirm_end = false;
                cx.notify();
            }))
            .into_any_element()
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
                    "Chat with Antigravity, Claude Code, Codex, OpenScience or Posit Assistant about this project. They use your own plans or configured models, can run code in your live R and Python sessions, and add cells to your files.",
                ),
            )
            .child(
                primary_button("agent-empty-new", "New conversation", &p)
                    .on_click(cx.listener(|this, _, window, cx| this.show(Show::New, window, cx))),
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
            rows.push(self.render_note("No conversations yet.", &p));
        }
        look::PaneScroll::new(
            "agent-history-scroll",
            look::STRIP,
            0.,
            div()
            .id("agent-history")
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
            ),
        )
            .into_any_element()
    }

    fn history_row(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let id = t.id.clone();
        let selected = self.selected.as_ref() == Some(&t.id);
        let idle_stopped = !t.running && t.exit.as_ref().is_some_and(|e| e.idle);
        let state = if self.waiting(t) {
            "waiting for you".to_string()
        } else if self.working(t) {
            "running".to_string()
        } else if idle_stopped {
            format!("stopped while idle · {}", time_label(self.last_active(t)))
        } else {
            time_label(self.last_active(t))
        };
        let sub = [model_label(t), Some(state)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let hover = p.ink(6.);
        let group: SharedString = format!("hist-group-{}", t.id).into();
        let asking = self.confirm_delete.as_deref() == Some(&t.id);
        let delete_id = t.id.clone();
        let delete = h_flex()
            .id(SharedString::from(format!("hist-delete-{}", t.id)))
            .flex_none()
            .ml_auto()
            .gap(px(4.))
            .px(px(5.))
            .h(px(22.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_size(px(look::SMALL))
            .map(|d| {
                if asking {
                    d.bg(p.err).text_color(gpui_kit::white()).child("Delete?")
                } else {
                    d.invisible()
                        .group_hover(group.clone(), |d| d.visible())
                        .text_color(p.faint)
                        .hover(|d| d.text_color(p.err))
                }
            })
            .child(icon("delete").size(px(14.)))
            .tooltip(|w, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Delete this conversation").build(w, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.delete_from_history(delete_id.clone(), window, cx)
            }));
        // Stops the agent but keeps the conversation; a message resumes it.
        let stop = t.running.then(|| {
            let stop_id = t.id.clone();
            let resumable = crate::agents::composer::can_resume(t);
            h_flex()
                .id(SharedString::from(format!("hist-stop-{}", t.id)))
                .flex_none()
                .px(px(5.))
                .h(px(22.))
                .rounded(px(6.))
                .cursor_pointer()
                .invisible()
                .group_hover(group.clone(), |d| d.visible())
                .text_color(p.faint)
                .hover(|d| d.text_color(p.text))
                .child(icon("stop").size(px(14.)))
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if resumable {
                        "Stop the agent. The conversation stays; a message picks it up again."
                    } else {
                        "Stop the agent. This conversation cannot be picked up again."
                    })
                    .build(w, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.daemon.fire(Call::StopTerminal {
                        terminal: stop_id.clone(),
                    });
                    cx.notify();
                }))
        });
        h_flex()
            .id(SharedString::from(format!("hist-{}", t.id)))
            .group(group)
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
            .child(div().flex_1())
            .children(stop)
            .child(delete)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_delete = None;
                this.select(id.clone(), window, cx)
            }))
            .into_any_element()
    }

    fn render_terminal(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let program = match t.kind.as_str() {
            "claude" => "claude",
            "codex" => "codex",
            "antigravity" => "agy",
            "posit" => "pa",
            other => other,
        };
        // An agent without a chat has no chat to go back to; once it ends,
        // a new conversation starts it again.
        let button = if !terminal_only(&t.kind) {
            Some(
                look::bar_button("back-to-chat", "Back to chat", Some("chat"), &p)
                    .on_click(cx.listener(|this, _, window, cx| this.show(Show::Chat, window, cx))),
            )
        } else if !t.running {
            Some(
                look::bar_button("start-again", "Start a new conversation", Some("add"), &p)
                    .on_click(cx.listener(|this, _, window, cx| this.start_again(window, cx))),
            )
        } else {
            None
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
                    .children(button),
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
        if (t.running || crate::agents::composer::can_resume(t))
            && matches!(t.kind.as_str(), "claude" | "codex" | "antigravity")
        {
            let account_id = t.options.account.as_deref().unwrap_or(&t.kind);
            let limited = self
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .is_some_and(crate::agents::account_at_limit)
                || (!self.working(t) && chat::latest_usage_limit(&chat));
            if limited {
                let mut alternatives = v_flex().gap(px(6.)).text_size(px(13.));
                for account in self.accounts.iter().filter(|account| {
                    account.kind == t.kind
                        && account.id != account_id
                        && account.signed_in != Some(false)
                        && !crate::agents::account_at_limit(account)
                }) {
                    let account = account.clone();
                    alternatives = alternatives.child(
                        div()
                            .id(SharedString::from(format!("continue-{}", account.id)))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(RADIUS))
                            .bg(p.accent_soft)
                            .text_color(p.accent)
                            .cursor_pointer()
                            .hover(move |d| d.bg(p.ink(8.)))
                            .child(format!("Continue with {}", account.name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.switch_account(account.clone(), window, cx);
                            })),
                    );
                }
                items.push(alternatives.into_any_element());
            }
        }
        if !t.running {
            let resumable = crate::agents::composer::can_resume(t);
            items.push(
                v_flex()
                    .self_center()
                    .items_center()
                    .gap(px(6.))
                    .pt(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(if t.exit.as_ref().is_some_and(|e| e.idle) {
                        format!("{agent} stopped after 30 minutes idle. Send a message to pick this conversation up again.")
                    } else if resumable {
                        format!("{agent} has stopped. Send a message to pick this conversation up again.")
                    } else {
                        format!("{agent} has stopped.")
                    })
                    .when(!resumable, |d| {
                        d.child(
                            div()
                                .id("chat-start-again")
                                .px(px(10.))
                                .py(px(3.))
                                .rounded(px(RADIUS))
                                .border_1()
                                .border_color(p.line)
                                .text_color(p.text)
                                .cursor_pointer()
                                .child("Start a new conversation like it")
                                .on_click(cx.listener(|this, _, window, cx| this.start_again(window, cx))),
                        )
                    })
                    .into_any_element(),
            );
        }
        look::PaneScroll::new(
            "chat-scroll",
            look::STRIP,
            0.,
            div().id("chat").child(
                v_flex()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .gap(px(8.))
                    .children(items),
            ),
        )
        .handle(&self.chat_scroll)
        .into_any_element()
    }

    /// Claude Code's question, the first time it works in a folder,
    /// whether the user trusts the files there: asked here, as its terminal
    /// is out of sight, and a message typed at it would end it.
    fn render_trust(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let folder = t.cwd.clone();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            let hover = p.hover;
            div()
                .id(id)
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(RADIUS))
                .border_1()
                .whitespace_nowrap()
                .text_size(px(look::SMALL + 1.))
                .cursor_pointer()
                .map(|d| {
                    if primary {
                        d.bg(p.accent)
                            .border_color(p.accent)
                            .text_color(p.primary_text)
                            .font_weight(FontWeight::SEMIBOLD)
                    } else {
                        d.bg(p.bg).border_color(p.line).hover(move |d| d.bg(hover))
                    }
                })
                .child(label)
        };
        let yes = t.clone();
        let no = t.clone();
        v_flex()
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
                    .child(icon("ask").size(px(18.)).text_color(p.accent))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Do you trust the files in this folder?"),
                            )
                            .child(
                                div()
                                    .text_size(px(look::SMALL + 1.))
                                    .text_color(p.muted)
                                    .child(format!(
                                        "Claude Code asks once for each folder before it reads, edits or runs files in it: {folder}"
                                    )),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(button("trust-yes", "Yes, I trust this folder", true).on_click(
                        cx.listener(move |this, _, _, cx| this.answer_trust(&yes, true, cx)),
                    ))
                    .child(button("trust-no", "No, end the conversation", false).on_click(
                        cx.listener(move |this, _, _, cx| this.answer_trust(&no, false, cx)),
                    )),
            )
            .into_any_element()
    }

    fn render_item(&self, item: &Item, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        match item {
            // Only what the user added with @ and #: messages carry no
            // document of their own, and older ones' recorded `place.file`
            // isn't shown either.
            Item::User { text, place, .. } => {
                let refs: Vec<String> = place["refs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect();
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
                    .when(!refs.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(refs.join(" · ")),
                        )
                    })
                    .into_any_element()
            }
            Item::Reply { key, text } => div()
                .line_height(rems(1.5))
                .child(
                    TextView::markdown(SharedString::from(format!("md-{key}")), text.clone())
                        .selectable(true)
                        // New words fade in one after another as the reply streams.
                        .stream_fade(true)
                        .code_block_actions(code_copy_button)
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
            Item::Account { name, .. } => h_flex()
                .gap(px(8.))
                .my(px(6.))
                .text_size(px(13.))
                .text_color(p.muted)
                .child(div().flex_1().h(px(1.)).bg(p.line))
                .child(format!("Switched to {name}"))
                .child(div().flex_1().h(px(1.)).bg(p.line))
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
                let file = view
                    .file
                    .clone()
                    .filter(|f| crate::editor::notebook::format_of(f).is_some());
                d.child(
                    v_flex()
                        .gap(px(4.))
                        .pl(px(29.))
                        .pr(px(6.))
                        .pb(px(8.))
                        .child(step_card(&view, &p))
                        .when_some(file, |d, file| {
                            let name = file.rsplit('/').next().unwrap_or(&file).to_string();
                            d.child(
                                div()
                                    .id(SharedString::from(format!("open-{key}")))
                                    .text_size(px(look::SMALL))
                                    .text_color(p.accent)
                                    .cursor_pointer()
                                    .hover(|d| d.underline())
                                    .child(format!("Open {name}"))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_path(&file, cx)
                                        }),
                                    ),
                            )
                        }),
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
                        icon(if !view.questions.is_empty() {
                            "help"
                        } else if view.plan.is_some() {
                            "plan"
                        } else {
                            "ask"
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
            let answers = ask
                .answer
                .as_ref()
                .and_then(|a| a["answers"].as_object())
                .map(|all| {
                    // In the order they were asked.
                    view.questions
                        .iter()
                        .filter_map(|q| all.get(&q.question).and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("; ")
                });
            let option = ask.answered_option();
            let text = if let Some(option) = option {
                option.name.clone()
            } else {
                match (answers, answer) {
                    (Some(answers), _) => answers,
                    (None, Some("allow")) if view.plan.is_some() => "Carried out".into(),
                    (None, Some("allow")) => "Allowed".into(),
                    (None, Some("deny")) if !view.questions.is_empty() => "Skipped".into(),
                    (None, Some("deny")) if view.plan.is_some() => "Kept planning".into(),
                    (None, Some("deny")) => "Denied".into(),
                    _ => "No longer waiting".to_string(),
                }
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
        let mut ready = true;
        let mut answers = serde_json::Map::new();
        for (qi, question) in view.questions.iter().enumerate() {
            let key = question_key(&ask.id, &question.question);
            let answer = self.answer_of(&key, cx);
            ready &= !answer.is_empty();
            answers.insert(question.question.clone(), Value::String(answer));
            let picked = self.picks.get(&key).cloned().unwrap_or_default();
            let multi = question.multi_select;
            let options = question
                .options
                .iter()
                .enumerate()
                .map(|(oi, (label, desc))| {
                    let on = picked.contains(label);
                    let key = key.clone();
                    let label_owned = label.clone();
                    let hover = p.faint;
                    v_flex()
                        .id(SharedString::from(format!("q-{}-{qi}-{oi}", ask.id)))
                        .px(px(9.))
                        .py(px(5.))
                        .rounded(px(RADIUS))
                        .border_1()
                        .bg(p.bg)
                        .cursor_pointer()
                        .map(|d| {
                            if on {
                                d.border_color(p.accent).bg(p.accent_soft)
                            } else {
                                d.border_color(p.line).hover(move |d| d.border_color(hover))
                            }
                        })
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
                            this.pick_option(key.clone(), label_owned.clone(), multi, cx)
                        }))
                });
            let other = self.others.get(&key).cloned();
            card = card.child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(question.question.clone()),
                            )
                            .when_some(question.header.clone(), |d, h| {
                                d.child(
                                    div()
                                        .px(px(6.))
                                        .rounded(px(5.))
                                        .bg(p.ink(7.))
                                        .text_size(px(look::SMALL))
                                        .text_color(p.muted)
                                        .child(h),
                                )
                            }),
                    )
                    .when(multi, |d| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child("Pick any that apply"),
                        )
                    })
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .children(options)
                            .when_some(other, |d, other| d.child(Input::new(&other).small())),
                    ),
            );
        }
        if !view.questions.is_empty() {
            let id = ask.id.clone();
            let send = json!({"behavior": "allow", "answers": answers});
            card = card.child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .id("q-answer")
                            .h(px(28.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(RADIUS))
                            .bg(p.accent)
                            .text_color(p.primary_text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(look::SMALL + 1.))
                            .cursor_pointer()
                            .child("Answer")
                            .when(!ready || sent, |d| d.opacity(0.5))
                            .when(ready && !sent, |d| d.on_click(Self::reply(id.clone(), send, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("q-skip")
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(RADIUS))
                            .border_1()
                            .border_color(p.line)
                            .bg(p.bg)
                            .text_size(px(look::SMALL + 1.))
                            .cursor_pointer()
                            .child("Skip")
                            .on_click(Self::reply(
                                id,
                                json!({"behavior": "deny", "message": "The user skipped your questions in Sandhill. Ask them in the chat instead."}),
                                cx,
                            )),
                    ),
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
                                .text_color(p.primary_text)
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
            if let Some(options) = &ask.options {
                for (index, option) in options.iter().enumerate() {
                    buttons = buttons.child(
                        button(
                            &format!("approval-{index}"),
                            &option.name,
                            option.kind == "allow_once",
                            option.rejects(),
                        )
                        .on_click(Self::reply(
                            ask.id.clone(),
                            option.answer(),
                            cx,
                        )),
                    );
                }
                return card.child(buttons).into_any_element();
            }
            let id = &ask.id;
            buttons = buttons.child(
                button(
                    "allow",
                    if plan {
                        "Yes, carry it out"
                    } else {
                        "Allow once"
                    },
                    true,
                    false,
                )
                .on_click(Self::reply(
                    id.clone(),
                    json!({"behavior": "allow", "scope": "once"}),
                    cx,
                )),
            );
            if plan && !ask.remember.is_empty() {
                buttons = buttons.child(
                    button("allow-edits", "Yes, and allow edits", false, false).on_click(
                        Self::reply(
                            id.clone(),
                            json!({"behavior": "allow", "scope": "chat"}),
                            cx,
                        ),
                    ),
                );
            }
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
                    !plan,
                )
                .on_click(Self::reply(
                    id.clone(),
                    if plan {
                        json!({"behavior": "deny", "message": "The user wants to keep planning. Ask them what to change in the plan."})
                    } else {
                        json!({"behavior": "deny"})
                    },
                    cx,
                )),
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

/// Shared integration artwork, including the user-supplied provider marks.
/// One-colour marks (OpenAI's for Codex, Synthetic Sciences' for
/// OpenScience) follow the text colour, so they read on light and dark
/// without a background; the rest keep their colours.
pub(crate) fn provider(kind: &str) -> Provider {
    let name = match kind {
        "claude" | "codex" | "antigravity" | "openscience" | "omp" | "pi" | "posit" => kind,
        _ => "generic",
    };
    Provider {
        path: format!("agents/{name}"),
        ink: matches!(name, "codex" | "openscience"),
        size: px(14.),
    }
}

#[derive(IntoElement)]
pub(crate) struct Provider {
    path: String,
    ink: bool,
    size: Pixels,
}

impl Provider {
    pub(crate) fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for Provider {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if self.ink {
            crate::look::ink_picture(&self.path)
                .size(self.size)
                .into_any_element()
        } else {
            picture(&self.path).size(self.size).into_any_element()
        }
    }
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
        "list_variables" | "get_outputs" | "view_table" => "table",
        "list_sessions" | "get_console" => "sessions",
        "get_plots" => "zoom",
        "get_help" => "help",
        "list_packages" => "package",
        "delete_cell" => "delete",
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

/// The copy button in the corner of a reply's code block.
fn code_copy_button(
    block: &gpui_kit::base::text::CodeBlock,
    _: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    use std::hash::{Hash as _, Hasher as _};
    let code = block.code();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hash);
    icon_button(
        ("chat-code-copy", hash.finish()),
        "edit.copy",
        Palette::get(cx),
        false,
    )
    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Copy").build(w, cx))
    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.to_string())))
}

fn primary_button(id: &'static str, label: &'static str, p: &Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .gap(px(5.))
        .px(px(12.))
        .py(px(4.))
        .rounded(px(RADIUS))
        .bg(p.accent)
        .text_color(p.primary_text)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(icon("add").size(px(14.)))
        .child(label)
}

/// "16:42", or "Oct 9" for another day.
pub(crate) fn time_label(ms: i64) -> String {
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
