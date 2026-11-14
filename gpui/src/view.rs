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

use crate::command_palette::Cmd;
use crate::explorer::Explorer;
use crate::look::{
    self, EDGE, GAP, Palette, RADIUS, bar_button, capsule_divider, dot, icon, icon_button, pane,
    picture, raised, wall,
};
use crate::notebook::{self, CellKind};
use crate::output_text::OutputText;
use crate::panes::Panel;
use crate::shown::{Shown, show};
use crate::workbench::{Link, Open, Plot, Run, Session, Split, Workbench, is_script, plot_key};
use crate::{Interrupt, RunAll, RunCell, RunLine, Save, ToggleConsole};

/// Height of one line of code (15px Maple Mono at 1.3), and an editor's
/// padding.
const LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING: f32 = 20.;
const PLOT_MAX_WIDTH: f32 = 720.;
/// How tall a cell's outputs grow before they scroll.
const OUTPUT_MAX_HEIGHT: f32 = 480.;
/// Whether a box of outputs took a wheel turn of `dy` (negative moves
/// down), so the notebook must not scroll too. Its own scroll listener runs
/// first: it adds a turn that can move it, unclamped, and skips one that
/// can't (at its top or bottom, or when it fits), leaving `offset` where it
/// was. `max` is how far down it can go. A turn that lands exactly on an
/// end reads as skipped, so the notebook moves once with it.
fn box_takes(offset: Pixels, max: Pixels, dy: Pixels) -> bool {
    !dy.is_zero() && (offset + dy).clamp(-max, px(0.)) != offset
}

/// Height of a web page under a cell.
const PAGE_HEIGHT: f32 = 420.;

/// Use the actual source icon key, including special names and compressed types.
fn collapsed_file_groups(paths: &[String]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (ix, path) in paths.iter().enumerate() {
        let key = look::file_icon(path);
        if let Some((_, files)) = groups.iter_mut().find(|(icon, _)| *icon == key) {
            files.push(ix);
        } else {
            groups.push((key, vec![ix]));
        }
    }
    groups
}

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
                dismissed |= this.notebook_ai_menu;
                this.notebook_ai_menu = false;
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
                    && this.trust.state != crate::trust::Trust::Asking
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
            .child(self.render_status(cx))
            .children(self.render_resource_popup(cx))
            .children(self.render_zoom(cx))
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
            .children(self.render_palette(cx))
            .children(self.render_signature(cx))
    }
}

/// Where the title bar's flat controls start; on a Mac the window buttons
/// come first.
pub(crate) const TITLE_BAR_LEFT: f32 = if cfg!(target_os = "macos") { 78. } else { 6. };

/// The height the left panel's two panes share: the window's, less the
/// title and status bars and the gaps.
fn left_column(height: f32) -> f32 {
    (height - look::TITLE_BAR - look::STATUS_BAR - 2. * GAP).max(0.)
}

/// The width the three panels share: the window's, less the room at its
/// edges and the gaps between them.
fn panel_room(width: f32) -> f32 {
    (width - 2. * EDGE - 2. * GAP).max(0.)
}

/// Shares refer to the normal three-column room, even with a column hidden.
fn column_widths(
    width: f32,
    left: bool,
    middle: bool,
    agents: bool,
    left_share: f32,
    agents_share: f32,
) -> (f32, f32, f32) {
    let count = left as u8 + middle as u8 + agents as u8;
    let available = (width - 2. * EDGE - GAP * count.saturating_sub(1) as f32).max(0.);
    if count == 0 {
        return (0., 0., 0.);
    }
    if !middle && !agents {
        return (available, 0., 0.);
    }
    if !middle {
        let l = if left {
            (panel_room(width) * left_share)
                .max(220.)
                .min((available - 300.).max(0.))
        } else {
            0.
        };
        return (l, 0., available - l);
    }
    let normal = (width - 2. * EDGE - GAP * (left as u8 + agents as u8) as f32).max(0.);
    let side_room = (normal - 360.).max(0.);
    let ai_min = if agents { 300_f32.min(side_room) } else { 0. };
    let l = if left {
        (panel_room(width) * left_share)
            .max(220.)
            .min((side_room - ai_min).max(0.))
    } else {
        0.
    };
    let a = if agents {
        (panel_room(width) * agents_share)
            .max(300.)
            .min((side_room - l).max(0.))
    } else {
        0.
    };
    (l, (available - l - a).max(0.), a)
}

impl Workbench {
    pub(crate) fn agents_available(&self) -> bool {
        self.agents.is_some() && !self.restricted()
    }

    pub(crate) fn reveal_middle(&mut self) {
        self.show_middle = true;
        if self
            .maximized_panel
            .is_some_and(|panel| panel != Panel::Editor)
        {
            self.maximized_panel = None;
        }
    }

    pub(crate) fn toggle_column(&mut self, panel: Panel, window: &Window, cx: &mut Context<Self>) {
        let agents = self.show_agents && self.agents_available();
        let can_hide = match panel {
            Panel::LeftTop => self.show_middle || agents,
            Panel::Editor => self.show_files || agents,
            Panel::Agents => self.show_files || self.show_middle,
            _ => return,
        };
        let keep_left = panel == Panel::Editor && self.show_middle && self.show_files;
        let shown = match panel {
            Panel::LeftTop => &mut self.show_files,
            Panel::Editor => &mut self.show_middle,
            Panel::Agents => &mut self.show_agents,
            _ => return,
        };
        if !*shown || can_hide {
            // Preserve the actual left width, including any narrow-window
            // clamping, when the middle hands its room to the AI pane.
            if keep_left {
                self.left_share =
                    self.files_width / panel_room(f32::from(window.viewport_size().width)).max(1.);
            }
            *shown = !*shown;
            self.maximized_panel = None;
            self.dragging = None;
            if let Some(agents) = self.agents.clone() {
                agents.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        }
    }

    fn render_agents_pane(&self, _cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .children(self.agents.clone())
            .into_any_element()
    }

    fn render_workspace(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(panel) = self.maximized_panel {
            return match panel {
                Panel::LeftTop | Panel::LeftBottom => self.render_left(cx).into_any_element(),
                Panel::Editor => div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .child(self.render_notebook(window, cx))
                    .into_any_element(),
                Panel::Console => self.render_console_pane(window, cx).into_any_element(),
                Panel::Agents => div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_agents_pane(cx))
                    .into_any_element(),
            };
        }
        let agents = self.show_agents && self.agents_available();
        h_flex()
            .size_full()
            .items_stretch()
            .when(self.show_files, |d| d.child(self.render_left(cx)))
            .when(self.show_files && (self.show_middle || agents), |d| {
                d.child(self.splitter(Split::Files, cx))
            })
            .when(self.show_middle, |d| {
                d.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(look::STRIP))
                                .flex()
                                .child(self.render_notebook(window, cx)),
                        )
                        .when(self.show_console, |d| {
                            d.child(self.splitter(Split::Console, cx))
                                .child(self.render_console_pane(window, cx))
                        }),
                )
            })
            .when(agents, |d| {
                d.when(self.show_middle, |d| {
                    d.child(self.splitter(Split::Agents, cx))
                })
                .child(
                    div()
                        .w(px(self.agents_width))
                        .flex_none()
                        .h_full()
                        .child(self.render_agents_pane(cx)),
                )
            })
            .into_any_element()
    }

    /// The left and AI panels' widths from their shares of the window: a
    /// quarter each at first (1:2:1), the middle taking what is left.
    fn size_panels(&mut self, window: &Window) {
        let width = f32::from(window.viewport_size().width);
        let (left, _, agents) = column_widths(
            width,
            self.show_files,
            self.show_middle,
            self.show_agents && self.agents_available(),
            self.left_share,
            self.agents_share,
        );
        self.files_width = left;
        self.agents_width = agents;
        let height = left_column(f32::from(window.viewport_size().height));
        self.variables_height = self.left_vertical.heights(height, self.variables_share).0;
        self.console_height = self
            .middle_vertical
            .heights(height, 1. - self.console_share)
            .1;
    }

    /// A gap between panes that can be dragged; an accent line shows under
    /// the pointer.
    pub(crate) fn splitter(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let across = matches!(split, Split::Variables | Split::Console);
        let dragging = self.dragging == Some(split);
        let id: SharedString = format!("split-{split:?}").into();
        let line = div()
            .absolute()
            .rounded(px(1.))
            .bg(p.accent)
            .map(|d| {
                if across {
                    d.top(px(0.5))
                        .h(px(2.))
                        .left(relative(0.3))
                        .right(relative(0.3))
                } else {
                    d.left(px(0.5))
                        .w(px(2.))
                        .top(relative(0.3))
                        .bottom(relative(0.3))
                }
            })
            .opacity(if dragging { 0.8 } else { 0. });
        let group: SharedString = format!("{id}-group").into();
        div()
            .id(id)
            .group(group.clone())
            .relative()
            .flex_none()
            .map(|d| {
                if across {
                    d.h(px(GAP)).w_full().cursor(CursorStyle::ResizeUpDown)
                } else {
                    d.w(px(GAP)).h_full().cursor(CursorStyle::ResizeLeftRight)
                }
            })
            .child(line.group_hover(group, |d| d.opacity(0.8)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(split);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// Resizes the panes beside a gap being dragged to the pointer.
    fn drag_split(
        &mut self,
        split: Split,
        at: Point<Pixels>,
        window: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let (width, height) = (f32::from(window.width), f32::from(window.height));
        let agents_shown = self.show_agents && self.agents_available();
        let visible = self.show_files as u8 + self.show_middle as u8 + agents_shown as u8;
        let available = (width - 2. * EDGE - GAP * visible.saturating_sub(1) as f32).max(0.);
        let room = panel_room(width).max(1.);
        let middle = if self.show_middle { 360. } else { 0. };
        match split {
            Split::Files => {
                let other = if self.show_middle {
                    if agents_shown { self.agents_width } else { 0. }
                } else if agents_shown {
                    300.
                } else {
                    0.
                };
                let most = (available - other - middle).max(0.);
                self.left_share = (x - EDGE).clamp(220_f32.min(most), most) / room;
            }
            Split::Agents => {
                let files = if self.show_files {
                    self.files_width
                } else {
                    0.
                };
                let most = (available - files - middle).max(0.);
                self.agents_share = (width - x - EDGE).clamp(300_f32.min(most), most) / room;
            }
            Split::Variables => {
                self.left_vertical = crate::panes::Vertical::Split;
                // The top pane's height, from the top of the column.
                let top = look::TITLE_BAR + GAP;
                let most = (height - look::STATUS_BAR - top - 120.).max(90.);
                self.variables_height = (y - top).clamp(90., most);
                self.variables_share = self.variables_height / left_column(height).max(1.);
            }
            Split::Console => {
                self.middle_vertical = crate::panes::Vertical::Split;
                let bottom = height - look::STATUS_BAR;
                let most = (bottom - look::TITLE_BAR - 160.).max(80.);
                self.console_height = (bottom - y).clamp(80., most);
                self.console_share = self.console_height / left_column(height).max(1.);
            }
        }
        cx.notify();
    }

    /// The title bar: window buttons directly on the glass after the Mac's
    /// own, and the theme button at the right; the window draws the project
    /// tabs between them.
    fn render_top_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        div()
            .relative()
            .flex_none()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                // Behind the controls: dragging here moves the window, and a
                // double click zooms it, as a title bar does.
                div()
                    .id("title-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            if cfg!(target_os = "macos") {
                                window.titlebar_double_click();
                            } else {
                                window.zoom_window();
                            }
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                h_flex()
                    .absolute()
                    .size_full()
                    .pt(px(6.))
                    .pl(px(if window.is_fullscreen() {
                        6.
                    } else {
                        TITLE_BAR_LEFT
                    }))
                    .pr(px(6.))
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(25.))
                            .px(px(2.))
                            .child(
                                icon_button("toggle-files", "sidebar.left", &p, true)
                                    .when(self.show_files, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the left panes",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::LeftTop, window, cx);
                                    })),
                            )
                            .child(
                                icon_button("toggle-editor", "sidebar.middle", &p, true)
                                    .when(self.show_middle, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the editor",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::Editor, window, cx);
                                    })),
                            )
                            .child({
                                let waiting = self
                                    .agents
                                    .as_ref()
                                    .is_some_and(|a| a.read(cx).any_waiting());
                                icon_button("toggle-agents", "chat", &p, true)
                                    .relative()
                                    .when(self.show_agents, |d| d.text_color(p.text))
                                    .when(waiting, |d| {
                                        d.child(
                                            dot(p.accent)
                                                .size(px(6.))
                                                .absolute()
                                                .top(px(2.))
                                                .right(px(5.)),
                                        )
                                    })
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the AI pane",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::Agents, window, cx);
                                    }))
                            })
                            .child(capsule_divider(&p))
                            .child(
                                icon_button("palette", "search", &p, true)
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Go to a file or run a command ⌘K",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_palette(window, cx)
                                    })),
                            )
                            .child(
                                icon_button("main-menu", "menu", &p, true)
                                    .on_prepaint(self.popup_triggers.track("main-menu"))
                                    .when(self.main_menu, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Menu")
                                            .build(w, cx)
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.main_menu = !this.main_menu;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(25.))
                            .px(px(2.))
                            .child(
                                icon_button(
                                    "theme",
                                    if p.dark { "theme.light" } else { "theme.dark" },
                                    &p,
                                    true,
                                )
                                .tooltip(move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(if p.dark {
                                        "Switch to the light theme"
                                    } else {
                                        "Switch to the dark theme"
                                    })
                                    .build(w, cx)
                                })
                                .on_click(|_, window, cx| look::toggle_appearance(window, cx)),
                            )
                            .child(
                                icon_button("welcome", "welcome", &p, true)
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Welcome")
                                            .build(w, cx)
                                    })
                                    .on_click(|_, _, cx| crate::app_window::show_welcome(None, cx)),
                            ),
                    ),
            )
    }

    /// The Files tab: the project's folders and files as a tree, right
    /// under the tab strip (its buttons are in the strip: `files_tools`).
    pub(crate) fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let count = self.file_lines().len();
        let project = self.project.clone().unwrap_or_default();
        let open_path = self.active_document_path().map(str::to_owned);
        let list = uniform_list(
            "files",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let lines = this.file_lines();
                let selected = this.files_ui.selected.as_ref().map(|(r, _)| r.clone());
                range
                    .filter_map(|ix| lines.get(ix).map(|line| (ix, line)))
                    .map(|(ix, line)| {
                        let row = match line {
                            crate::tree_actions::Line::Field { depth } => {
                                return this.render_name_field(*depth, cx);
                            }
                            crate::tree_actions::Line::Row(row) => row,
                        };
                        let indent = 8. + 16. * row.depth as f32;
                        let hover = p.ink(7.);
                        let line = h_flex()
                            .id(("file", ix))
                            .relative()
                            .w_full()
                            .h(px(22.))
                            .pl(px(indent))
                            .pr(px(8.))
                            .gap(px(6.))
                            .rounded(px(RADIUS))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            // A line down from each folder above, as a guide.
                            .children((0..row.depth).map(|level| {
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(8. + 16. * level as f32 + 6.5))
                                    .w(px(1.))
                                    .bg(p.ink(12.))
                            }));
                        if let Some(note) = &row.note {
                            return line
                                .pl(px(indent + 20.))
                                .text_color(p.faint)
                                .child(note.clone())
                                .into_any_element();
                        }
                        let rel = row.rel.clone();
                        let is_selected = selected.as_deref() == Some(row.rel.as_str());
                        let menu_rel = row.rel.clone();
                        let menu_dir = row.dir;
                        let line = line.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.tree_menu(menu_rel.clone(), menu_dir, event.position, cx)
                            }),
                        );
                        let name = div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(row.name.clone());
                        let by_size = this.tree.sort == crate::tree::SortBy::Size;
                        let tip = crate::tree::row_tip(
                            row,
                            !row.dir || by_size,
                            chrono::Utc::now().timestamp_millis(),
                        );
                        let line = line.tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        });
                        let meta = |shown: bool| {
                            shown.then(|| {
                                div()
                                    .flex_none()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(crate::machine::format_bytes(row.size))
                            })
                        };
                        if row.dir {
                            return line
                                .cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .child(
                                    div().w(px(14.)).flex_none().text_color(p.faint).child(
                                        icon(if row.open {
                                            "chevron.down"
                                        } else {
                                            "chevron.right"
                                        })
                                        .size(px(13.)),
                                    ),
                                )
                                .child(picture(&look::folder_icon(&row.name)))
                                .child(name)
                                .children(meta(by_size))
                                .when(is_selected, |d| d.bg(p.ink(9.)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.files_ui.selected = Some((rel.clone(), true));
                                    this.toggle_folder(rel.clone(), cx)
                                }))
                                .into_any_element();
                        }
                        let openable = notebook::can_open(&row.name);
                        let full = format!("{project}/{rel}");
                        let is_open = open_path.as_deref() == Some(full.as_str());
                        let document = this
                            .open
                            .iter()
                            .chain(&this.others)
                            .find(|o| o.notebook.path == full);
                        let dirty = document.is_some_and(|o| o.dirty);
                        // Open in a tab: a hollow ring, or the kernel's dot
                        // (green once it runs) when the document has one.
                        let opened = this.tabs.contains(&full).then(|| match document {
                            Some(o) if !matches!(o.session, Session::None) => {
                                Self::session_dot(&o.session, &p)
                            }
                            _ => div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(p.faint),
                        });
                        line.pl(px(indent + 20.))
                            .when(!openable, |d| d.text_color(p.muted))
                            .when(is_open, |d| d.bg(p.accent.opacity(0.2)))
                            .when(!is_open && is_selected, |d| d.bg(p.ink(9.)))
                            .when(!is_open, |d| d.hover(move |d| d.bg(hover)))
                            .cursor_pointer()
                            .child(picture(&look::file_icon(&row.name)))
                            .map(|d| {
                                if dirty {
                                    d.child(
                                        h_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap(px(4.))
                                            .child(name.flex_initial())
                                            .child(dot(p.muted).size(px(4.))),
                                    )
                                } else {
                                    d.child(name)
                                }
                            })
                            .children(meta(true))
                            .children(opened)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files_ui.selected = Some((rel.clone(), false));
                                this.open_file(rel.clone(), cx)
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.render_output_file_scroll())
        // UniformList positions and reveals rows relative to its padded bounds.
        .pt(px(look::STRIP + 4.))
        .size_full();
        v_flex()
            .id("files-pane")
            .relative()
            .size_full()
            .when(self.files_ui.error.is_some(), |d| {
                d.child(
                    div()
                        .pt(px(look::STRIP + 4.))
                        .children(self.render_name_error(cx)),
                )
            })
            .child(div().flex_1().min_h_0().px(px(6.)).pb(px(10.)).child(list))
            .child(look::inset_scrollbar(
                &self.render_output_file_scroll(),
                look::STRIP,
                cx,
            ))
            .children(self.render_new_menu(cx))
            .children(self.render_sort_menu(cx))
    }

    /// The Files tab's sort, new and refresh buttons, in its pane's strip.
    pub(crate) fn files_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.project.is_none() {
            return Vec::new();
        }
        let p = *Palette::get(cx);
        let sort = self.tree.sort;
        vec![
            icon_button("sort-files", "sort", &p, false)
                .on_prepaint(self.popup_triggers.track("files-sort"))
                .when(self.files_ui.sort_menu, |d| d.text_color(p.text))
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "Sort by {}",
                        sort.label().to_lowercase()
                    ))
                    .build(w, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.files_ui.sort_menu = !this.files_ui.sort_menu;
                    this.files_ui.new_menu = false;
                    cx.notify();
                }))
                .into_any_element(),
            icon_button("new-item", "add", &p, false)
                .on_prepaint(self.popup_triggers.track("files-new"))
                .when(self.files_ui.new_menu, |d| d.text_color(p.text))
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("New file or folder").build(w, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.files_ui.new_menu = !this.files_ui.new_menu;
                    this.files_ui.sort_menu = false;
                    cx.notify();
                }))
                .into_any_element(),
            icon_button("refresh-files", "refresh", &p, false)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx)))
                .into_any_element(),
        ]
    }

    /// A pane's tab strip: its tabs in a capsule, then anything else.
    pub(crate) fn strip(p: &Palette) -> Div {
        h_flex()
            .flex_none()
            .h(px(look::STRIP))
            .px(px(4.))
            .gap(px(4.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .overflow_hidden()
    }

    /// One tab in a strip's capsule: a pill, raised when picked.
    pub(crate) fn tab(
        id: impl Into<ElementId>,
        leading: Option<AnyElement>,
        label: impl Into<SharedString>,
        active: bool,
        p: &Palette,
    ) -> Stateful<Div> {
        let hover = p.ink(6.);
        let text = p.text;
        let tab = h_flex()
            .id(id)
            .group(look::TAB_GROUP)
            .flex_none()
            .h(px(look::TAB_H))
            .gap(px(5.))
            .px(px(10.))
            .rounded_full()
            .text_size(px(look::TAB_TEXT))
            .whitespace_nowrap()
            .text_color(p.muted)
            .cursor_pointer()
            .children(leading)
            .child(label.into());
        if active {
            raised(tab, p)
        } else {
            tab.hover(move |d| d.bg(hover).text_color(text))
        }
    }

    /// The notebook and source jumps share the same unobscured viewport edge.
    /// Find has a 24px input, 12px padding and 1px edge; Replace adds 28px.
    pub(crate) fn notebook_header_height(&self) -> f32 {
        look::STRIP
            + if self.find.open {
                if self.find.replace { 65. } else { 37. }
            } else {
                0.
            }
    }

    fn render_notebook(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        if self.panel_collapsed(crate::panes::Panel::Editor) {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .into_any_element();
        }
        if let Some(path) = &self.opening_shown
            && !self.objects.is_shown()
            && !self.explorer_shown
            && !self.diff_shown
        {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .child(
                    gpui_kit::component::progress::Progress::new("document-opening")
                        .loading(true)
                        .color(p.accent)
                        .h(px(2.))
                        .accessibility_label("Opening document"),
                )
                .child(
                    h_flex()
                        .px(px(16.))
                        .py(px(14.))
                        .gap(px(8.))
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(crate::files::base_name(path).to_string()),
                        )
                        .child(div().flex_none().child("Opening…")),
                )
                .into_any_element();
        }
        if let Some(browser) = self.render_object_explorer(cx) {
            return browser;
        }
        if self.diff_shown && self.diff.is_some() {
            return self.render_diff(cx);
        }
        if !self.diff_shown
            && !self.explorer_shown
            && let Some(media) = self
                .media_shown
                .as_ref()
                .and_then(|path| self.media.get(path))
        {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .child(div().flex_1().min_h_0().child(media.clone()))
                .into_any_element();
        }
        if let Some(explorer) = self
            .explorer
            .as_ref()
            .filter(|_| self.explorer_shown || self.open.is_none())
        {
            return self.render_explorer(explorer, cx);
        }
        let Some(open) = &self.open else {
            return self.render_start(cx);
        };
        if open.single_editor() {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .children(self.render_find(cx))
                .children(self.render_truncated_note(open, cx))
                .child(self.render_source_document(open, cx))
                .into_any_element();
        }
        let bodies = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| {
                if !self.placed.shows(ix) {
                    return None;
                }
                Some(match cell.kind {
                    // As in the Tauri app, a notebook's Markdown shows
                    // formatted; a Quarto document's prose stays text.
                    CellKind::Text => {
                        if open.notebook.format != notebook::Format::Ipynb
                            || open.editing.contains(&cell.id)
                            || self.find.replaying
                            || cell_text(open, ix, cx).trim().is_empty()
                        {
                            self.render_cell_editor(open, ix, false, window, cx)
                        } else {
                            self.render_markdown(open, ix, cx)
                        }
                    }
                    CellKind::Code => self.render_code_cell(open, ix, window, cx),
                })
            })
            .collect::<Vec<_>>();
        let cells = self.render_cells(open, bodies, window, cx);
        let commanding = self.cells_focus.is_focused(window);
        let header_height = self.notebook_header_height();
        let target = self.notebook_scroll_target.take();
        let scroll = self.notebook_scroll.clone();
        let anchor = (scroll.children_count() > 0).then(|| scroll.logical_scroll_top());
        let original_offset = scroll.offset();
        let scroller = div()
            .on_children_prepainted(move |_, window, _| {
                if let Some(item) = target
                    && let Some(bounds) = scroll.bounds_for_item(item)
                {
                    let y = look::header_scroll_offset(
                        scroll.offset().y,
                        bounds.top() - scroll.bounds().top(),
                        bounds.bottom() - scroll.bounds().top(),
                        scroll.bounds().size.height,
                        px(header_height),
                        scroll.max_offset().y,
                    );
                    scroll.set_offset(Point {
                        x: original_offset.x,
                        y,
                    });
                    window.refresh();
                    return;
                }
                // Replacing an estimated row above the viewport must not move
                // the visible cell. Child bounds include the header padding,
                // so this anchor correction preserves it without adding it twice.
                // Explicit wheel/jump requests remain authoritative.
                if scroll.offset() != original_offset {
                    return;
                }
                if let Some((item, within)) = anchor
                    && let Some(bounds) = scroll.bounds_for_item(item)
                {
                    let y = (scroll.bounds().top() + within - bounds.top())
                        .clamp(-scroll.max_offset().y, px(0.));
                    if (y - original_offset.y).abs() > px(0.5) {
                        scroll.set_offset(Point {
                            x: original_offset.x,
                            y,
                        });
                        window.refresh();
                    }
                }
            })
            .id("cells")
            .flex()
            .flex_col()
            .on_scroll(cx.listener(|this, _, _, cx| {
                // Hover geometry belongs to the pre-scroll editor layout.
                this.clear_language_popovers(None, cx);
            }))
            .track_focus(&self.cells_focus)
            .when(commanding, |d| d.key_context(crate::CELLS))
            .pb(px(200.))
            .pl(px(6.))
            .pr(px(24.))
            // Between cells that can be added to, the rows that add them.
            .when(!open.notebook.can_add_cells(), |d| {
                d.gap(px(crate::cell_view::TOOLS_HEIGHT + 4.))
            })
            .children(cells);
        pane(&p)
            .relative()
            .flex_1()
            .h_full()
            .child(
                look::PaneScroll::new(
                    "notebook-scroll",
                    header_height,
                    10.,
                    self.cell_keys(scroller, cx),
                )
                .handle(&self.notebook_scroll),
            )
            .child(look::floating_header(
                &p,
                0.,
                true,
                v_flex()
                    .child(self.render_tabs(cx))
                    .children(self.render_find(cx)),
            ))
            .into_any_element()
    }

    /// The middle pane's tabs: the open notebooks, in the order they were
    /// opened, then the Data Explorer.
    pub(crate) fn render_document_tabs(
        &self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let shown = if self.objects.is_shown() || self.explorer_shown || self.diff_shown {
            None
        } else {
            self.active_document_path()
        };
        let groups = if expanded {
            self.tabs
                .iter()
                .enumerate()
                .map(|(ix, path)| (look::file_icon(path), vec![ix]))
                .collect()
        } else {
            collapsed_file_groups(&self.tabs)
        };
        let mut tabs: Vec<AnyElement> = groups
            .into_iter()
            .map(|(file_icon, files)| {
                let ix = files[0];
                let path = &self.tabs[ix];
                let name = if expanded {
                    crate::files::base_name(path).to_string()
                } else if files.len() > 1 {
                    files.len().to_string()
                } else {
                    String::new()
                };
                let show = path.clone();
                let tip = files
                    .iter()
                    .map(|ix| self.tabs[*ix].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let dirty = files.iter().any(|ix| self.tab_dirty(&self.tabs[*ix]));
                let picked = expanded && shown == Some(path.as_str());
                let lead = picture(&file_icon).size(px(14.)).into_any_element();
                let (lead, trailing) = if expanded {
                    look::place_close(
                        lead,
                        self.close_button(ix, path.clone(), dirty, picked, cx),
                        picked || dirty,
                    )
                } else {
                    (lead, None)
                };
                Self::tab(("file-tab", ix), Some(lead), name, picked, &p)
                    .flex_none()
                    .when(!expanded, |d| d.px(px(5.)).gap(px(3.)))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                    })
                    .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
                    .when(!expanded && dirty, |d| d.child(dot(p.muted).size(px(4.))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_file_tab(&show, cx);
                        if this.panel_collapsed(crate::panes::Panel::Editor) {
                            this.middle_vertical = crate::panes::Vertical::Split;
                        }
                    }))
                    .into_any_element()
            })
            .collect();
        if let Some(explorer) = &self.explorer {
            let picked =
                expanded && self.explorer_shown && !self.diff_shown && !self.objects.is_shown();
            let lead = icon("table").size(px(14.)).into_any_element();
            let (lead, trailing) = if expanded {
                look::place_close(
                    lead,
                    look::tab_close("explorer-close", picked, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_explorer(cx)
                        },
                    )),
                    picked,
                )
            } else {
                (lead, None)
            };
            tabs.push(
                Self::tab(
                    "explorer-tab",
                    Some(lead),
                    if expanded {
                        explorer.read(cx).name()
                    } else {
                        String::new()
                    },
                    picked,
                    &p,
                )
                .flex_none()
                .when(!expanded, |d| d.px(px(5.)).gap_0())
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Data Explorer").build(w, cx)
                })
                .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.reveal_middle();
                    this.hide_object_explorer();
                    this.explorer_shown = true;
                    this.diff_shown = false;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        tabs.extend(self.diff_tab_element(expanded, cx));
        tabs.extend(self.object_tab_elements(expanded, cx));
        tabs
    }

    /// The editor's actions and pane controls share one row. File labels
    /// remain in the project's titlebar capsule.
    pub(crate) fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let document = self.open.as_ref().filter(|_| {
            !self.panel_collapsed(crate::panes::Panel::Editor)
                && !self.diff_shown
                && !self.explorer_shown
                && !self.objects.is_shown()
                && self.media_shown.is_none()
        });
        let media = self
            .media_shown
            .as_ref()
            .filter(|_| {
                !self.panel_collapsed(crate::panes::Panel::Editor)
                    && !self.diff_shown
                    && !self.explorer_shown
                    && !self.objects.is_shown()
            })
            .and_then(|path| self.media.get(path));
        let explorer = self.explorer.as_ref().filter(|_| {
            self.explorer_shown
                && !self.diff_shown
                && !self.objects.is_shown()
                && !self.panel_collapsed(crate::panes::Panel::Editor)
        });
        Self::strip(&p)
            .child(
                h_flex()
                    .id("editor-actions")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .when_some(document, |d, open| {
                        d.child(self.render_notebook_bar(open, cx))
                    })
                    .when_some(media, |d, media| {
                        d.child(media.update(cx, |media, cx| media.render_toolbar(cx)))
                    })
                    .when_some(explorer, |d, explorer| d.child(explorer.read(cx).toolbar())),
            )
            .when(document.is_some(), |d| d.child(self.outline_button(cx)))
            .child(self.panel_buttons(crate::panes::Panel::Editor, cx))
    }

    /// A document tab's close button. Unsaved, a dot that stays after the
    /// label and turns into the cross under the pointer.
    fn close_button(
        &self,
        ix: usize,
        path: String,
        dirty: bool,
        picked: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = *Palette::get(cx);
        let close = if dirty {
            let group: SharedString = format!("close-tab-{ix}").into();
            look::close_circle(("close-tab", ix), &p)
                .group(group.clone())
                .child(
                    dot(p.muted)
                        .group_hover(group.clone(), |d| d.invisible())
                        .absolute(),
                )
                .child(
                    div()
                        .invisible()
                        .group_hover(group, |d| d.visible())
                        .child(icon("close").size(px(12.))),
                )
        } else {
            look::tab_close(("close-tab", ix), picked, &p)
        };
        close.on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.ask_close(path.clone(), window, cx)
        }))
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

    /// Restart and Shut down for the shown document's kernel, while it runs.
    fn session_buttons(&self, running: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !running {
            return None;
        }
        let p = *Palette::get(cx);
        Some(
            h_flex()
                .child(
                    icon_button("session-restart", "refresh", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "Restart: a new kernel, with nothing defined",
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.restart_session(cx))),
                )
                .child(
                    icon_button("session-stop", "power", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "Shut down: the next run starts a new kernel",
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.shut_down_session(cx))),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn session_text(session: &Session) -> String {
        match session {
            Session::None => "Kernel not started".into(),
            Session::Starting => "Starting kernel…".into(),
            Session::Ready(_) => "Kernel ready".into(),
            Session::Failed(err) => format!("Kernel: {err}"),
        }
    }

    pub(crate) fn session_dot(session: &Session, p: &Palette) -> Div {
        dot(match session {
            Session::None => p.faint,
            Session::Starting => p.busy,
            Session::Ready(_) => p.ok,
            Session::Failed(_) => p.err,
        })
    }

    fn render_source_document(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        if open.notebook.format == notebook::Format::Delimited {
            return self.render_delimited(open, cx);
        }
        if open.notebook.format == notebook::Format::Markdown && open.markdown_preview {
            return div()
                .id("markdown-document-preview")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.notebook_scroll)
                .px(px(18.))
                .py(px(12.))
                .child(
                    crate::md_images::markdown(open, 0, "markdown-document", cx)
                        .style(TextViewStyle::default().paragraph_gap(rems(0.5))),
                )
                .into_any_element();
        }
        self.render_script(open, cx).into_any_element()
    }

    /// A script: one editor with line numbers, filling the pane. ⌘↵ runs
    /// the line or statement; the runs show in the Console.
    fn render_script(&self, open: &Open, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .pt(px(6.))
            .on_action(cx.listener(|this, _: &RunLine, window, cx| this.run_statement(window, cx)))
            // Shift+Enter does the same, as in the Tauri app's scripts.
            .on_action(cx.listener(|this, _: &RunCell, window, cx| this.run_statement(window, cx)))
            .child(
                Editor::new(&open.editors[0])
                    .readonly(open.notebook.truncated)
                    .bordered(false)
                    .appearance(false)
                    .line_height(px(LINE_HEIGHT))
                    .h_full(),
            )
    }

    /// A notebook's Markdown cell shown formatted, as in the Tauri app; a
    /// double click edits it, and Shift+Enter or leaving it shows it formatted
    /// again.
    fn render_markdown(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let editor = open.editors[ix].clone();
        let path = open.notebook.path.clone();
        div()
            .id(("markdown", ix))
            .w_full()
            .px(px(8.))
            .py(px(2.))
            .text_size(px(look::CHROME))
            .line_height(relative(1.4))
            .cursor_text()
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() < 2 {
                    return;
                }
                if let Some(open) = this.open.as_mut().filter(|o| o.notebook.path == path) {
                    let id = open.notebook.cells[ix].id.clone();
                    open.editing.insert(id);
                    editor.update(cx, |editor, cx| editor.focus(window, cx));
                    cx.notify();
                }
            }))
            .child(
                crate::md_images::markdown(open, ix, ("md", ix), cx).style(
                    TextViewStyle::default()
                        .paragraph_gap(rems(0.5))
                        .heading_font_size(|level, _| {
                            px(look::CHROME
                                * match level {
                                    1 => 1.6,
                                    2 => 1.35,
                                    3 => 1.15,
                                    _ => 1.0,
                                })
                        }),
                ),
            )
            .into_any_element()
    }

    /// Document actions inside the editor's single header row.
    fn render_notebook_bar(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let wrap_button = Self::editor_wrap_type(open).map(|kind| {
            icon_button("editor-wrap", "editor.wrap", &p, false)
                .when(crate::prefs::editor_soft_wrap(kind), |button| {
                    button.bg(p.accent_soft).text_color(p.accent)
                })
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Wrap long lines").build(window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_editor_wrap(window, cx);
                }))
        });
        if open.notebook.is_text_document() {
            return h_flex()
                .flex_none()
                .gap(px(4.))
                .when(open.notebook.quarto_source(), |bar| {
                    bar.child(
                        bar_button("quarto-view", "Notebook", Some("doc"), &p)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Switch to Notebook view",
                                )
                                .build(w, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_quarto_view(window, cx);
                            })),
                    )
                })
                .when(open.notebook.format == notebook::Format::Markdown, |bar| {
                    bar.child(
                        bar_button(
                            "markdown-view",
                            if open.markdown_preview {
                                "Edit source"
                            } else {
                                "Rendered view"
                            },
                            Some("doc"),
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(open) = this.open.as_mut() {
                                open.markdown_preview = !open.markdown_preview;
                                cx.notify();
                            }
                        })),
                    )
                })
                .when(crate::jobs::is_job_script(&open.notebook.path), |bar| {
                    bar.child(
                        bar_button("run-job", "Run as job", Some("terminal"), &p)
                            .on_click(cx.listener(|this, _, _, cx| this.run_open_as_job(cx))),
                    )
                })
                .children(wrap_button)
                .child(
                    bar_button("save", "Save", Some("doc.save"), &p)
                        .when(!open.dirty, |d| d.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .into_any_element();
        }
        h_flex()
            .flex_none()
            .gap(px(4.))
            .whitespace_nowrap()
            .when(open.notebook.format == notebook::Format::Quarto, |bar| {
                bar.child(
                    bar_button("quarto-view", "Source", Some("doc"), &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Switch to Source view")
                                .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_quarto_view(window, cx);
                        })),
                )
            })
            .child(self.kernel_picker(true, cx))
            .children(self.language_status(&open.notebook.path).map(|reason| {
                div()
                    .id("language-status")
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child("Analysis unavailable")
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(reason.clone()).build(window, cx)
                    })
            }))
            .when(open.is_script(), |bar| {
                bar.child(
                    bar_button("run-line", "Run line", Some("run"), &p).on_click(
                        cx.listener(|this, _, window, cx| this.run_statement(window, cx)),
                    ),
                )
            })
            .child(
                bar_button(
                    "run-all",
                    if open.is_script() {
                        "Run file"
                    } else {
                        "Run All"
                    },
                    Some("run"),
                    &p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.run_all(cx))),
            )
            .when(crate::viewer::is_previewable(&open.notebook.path), |bar| {
                let path = open.notebook.path.clone();
                bar.child(
                    bar_button("preview", "Preview", Some("globe"), &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                if cfg!(target_os = "macos") {
                                    "Preview with Quarto in the Viewer (⇧⌘K)"
                                } else {
                                    "Preview with Quarto in the Viewer (Ctrl+Shift+K)"
                                },
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_document(path.clone(), cx)
                        })),
                )
                .child(self.render_document_button(cx))
            })
            .when(crate::jobs::is_job_script(&open.notebook.path), |bar| {
                bar.child(
                    bar_button("run-job", "Run as job", Some("terminal"), &p)
                        .on_click(cx.listener(|this, _, _, cx| this.run_open_as_job(cx))),
                )
            })
            .when(open.notebook.can_restructure(), |bar| {
                let on = self.ghost_cells;
                bar.child(
                    bar_button("ghost-cells", "Suggest", Some("sparkle.ai"), &p)
                        .when(on, |d| d.bg(p.accent_soft).text_color(p.accent))
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if on {
                                "Suggesting the next cell after each run: click to stop"
                            } else {
                                "Suggest the next cell after each run, with Claude Code or Codex"
                            })
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_ghost_cells(cx))),
                )
            })
            .when(!open.is_script(), |bar| {
                // Icon-only so Save stays visible beside Render and AI at
                // the default window width.
                bar.child(
                    icon_button("clear-all", "outputs.clear", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Clear all outputs")
                                .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.clear_outputs(None, cx))),
                )
            })
            .when(!open.is_script(), |bar| {
                bar.child(self.notebook_ai_button(cx))
            })
            .children(wrap_button)
            .when(!open.notebook.truncated, |bar| {
                bar.child(
                    bar_button("save", "Save", Some("doc.save"), &p)
                        .when(!open.dirty, |d| d.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
            })
            .children(self.session_buttons(open.session.id().is_some(), cx))
            .into_any_element()
    }

    /// A cell's editor, as tall as its text, so the notebook scrolls and
    /// the cell doesn't. Code sits in a box; text doesn't.
    fn render_cell_editor(
        &self,
        open: &Open,
        ix: usize,
        boxed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let editor = &open.editors[ix];
        let state = editor.read(cx);
        let line_count = state.value().split('\n').count().max(1);
        let lines = if state.display_map().folded_ranges().is_empty() {
            line_count
        } else {
            (0..line_count)
                .filter(|line| !state.display_map().is_buffer_line_hidden(*line))
                .count()
                .max(1)
        };
        let focused = open.current == Some(ix)
            || (open.current.is_none()
                && editor
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx))
            // A cell the agent just added or changed lights up.
            || open
                .lit
                .as_ref()
                .is_some_and(|(id, _)| *id == open.notebook.cells[ix].id);
        div()
            .w_full()
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                this.clear_language_popovers(Some(ix), cx);
            }))
            .when(boxed, |d| {
                d.bg(p.chunk)
                    .border_1()
                    .border_color(if focused { p.accent } else { p.line })
                    .rounded(px(RADIUS))
                    .overflow_hidden()
            })
            .on_action(cx.listener(move |this, _: &RunCell, window, cx| {
                this.run_and_advance(ix, window, cx)
            }))
            .on_action(cx.listener(move |this, _: &RunLine, _, cx| this.run_cell(ix, cx)))
            .child(
                Editor::new(editor)
                    .bordered(false)
                    .appearance(false)
                    .line_height(px(LINE_HEIGHT))
                    .when(!boxed, |e| {
                        e.font_family(".SystemUIFont").text_size(px(look::CHROME))
                    })
                    .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
            )
            .into_any_element()
    }

    /// A code cell's editor and outputs; `render_cells` puts the gutter
    /// and tools around them. Long outputs scroll in a box of their own;
    /// folded ones show a line that unfolds them.
    fn render_code_cell(
        &self,
        open: &Open,
        ix: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let cell = &open.notebook.cells[ix];
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id));
        let busy = open.sending.contains(&cell.id)
            || open.waiting.contains(&cell.id)
            || run.is_some_and(|r| !r.finished);
        let outputs: &[Value] = match run {
            Some(run) => &run.outputs,
            None => &cell.outputs,
        };
        let shown: Vec<Shown> = outputs.iter().map(show).collect();
        let pages = shown.iter().any(|s| matches!(s, Shown::Page(_)));
        // An error can come from a run that ended fine, as when a plot
        // fails to draw.
        let error_shown = shown.iter().any(|s| matches!(s, Shown::Error(_)));
        let count = shown.len();
        let failed = run.and_then(failed_status);
        let errored = run.and_then(|r| r.status.as_deref()) == Some("error");
        let folded = open.collapsed.contains(&cell.id);
        let outputs: Vec<AnyElement> = if folded {
            Vec::new()
        } else {
            shown
                .into_iter()
                .enumerate()
                .map(|(n, s)| {
                    let id = SharedString::from(format!("out-{}-{n}", cell.id));
                    self.render_output(s, id, PLOT_MAX_WIDTH, Some(ix), None, cx)
                })
                .collect()
        };
        let selected = open.current == Some(ix)
            || (open.current.is_none()
                && open.editors[ix]
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx))
            || open.lit.as_ref().is_some_and(|(id, _)| *id == cell.id);
        let border = if selected {
            p.accent
        } else if busy {
            p.busy
        } else if errored || failed.is_some() {
            p.err
        } else {
            p.line
        };
        let fixable = (errored || error_shown) && !busy && !folded;
        let line = self.run_line(open, ix, cx);
        let framed = line.is_some() || count > 0 || busy || failed.is_some();
        v_flex()
            .w_full()
            .child(self.render_cell_editor(open, ix, true, window, cx))
            .child(
                v_flex()
                    .when(framed, |d| {
                        // Start at the code box's lower edge, including the
                        // run metadata, so the two side lines stay connected.
                        d.mx(px(8.))
                            .mb(px(2.))
                            .border_l_1()
                            .border_r_1()
                            .border_b_1()
                            .rounded_b(px(RADIUS))
                            .border_color(border)
                    })
                    .children(line)
                    .children(self.render_agent_change(open, ix, cx))
                    .when(folded && count > 0, |d| {
                        d.child(
                            div()
                                .id(("unfold", ix))
                                .py(px(4.))
                                .px(px(12.))
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .cursor_pointer()
                                .hover(|d| d.text_color(p.accent))
                                .child(if count == 1 {
                                    "1 output hidden: show".to_string()
                                } else {
                                    format!("{count} outputs hidden: show")
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.toggle_outputs(ix, cx)),
                                ),
                        )
                    })
                    .when(
                        !outputs.is_empty() || failed.is_some() || busy && !folded,
                        |d| {
                            let scroll = self
                                .output_scrolls
                                .borrow_mut()
                                .entry(cell.id.clone())
                                .or_default()
                                .clone();
                            let inner = scroll.clone();
                            d.child(
                                // The wheel capture area exactly matches the
                                // scroll box. Metadata and borders outside it
                                // leave the wheel to the notebook.
                                div()
                                    .id(("outputs-wheel", ix))
                                    .w_full()
                                    .on_scroll(move |event, window, cx| {
                                        let dy = event.delta.pixel_delta(window.line_height()).y;
                                        if box_takes(inner.offset().y, inner.max_offset().y, dy) {
                                            cx.stop_propagation();
                                        }
                                    })
                                    .child(
                                        v_flex()
                                            .id(("outputs", ix))
                                            .track_scroll(&scroll)
                                            .w_full()
                                            .px(px(12.))
                                            .pt(px(2.))
                                            .pb(px(8.))
                                            .gap_1()
                                            // Long outputs scroll, while web pages
                                            // keep their own height.
                                            .when(!pages, |d| {
                                                d.max_h(px(OUTPUT_MAX_HEIGHT)).overflow_y_scroll()
                                            })
                                            .children(outputs)
                                            .when_some(failed, |d, s| {
                                                d.child(
                                                    div().text_color(p.err).cursor_text().child(
                                                        OutputText::new(
                                                            ("failed-output", ix),
                                                            format!("Run ended: {s}"),
                                                        ),
                                                    ),
                                                )
                                            }),
                                    ),
                            )
                        },
                    )
                    .when(fixable, |d| d.children(self.render_error_actions(ix, cx))),
            )
            .into_any_element()
    }

    /// One output. `cell` is the notebook cell it is under: web pages are
    /// drawn there (the Console leaves them to the Viewer), and its plots
    /// can be saved. Text can be selected and copied.
    fn render_output(
        &self,
        shown: Shown,
        id: SharedString,
        max_width: f32,
        cell: Option<usize>,
        console_run: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let pages = cell.is_some();
        let text_id = id.clone();
        let text = move |s: String| {
            div()
                .w_full()
                .font_family(look::MONO)
                .text_size(px(look::CODE))
                .line_height(px(LINE_HEIGHT))
                .when(cell.is_some(), |d| d.cursor_text())
                .child(OutputText::new(
                    text_id.clone(),
                    s.trim_end_matches('\n').to_string(),
                ))
        };
        let note_id = id.clone();
        let note = move |s: String| {
            div()
                .text_color(p.faint)
                .when(cell.is_some(), |d| d.cursor_text())
                .child(OutputText::new(note_id.clone(), s))
                .into_any_element()
        };
        match shown {
            Shown::Text(s) => text(s).into_any_element(),
            Shown::Warning(s) => text(s).text_color(p.warn).into_any_element(),
            Shown::Error(s) => text(s).text_color(p.err).into_any_element(),
            Shown::Note(s) => note(s),
            Shown::Table(table) => self.render_table(table, id, cell, cx),
            Shown::Page(_) if !pages => note("A web page: see the Viewer.".into()),
            Shown::Page(page) => match self.pages.get(&page.key()) {
                Some(view) => div()
                    .w_full()
                    .h(px(PAGE_HEIGHT))
                    .rounded(px(RADIUS))
                    .overflow_hidden()
                    .child(view.clone())
                    .into_any_element(),
                None => note("Loading the page…".into()),
            },
            Shown::Plot { value, .. } => match self.plots.get(&plot_key(&value)) {
                Some(Plot::Ready(image)) => {
                    let key = plot_key(&value);
                    let group: SharedString = format!("{id}-plot").into();
                    let copied = image.clone();
                    let run = console_run.map(str::to_string);
                    let sent_run = run.clone();
                    let sent_key = key.clone();
                    let zoom_key = key.clone();
                    let zoom_run = run.clone();
                    let bounds_key = format!("{id}-image");
                    let zoom_bounds_key = bounds_key.clone();
                    let save_bounds_key = bounds_key.clone();
                    let tools = Some(
                        h_flex()
                            .absolute()
                            .top(px(6.))
                            .right(px(6.))
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(8.))
                            .bg(p.bg)
                            .border_1()
                            .border_color(p.line)
                            .invisible()
                            .group_hover(group.clone(), |d| d.visible())
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-zoom")),
                                    "zoom",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Enlarge the image")
                                        .build(w, cx)
                                })
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(ix) = cell {
                                            this.open_cell_plot(
                                                ix,
                                                &zoom_key,
                                                &zoom_bounds_key,
                                                window,
                                                cx,
                                            );
                                        } else if let Some(run) = &zoom_run {
                                            this.open_console_plot(
                                                run,
                                                &zoom_key,
                                                &zoom_bounds_key,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-save")),
                                    "doc.save",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Save the plot…")
                                        .build(w, cx)
                                })
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(ix) = cell {
                                            this.open_cell_plot(
                                                ix,
                                                &key,
                                                &save_bounds_key,
                                                window,
                                                cx,
                                            );
                                        } else if let Some(run) = &run {
                                            this.open_console_plot(
                                                run,
                                                &key,
                                                &save_bounds_key,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-copy")),
                                    "edit.copy",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Copy the plot")
                                        .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.copy_image(&copied, cx)),
                                ),
                            )
                            .when_some(sent_run, |d, run| {
                                d.child(
                                    icon_button(
                                        SharedString::from(format!("{id}-send")),
                                        "send",
                                        &p,
                                        false,
                                    )
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Send to Plots")
                                            .build(w, cx)
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.send_console_plot(&run, &sent_key, cx);
                                        },
                                    )),
                                )
                            }),
                    );
                    div()
                        .id(SharedString::from(format!("{id}-box")))
                        .group(group)
                        .relative()
                        .max_w(px(max_width))
                        .on_prepaint(self.track_plot_bounds(bounds_key, image.clone()))
                        // No wider than the notebook, however wide drawn.
                        .child(
                            img(image.clone())
                                .max_w(relative(1.))
                                .object_fit(ObjectFit::Contain),
                        )
                        .children(tools)
                        .into_any_element()
                }
                Some(Plot::Failed(err)) => note(format!("The plot could not be shown: {err}")),
                _ => note("Loading the plot…".into()),
            },
        }
    }

    pub(crate) fn render_console_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let state = &self.loose;
        h_flex()
            .id("console-tools")
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .justify_end()
            .gap(px(4.))
            .text_color(p.muted)
            .when(self.project.is_some(), |d| {
                d.child(self.console_kernel_picker(cx))
            })
            .when(state.id().is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_none()
                        .child(
                            icon_button("console-restart", "refresh", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Restart: a new kernel, with nothing defined",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.restart_console_session(cx)),
                                ),
                        )
                        .child(
                            icon_button("console-stop", "power", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Shut down this session",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.stop_console_session(cx)),
                                ),
                        )
                        .child(
                            icon_button("console-send-plots", "send", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Send all plots to Plots",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.send_all_console_plots(cx)),
                                ),
                        ),
                )
            })
            .when(self.console_target().is_some(), |d| {
                d.child(
                    icon_button("interrupt", "interrupt", &p, false).on_click(cx.listener(
                        |this, _, _, _| {
                            if let (Some(daemon), Some(session)) =
                                (this.daemon(), this.console_target())
                            {
                                daemon.fire(sandhill_proto::Call::Interrupt {
                                    session: session.to_string(),
                                });
                            }
                        },
                    )),
                )
            })
    }

    pub(crate) fn render_console(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let focused = self
            .console_input
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        let edge = if focused {
            p.accent.opacity(0.6)
        } else {
            p.ink(if p.dark { 26. } else { 22. })
        };
        let entries = self
            .console
            .iter()
            .filter_map(|id| self.runs.get(id).map(|run| (id, run)))
            .filter(|(_, run)| self.notebook_console || !crate::plots::from_notebook(&run.cell))
            .map(|(run_id, run)| {
                let source = match (&run.cell, &run.by) {
                    // A script's runs are labelled with the script alone.
                    (Some((file, _)), _) if is_script(file) => {
                        Some(file.rsplit('/').next().unwrap_or(file).to_string())
                    }
                    (Some((file, cell)), _) => Some(format!(
                        "{} · {cell}",
                        file.rsplit('/').next().unwrap_or(file)
                    )),
                    (None, Some(by)) => Some(format!("run by {by}")),
                    _ => None,
                };
                let failed = failed_status(run);
                v_flex()
                    .gap_1()
                    .pb_2()
                    .when_some(source, |d, s| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(s),
                        )
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_1()
                            .font_family(look::MONO)
                            .text_size(px(look::CODE))
                            .line_height(px(LINE_HEIGHT))
                            .child(div().text_color(p.accent).child("›"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(p.muted)
                                    .child(run.highlighted_code(self.run_language(run), cx)),
                            ),
                    )
                    .children(run.outputs.iter().enumerate().map(|(n, o)| {
                        let id = SharedString::from(format!("cout-{run_id}-{n}"));
                        match show(o) {
                            Shown::Page(_)
                                if run.cell.as_ref().is_some_and(|(f, _)| !is_script(f)) =>
                            {
                                self.render_output(
                                    Shown::Note("A web page: shown under its cell.".into()),
                                    id,
                                    0.,
                                    None,
                                    Some(run_id),
                                    cx,
                                )
                            }
                            shown => self.render_output(
                                shown,
                                id,
                                self.files_width - 40.,
                                None,
                                Some(run_id),
                                cx,
                            ),
                        }
                    }))
                    .when_some(failed, |d, s| {
                        d.child(div().text_color(p.err).child(format!("Run ended: {s}")))
                    })
                    .when(!run.finished, |d| {
                        d.child(div().text_color(p.busy).child("running…"))
                    })
            })
            .collect::<Vec<_>>();
        let lines = self
            .console_input
            .read(cx)
            .value()
            .split('\n')
            .count()
            .clamp(1, 8);
        v_flex()
            .size_full()
            .child(
                look::PaneScroll::new(
                    "console-scroll",
                    look::STRIP,
                    6.,
                    div()
                        .id("console")
                        .px(px(14.))
                        .when(entries.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_color(p.muted)
                                    .child("Code run in this kernel shows here."),
                            )
                        })
                        .children(entries),
                )
                .handle(&self.console_scroll),
            )
            .child(
                div()
                    .flex_none()
                    .m(px(6.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(edge)
                    .px(px(4.))
                    .overflow_hidden()
                    .on_action(
                        cx.listener(|this, _: &RunCell, window, cx| this.run_console(window, cx)),
                    )
                    // History walk; otherwise Up/Down reach the editor.
                    .capture_action(cx.listener(|this, _: &input::MoveUp, window, cx| {
                        if this.console_history_step(true, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input::MoveDown, window, cx| {
                        if this.console_history_step(false, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Editor::new(&self.console_input)
                            .bordered(false)
                            .appearance(false)
                            .line_height(px(LINE_HEIGHT))
                            .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note = |s: String, color: Hsla| {
            div()
                .px(px(14.))
                .py(px(6.))
                .text_color(color)
                .child(s)
                .into_any_element()
        };
        match (&self.terminal, &self.terminal_problem) {
            (Some(view), _) => div()
                .size_full()
                .bg(p.term)
                .child(view.clone())
                .into_any_element(),
            (None, Some(err)) => note(format!("The terminal could not start: {err}"), p.err),
            (None, None) if self.project.is_none() => {
                note("Open a project to start a terminal in it.".into(), p.muted)
            }
            (None, None) => note("Starting the terminal…".into(), p.muted),
        }
    }

    pub(crate) fn render_viewer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some((item, ix, count)) = self.viewer_page() else {
            return div()
                .px(px(14.))
                .py(px(6.))
                .text_color(p.muted)
                .child("Web pages shown from the Console (plotly, DT, leaflet, Shiny…), Quarto previews and HTML files appear here.")
                .into_any_element();
        };
        let body = match (&item.page, &item.note) {
            (_, Some(note)) => div()
                .px(px(14.))
                .py(px(10.))
                .text_color(p.muted)
                .whitespace_normal()
                .child(note.clone())
                .into_any_element(),
            (Some(page), None) => match self.pages.get(&page.key()) {
                Some(view) => view.clone().into_any_element(),
                None => div().child("Loading the page…").into_any_element(),
            },
            (None, None) => div().into_any_element(),
        };
        let extra = item.extra.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(30.))
                    .px(px(4.))
                    .gap(px(2.))
                    .border_b(px(0.5))
                    .border_color(p.glass_edge)
                    .text_color(p.muted)
                    .child(
                        icon_button("viewer-back", "previous", &p, false)
                            .when(ix == 0, |d| d.opacity(0.35))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_viewer_page(ix.saturating_sub(1), cx)
                            })),
                    )
                    .child(
                        icon_button("viewer-next", "next", &p, false)
                            .when(ix + 1 >= count, |d| d.opacity(0.35))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ix + 1 < count {
                                    this.show_viewer_page(ix + 1, cx)
                                }
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(look::SMALL))
                            .child(format!("{} of {count}", ix + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl_2()
                            .text_color(p.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(item.title.clone()),
                    )
                    .child(
                        icon_button("viewer-reload", "refresh", &p, false)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Reload the page")
                                    .build(w, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.reload_viewer(cx))),
                    )
                    .child(
                        icon_button("viewer-browser", "globe", &p, false)
                            .when(item.page.is_none(), |d| d.opacity(0.35))
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Open in browser")
                                    .build(w, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_viewer_in_browser(cx)),
                            ),
                    )
                    .when_some(extra, |d, key| {
                        d.child(
                            bar_button(
                                "viewer-close",
                                if item.stoppable { "Stop" } else { "Close" },
                                Some(if item.stoppable { "stop" } else { "close" }),
                                &p,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.close_extra(&key, cx)),
                            ),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The Variables tab's body.
    pub(crate) fn render_variables(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let session = self.variables_session();
        // A list from a kernel that has gone, or from another one, is not shown.
        let live = session.is_some() && self.vars.session.as_deref() == session;
        let list: &[Variable] = if live { &self.vars.list } else { &[] };
        let mut rows = Vec::new();
        let filter = self.vars_filter.read(cx).value().to_string();
        let shown = filter_variables(list, &filter);
        self.variable_rows(&shown, &[], &mut rows, cx);
        let status = if session.is_none() {
            Some("No kernel running.".to_string())
        } else if let Some(err) = &self.vars.error {
            Some(err.clone())
        } else if list.is_empty() && !self.vars.loading {
            Some("No variables yet.".into())
        } else if shown.is_empty() && !list.is_empty() {
            Some(format!("No variables match “{}”.", filter.trim()))
        } else {
            None
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(look::PaneScroll::new(
                "variables-scroll",
                look::STRIP + if session.is_some() { 36. } else { 0. },
                4.,
                div()
                    .id("variables")
                    .pb(px(4.))
                    .when_some(status, |d, s| {
                        d.child(div().px(px(14.)).text_color(p.muted).child(s))
                    })
                    .children(rows),
            ))
            // The filter, Import and refresh, while a kernel runs.
            .when(session.is_some(), |d| {
                d.child(
                    h_flex()
                        .h(px(36.))
                        .gap(px(4.))
                        .pl(px(10.))
                        .pr(px(6.))
                        .pt(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(input::Input::new(&self.vars_filter).small()),
                        )
                        .child(self.import_button(cx))
                        .child(
                            icon_button("refresh-variables", "refresh", &p, false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(session) = this.vars.session.clone() {
                                        this.refresh_object_explorers(&session, cx);
                                    }
                                    this.refresh_variables(cx);
                                }),
                            ),
                        )
                        .map(|row| look::floating_header(&p, look::STRIP, false, row)),
                )
            })
            .into_any_element()
    }

    fn variable_rows(
        &self,
        list: &[Variable],
        parent: &[String],
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let p = *Palette::get(cx);
        for variable in list {
            let mut path = parent.to_vec();
            path.push(variable.name.clone());
            let open = self.vars.expanded.contains(&path);
            let toggle_path = path.clone();
            let table_path = path.clone();
            let hover = p.ink(7.);
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("var-{}", path.join("\u{1f}"))))
                    .mx(px(6.))
                    .px(px(8.))
                    .pl(px(8. + 14. * parent.len() as f32))
                    .gap_2()
                    .h(px(24.))
                    .rounded(px(RADIUS))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .hover(move |d| d.bg(hover))
                    .when(variable.children, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_variable(toggle_path.clone(), cx)
                            }))
                    })
                    .child(div().w(px(14.)).flex_none().text_color(p.faint).when(
                        variable.children,
                        |d| {
                            d.child(
                                icon(if open {
                                    "chevron.down"
                                } else {
                                    "chevron.right"
                                })
                                .size(px(13.)),
                            )
                        },
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(variable.name.clone()),
                    )
                    .child(div().text_color(p.faint).child(variable.class.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.muted)
                            .child(variable.summary.clone()),
                    )
                    .when(variable.table, |d| {
                        d.child(
                            icon_button(
                                SharedString::from(format!("view-{}", path.join("\u{1f}"))),
                                "table",
                                &p,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_explorer(table_path.clone(), window, cx)
                                },
                            )),
                        )
                    })
                    .child(self.variable_object_actions(variable, &path, cx))
                    .into_any_element(),
            );
            if open && let Some(children) = self.vars.children.get(&path) {
                self.variable_rows(children, &path, rows, cx);
            }
        }
    }

    /// The status bar: the current device and its resources, then the
    /// latest problem or notice.
    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        h_flex()
            .flex_none()
            .w_full()
            .h(px(look::STATUS_BAR))
            .pl(px(14.))
            .pr(px(12.))
            .gap(px(14.))
            .whitespace_nowrap()
            .overflow_hidden()
            .text_color(p.text)
            .child(self.render_connection_status(cx))
            .children(self.render_restricted_chip(cx))
            .children(self.render_resources(cx))
            .children(
                self.problem
                    .clone()
                    .or_else(|| self.notice.clone())
                    .map(|text| {
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(self.problem.is_some(), |d| d.text_color(p.err))
                            .child(text)
                    }),
            )
            .child(div().flex_1())
            .when_some(self.quota.clone(), |bar, quota| bar.child(quota))
    }
}

/// How a run ended, when that wasn't fine (or a plain error, which its
/// output already shows).
fn failed_status(run: &Run) -> Option<String> {
    run.status
        .as_deref()
        .filter(|s| *s != "ok" && *s != "error")
        .map(str::to_string)
}

fn cell_text(open: &Open, ix: usize, cx: &App) -> String {
    open.editors[ix].read(cx).value().to_string()
}

/// The variables whose names hold what was typed in the filter box,
/// ignoring case (all of them when nothing was typed).
pub(crate) fn filter_variables(list: &[Variable], filter: &str) -> Vec<Variable> {
    let wanted = filter.trim().to_lowercase();
    list.iter()
        .filter(|v| wanted.is_empty() || v.name.to_lowercase().contains(&wanted))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{box_takes, collapsed_file_groups, column_widths, filter_variables};
    use gpui_kit::px;

    #[test]
    fn visible_columns_share_all_available_room() {
        for width in [1000., 1440., 1920.] {
            for mask in 1u8..8 {
                let (left, middle, agents) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
                let (l, m, a) = column_widths(width, left, middle, agents, 0.25, 0.25);
                let gaps = (mask.count_ones() - 1) as f32 * crate::look::GAP;
                assert!((l + m + a + gaps + 2. * crate::look::EDGE - width).abs() < 0.01);
                assert_eq!(l > 0., left);
                assert_eq!(m > 0., middle);
                assert_eq!(a > 0., agents);
                if middle {
                    assert!(m >= 360.);
                }
            }
            let normal = column_widths(width, true, true, true, 0.25, 0.25);
            let hidden = column_widths(width, true, false, true, 0.25, 0.25);
            assert_eq!(normal.0, hidden.0);
            assert!(hidden.2 > normal.2);
        }
    }

    #[test]
    fn side_only_split_can_use_the_former_middle_width() {
        let (left, middle, agents) = column_widths(1440., true, false, true, 0.7, 0.25);
        assert!(left > 900.);
        assert_eq!(middle, 0.);
        assert!(agents >= 300.);
        let (_, middle, _) = column_widths(1000., true, true, true, 0.9, 0.9);
        assert!(middle >= 360.);
        assert_eq!(
            column_widths(0., true, true, true, 0.25, 0.25),
            (0., 0., 0.)
        );
    }

    #[test]
    fn collapsed_file_icons_group_by_source_type_in_first_occurrence_order() {
        let paths = [
            "first.R",
            "a.py",
            "second.r",
            "reads.fastq.gz",
            "reads.fastq",
            "notes.md",
        ]
        .map(str::to_string);
        let groups = collapsed_file_groups(&paths);
        assert_eq!(
            groups
                .iter()
                .map(|(_, files)| files.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 2], vec![1], vec![3, 4], vec![5]]
        );
        assert_eq!(groups[0].0, crate::look::file_icon("first.R"));
        assert_eq!(groups[2].0, crate::look::file_icon("reads.fastq.gz"));
    }

    #[test]
    fn outputs_hand_the_wheel_on_at_their_ends() {
        let max = px(300.);
        // At the top: up was skipped and goes to the notebook; down moved it.
        assert!(!box_takes(px(0.), max, px(10.)));
        assert!(box_takes(px(-10.), max, px(-10.)));
        // In the middle the box takes both ways.
        assert!(box_takes(px(-90.), max, px(10.)));
        assert!(box_takes(px(-110.), max, px(-10.)));
        // Past the bottom by a turn that moved it: kept.
        assert!(box_takes(px(-305.), max, px(-10.)));
        // At the bottom: down was skipped and goes on; up moved it.
        assert!(!box_takes(px(-300.), max, px(-10.)));
        assert!(box_takes(px(-290.), max, px(10.)));
        // A box that fits never takes it, nor a sideways turn.
        assert!(!box_takes(px(0.), px(0.), px(-10.)));
        assert!(!box_takes(px(-100.), max, px(0.)));
    }
    use sandhill_proto::Variable;

    #[test]
    fn filters_variables_by_name() {
        let var = |name: &str| Variable {
            name: name.into(),
            kind: "data".into(),
            class: "data.frame".into(),
            size: None,
            children: false,
            table: true,
            summary: String::new(),
            accessor: None,
        };
        let list = [var("seu"), var("Markers"), var("counts")];
        let names = |filter| {
            filter_variables(&list, filter)
                .into_iter()
                .map(|v| v.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["seu", "Markers", "counts"]);
        assert_eq!(names(" mark "), ["Markers"]);
        assert!(names("zzz").is_empty());
    }
}
