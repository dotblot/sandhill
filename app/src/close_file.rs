//! Closing a file whose kernel or AI conversations still run: one card,
//! the quit card's style, asks whether to shut the kernel down and which
//! conversations to stop, with the save question folded in. Nothing is
//! ticked at first, so closing keeps background work, as quitting does by
//! default (Huan, 2026-11-15; `docs/design/done/close-file-plan.md`).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, SessionStatus, TerminalInfo};
use serde_json::Value;

use crate::files::base_name;
use crate::look::{self, Palette};
use crate::workbench::Workbench;

/// The question shown while a file closes.
pub(crate) struct CloseAsk {
    path: String,
    dirty: bool,
    kernel: Option<KernelLink>,
    conversations: Vec<ConversationLink>,
    focus: FocusHandle,
}

struct KernelLink {
    id: String,
    /// "Python 3.12 (omicverse)".
    label: String,
    busy: bool,
    /// The other open files (and the Console) on the same kernel.
    others: Vec<String>,
    ticked: bool,
}

struct ConversationLink {
    id: String,
    title: String,
    /// "Codex".
    agent: String,
    working: bool,
    ticked: bool,
}

/// Whether a conversation is about the file: pinned to it, given it at
/// the start, working in it now, or with it @-mentioned in any message.
/// `names` are the file's path relative to the project and in full.
fn mentions(t: &TerminalInfo, entries: &[crate::agents::chat::Entry], names: &[&str]) -> bool {
    let is = |v: &Value| v.as_str().is_some_and(|s| names.contains(&s));
    let report = t.agent.as_ref();
    let pinned = report.is_some_and(|a| is(&a["pin"]))
        || t.options.pin.as_deref().is_some_and(|p| names.contains(&p));
    let working_in = report.is_some_and(|a| is(&a["document"]));
    let given = t
        .options
        .refs
        .iter()
        .any(|r| r["kind"] == "file" && is(&r["path"]));
    let mentioned = entries.iter().any(|e| {
        e.event["kind"] == "prompt"
            && e.event["place"]["refs"]
                .as_array()
                .is_some_and(|refs| refs.iter().any(is))
    });
    pinned || working_in || given || mentioned
}

impl Workbench {
    /// The kernel and running conversations a file's closing could stop;
    /// `None` when there are none, so it closes as before.
    pub(crate) fn close_ask(
        &self,
        path: &str,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> Option<CloseAsk> {
        let name = base_name(path).to_string();
        let kernel = self
            .open
            .iter()
            .chain(&self.others)
            .find(|o| o.notebook.path == path)
            .and_then(|o| o.session.id())
            .and_then(|id| {
                self.machine
                    .sessions
                    .iter()
                    .find(|s| s.id == id && s.status != SessionStatus::Ended)
            })
            .map(|s| {
                let mut others = self.used_by(&s.id);
                if let Some(me) = others.iter().position(|n| *n == name) {
                    others.remove(me);
                }
                KernelLink {
                    id: s.id.clone(),
                    label: crate::machine::session_label(s, &self.kernels),
                    busy: s.pending_runs > 0,
                    others,
                    ticked: false,
                }
            });
        let relative = self
            .project
            .as_deref()
            .and_then(|p| path.strip_prefix(&format!("{}/", p.trim_end_matches('/'))))
            .unwrap_or(path)
            .to_string();
        let names = [relative.as_str(), path];
        let conversations: Vec<ConversationLink> = self
            .agents
            .as_ref()
            .map(|agents| {
                let agents = agents.read(cx);
                agents
                    .terminals
                    .iter()
                    .filter(|t| t.running && mentions(t, agents.entries(&t.id), &names))
                    .map(|t| ConversationLink {
                        id: t.id.clone(),
                        title: agents.title(t),
                        agent: crate::agents::kind_of(&t.kind)
                            .map_or_else(|| t.title.clone(), |k| k.name.to_string()),
                        working: agents.working(t),
                        ticked: false,
                    })
                    .collect()
            })
            .unwrap_or_default();
        if kernel.is_none() && conversations.is_empty() {
            return None;
        }
        Some(CloseAsk {
            path: path.to_string(),
            dirty,
            kernel,
            conversations,
            focus: cx.focus_handle(),
        })
    }

    /// Shows the card for closing a file, taking the keyboard.
    pub(crate) fn ask_close_with(
        &mut self,
        ask: CloseAsk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        ask.focus.focus(window, cx);
        self.closing = Some(ask);
        cx.notify();
    }

    /// The card's answer: `save` (or not) and close, stopping what was
    /// ticked; none cancels.
    fn answer_close(&mut self, save: Option<bool>, cx: &mut Context<Self>) {
        let Some(ask) = self.closing.take() else {
            return;
        };
        cx.notify();
        let Some(save) = save else {
            return;
        };
        if let Some(kernel) = ask.kernel.filter(|k| k.ticked) {
            self.shut_down(vec![kernel.id], cx);
        }
        if let Some(daemon) = self.daemon() {
            for conversation in ask.conversations.iter().filter(|c| c.ticked) {
                // Stopped, not forgotten: it stays in the AI pane's history.
                daemon.fire(Call::StopTerminal {
                    terminal: conversation.id.clone(),
                });
            }
        }
        if !save && let Some(open) = self.open_at(&ask.path) {
            open.dirty = false;
        }
        self.close_tab(&ask.path, cx);
    }

    pub(crate) fn render_close_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ask = self.closing.as_ref()?;
        let p = *Palette::get(cx);
        let name = base_name(&ask.path).to_string();
        let conversations = ask.conversations.len();
        let running = match (&ask.kernel, conversations) {
            (Some(_), 0) => "Its kernel is still running.".to_string(),
            (None, 1) => "An AI conversation about it is still running.".to_string(),
            (None, n) => format!("{n} AI conversations about it are still running."),
            (Some(_), 1) => "Its kernel and an AI conversation about it are still running.".into(),
            (Some(_), n) => {
                format!("Its kernel and {n} AI conversations about it are still running.")
            }
        };
        let choice = |id: SharedString,
                      ticked: bool,
                      label: String,
                      note: Option<String>,
                      toggle: Box<dyn Fn(&mut CloseAsk)>,
                      cx: &mut Context<Self>| {
            h_flex()
                .items_start()
                .gap(px(8.))
                .child(Checkbox::new(id).checked(ticked).on_click(cx.listener(
                    move |this, _: &bool, _, cx| {
                        if let Some(ask) = this.closing.as_mut() {
                            toggle(ask);
                        }
                        cx.notify();
                    },
                )))
                .child(v_flex().min_w_0().child(label).children(note.map(|note| {
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(note)
                })))
        };
        let mut choices = v_flex().gap(px(10.));
        if let Some(kernel) = &ask.kernel {
            let note = match (kernel.busy, kernel.others.is_empty()) {
                (true, true) => Some("Running code now".to_string()),
                (true, false) => Some(format!(
                    "Running code now · also used by {}",
                    kernel.others.join(", ")
                )),
                (false, false) => Some(format!("Also used by {}", kernel.others.join(", "))),
                (false, true) => None,
            };
            choices = choices.child(choice(
                "close-kernel".into(),
                kernel.ticked,
                format!("Shut down {}", kernel.label),
                note,
                Box::new(|ask| {
                    if let Some(k) = ask.kernel.as_mut() {
                        k.ticked = !k.ticked;
                    }
                }),
                cx,
            ));
        }
        for (ix, conversation) in ask.conversations.iter().enumerate() {
            choices = choices.child(choice(
                SharedString::from(format!("close-conversation-{ix}")),
                conversation.ticked,
                format!("Stop \u{201c}{}\u{201d}", conversation.title),
                Some(format!(
                    "{} · {}",
                    conversation.agent,
                    if conversation.working {
                        "working"
                    } else {
                        "waiting for a message"
                    }
                )),
                Box::new(move |ask| {
                    if let Some(c) = ask.conversations.get_mut(ix) {
                        c.ticked = !c.ticked;
                    }
                }),
                cx,
            ));
        }
        let dirty = ask.dirty;
        let wide = |button: Button| button.w_full();
        let card = v_flex()
            .id("close-card")
            .track_focus(&ask.focus)
            .occlude()
            .w(px(380.))
            .max_w(relative(0.95))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .p(px(20.))
            .gap(px(14.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.answer_close(None, cx),
                    "enter" => this.answer_close(Some(true), cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .gap(px(12.))
                    .child(
                        img(crate::start_screen::brand_icon())
                            .size(px(40.))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Close {name}?")),
                    ),
            )
            .child(div().text_color(p.muted).child(running))
            .child(choices)
            .child(
                v_flex()
                    .gap(px(8.))
                    .child(wide(
                        Button::new("close-file")
                            .primary()
                            .label(if dirty { "Save and Close" } else { "Close" })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.answer_close(Some(true), cx)),
                            ),
                    ))
                    .when(dirty, |d| {
                        d.child(wide(
                            Button::new("close-discard").label("Don't Save").on_click(
                                cx.listener(|this, _, _, cx| this.answer_close(Some(false), cx)),
                            ),
                        ))
                    })
                    .child(wide(Button::new("close-cancel").label("Cancel").on_click(
                        cx.listener(|this, _, _, cx| this.answer_close(None, cx)),
                    ))),
            );
        Some(
            deferred(
                div()
                    .id("close-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.answer_close(None, cx);
                        }),
                    ),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::mentions;
    use crate::agents::chat::Entry;
    use sandhill_proto::TerminalInfo;
    use serde_json::json;

    fn conversation(agent: serde_json::Value, options: serde_json::Value) -> TerminalInfo {
        serde_json::from_value(json!({
            "id": "t1", "kind": "codex", "title": "Codex", "cwd": "/p", "running": true,
            "created_ms": 0, "end": 0, "agent": agent, "options": options
        }))
        .unwrap()
    }

    #[test]
    fn a_conversation_is_about_a_file_it_pins_was_given_works_in_or_mentions() {
        let names = ["qc.ipynb", "/p/qc.ipynb"];
        let none = conversation(json!({}), json!({}));
        assert!(!mentions(&none, &[], &names));
        assert!(mentions(
            &conversation(json!({}), json!({"pin": "qc.ipynb"})),
            &[],
            &names
        ));
        assert!(mentions(
            &conversation(json!({"pin": "qc.ipynb"}), json!({})),
            &[],
            &names
        ));
        assert!(mentions(
            &conversation(json!({"document": "/p/qc.ipynb"}), json!({})),
            &[],
            &names
        ));
        let given = json!({"refs": [{"kind": "file", "name": "qc.ipynb", "path": "qc.ipynb"}]});
        assert!(mentions(&conversation(json!({}), given), &[], &names));
        let said = |refs: serde_json::Value| Entry {
            seq: 1,
            event: json!({"kind": "prompt", "text": "look", "place": {"refs": refs}}),
        };
        assert!(mentions(&none, &[said(json!(["qc.ipynb"]))], &names));
        // Another file, or a chunk of this one named differently, is not it.
        assert!(!mentions(
            &none,
            &[said(json!(["de.ipynb", "qc.ipynb › markers"]))],
            &names
        ));
    }
}
