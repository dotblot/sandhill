//! Trusting a project folder (Huan, 2026-11-12; plan in
//! `docs/design/folder-trust-plan.md`).
//!
//! The daemon says whether the folder is trusted when it opens the project,
//! and refuses to start anything in one that is not. Here: the question the
//! first time a folder is opened, Restricted Mode for a folder left
//! untrusted (files, saved outputs and previews only), and the way back.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::Call;

use crate::look::{self, Palette, icon};
use crate::workbench::Workbench;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Trust {
    /// Trusted, or no project is open.
    #[default]
    Trusted,
    /// Not trusted, and the question is up.
    Asking,
    /// Not trusted: Restricted Mode.
    Restricted,
}

#[derive(Default)]
pub(crate) struct FolderTrust {
    pub(crate) state: Trust,
    saving: bool,
    error: Option<String>,
}

impl FolderTrust {
    /// A project just opened: one the daemon does not trust is asked about,
    /// unless the question was answered "Restricted Mode" for this folder
    /// while the window stayed open (`answered`, as after a reconnect).
    pub(crate) fn opened(trusted: bool, answered: bool) -> Self {
        FolderTrust {
            state: match (trusted, answered) {
                (true, _) => Trust::Trusted,
                (false, true) => Trust::Restricted,
                (false, false) => Trust::Asking,
            },
            ..Default::default()
        }
    }
}

impl Workbench {
    /// Whether the open project waits for trust, so nothing starts in it.
    pub(crate) fn restricted(&self) -> bool {
        self.project.is_some() && self.trust.state != Trust::Trusted
    }

    /// In Restricted Mode nothing starts: says so, and returns true.
    pub(crate) fn held_back(&mut self, what: &str, cx: &mut Context<Self>) -> bool {
        if !self.restricted() {
            return false;
        }
        self.notice = Some(restricted_text(what));
        cx.notify();
        true
    }

    pub(crate) fn ask_trust(&mut self, cx: &mut Context<Self>) {
        if self.restricted() {
            self.trust.state = Trust::Asking;
            self.trust.error = None;
            cx.notify();
        }
    }

    fn stay_restricted(&mut self, cx: &mut Context<Self>) {
        if self.trust.saving {
            return;
        }
        self.trust.state = Trust::Restricted;
        self.trust.error = None;
        cx.notify();
    }

    /// Trusts the open project's folder on its device, then starts what
    /// waited for it.
    fn trust_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        if self.trust.saving {
            return;
        }
        self.trust.saving = true;
        self.trust.error = None;
        let generation = self.generation;
        let trusted = daemon.call::<()>(Call::TrustFolder {
            path: project.clone(),
        });
        cx.spawn_in(window, async move |this, cx| {
            let trusted = trusted.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation || this.project.as_deref() != Some(&project) {
                    return;
                }
                this.trust.saving = false;
                match trusted {
                    Ok(()) => {
                        this.trust.state = Trust::Trusted;
                        this.notice = None;
                        this.start_project(window, cx);
                    }
                    Err(err) => {
                        this.trust.error = Some(format!("Could not trust the folder: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// What a pane shows in place of its content in Restricted Mode. Panes
    /// whose tab bar floats over their content start below it.
    pub(crate) fn restricted_note(
        &self,
        what: &str,
        floating: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        v_flex()
            .size_full()
            .px(px(14.))
            .pt(px(if floating { look::STRIP + 10. } else { 10. }))
            .pb(px(10.))
            .gap(px(8.))
            .text_color(p.muted)
            .child(div().whitespace_normal().child(restricted_text(what)))
            .child(
                h_flex().child(
                    Button::new(SharedString::from(format!("trust-from-{what}")))
                        .small()
                        .label("Trust Folder…")
                        .on_click(cx.listener(|this, _, _, cx| this.ask_trust(cx))),
                ),
            )
            .into_any_element()
    }

    /// The status bar's mark of Restricted Mode; it asks again.
    pub(crate) fn render_restricted_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.restricted() {
            return None;
        }
        let p = *Palette::get(cx);
        Some(
            h_flex()
                .id("restricted-chip")
                .flex_none()
                .h(px(20.))
                .px(px(8.))
                .gap(px(5.))
                .rounded_full()
                .border_1()
                .border_color(p.warn.opacity(0.6))
                .text_size(px(look::SMALL))
                .text_color(p.text)
                .cursor_pointer()
                .hover(|d| d.bg(p.ink(6.)))
                .child(icon("lock").size(px(12.)).text_color(p.warn))
                .child("Restricted")
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        "Restricted Mode: nothing starts in this folder until you trust it",
                    )
                    .build(w, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.ask_trust(cx)))
                .into_any_element(),
        )
    }

    pub(crate) fn render_trust_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.trust.state != Trust::Asking {
            return None;
        }
        let project = self.project.clone()?;
        let p = *Palette::get(cx);
        let name = crate::files::base_name(&project).to_string();
        let host = self.host();
        let device = crate::app_window::machine_name(&host);
        let saving = self.trust.saving;
        let card = v_flex()
            .id("trust-card")
            .occlude()
            .w(px(520.))
            .max_w(relative(0.9))
            .gap(px(12.))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(icon("lock").size(px(16.)).text_color(p.muted))
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Do you trust the files in {name}?")),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .child(
                        icon(if host == sandhill_link::LOCAL {
                            "device.laptop"
                        } else {
                            "device.server"
                        })
                        .size(px(13.)),
                    )
                    .child(device)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(look::MONO)
                            .child(project),
                    ),
            )
            .child(
                div()
                    .whitespace_normal()
                    .child("Trusting it lets R, Python, the Terminal, jobs, Git and AI agents start here. They act on files in the folder, such as .Rprofile, env files and agent instructions, so trust only folders whose authors you trust."),
            )
            .child(
                div()
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .whitespace_normal()
                    .child("Restricted Mode shows files, saved outputs and previews, and starts nothing. Trust is remembered; an untrusted folder asks again when you next open it."),
            )
            .when_some(self.trust.error.clone(), |d, error| {
                d.child(div().text_color(p.err).whitespace_normal().child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("trust-restricted")
                            .small()
                            .label("Open in Restricted Mode")
                            .disabled(saving)
                            .on_click(cx.listener(|this, _, _, cx| this.stay_restricted(cx))),
                    )
                    .child(
                        Button::new("trust-folder")
                            .small()
                            .primary()
                            .label(if saving { "Trusting…" } else { "Trust Folder" })
                            .disabled(saving)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.trust_folder(window, cx)),
                            ),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("trust-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    // Dismissing it trusts nothing.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.stay_restricted(cx)),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => this.stay_restricted(cx),
                            "enter" => this.trust_folder(window, cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    })),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// "Restricted Mode: trust this folder to run code."
pub(crate) fn restricted_text(what: &str) -> String {
    format!("Restricted Mode: trust this folder to {what}.")
}
