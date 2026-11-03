//! Drawing the window, in the Tauri app's look (see `look`): the title bar,
//! files on the wall, the notebook (or the Data Explorer) in a glass pane,
//! and on the right the Console, Terminal or Viewer above Variables.

use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{self, Editor};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::Variable;
use serde_json::Value;

use crate::command_palette::Cmd;
use crate::explorer::Explorer;
use crate::look::{
    self, EDGE, GAP, Palette, RADIUS, bar_button, capsule, capsule_divider, dot, icon, icon_button,
    pane, picture, raised, wall,
};
use crate::notebook::{self, CellKind};
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
/// Height of a web page under a cell.
const PAGE_HEIGHT: f32 = 420.;

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.report_focus(window, cx);
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
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
                if event.keystroke.key != "escape" {
                    return;
                }
                if this.plot_history.zoom.take().is_some()
                    || this.log.take().is_some()
                    || this.git.rewrite.take().is_some()
                {
                    cx.stop_propagation();
                    cx.notify();
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
                    .when(self.show_files, |d| {
                        d.child(self.render_left(cx))
                            .child(self.splitter(Split::Files, cx))
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(320.))
                            .h_full()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h(px(120.))
                                    .flex()
                                    .child(self.render_notebook(window, cx)),
                            )
                            .when(self.show_console, |d| {
                                d.child(self.splitter(Split::Console, cx))
                                    .child(self.render_console_pane(cx))
                            }),
                    )
                    .when_some(
                        self.agents.clone().filter(|_| self.show_agents),
                        |d, agents| {
                            d.child(self.splitter(Split::Agents, cx)).child(
                                div()
                                    .w(px(self.agents_width))
                                    .flex_shrink(1.)
                                    .min_w(px(300.))
                                    .h_full()
                                    .child(agents),
                            )
                        },
                    ),
            )
            .child(self.render_status(cx))
            .children(self.render_resource_popup(cx))
            .children(self.render_zoom(cx))
            .children(self.render_settings_dialog(cx))
            .children(self.render_import_dialog(cx))
            .children(self.render_main_menu(cx))
            .children(self.render_connection_menu(cx))
            .children(self.render_log_dialog(cx))
            .children(self.render_rewrite(cx))
            .children(self.render_tree_menu(cx))
            .children(self.render_palette(cx))
    }
}

/// Where the title bar's first capsule starts; on a Mac the window buttons
/// come first.
pub(crate) const TITLE_BAR_LEFT: f32 = if cfg!(target_os = "macos") { 78. } else { 6. };

/// The width the three panels share: the window's, less the room at its
/// edges and the gaps between them.
fn panel_room(width: f32) -> f32 {
    (width - 2. * EDGE - 2. * GAP).max(0.)
}

impl Workbench {
    /// The left and AI panels' widths from their shares of the window: a
    /// quarter each at first (1:2:1), the middle taking what is left.
    fn size_panels(&mut self, window: &Window) {
        let room = panel_room(f32::from(window.viewport_size().width));
        self.files_width = (room * self.left_share).max(220.);
        self.agents_width = (room * self.agents_share).max(300.);
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
        // The middle keeps at least this much room.
        let middle = 360.;
        let room = panel_room(width);
        let agents = if self.show_agents && self.agents.is_some() {
            self.agents_width
        } else {
            0.
        };
        let files = if self.show_files {
            self.files_width
        } else {
            0.
        };
        match split {
            Split::Files => {
                let most = (width - agents - middle).max(220.);
                self.left_share = (x - EDGE).clamp(220., most) / room;
            }
            Split::Agents => {
                let most = (width - files - middle).max(300.);
                self.agents_share = (width - x - EDGE).clamp(300., most) / room;
            }
            Split::Variables => {
                // The top pane's height, from the top of the column.
                let top = look::TITLE_BAR + GAP;
                let most = (height - look::STATUS_BAR - top - 120.).max(90.);
                self.variables_height = (y - top).clamp(90., most);
            }
            Split::Console => {
                let bottom = height - look::STATUS_BAR;
                let most = (bottom - look::TITLE_BAR - 160.).max(80.);
                self.console_height = (bottom - y).clamp(80., most);
            }
        }
        cx.notify();
    }

    /// The title bar: a capsule of window buttons after the Mac's own, and
    /// the theme button and the edit capsule at the right; the window draws the machine tabs
    /// between them.
    fn render_top_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let edit = |id: &'static str, name: &str, action: Box<dyn Action>| {
            icon_button(id, name, &p, true)
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        div()
            .relative()
            .flex_none()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                // Behind the capsules: dragging here moves the window, and a
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
                        capsule(&p, 25.)
                            .child(
                                icon_button("toggle-files", "sidebar.left", &p, true)
                                    .when(self.show_files, |d| d.text_color(p.text))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_files = !this.show_files;
                                        cx.notify();
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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_agents = !this.show_agents;
                                        cx.notify();
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
                        capsule(&p, 25.).child(
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
                        ),
                    )
                    .child(
                        capsule(&p, 25.)
                            .child(edit("undo", "edit.undo", Box::new(input::Undo)))
                            .child(edit("redo", "edit.redo", Box::new(input::Redo)))
                            .child(capsule_divider(&p))
                            .child(edit("cut", "edit.cut", Box::new(input::Cut)))
                            .child(edit("copy", "edit.copy", Box::new(input::Copy)))
                            .child(edit("paste", "edit.paste", Box::new(input::Paste)))
                            .child(capsule_divider(&p))
                            .child(
                                edit("save", "doc.save", Box::new(Save))
                                    .when(!self.open.as_ref().is_some_and(|o| o.dirty), |d| {
                                        d.opacity(0.4)
                                    }),
                            )
                            .child(
                                edit("save-all", "doc.save-all", Box::new(crate::SaveAll))
                                    .when(!self.any_dirty(), |d| d.opacity(0.4)),
                            ),
                    ),
            )
    }

    pub(crate) fn connection_text(&self) -> String {
        match (&self.link, &self.about) {
            (Link::Connecting, _) => "Connecting…".into(),
            (Link::Failed(_), _) => "Not connected".into(),
            (Link::Ready(_), Some(about)) => {
                format!("{} · sandhilld {}", about.hostname, about.daemon_version)
            }
            (Link::Ready(_), None) => "Connected".into(),
        }
    }

    /// The Files tab: the project's name, then its folders and files as a
    /// tree.
    pub(crate) fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let count = self.file_lines().len();
        let project = self.project.clone().unwrap_or_default();
        let open_path = self.open.as_ref().map(|o| o.notebook.path.clone());
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
                        let openable = notebook::format_of(&row.name).is_some();
                        let is_open = open_path.as_deref() == Some(&format!("{project}/{rel}"));
                        line.pl(px(indent + 20.))
                            .when(!openable, |d| d.text_color(p.muted))
                            .when(is_open, |d| d.bg(p.accent.opacity(0.2)))
                            .when(!is_open && is_selected, |d| d.bg(p.ink(9.)))
                            .when(!is_open, |d| d.hover(move |d| d.bg(hover)))
                            .cursor_pointer()
                            .child(picture(&look::file_icon(&row.name)))
                            .child(name)
                            .children(meta(!openable && !crate::viewer::is_html(&rel)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files_ui.selected = Some((rel.clone(), false));
                                if openable || crate::viewer::is_html(&rel) {
                                    this.open_file(rel.clone(), cx)
                                } else {
                                    cx.notify()
                                }
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full();
        v_flex()
            .relative()
            .size_full()
            .child(
                h_flex()
                    .h(px(30.))
                    .flex_none()
                    .pl(px(14.))
                    .pr(px(8.))
                    .gap(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(self.render_project_button(cx))
                    .when_some(self.tree.top_count(), |d, n| {
                        d.child(
                            div()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(p.faint)
                                .text_size(px(look::SMALL))
                                .child(n.to_string()),
                        )
                    })
                    .child(div().flex_1())
                    .when(self.project.is_some(), |d| {
                        let sort = self.tree.sort;
                        d.child(
                            icon_button("sort-files", "sort", &p, false)
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
                                })),
                        )
                        .child(
                            icon_button("new-item", "add", &p, false)
                                .when(self.files_ui.new_menu, |d| d.text_color(p.text))
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("New file or folder")
                                        .build(w, cx)
                                })
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.files_ui.new_menu = !this.files_ui.new_menu;
                                    this.files_ui.sort_menu = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            icon_button("refresh-files", "refresh", &p, false)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx))),
                        )
                    }),
            )
            .children(self.render_name_error(cx))
            .child(div().flex_1().min_h_0().px(px(6.)).pb(px(10.)).child(list))
            .children(self.render_new_menu(cx))
            .children(self.render_sort_menu(cx))
            .children(self.render_project_menu(cx))
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

    /// One tab in a strip's capsule.
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
            .flex_none()
            .h(px(18.))
            .gap(px(5.))
            .pl(px(9.))
            .pr(px(8.))
            .rounded_full()
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

    fn render_notebook(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        if self.diff_shown && self.diff.is_some() {
            return self.render_diff(cx);
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
        if open.is_script() {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .child(self.render_notebook_bar(open, cx))
                .child(self.render_script(open, cx))
                .into_any_element();
        }
        let bodies = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| match cell.kind {
                // As in the Tauri app, a notebook's Markdown shows
                // formatted; a Quarto document's prose stays text.
                CellKind::Text => {
                    if open.notebook.format != notebook::Format::Ipynb
                        || open.editing.contains(&cell.id)
                        || cell_text(open, ix, cx).trim().is_empty()
                    {
                        self.render_cell_editor(open, ix, false, window, cx)
                    } else {
                        self.render_markdown(open, ix, cx)
                    }
                }
                CellKind::Code => self.render_code_cell(open, ix, window, cx),
            })
            .collect::<Vec<_>>();
        let cells = self.render_cells(open, bodies, window, cx);
        let commanding = self.cells_focus.is_focused(window);
        let scroller = div()
            .id("cells")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.notebook_scroll)
            .track_focus(&self.cells_focus)
            .when(commanding, |d| d.key_context(crate::CELLS))
            .pt(px(10.))
            .pb(px(200.))
            .pl(px(6.))
            .pr(px(24.))
            // Between cells that can be added to, the rows that add them.
            .when(!open.notebook.can_restructure(), |d| d.gap(px(8.)))
            .children(cells);
        pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx))
            .child(self.render_notebook_bar(open, cx))
            .child(self.cell_keys(scroller, cx))
            .into_any_element()
    }

    /// The middle pane's tabs: the open notebooks, in the order they were
    /// opened, then the Data Explorer.
    pub(crate) fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let shown = match (&self.open, self.explorer_shown || self.diff_shown) {
            (_, true) => None,
            (Some(open), false) => Some(open.notebook.path.clone()),
            (None, false) => None,
        };
        let mut tabs: Vec<AnyElement> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, path)| {
                let name = path.rsplit('/').next().unwrap_or(path).to_string();
                let show = path.clone();
                Self::tab(
                    ("file-tab", ix),
                    Some(
                        picture(&look::file_icon(path))
                            .size(px(14.))
                            .into_any_element(),
                    ),
                    name,
                    shown.as_deref() == Some(path),
                    &p,
                )
                .child(self.close_button(ix, path.clone(), self.tab_dirty(path), cx))
                .on_click(cx.listener(move |this, _, _, cx| this.show_file_tab(&show, cx)))
                .into_any_element()
            })
            .collect();
        if let Some(explorer) = &self.explorer {
            let hover = p.ink(8.);
            let text = p.text;
            tabs.push(
                Self::tab(
                    "explorer-tab",
                    Some(icon("table").size(px(14.)).into_any_element()),
                    explorer.read(cx).name(),
                    self.explorer_shown && !self.diff_shown,
                    &p,
                )
                .child(
                    div()
                        .id("explorer-close")
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_color(p.faint)
                        .hover(move |d| d.bg(hover).text_color(text))
                        .child(icon("close").size(px(12.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_explorer(cx)
                        })),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.explorer_shown = true;
                    this.diff_shown = false;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        tabs.extend(self.diff_tab_element(cx));
        Self::strip(&p).child(capsule(&p, 22.).gap(px(1.)).min_w_0().children(tabs))
    }

    /// A tab's close button; unsaved, a dot that turns into it under the
    /// pointer.
    fn close_button(
        &self,
        ix: usize,
        path: String,
        dirty: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        let hover = p.ink(8.);
        let text = p.text;
        let group: SharedString = format!("close-tab-{ix}").into();
        div()
            .id(("close-tab", ix))
            .group(group.clone())
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .text_color(p.faint)
            .hover(move |d| d.bg(hover).text_color(text))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.ask_close(path.clone(), window, cx)
            }))
            .map(|d| {
                if dirty {
                    d.child(
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
                    d.child(icon("close").size(px(12.)))
                }
            })
    }

    fn render_explorer(&self, explorer: &Entity<Explorer>, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx).child(div().flex_1()))
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
                    "Right-click the machine's tab at the top to close it, then connect again from the Start screen.",
                ))
                .into_any_element(),
            Link::Ready(_) if self.project.is_some() => div()
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
            .px_6()
            .items_center()
            .justify_center()
            .text_color(p.muted)
            .line_height(rems(1.4))
            .child(body)
            .into_any_element()
    }

    /// The kernels to pick from, each a pill with its language's icon.
    fn kernel_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let chosen = self.active_kernel().map(str::to_string);
        // A script offers only kernels for its language.
        let only = self
            .open
            .as_ref()
            .filter(|o| o.is_script())
            .and_then(|o| o.notebook.language.clone());
        self.kernels
            .iter()
            .enumerate()
            .filter(|(_, spec)| {
                only.as_ref()
                    .is_none_or(|l| spec.language.eq_ignore_ascii_case(l))
            })
            .map(|(ix, spec)| {
                let name = spec.name.clone();
                let picked = chosen.as_deref() == Some(&spec.name);
                let hover = p.ink(8.);
                h_flex()
                    .id(("kernel", ix))
                    .flex_none()
                    .h(px(22.))
                    .gap(px(5.))
                    .pl(px(3.))
                    .pr(px(8.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .map(|d| {
                        if picked {
                            d.border_color(p.accent)
                                .bg(p.accent_soft)
                                .text_color(p.text)
                        } else {
                            d.border_color(p.line)
                                .text_color(p.muted)
                                .hover(move |d| d.bg(hover))
                        }
                    })
                    .map(|d| match look::language_icon(&spec.language) {
                        Some(lang) => d.child(picture(lang).size(px(14.))),
                        None => d.pl(px(8.)),
                    })
                    .child(spec.display_name.clone())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.choose_kernel(name.clone(), cx)),
                    )
                    .into_any_element()
            })
            .collect()
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

    fn session_text(session: &Session) -> String {
        match session {
            Session::None => "Kernel not started".into(),
            Session::Starting => "Starting kernel…".into(),
            Session::Ready(_) => "Kernel ready".into(),
            Session::Failed(err) => format!("Kernel: {err}"),
        }
    }

    fn session_dot(session: &Session, p: &Palette) -> Div {
        dot(match session {
            Session::None => p.faint,
            Session::Starting => p.busy,
            Session::Ready(_) => p.ok,
            Session::Failed(_) => p.err,
        })
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
        let source = cell_text(open, ix, cx);
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
                TextView::markdown(("md", ix), source).style(
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

    /// The bar under the notebook's tab: the kernels, then actions.
    fn render_notebook_bar(&self, open: &Open, cx: &mut Context<Self>) -> impl IntoElement {
        let kernels = self.kernel_buttons(cx);
        let p = *Palette::get(cx);
        h_flex()
            .flex_none()
            .h(px(30.))
            .px(px(4.))
            .gap(px(4.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .whitespace_nowrap()
            .overflow_hidden()
            .children(kernels)
            .child(div().w(px(8.)))
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
            })
            .when(crate::jobs::is_job_script(&open.notebook.path), |bar| {
                bar.child(
                    bar_button("run-job", "Run as job", Some("terminal"), &p)
                        .on_click(cx.listener(|this, _, _, cx| this.run_open_as_job(cx))),
                )
            })
            .when(!open.is_script(), |bar| {
                bar.child(
                    bar_button("clear-all", "Clear outputs", Some("outputs.clear"), &p)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_outputs(None, cx))),
                )
            })
            .child(
                bar_button("save", "Save", Some("doc.save"), &p)
                    .when(!open.dirty, |d| d.opacity(0.45))
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(px(6.))
                    .pr(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .child(Self::session_dot(&open.session, &p))
                    .child(Self::session_text(&open.session))
                    .children(self.session_buttons(open.session.id().is_some(), cx)),
            )
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
        let lines = editor.read(cx).value().split('\n').count().max(1);
        let focused = editor
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
            // A cell the agent just added or changed lights up.
            || open
                .lit
                .as_ref()
                .is_some_and(|(id, _)| *id == open.notebook.cells[ix].id);
        div()
            .w_full()
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
                    self.render_output(s, id, PLOT_MAX_WIDTH, Some(ix), cx)
                })
                .collect()
        };
        let border = if busy {
            p.busy
        } else if errored || failed.is_some() {
            p.err
        } else {
            p.line
        };
        let fixable = errored && !busy && !folded;
        v_flex()
            .w_full()
            .child(self.render_cell_editor(open, ix, true, window, cx))
            .children(self.render_agent_change(open, ix, cx))
            .when(folded && count > 0, |d| {
                d.child(
                    div()
                        .id(("unfold", ix))
                        .mt(px(4.))
                        .pl(px(12.))
                        .border_l_2()
                        .border_color(border)
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .cursor_pointer()
                        .hover(|d| d.text_color(p.accent))
                        .child(if count == 1 {
                            "1 output hidden: show".to_string()
                        } else {
                            format!("{count} outputs hidden: show")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_outputs(ix, cx))),
                )
            })
            .when(
                !outputs.is_empty() || failed.is_some() || busy && !folded,
                |d| {
                    d.child(
                        v_flex()
                            .id(("outputs", ix))
                            .mt(px(4.))
                            .my(px(2.))
                            .pl(px(12.))
                            .py(px(2.))
                            .gap_1()
                            .border_l_2()
                            .border_color(border)
                            // Long outputs scroll in their box, so the cells
                            // below stay near; web pages keep their own height.
                            .when(!pages, |d| {
                                d.max_h(px(OUTPUT_MAX_HEIGHT)).overflow_y_scroll()
                            })
                            .children(outputs)
                            .when_some(failed, |d, s| {
                                d.child(div().text_color(p.err).child(format!("Run ended: {s}")))
                            }),
                    )
                },
            )
            .when(fixable, |d| d.children(self.render_error_actions(ix, cx)))
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
                .child(SelectableText::new(
                    text_id.clone(),
                    s.trim_end_matches('\n').to_string(),
                ))
        };
        let note = |s: String| div().text_color(p.faint).child(s).into_any_element();
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
                    let tools = cell.map(|ix| {
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
                                        this.open_cell_plot(ix, &key, window, cx)
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
                    });
                    div()
                        .id(SharedString::from(format!("{id}-box")))
                        .group(group)
                        .relative()
                        .max_w(px(max_width))
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

    pub(crate) fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
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
                                    .child(run.code.trim_end().to_string()),
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
                                    cx,
                                )
                            }
                            shown => {
                                self.render_output(shown, id, self.files_width - 40., None, cx)
                            }
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
        let state = self.active_state();
        let header = h_flex()
            .flex_none()
            .min_h(px(30.))
            .px(px(4.))
            .gap(px(4.))
            .flex_wrap()
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .text_color(p.muted)
            .when(self.open.is_none() && self.project.is_some(), |d| {
                d.children(self.kernel_buttons(cx)).when(
                    matches!(self.loose, Session::None | Session::Failed(_)),
                    |d| {
                        d.child(
                            Button::new("start-console")
                                .xsmall()
                                .primary()
                                .label("Start")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.start_console_kernel(cx)),
                                ),
                        )
                    },
                )
            })
            .when_some(self.open.as_ref(), |d, open| {
                d.child(
                    h_flex()
                        .pl(px(6.))
                        .gap(px(6.))
                        .child(picture(&look::file_icon(&open.notebook.path)).size(px(14.)))
                        .child(open.notebook.name().to_string()),
                )
            })
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(px(6.))
                    .text_size(px(look::SMALL))
                    .child(Self::session_dot(state, &p))
                    .child(Self::session_text(state))
                    .children(self.session_buttons(state.id().is_some(), cx)),
            )
            .when(self.active_session().is_some(), |d| {
                d.child(
                    icon_button("interrupt", "interrupt", &p, false)
                        .on_click(cx.listener(|this, _, _, _| this.interrupt())),
                )
            });
        let lines = self
            .console_input
            .read(cx)
            .value()
            .split('\n')
            .count()
            .clamp(1, 8);
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("console")
                    .flex_1()
                    .min_h_0()
                    .px(px(14.))
                    .pt(px(6.))
                    .overflow_y_scroll()
                    .track_scroll(&self.console_scroll)
                    .when(entries.is_empty(), |d| {
                        d.child(
                            div()
                                .text_color(p.muted)
                                .child("Code run in this kernel shows here."),
                        )
                    })
                    .children(entries),
            )
            .child(
                div()
                    .flex_none()
                    .border_t(px(0.5))
                    .border_color(p.glass_edge)
                    .px(px(4.))
                    .on_action(
                        cx.listener(|this, _: &RunCell, window, cx| this.run_console(window, cx)),
                    )
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
        let mut rows = Vec::new();
        let filter = self.vars_filter.read(cx).value().to_string();
        let shown = filter_variables(&self.vars.list, &filter);
        self.variable_rows(&shown, &[], &mut rows, cx);
        let status = if self.active_session().is_none() {
            Some("No kernel running.".to_string())
        } else if let Some(err) = &self.vars.error {
            Some(err.clone())
        } else if self.vars.list.is_empty() && !self.vars.loading {
            Some("No variables yet.".into())
        } else if shown.is_empty() && !self.vars.list.is_empty() {
            Some(format!("No variables match “{}”.", filter.trim()))
        } else {
            None
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            // The filter, Import and refresh, while a kernel runs.
            .when(self.active_session().is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_none()
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
                            icon_button("refresh-variables", "refresh", &p, false)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_variables(cx))),
                        ),
                )
            })
            .child(
                div()
                    .id("variables")
                    .flex_1()
                    .min_h_0()
                    .py(px(4.))
                    .overflow_y_scroll()
                    .when_some(status, |d, s| {
                        d.child(div().px(px(14.)).text_color(p.muted).child(s))
                    })
                    .children(rows),
            )
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
                    .into_any_element(),
            );
            if open && let Some(children) = self.vars.children.get(&path) {
                self.variable_rows(children, &path, rows, cx);
            }
        }
    }

    /// The status bar: the connection, then the latest problem or a hint,
    /// and the machine's resources at the right.
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
            .text_color(p.muted)
            .child(self.render_connection_item(cx))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(self.problem.is_some(), |d| d.text_color(p.err))
                    .child(
                        self.problem
                            .clone()
                            .or_else(|| self.notice.clone())
                            .unwrap_or_else(|| {
                                if self.open.as_ref().is_some_and(|o| o.is_script()) {
                                    "⌘↵ runs the line · ⇧⌘↵ runs the file · ⌘S saves".into()
                                } else {
                                    "Shift+Enter runs a cell · ⌘↵ runs it in place · ⌘S saves"
                                        .into()
                                }
                            }),
                    ),
            )
            .child(div().flex_1())
            .children(self.render_resources(cx))
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
    use super::filter_variables;
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
