//! The context panel for Claude Code and Codex (Huan, 2026-11-16;
//! `docs/design/done/context-plan.md`), opened from the ring in the message
//! box: how full the conversation is, what the last request was made of,
//! the conversation's totals, Codex's instruction files, and what can be
//! done with it: compact, Claude Code's full breakdown, Codex's undo and
//! continue as new, and a new conversation with a summary. Other agents
//! keep the plain popup.

use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::TerminalInfo;
use serde_json::{Value, json};

use crate::agents::chat::Item;
use crate::agents::composer::{self, tokens};
use crate::agents::composer_view::popup;
use crate::agents::{Agents, Choice, Show, kind_of};
use crate::look::{self, Palette};

/// What the agent is asked for, to continue in a new conversation.
const SUMMARY_PROMPT: &str = "Write a summary of this conversation so it can continue in a new one: the goal, the decisions made, the current state, the files involved and the next steps. Reply with only the summary.";
/// How long the agent may take over it.
const SUMMARY_LIMIT: Duration = Duration::from_secs(600);
const SUMMARY_POLL: Duration = Duration::from_millis(700);

/// The agents with the full panel.
pub(crate) fn has_panel(kind: &str) -> bool {
    matches!(kind, "claude" | "codex")
}

fn count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|n| n.max(0.).round() as u64))
}

/// "48s", "6m 12s", "1h 04m".
pub(crate) fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s % 3600 / 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Token counts with a decimal below 100K: "3.1K", "71.2K", "412K", "1.2M".
pub(crate) fn fine(n: u64) -> String {
    if (1000..100_000).contains(&n) {
        let k = format!("{:.1}", n as f64 / 1000.);
        format!("{}K", k.trim_end_matches(".0"))
    } else {
        tokens(n)
    }
}

/// "$1.84", "$0.07", "<$0.01".
pub(crate) fn dollars(usd: f64) -> String {
    if usd > 0. && usd < 0.005 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

/// The text the agent replied with after the last message the user sent.
fn last_reply(items: &[Item]) -> String {
    let start = items
        .iter()
        .rposition(|i| matches!(i, Item::User { .. }))
        .map_or(0, |at| at + 1);
    items[start..]
        .iter()
        .filter_map(|i| match i {
            Item::Reply { text, .. } => Some(text.trim()),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn heading(text: &'static str, p: &Palette) -> Div {
    div()
        .text_size(px(look::SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.muted)
        .child(text)
}

fn rule(p: &Palette) -> Div {
    div().h(px(1.)).my(px(12.)).bg(p.line)
}

/// A row of the last request: a dot in its colour in the bar, the name,
/// the number at the right.
fn part_row(dot: Div, label: &'static str, value: u64, p: &Palette) -> Div {
    h_flex()
        .h(px(22.))
        .gap(px(8.))
        .child(div().w(px(8.)).flex_none().child(dot))
        .child(div().flex_1().child(label))
        .child(div().text_color(p.text).child(fine(value)))
}

fn filled(color: Hsla) -> Div {
    div().size(px(8.)).rounded_full().bg(color)
}

fn ringed(color: Hsla) -> Div {
    div()
        .size(px(8.))
        .rounded_full()
        .border_1()
        .border_color(color)
}

/// A total: its number large, what it is small under it.
fn tile(value: AnyElement, label: String, p: &Palette) -> Div {
    v_flex()
        .w(relative(1. / 3.))
        .min_w(px(96.))
        .gap(px(1.))
        .pb(px(8.))
        .child(
            div()
                .text_size(px(look::TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(
            div()
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .child(label),
        )
}

fn note(text: &'static str, p: &Palette) -> Div {
    div()
        .pt(px(6.))
        .text_size(px(look::SMALL))
        .text_color(p.muted)
        .child(text)
}

impl Agents {
    pub(crate) fn render_context_panel(
        &self,
        t: &TerminalInfo,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let status = t
            .agent
            .as_ref()
            .map(|a| a["status"].clone())
            .unwrap_or_default();
        let codex = t.kind == "codex";
        let busy = self.working(t);
        let percent = status["context"].as_f64().unwrap_or(0.).clamp(0., 100.);
        let size = count(&status["context_size"]).filter(|s| *s > 0);
        let used = size.map(|s| (s as f64 * percent / 100.).round() as u64);
        let level = if percent >= 90. {
            p.err
        } else if percent >= 70. {
            p.warn
        } else {
            p.text
        };
        let name = kind_of(&t.kind).map_or("Agent", |k| k.name);
        let last = &status["last"];
        let total = &status["total"];

        // How full: one large number, then the bar made of the last
        // request's parts.
        let read = p.accent;
        let written = p.accent.opacity(0.55);
        let fresh = p.accent.opacity(0.35);
        let parts: Vec<(&str, Hsla)> = vec![
            ("cache_read", read),
            ("cache_write", written),
            ("input", fresh),
        ];
        let mut bar = h_flex()
            .h(px(8.))
            .w_full()
            .rounded_full()
            .overflow_hidden()
            .bg(p.ink(10.));
        let measured: Vec<(f32, Hsla)> = size
            .map(|s| {
                parts
                    .iter()
                    .filter_map(|(key, color)| {
                        count(&last[*key])
                            .filter(|n| *n > 0)
                            .map(|n| ((n as f64 / s as f64).min(1.) as f32, *color))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if measured.is_empty() {
            bar = bar.child(div().h_full().w(relative((percent / 100.) as f32)).bg(read));
        } else {
            for (share, color) in measured {
                bar = bar.child(div().h_full().w(relative(share)).bg(color));
            }
        }
        let free = size
            .zip(used)
            .map(|(s, u)| format!("{} free", tokens(s.saturating_sub(u))));

        let mut panel = popup(p)
            .id("context-menu")
            .w(px(360.))
            .gap(px(0.))
            .p(px(14.))
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Context"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(p.muted)
                            .child(format!("{name} · {}", composer::model_label(t))),
                    ),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap(px(6.))
                    .pt(px(10.))
                    .pb(px(8.))
                    .child(
                        div()
                            .text_size(px(look::TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_height(px(28.))
                            .child(used.map_or(format!("{}%", percent.round()), tokens)),
                    )
                    .when_some(size, |d, s| {
                        d.child(
                            div()
                                .pb(px(2.))
                                .text_color(p.muted)
                                .child(format!("of {}", tokens(s))),
                        )
                    })
                    .child(div().flex_1())
                    .when(size.is_some(), |d| {
                        d.child(
                            div()
                                .pb(px(2.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(level)
                                .child(format!("{}%", percent.round())),
                        )
                    }),
            )
            .child(bar)
            .when_some(free, |d, free| {
                d.child(
                    div()
                        .pt(px(6.))
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(free),
                )
            });

        // The last request, part by part; what wasn't reported isn't shown.
        let rows: Vec<Div> = [
            ("cache_read", "Read from cache", filled(read)),
            ("cache_write", "Written to cache", filled(written)),
            ("input", "New input", filled(fresh)),
            ("output", "Reply", ringed(p.muted)),
            ("reasoning", "Reasoning", ringed(p.faint)),
        ]
        .into_iter()
        .filter_map(|(key, label, dot)| {
            count(&last[key])
                .filter(|n| *n > 0 || key == "input")
                .map(|n| part_row(dot, label, n, p))
        })
        .collect();
        if !rows.is_empty() {
            panel = panel
                .child(rule(p))
                .child(heading("Last request", p).pb(px(4.)))
                .children(rows);
        }

        // The conversation's totals.
        let mut tiles: Vec<Div> = Vec::new();
        let number = |text: String| div().child(text).into_any_element();
        if let Some(n) = count(&total["input"]) {
            tiles.push(tile(number(fine(n)), "tokens in".into(), p));
        }
        if let Some(n) = count(&total["output"]) {
            tiles.push(tile(number(fine(n)), "tokens out".into(), p));
        }
        if let Some(n) = count(&total["reasoning"]).filter(|n| *n > 0) {
            tiles.push(tile(number(fine(n)), "reasoning".into(), p));
        }
        if let Some(usd) = status["cost_usd"].as_f64() {
            tiles.push(tile(number(dollars(usd)), "at API prices".into(), p));
        }
        if let Some(ms) = count(&status["duration_ms"]) {
            let label = count(&status["api_duration_ms"]).map_or("so far".to_string(), |api| {
                format!("{} thinking", duration(api))
            });
            tiles.push(tile(number(duration(ms)), label, p));
        }
        let added = count(&status["lines_added"]);
        let removed = count(&status["lines_removed"]);
        if added.is_some() || removed.is_some() {
            let lines = h_flex()
                .gap(px(6.))
                .child(
                    div()
                        .text_color(p.ok)
                        .child(format!("+{}", added.unwrap_or(0))),
                )
                .child(
                    div()
                        .text_color(p.err)
                        .child(format!("−{}", removed.unwrap_or(0))),
                )
                .into_any_element();
            tiles.push(tile(lines, "lines changed".into(), p));
        }
        if !tiles.is_empty() {
            panel = panel
                .child(rule(p))
                .child(heading("This conversation", p).pb(px(8.)))
                .child(h_flex().flex_wrap().items_start().children(tiles));
        }

        // Codex's instruction files: a project file opens.
        let files: Vec<String> = status["instructions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if !files.is_empty() {
            let rows = files.into_iter().enumerate().map(|(i, path)| {
                let inside = self.project_file(&path).is_some();
                let file = path.rsplit('/').next().unwrap_or(&path).to_string();
                let place = if inside {
                    "this project"
                } else if path.contains("/.codex/") {
                    "your Codex folder"
                } else {
                    "elsewhere"
                };
                h_flex()
                    .id(("instructions", i))
                    .h(px(22.))
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(if inside { p.accent } else { p.text })
                            .when(inside, |d| d.hover(|d| d.underline()))
                            .child(file),
                    )
                    .child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.muted)
                            .child(place),
                    )
                    .when(inside, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.menu = None;
                                this.open_path(&path, cx);
                            }))
                    })
            });
            panel = panel
                .child(rule(p))
                .child(heading("Instructions it read", p).pb(px(4.)))
                .children(rows);
        }

        // What can be done with it.
        let summarizing = self.summarizing.as_deref() == Some(t.id.as_str());
        let ready = t.running && !busy && self.summarizing.is_none();
        let button = |id, label, primary| {
            look::button(id, label, None, look::ButtonSize::Card)
                .when(primary, |button| {
                    button.primary().font_weight(FontWeight::SEMIBOLD)
                })
                .disabled(!ready)
        };
        let on =
            |button: Button,
             f: fn(&mut Agents, &TerminalInfo, &mut Window, &mut Context<Agents>)| {
                let t = t.clone();
                button.on_click(cx.listener(move |this, _, window, cx| f(this, &t, window, cx)))
            };
        let summary_label = if summarizing {
            "Writing a summary…"
        } else {
            "New conversation with a summary"
        };
        panel = panel.child(rule(p));
        if codex {
            panel = panel
                .child(on(button("ctx-compact", "Compact", true), Agents::compact_from_panel))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .pt(px(8.))
                        .child(
                            on(button("ctx-undo", "Undo last exchange", false), Agents::undo_exchange)
                                .flex_1(),
                        )
                        .child(
                            on(button("ctx-fork", "Continue as new", false), Agents::continue_as_new)
                                .flex_1(),
                        ),
                )
                .child(
                    on(button("ctx-summary", summary_label, false), Agents::new_with_summary)
                        .mt(px(8.)),
                )
                .child(note(
                    "Undo takes the last message and its reply out of the chat. Continue as new copies this conversation into a new one.",
                    p,
                ));
        } else {
            panel = panel
                .child(
                    h_flex()
                        .gap(px(8.))
                        .when_some(self.compact_focus.clone(), |d, focus| {
                            d.child(div().flex_1().min_w_0().child(Input::new(&focus).small()))
                        })
                        .child(on(
                            button("ctx-compact", "Compact", true),
                            Agents::compact_from_panel,
                        )),
                )
                .child(note(
                    "Compact replaces the conversation with a summary, to make room.",
                    p,
                ))
                .child(
                    on(
                        button("ctx-breakdown", "Full breakdown ↗", false),
                        Agents::full_breakdown,
                    )
                    .mt(px(10.)),
                )
                .child(
                    on(
                        button("ctx-summary", summary_label, false),
                        Agents::new_with_summary,
                    )
                    .mt(px(8.)),
                )
                .child(note(
                    "Full breakdown runs /context on Claude's own screen.",
                    p,
                ));
        }
        if t.running && busy && !summarizing {
            panel = panel.child(note("These wait until the agent finishes its reply.", p));
        }
        panel
    }

    /// Compact, with Claude Code's "keep in mind" when one was written.
    fn compact_from_panel(
        &mut self,
        t: &TerminalInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self
            .compact_focus
            .as_ref()
            .map(|f| f.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if t.kind == "claude" && !focus.is_empty() {
            self.menu = None;
            self.command(t, format!("/compact {focus}"), cx);
            if let Some(f) = &self.compact_focus {
                f.update(cx, |f, cx| f.set_value("", window, cx));
            }
            cx.notify();
        } else {
            self.compact(cx);
        }
    }

    /// Claude Code's own breakdown: its screen, running /context.
    fn full_breakdown(&mut self, t: &TerminalInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.show(Show::Terminal, window, cx);
        self.command(t, "/context".into(), cx);
        cx.notify();
    }

    fn undo_exchange(&mut self, t: &TerminalInfo, _: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.tell(t, json!({"kind": "undo"}));
        cx.notify();
    }

    /// A new Codex conversation forked from this one, with its history.
    fn continue_as_new(&mut self, t: &TerminalInfo, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = kind_of(&t.kind).map(|k| k.kind) else {
            return;
        };
        self.menu = None;
        let mut options = self.resume_options(t);
        options.fork = Some(t.id.clone());
        self.start(kind, None, options, window, cx);
    }

    /// Asks the agent for a summary in this conversation; when it has
    /// replied, opens a new conversation with the same agent, model and mode
    /// and the summary attached.
    fn new_with_summary(&mut self, t: &TerminalInfo, window: &mut Window, cx: &mut Context<Self>) {
        let after = self
            .chats
            .get(&t.id)
            .and_then(|c| c.last())
            .map_or(0, |e| e.seq);
        if !self.send(t, SUMMARY_PROMPT, &[], cx) {
            return;
        }
        self.menu = None;
        self.summarizing = Some(t.id.clone());
        cx.notify();
        let id = t.id.clone();
        let began = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SUMMARY_POLL).await;
                let Ok(outcome) = this.update(cx, |this, _| this.summary_outcome(&id, after))
                else {
                    return;
                };
                let outcome = match outcome {
                    Some(outcome) => outcome,
                    None if began.elapsed() < SUMMARY_LIMIT => continue,
                    None => Err("The summary took too long.".to_string()),
                };
                this.update_in(cx, |this, window, cx| {
                    this.summarizing = None;
                    match outcome {
                        Ok((t, text)) => this.open_with_summary(&t, text, window, cx),
                        Err(why) => this.problem = Some(why),
                    }
                    cx.notify();
                })
                .ok();
                return;
            }
        })
        .detach();
    }

    /// The summary, once the agent has replied to the request sent after
    /// entry `after` and is idle; `None` while it is still writing.
    fn summary_outcome(
        &self,
        id: &str,
        after: u64,
    ) -> Option<Result<(TerminalInfo, String), String>> {
        let Some(t) = self.terminals.iter().find(|t| t.id == id) else {
            return Some(Err(
                "The conversation closed before its summary came.".into()
            ));
        };
        if !t.running {
            return Some(Err("The conversation ended before its summary came.".into()));
        }
        let asked = self.chats.get(id).is_some_and(|c| {
            c.iter()
                .any(|e| e.seq > after && e.event["kind"] == "prompt")
        });
        if !asked || self.working(t) {
            return None;
        }
        let text = last_reply(&self.chat(t).items);
        Some(if text.is_empty() {
            Err("The agent didn't reply with a summary.".into())
        } else {
            Ok((t.clone(), text))
        })
    }

    fn open_with_summary(
        &mut self,
        t: &TerminalInfo,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = kind_of(&t.kind).map(|k| k.kind) else {
            return;
        };
        let title = self.chat(t).title.unwrap_or_else(|| "conversation".into());
        let slug: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .take(6)
            .collect::<Vec<_>>()
            .join("-");
        let name = format!(
            "summary-{}.md",
            if slug.is_empty() {
                "conversation"
            } else {
                &slug
            }
        );
        self.show(Show::New, window, cx);
        self.new_kind = kind;
        let models = self.models_of(kind);
        self.new_choice = Choice {
            model: composer::model_id(t).map(|id| models.choice(&id)),
            effort: composer::effort_of(t),
            mode: composer::current_mode(t).map(|m| m.permission),
            console: Some(composer::code_to_console(t)),
        };
        self.attach_text(name, format!("# Summary of “{title}”\n\n{text}\n"), cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, dollars, duration, fine, last_reply};
    use serde_json::Value;

    #[test]
    fn times_and_costs_read_at_a_glance() {
        assert_eq!(duration(48_000), "48s");
        assert_eq!(duration(372_000), "6m 12s");
        assert_eq!(duration(3_840_000), "1h 04m");
        assert_eq!(dollars(1.844), "$1.84");
        assert_eq!(dollars(0.001), "<$0.01");
        assert_eq!(fine(71_200), "71.2K");
        assert_eq!(fine(3_000), "3K");
        assert_eq!(fine(412_300), "412K");
    }

    #[test]
    fn the_summary_is_the_reply_after_the_last_message() {
        let items = vec![
            Item::User {
                key: "u1".into(),
                text: "first".into(),
                place: Value::Null,
            },
            Item::Reply {
                key: "r1".into(),
                text: "old".into(),
            },
            Item::User {
                key: "u2".into(),
                text: "summarise".into(),
                place: Value::Null,
            },
            Item::Reply {
                key: "r2".into(),
                text: "Goal: x".into(),
            },
            Item::Reply {
                key: "r3".into(),
                text: "Next: y".into(),
            },
        ];
        assert_eq!(last_reply(&items), "Goal: x\n\nNext: y");
    }
}
