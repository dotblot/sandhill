//! Where the panes sit: the left panel (a quarter of the window) holds
//! Variables, Kernels, Help and Packages over Files, Git, Plots and the
//! Viewer; the middle (half) has the editor with the Console, Terminal and
//! Jobs under it; the AI pane is the right panel (a quarter). The machine's
//! resources are at the right of the status bar.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::look::{Palette, icon_button, side_pane};
use crate::workbench::{BottomTab, ConsoleTab, Split, TopTab, Workbench};

/// The five independently maximizable pane groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    LeftTop,
    LeftBottom,
    Editor,
    Console,
    Agents,
}

impl Panel {
    fn top(self) -> bool {
        matches!(self, Self::LeftTop | Self::Editor)
    }

    fn id(self) -> &'static str {
        match self {
            Self::LeftTop => "left-top",
            Self::LeftBottom => "left-bottom",
            Self::Editor => "editor",
            Self::Console => "console",
            Self::Agents => "agents",
        }
    }
}

/// Keep the normal split untouched while either pane fills its column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Vertical {
    #[default]
    Split,
    Top,
    Bottom,
}

impl Vertical {
    fn expanded(self, top: bool) -> bool {
        matches!((self, top), (Self::Top, true) | (Self::Bottom, false))
    }

    fn collapsed(self, top: bool) -> bool {
        self != Self::Split && !self.expanded(top)
    }

    pub(crate) fn heights(self, available: f32, top_share: f32) -> (f32, f32) {
        let available = available.max(0.);
        let header = crate::look::STRIP.min(available / 2.);
        let top = match self {
            Self::Split => (available * top_share).clamp(header, available - header),
            Self::Top => available - header,
            Self::Bottom => header,
        };
        (top, available - top)
    }
}

impl Workbench {
    fn vertical(&self, panel: Panel) -> Vertical {
        match panel {
            Panel::LeftTop | Panel::LeftBottom => self.left_vertical,
            Panel::Editor | Panel::Console => self.middle_vertical,
            Panel::Agents => Vertical::Split,
        }
    }

    pub(crate) fn panel_collapsed(&self, panel: Panel) -> bool {
        if self.maximized_panel == Some(panel) {
            return false;
        }
        if matches!(panel, Panel::Editor) && !self.show_console {
            return false;
        }
        self.vertical(panel).collapsed(panel.top())
    }

    pub(crate) fn toggle_panel(&mut self, panel: Panel, maximize: bool, cx: &mut Context<Self>) {
        let vertical = match panel {
            Panel::LeftTop | Panel::LeftBottom => &mut self.left_vertical,
            Panel::Editor | Panel::Console => {
                self.show_console = true;
                &mut self.middle_vertical
            }
            Panel::Agents => return,
        };
        let top = panel.top();
        let restore = if maximize {
            vertical.expanded(top)
        } else {
            vertical.collapsed(top)
        };
        *vertical = if restore {
            Vertical::Split
        } else if top == maximize {
            Vertical::Top
        } else {
            Vertical::Bottom
        };
        cx.notify();
    }

    pub(crate) fn panel_buttons(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let collapsed = self.panel_collapsed(panel);
        let expanded = self.vertical(panel).expanded(panel.top());
        // Each icon shows what a click does: a short window (collapse to
        // the title row), a tall one (fill the column), or two overlapping
        // windows (back to the split), so a pane never shows two alike
        // (Huan, 2026-11-14).
        let button = |maximize: bool, restore: bool| {
            let tip = if restore {
                "Restore pane heights"
            } else if maximize {
                "Fill the column"
            } else {
                "Collapse to title row"
            };
            icon_button(
                SharedString::from(format!("panel-{}-{maximize}", panel.id())),
                if restore {
                    "window.restore"
                } else if maximize {
                    "window.maximize"
                } else {
                    "window.minimize"
                },
                &p,
                false,
            )
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(panel, maximize, cx)))
        };
        h_flex()
            .flex_none()
            .when(self.maximized_panel.is_none(), |d| {
                d.child(button(false, collapsed))
                    .child(button(true, expanded))
            })
            .child(self.maximize_button(panel, cx))
    }

    pub(crate) fn panel_visible(&self, panel: Panel) -> bool {
        if let Some(maximized) = self.maximized_panel {
            return maximized == panel;
        }
        match panel {
            Panel::LeftTop | Panel::LeftBottom => self.show_files,
            Panel::Editor => self.show_middle,
            Panel::Console => self.show_middle && self.show_console,
            Panel::Agents => self.show_agents && self.agents_available(),
        }
    }

    pub(crate) fn maximize_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.maximized_panel = (self.maximized_panel != Some(panel)).then_some(panel);
        self.dragging = None;
        cx.notify();
    }

    pub(crate) fn maximize_button(&self, panel: Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let maximized = self.maximized_panel == Some(panel);
        icon_button(
            SharedString::from(format!("maximize-{}", panel.id())),
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
        .on_click(cx.listener(move |this, _, _, cx| this.maximize_panel(panel, cx)))
    }
}

impl Workbench {
    /// The pane under the editor: the Console, the Terminal or Jobs.
    pub(crate) fn render_console_pane(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        let tab =
            |id: &'static str, label: &'static str, which: ConsoleTab, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.console_tab == which, &p).on_click(
                    cx.listener(move |this, _, window, cx| this.show_tab(which, window, cx)),
                )
            };
        let body: AnyElement = match self.console_tab {
            ConsoleTab::Console if self.restricted() => self.restricted_note("run code", true, cx),
            ConsoleTab::Terminal if self.restricted() => {
                self.restricted_note("use the Terminal", false, cx)
            }
            ConsoleTab::Jobs if self.restricted() => self.restricted_note("run jobs", true, cx),
            ConsoleTab::Console => self.render_console(window, cx),
            ConsoleTab::Terminal => self.render_terminal(cx),
            ConsoleTab::Jobs => self.render_jobs(cx),
        };
        let floating =
            self.console_tab != ConsoleTab::Terminal && !self.panel_collapsed(Panel::Console);
        let mut body =
            (!self.panel_collapsed(Panel::Console)).then(|| div().flex_1().min_h_0().child(body));
        side_pane(&p)
            .relative()
            .h(px(self.console_height))
            .flex_none()
            .when(self.maximized_panel == Some(Panel::Console), |d| {
                d.h_full().flex_1()
            })
            .when(floating, |d| d.children(body.take()))
            .child(
                Self::strip(&p)
                    .child(
                        crate::look::tab_capsule(&p)
                            .id("console-tabs")
                            .min_w_0()
                            .flex_shrink(1.)
                            .overflow_x_scroll()
                            .gap(px(1.))
                            .child(tab("tab-console", "Console", ConsoleTab::Console, cx))
                            .child(tab("tab-terminal", "Terminal", ConsoleTab::Terminal, cx))
                            .child(tab("tab-jobs", "Jobs", ConsoleTab::Jobs, cx)),
                    )
                    .when(
                        self.console_tab == ConsoleTab::Console
                            && !self.panel_collapsed(Panel::Console),
                        |d| d.child(self.render_console_tools(cx)),
                    )
                    .when(
                        self.console_tab != ConsoleTab::Console
                            || self.panel_collapsed(Panel::Console),
                        |d| d.child(div().flex_1()),
                    )
                    .when(
                        self.console_tab == ConsoleTab::Console
                            && !self.panel_collapsed(Panel::Console)
                            && self.console_target().is_some(),
                        |d| d.child(self.console_history_button(cx)),
                    )
                    .child(self.panel_buttons(Panel::Console, cx))
                    .map(|strip| {
                        if self.console_tab != ConsoleTab::Terminal
                            && !self.panel_collapsed(Panel::Console)
                        {
                            crate::look::floating_header(&p, 0., false, strip)
                        } else {
                            strip.into_any_element()
                        }
                    }),
            )
            .children(body)
    }

    /// The left panel: Variables, Kernels, Help and Packages over Files,
    /// Git, Plots and the Viewer.
    pub(crate) fn render_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let top_tab =
            |id: &'static str, label: &'static str, which: TopTab, cx: &mut Context<Self>| {
                // Four tabs share a narrow panel: a little tighter.
                Self::tab(id, None, label, self.top_tab == which, &p)
                    .px(px(7.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.top_tab = which;
                        if this.panel_collapsed(Panel::LeftTop) {
                            this.left_vertical = Vertical::Split;
                        }
                        cx.notify();
                    }))
            };
        let bottom_tab =
            |id: &'static str, label: &'static str, which: BottomTab, cx: &mut Context<Self>| {
                // As above: with the Plots tools beside them, wider tabs cut
                // "Viewer" short.
                Self::tab(id, None, label, self.bottom_tab == which, &p)
                    .px(px(7.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.bottom_tab = which;
                        if this.panel_collapsed(Panel::LeftBottom) {
                            this.left_vertical = Vertical::Split;
                        }
                        cx.notify();
                    }))
            };
        let restricted = self.restricted();
        let (top, refresh): (AnyElement, Vec<AnyElement>) = match self.top_tab {
            TopTab::Variables if restricted => {
                (self.restricted_note("see variables", true, cx), Vec::new())
            }
            TopTab::Kernels if restricted => {
                (self.restricted_note("use kernels", true, cx), Vec::new())
            }
            TopTab::Help if restricted => (self.restricted_note("read help", true, cx), Vec::new()),
            TopTab::Packages if restricted => {
                (self.restricted_note("see packages", true, cx), Vec::new())
            }
            // Its Import and refresh buttons are beside its filter, leaving
            // the strip to the four tabs.
            TopTab::Variables => (self.render_variables(cx), Vec::new()),
            TopTab::Kernels => (self.render_kernels(cx), Vec::new()),
            TopTab::Help => (self.render_help(cx), Vec::new()),
            TopTab::Packages => (self.render_packages(cx), self.package_tools(cx)),
        };
        let (bottom, tools): (AnyElement, Vec<AnyElement>) = match self.bottom_tab {
            BottomTab::Files => (
                self.render_files(cx).into_any_element(),
                self.files_tools(cx),
            ),
            BottomTab::Git if restricted => (self.restricted_note("use Git", true, cx), Vec::new()),
            BottomTab::Git => (self.render_git(cx), self.git_tools(cx)),
            BottomTab::Plots => (self.render_plots(cx), self.plot_tools(cx)),
            BottomTab::Viewer => (self.render_viewer(cx), Vec::new()),
        };
        let floating_bottom = !self.panel_collapsed(Panel::LeftBottom)
            && matches!(self.bottom_tab, BottomTab::Files | BottomTab::Git);
        let mut bottom = (!self.panel_collapsed(Panel::LeftBottom))
            .then(|| div().flex_1().min_h_0().child(bottom));
        v_flex()
            .relative()
            .w(px(self.files_width))
            .h_full()
            .flex_none()
            .when(self.maximized_panel.is_some(), |d| d.w_full().flex_1())
            .when(self.panel_visible(Panel::LeftTop), |d| {
                d.child(
                    side_pane(&p)
                        .relative()
                        .h(px(self.variables_height))
                        .flex_none()
                        .when(self.maximized_panel == Some(Panel::LeftTop), |d| {
                            d.h_full().flex_1()
                        })
                        .when(!self.panel_collapsed(Panel::LeftTop), |d| {
                            d.child(div().flex_1().min_h_0().child(top))
                                .when(self.top_tab == TopTab::Variables, |d| {
                                    d.children(self.render_import_menu(cx))
                                })
                        })
                        .child(
                            Self::strip(&p)
                                .child(
                                    crate::look::tab_capsule(&p)
                                        .id("left-top-tabs")
                                        .min_w_0()
                                        .flex_shrink(1.)
                                        .overflow_x_scroll()
                                        .gap(px(1.))
                                        .child(top_tab(
                                            "tab-variables",
                                            "Variables",
                                            TopTab::Variables,
                                            cx,
                                        ))
                                        .child(top_tab(
                                            "tab-kernels",
                                            "Kernels",
                                            TopTab::Kernels,
                                            cx,
                                        ))
                                        .child(top_tab("tab-help", "Help", TopTab::Help, cx))
                                        .child(top_tab(
                                            "tab-packages",
                                            "Packages",
                                            TopTab::Packages,
                                            cx,
                                        )),
                                )
                                .when(
                                    !self.panel_collapsed(Panel::LeftTop)
                                        && self.top_tab == TopTab::Variables
                                        && self.vars.loading,
                                    |d| {
                                        d.child(
                                            div()
                                                .pl_1()
                                                .text_size(px(crate::look::SMALL))
                                                .text_color(p.faint)
                                                .child("updating…"),
                                        )
                                    },
                                )
                                .child(div().flex_1())
                                .when(!self.panel_collapsed(Panel::LeftTop), |d| {
                                    d.child(h_flex().flex_none().gap(px(1.)).children(refresh))
                                })
                                .child(self.panel_buttons(Panel::LeftTop, cx))
                                .map(|strip| {
                                    if self.panel_collapsed(Panel::LeftTop) {
                                        strip.into_any_element()
                                    } else {
                                        crate::look::floating_header(&p, 0., false, strip)
                                    }
                                }),
                        ),
                )
            })
            .when(self.maximized_panel.is_none(), |d| {
                d.child(self.splitter(Split::Variables, cx))
            })
            .when(self.panel_visible(Panel::LeftBottom), |d| {
                d.child(
                    side_pane(&p)
                        .relative()
                        .flex_1()
                        .min_h(px(crate::look::STRIP))
                        .when(floating_bottom, |d| d.children(bottom.take()))
                        .child(
                            Self::strip(&p)
                                .child(
                                    crate::look::tab_capsule(&p)
                                        .id("left-bottom-tabs")
                                        .min_w_0()
                                        .flex_shrink(1.)
                                        .overflow_x_scroll()
                                        .gap(px(1.))
                                        .child(bottom_tab(
                                            "tab-files",
                                            "Files",
                                            BottomTab::Files,
                                            cx,
                                        ))
                                        .child(bottom_tab("tab-git", "Git", BottomTab::Git, cx))
                                        .child(bottom_tab(
                                            "tab-plots",
                                            "Plots",
                                            BottomTab::Plots,
                                            cx,
                                        ))
                                        .child(bottom_tab(
                                            "tab-viewer",
                                            "Viewer",
                                            BottomTab::Viewer,
                                            cx,
                                        )),
                                )
                                .child(div().flex_1())
                                .when(!self.panel_collapsed(Panel::LeftBottom), |d| {
                                    d.child(h_flex().flex_none().gap(px(1.)).children(tools))
                                })
                                .child(self.panel_buttons(Panel::LeftBottom, cx))
                                .map(|strip| {
                                    if !self.panel_collapsed(Panel::LeftBottom)
                                        && matches!(
                                            self.bottom_tab,
                                            BottomTab::Files | BottomTab::Git
                                        )
                                    {
                                        crate::look::floating_header(&p, 0., false, strip)
                                    } else {
                                        strip.into_any_element()
                                    }
                                }),
                        )
                        .children(bottom),
                )
            })
    }
}
