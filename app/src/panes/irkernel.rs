//! Offering IRkernel to R installations without it (see the daemon's
//! `irkernel`): an action under the kernel picker and in project settings,
//! a confirmation saying what will change, then the install as a job in the
//! Jobs tab. When the job ends, the kernel lists are read again.

use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, KernelspecInfo, RWithoutIrkernel, TerminalInfo};

use crate::look::{self, Palette, icon};
use crate::workbench::{ConsoleTab, Workbench};

#[derive(Default)]
pub(crate) struct IrkernelOffers {
    /// The R installations on the device without IRkernel.
    pub list: Vec<RWithoutIrkernel>,
    /// The one whose install is being confirmed.
    confirming: Option<RWithoutIrkernel>,
    starting: bool,
    error: Option<String>,
    /// Install jobs running; the kernels are read again when one ends.
    jobs: HashSet<String>,
}

/// "R 4.6.2", or "R" when the version is unknown.
pub(crate) fn r_name(offer: &RWithoutIrkernel) -> String {
    if offer.version.is_empty() {
        "R".into()
    } else {
        format!("R {}", offer.version)
    }
}

/// Where an offered R is: its environment, else its path.
pub(crate) fn r_place(offer: &RWithoutIrkernel) -> String {
    offer.env.clone().unwrap_or_else(|| offer.r.clone())
}

impl Workbench {
    /// Reads which R installations could have IRkernel added.
    pub(crate) fn load_irkernel_offers(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let project = self.project.clone();
        let listed = daemon.call::<Vec<RWithoutIrkernel>>(Call::ListRWithoutIrkernel {
            project: project.clone(),
        });
        cx.spawn(async move |this, cx| {
            // An older daemon doesn't know the call: nothing is offered.
            let listed = listed.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                if this.generation == generation && this.project == project {
                    this.irkernel.list = listed;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Reads the kernels and the R without IRkernel again, after an install.
    fn reload_kernels(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let project = self.project.clone();
        let kernels = match &project {
            Some(path) => daemon
                .call::<Vec<KernelspecInfo>>(Call::ListProjectKernelspecs { path: path.clone() }),
            None => daemon.call::<Vec<KernelspecInfo>>(Call::ListKernelspecs),
        };
        cx.spawn(async move |this, cx| {
            let kernels = kernels.await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project != project {
                    return;
                }
                if let Ok(kernels) = kernels {
                    if this.project.is_some() {
                        this.setup.kernels = kernels.clone();
                    }
                    this.kernels = kernels;
                }
                this.load_irkernel_offers(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn ask_install_irkernel(&mut self, offer: RWithoutIrkernel, cx: &mut Context<Self>) {
        self.kernel_menu = false;
        self.irkernel.confirming = Some(offer);
        self.irkernel.error = None;
        cx.notify();
    }

    fn cancel_install_irkernel(&mut self, cx: &mut Context<Self>) {
        if !self.irkernel.starting {
            self.irkernel.confirming = None;
            self.irkernel.error = None;
            cx.notify();
        }
    }

    fn install_irkernel(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(offer)) = (self.daemon(), self.irkernel.confirming.clone()) else {
            return;
        };
        if self.irkernel.starting {
            return;
        }
        self.irkernel.starting = true;
        self.irkernel.error = None;
        let generation = self.generation;
        let project = self.project.clone();
        let started = daemon.call::<TerminalInfo>(Call::InstallIrkernel {
            r: offer.r.clone(),
            project: project.clone(),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                this.irkernel.starting = false;
                if this.generation != generation || this.project != project {
                    return;
                }
                match started {
                    Ok(info) => {
                        this.irkernel.confirming = None;
                        this.irkernel.jobs.insert(info.id.clone());
                        this.show_started_job(info, cx);
                    }
                    Err(err) => this.irkernel.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// An install job ended: its kernel, if any, can now be picked.
    pub(crate) fn irkernel_job_ended(&mut self, terminal: &str, cx: &mut Context<Self>) {
        if self.irkernel.jobs.remove(terminal) {
            self.reload_kernels(cx);
        }
    }

    /// The picker's install actions, for kernels of `only`'s language.
    pub(crate) fn irkernel_menu_rows(
        &self,
        only: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if self.irkernel.list.is_empty() || only.is_some_and(|l| !l.eq_ignore_ascii_case("r")) {
            return Vec::new();
        }
        let p = *Palette::get(cx);
        let hover = p.ink(look::ROW_HOVER);
        let mut rows = vec![
            div()
                .mx(px(look::GAP_8))
                .my(px(look::GAP_6))
                .h(px(1.))
                .bg(p.line)
                .into_any_element(),
        ];
        for (ix, offer) in self.irkernel.list.iter().enumerate() {
            let picked = offer.clone();
            rows.push(
                h_flex()
                    .id(("install-irkernel", ix))
                    .h(px(look::ROW))
                    .gap(px(look::GAP_4))
                    .px(px(look::GAP_8))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(
                        div()
                            .w(px(look::ICON))
                            .flex_none()
                            .text_color(p.muted)
                            .child(icon("add").size(px(look::ICON))),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .line_height(px(look::ROW / 2.))
                            .whitespace_nowrap()
                            .child(format!("Install IRkernel for {}…", r_name(offer)))
                            .child(
                                div()
                                    .text_color(p.faint)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(r_place(offer)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ask_install_irkernel(picked.clone(), cx)
                    }))
                    .into_any_element(),
            );
        }
        rows
    }

    pub(crate) fn render_irkernel_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let offer = self.irkernel.confirming.as_ref()?;
        let p = *Palette::get(cx);
        let starting = self.irkernel.starting;
        let device = crate::welcome::start_screen::host_label(&self.host());
        let card = v_flex()
            .id("irkernel-card")
            .occlude()
            .w(px(520.))
            .max_w(relative(0.9))
            .gap(px(look::GAP_12))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(look::TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Install IRkernel for {}?", r_name(offer))),
            )
            .child(
                h_flex()
                    .gap(px(look::GAP_6))
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .child(device)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(look::MONO)
                            .child(offer.r.clone()),
                    ),
            )
            .child(div().whitespace_normal().child(offer.plan.clone()))
            .child(
                div()
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .whitespace_normal()
                    .child("It runs in the Jobs tab and goes on if Sandhill disconnects. Sandhill never removes it; uninstall it yourself if you no longer want it."),
            )
            .when_some(self.irkernel.error.clone(), |d, error| {
                d.child(div().text_color(p.err).whitespace_normal().child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .child(
                        Button::new("irkernel-cancel")
                            .small()
                            .label("Cancel")
                            .disabled(starting)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_install_irkernel(cx)
                            })),
                    )
                    .child(
                        Button::new("irkernel-install")
                            .small()
                            .primary()
                            .label(if starting { "Starting…" } else { "Install" })
                            .disabled(starting)
                            .on_click(cx.listener(|this, _, _, cx| this.install_irkernel(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("irkernel-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.cancel_install_irkernel(cx)),
                    )
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.cancel_install_irkernel(cx);
                            cx.stop_propagation();
                        }
                    })),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }

    /// Shows a job the daemon started in the Jobs tab, following its log.
    fn show_started_job(&mut self, info: TerminalInfo, cx: &mut Context<Self>) {
        self.console_tab = ConsoleTab::Jobs;
        self.show_console = true;
        self.jobs_started(info, cx);
    }
}
