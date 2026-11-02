//! Where the panes sit: the left panel (a fifth of the window) holds
//! Variables, Help and Packages over Files, Git, Plots and the Viewer, then
//! the project's kernels and the machine's resources; the middle has the
//! editor with the Console, Terminal and Jobs under it; the AI pane is the
//! right panel. The middle and the AI pane take two fifths each.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::look::{Palette, capsule, icon_button, pane};
use crate::workbench::{BottomTab, ConsoleTab, Split, TopTab, Workbench};

impl Workbench {
    /// The pane under the editor: the Console, the Terminal or Jobs.
    pub(crate) fn render_console_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let tab =
            |id: &'static str, label: &'static str, which: ConsoleTab, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.console_tab == which, &p).on_click(
                    cx.listener(move |this, _, window, cx| this.show_tab(which, window, cx)),
                )
            };
        let body: AnyElement = match self.console_tab {
            ConsoleTab::Console => self.render_console(cx),
            ConsoleTab::Terminal => self.render_terminal(cx),
            ConsoleTab::Jobs => self.render_jobs(cx),
        };
        pane(&p)
            .h(px(self.console_height))
            .flex_none()
            .child(
                Self::strip(&p)
                    .child(
                        capsule(&p, 22.)
                            .gap(px(1.))
                            .child(tab("tab-console", "Console", ConsoleTab::Console, cx))
                            .child(tab("tab-terminal", "Terminal", ConsoleTab::Terminal, cx))
                            .child(tab("tab-jobs", "Jobs", ConsoleTab::Jobs, cx)),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("hide-console", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.show_console = false;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }

    /// The left panel: Variables, Help and Packages over Files, Git, Plots
    /// and the Viewer, then Kernels and Resources.
    pub(crate) fn render_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let top_tab =
            |id: &'static str, label: &'static str, which: TopTab, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.top_tab == which, &p).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.top_tab = which;
                        cx.notify();
                    },
                ))
            };
        let bottom_tab =
            |id: &'static str, label: &'static str, which: BottomTab, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.bottom_tab == which, &p).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.bottom_tab = which;
                        cx.notify();
                    },
                ))
            };
        let (top, refresh): (AnyElement, Vec<AnyElement>) = match self.top_tab {
            TopTab::Variables => (
                self.render_variables(cx),
                vec![
                    self.import_button(cx),
                    icon_button("refresh-variables", "refresh", &p, false)
                        .when(self.active_session().is_none(), |d| d.opacity(0.35))
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_variables(cx)))
                        .into_any_element(),
                ],
            ),
            TopTab::Help => (self.render_help(cx), Vec::new()),
            TopTab::Packages => (self.render_packages(cx), self.package_tools(cx)),
        };
        let (bottom, tools): (AnyElement, Vec<AnyElement>) = match self.bottom_tab {
            BottomTab::Files => (self.render_files(cx).into_any_element(), Vec::new()),
            BottomTab::Git => (self.render_git(cx), self.git_tools(cx)),
            BottomTab::Plots => (self.render_plots(cx), self.plot_tools(cx)),
            BottomTab::Viewer => (self.render_viewer(cx), Vec::new()),
        };
        v_flex()
            .relative()
            .w(px(self.files_width))
            .h_full()
            // Gives way to the middle in a narrow window.
            .flex_shrink(1.)
            .min_w(px(220.))
            .child(
                pane(&p)
                    .relative()
                    .h(px(self.variables_height))
                    .flex_none()
                    .child(
                        Self::strip(&p)
                            .child(
                                capsule(&p, 22.)
                                    .gap(px(1.))
                                    .child(top_tab(
                                        "tab-variables",
                                        "Variables",
                                        TopTab::Variables,
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
                                self.top_tab == TopTab::Variables && self.vars.loading,
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
                            .child(h_flex().gap(px(1.)).children(refresh)),
                    )
                    .child(div().flex_1().min_h_0().child(top))
                    .when(self.top_tab == TopTab::Variables, |d| {
                        d.children(self.render_import_menu(cx))
                    }),
            )
            .child(self.splitter(Split::Variables, cx))
            .child(
                pane(&p)
                    .relative()
                    .flex_1()
                    .min_h(px(120.))
                    .child(
                        Self::strip(&p)
                            .child(
                                capsule(&p, 22.)
                                    .gap(px(1.))
                                    .child(bottom_tab("tab-files", "Files", BottomTab::Files, cx))
                                    .child(bottom_tab("tab-git", "Git", BottomTab::Git, cx))
                                    .child(bottom_tab("tab-plots", "Plots", BottomTab::Plots, cx))
                                    .child(bottom_tab(
                                        "tab-viewer",
                                        "Viewer",
                                        BottomTab::Viewer,
                                        cx,
                                    )),
                            )
                            .child(div().flex_1())
                            .child(h_flex().gap(px(1.)).children(tools)),
                    )
                    .child(div().flex_1().min_h_0().child(bottom)),
            )
            .children(self.render_kernels(cx))
            .children(self.render_resources(cx))
    }
}
