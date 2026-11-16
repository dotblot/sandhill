//! Drawing the window (see `look`): the title bar over the frosted glass,
//! the side panels and Console group as translucent panes on it, and the
//! editor (or the Data Explorer) in a solid pane.

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{self, Editor};
use gpui_kit::component::text::TextViewStyle;
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::Variable;
use serde_json::Value;

use crate::editor::notebook::{self, CellKind};
use crate::editor::output_text::OutputText;
use crate::editor::shown::{Shown, show};
use crate::look::{
    self, EDGE, GAP, Palette, RADIUS, bar_button, capsule_divider, dot, icon, icon_button, pane,
    picture, raised, wall,
};
use crate::panes::explorer::Explorer;
use crate::shell::command_palette::Cmd;
use crate::shell::pane_sizes::Panel;
use crate::workbench::{Link, Open, Plot, Run, Session, Split, Workbench, is_script, plot_key};
use crate::{Interrupt, RunAll, RunCell, RunLine, Save, ToggleConsole};

mod cells;
mod console_pane;
mod layout;
mod side;
mod top_bar;

/// Height of one line of code (15px Maple Mono at 1.3), and an editor's
/// padding.
const LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING: f32 = 20.;
const PLOT_MAX_WIDTH: f32 = 720.;
/// How tall a cell's outputs grow before they scroll.
const OUTPUT_MAX_HEIGHT: f32 = 480.;

/// Height of a web page under a cell.
const PAGE_HEIGHT: f32 = 420.;

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A disconnected/removed AI pane cannot leave an empty workspace.
        if !self.show_files && !self.show_middle && !self.agents_available() {
            self.show_middle = true;
        }
        if self.maximized_panel == Some(Panel::Agents) && !self.agents_available() {
            self.maximized_panel = None;
        }
        self.report_focus(window, cx);
        self.sync_find_document(cx);
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        self.place_cells(window);
        self.fetch_notebook_plots(cx);
        self.sync_markdown_images(cx);
        self.sync_pages(window, cx);
        self.sync_plots(cx);
        self.sync_packages(cx);
        self.sync_jobs(cx);
        self.sync_git(cx);
        self.fetch_plot_images(cx);
        self.size_panels(window);
        let p = *Palette::get(cx);
        wall(&p)
            .relative()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .text_color(p.text)
            .text_size(px(look::CHROME))
            .line_height(rems(1.25))
            // Esc closes the large plot and the log, before a field in
            // them takes it.
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // ⌥1–9 pick a model while the agents' model menu is open,
                // wherever the focus is.
                if let Some(agents) = this.agents.clone()
                    && agents.update(cx, |a, cx| a.model_shortcut(event, window, cx))
                {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.dismiss_signature(cx) {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.outline.open {
                    this.outline.open = false;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.r_shortcut(event, window, cx) || this.find_keys(event, window, cx) {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.dismiss_zoom_units() {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                let mut dismissed = this.plot_history.zoom.take().is_some();
                dismissed |= this.dismiss_plot_size_menu();
                dismissed |= this.log.take().is_some();
                dismissed |= this.git.rewrite.take().is_some();
                dismissed |= this.notebook_ai_menu || this.colouring_menu;
                this.notebook_ai_menu = false;
                this.colouring_menu = false;
                dismissed |= this.main_menu
                    || this.connection_menu
                    || this.kernel_menu
                    || this.console_sessions.history_open
                    || this.files_ui.new_menu
                    || this.files_ui.sort_menu
                    || this.import.menu;
                this.main_menu = false;
                this.connection_menu = false;
                this.kernel_menu = false;
                this.console_sessions.history_open = false;
                this.files_ui.new_menu = false;
                this.files_ui.sort_menu = false;
                this.import.menu = false;
                dismissed |= this.files_ui.menu.take().is_some();
                dismissed |= this.machine.resource.take().is_some();
                dismissed |= this.git.menu.take().is_some();
                dismissed |= this.delimited.take().is_some();
                dismissed |= this.dismiss_settings_menu(cx);
                if let Some(agents) = this.agents.clone() {
                    dismissed |= agents.update(cx, |a, cx| a.dismiss_popups(window, cx));
                }
                if let Some(quota) = this.quota.clone() {
                    dismissed |= quota.update(cx, |quota, cx| quota.dismiss_popup(cx));
                }
                if dismissed {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            // Bubble only after focused editors, dropdowns and dialogs have
            // had their chance to consume Escape.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && this.maximized_panel.is_some()
                    && !this.palette.shown
                    && this.setup.dialog.is_none()
                    && this.import.dialog.is_none()
                    && this.trust.state != crate::welcome::trust::Trust::Asking
                {
                    this.maximized_panel = None;
                    if let Some(agents) = this.agents.clone() {
                        agents.update(cx, |_, cx| cx.notify());
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .when(!self.find.replaying, |d| {
                d.capture_action(cx.listener(|this, _: &input::Undo, window, cx| {
                    if this.notebook_find_history(false, window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .capture_action(cx.listener(
                    |this, _: &input::Redo, window, cx| {
                        if this.notebook_find_history(true, window, cx) {
                            cx.stop_propagation();
                        }
                    },
                ))
            })
            // Tab after a name opens completions in R and Python code.
            .capture_action(cx.listener(|this, _: &input::IndentInline, window, cx| {
                if this.complete_on_tab(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run_all(cx)))
            .on_action(cx.listener(|this, _: &Interrupt, _, _| this.interrupt()))
            .on_action(cx.listener(|this, _: &crate::Preview, window, cx| {
                this.run_command(Cmd::Preview, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleConsole, window, cx| {
                this.run_command(Cmd::ToggleConsole, window, cx)
            }))
            .map(|d| self.window_actions(d, cx))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if let Some(split) = this.dragging {
                    this.drag_split(split, event.position, window.viewport_size(), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .map(|d| match self.dragging {
                Some(Split::Variables | Split::Console) => d.cursor(CursorStyle::ResizeUpDown),
                Some(_) => d.cursor(CursorStyle::ResizeLeftRight),
                None => d,
            })
            .child(self.render_top_bar(window, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .pt(px(GAP))
                    .px(px(EDGE))
                    .child(self.render_workspace(window, cx)),
            )
            .children(self.render_document_notice(cx))
            .child(self.render_status(window, cx))
            .children(self.render_resource_popup(cx))
            .children(self.render_zoom(window, cx))
            .children(self.render_settings_dialog(cx))
            .children(self.render_trust_dialog(cx))
            .children(self.render_irkernel_dialog(cx))
            .children(self.render_import_dialog(cx))
            .children(self.render_delimited_dialog(cx))
            .children(self.render_main_menu(cx))
            .children(self.render_connection_menu(cx))
            .children(self.render_log_dialog(cx))
            .children(self.render_rewrite(cx))
            .children(self.render_tree_menu(cx))
            .children(self.render_console_menu(cx))
            .children(self.render_palette(cx))
            .children(self.render_signature(cx))
            .children(self.render_close_card(cx))
    }
}

/// Where the title bar's flat controls start; on a Mac the window buttons
/// come first.
pub(crate) const TITLE_BAR_LEFT: f32 = if cfg!(target_os = "macos") { 78. } else { 6. };

impl Workbench {
    pub(crate) fn agents_available(&self) -> bool {
        self.agents.is_some() && !self.restricted()
    }

    fn render_agents_pane(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .children(self.agents.clone())
            .into_any_element()
    }

    fn render_explorer(&self, explorer: &Entity<Explorer>, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx))
            .child(div().flex_1().min_h_0().child(explorer.clone()))
            .into_any_element()
    }

    /// What the middle shows with no notebook open: how to open a project,
    /// and the projects opened before on this machine.
    fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let body: AnyElement = match &self.link {
            Link::Connecting => div().child("Connecting…").into_any_element(),
            Link::Failed(err) => v_flex()
                .gap_2()
                .max_w(px(520.))
                .child(
                    div()
                        .text_color(p.text)
                        .child(format!("Could not reach sandhilld: {err}")),
                )
                .child(div().child(
                    "Right-click the project's capsule at the top to close it, then connect again from the Start screen.",
                ))
                .into_any_element(),
            Link::Ready(_) if self.project.is_some() => div()
                .max_w(px(420.))
                .text_center()
                .child("Pick a notebook (.ipynb, .qmd, .Rmd) or a script (.R, .py) on the left, or use the Console.")
                .into_any_element(),
            Link::Ready(_) => v_flex()
                .gap_3()
                .items_center()
                .child("No project is open on this machine.")
                .child(
                    Button::new("open-folder")
                        .small()
                        .primary()
                        .label("Open Folder…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.run_command(Cmd::OpenProject, window, cx)
                        })),
                )
                .into_any_element(),
        };
        pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .px_6()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted)
                    .line_height(rems(1.4))
                    .child(body),
            )
            .into_any_element()
    }

    /// The status bar (Huan, 2026-11-15; `docs/design/done/status-bar-plan.md`):
    /// where you are (the device and its resources, Restricted Mode), what
    /// is happening (a running task, the latest message), then the AI
    /// limits. All in 20px capsules; on a narrow window the message
    /// shortens first, then the resource numbers hide.
    fn render_status(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let wide = window.viewport_size().width >= px(STATUS_WIDE);
        let message = self
            .problem
            .clone()
            .map(|text| (text, true))
            .or_else(|| self.notice.clone().map(|text| (text, false)));
        h_flex()
            .flex_none()
            .w_full()
            .h(px(look::STATUS_BAR))
            .pl(px(10.))
            .pr(px(10.))
            .gap(px(8.))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_size(px(look::SMALL))
            .text_color(p.text)
            .child(self.render_device_capsule(wide, cx))
            .children(self.render_restricted_chip(cx))
            .children(self.render_archive_progress(cx))
            .children(message.map(|(text, problem)| {
                let full = text.clone();
                h_flex()
                    .id("status-message")
                    .min_w_0()
                    .flex_shrink(1.)
                    .gap(px(4.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if problem { p.err } else { p.muted })
                            .child(text),
                    )
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(w, cx)
                    })
                    .when(problem, |d| {
                        d.child(
                            look::close_circle("status-dismiss", &p)
                                .child(look::icon("close").size(px(12.)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.problem = None;
                                    cx.notify();
                                })),
                        )
                    })
            }))
            .child(div().flex_1())
            .when_some(self.quota.clone(), |bar, quota| bar.child(quota))
    }
}

/// Below this window width the status bar drops its resource numbers.
const STATUS_WIDE: f32 = 1000.;

/// How a run ended, when that wasn't fine (or a plain error, which its
/// output already shows).
fn failed_status(run: &Run) -> Option<String> {
    run.status
        .as_deref()
        .filter(|s| *s != "ok" && *s != "error")
        .map(str::to_string)
}
