//! Drawing the window, in the Tauri app's look (see `look`): the title bar,
//! files on the wall, the notebook (or the Data Explorer) in a glass pane,
//! and on the right the Console, Terminal or Viewer above Variables.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{self, Editor, Input};
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::Variable;
use serde_json::Value;

use crate::explorer::Explorer;
use crate::look::{
    self, GAP, Palette, RADIUS, bar_button, capsule, capsule_divider, dot, icon, icon_button, pane,
    picture, raised, wall,
};
use crate::notebook::{self, CellKind};
use crate::shown::{Shown, show};
use crate::workbench::{
    Link, Open, Plot, RightTab, Run, Session, Split, Workbench, is_script, plot_key,
};
use crate::{Interrupt, RunAll, RunCell, RunLine, Save};

/// Height of one line of code (15px Maple Mono at 1.3), and an editor's
/// padding.
const LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING: f32 = 20.;
const PLOT_MAX_WIDTH: f32 = 720.;
/// The notebook's gutter, with each cell's number.
const GUTTER: f32 = 52.;
/// Height of a web page under a cell.
const PAGE_HEIGHT: f32 = 420.;

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.report_focus(window, cx);
        self.sync_pages(window, cx);
        let p = *Palette::get(cx);
        wall(&p)
            .flex()
            .flex_col()
            .size_full()
            .text_color(p.text)
            .text_size(px(look::CHROME))
            .line_height(rems(1.25))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run_all(cx)))
            .on_action(cx.listener(|this, _: &Interrupt, _, _| this.interrupt()))
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
                Some(Split::Variables) => d.cursor(CursorStyle::ResizeUpDown),
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
                    .pr(px(GAP))
                    .when(!self.show_files, |d| d.pl(px(GAP)))
                    .when(self.show_files, |d| {
                        d.child(self.render_files(cx))
                            .child(self.splitter(Split::Files, cx))
                    })
                    .when_some(
                        self.agents.clone().filter(|_| self.show_agents),
                        |d, agents| {
                            d.child(
                                div()
                                    .w(px(self.agents_width))
                                    .flex_shrink(1.)
                                    .min_w(px(300.))
                                    .h_full()
                                    .child(agents),
                            )
                            .child(self.splitter(Split::Agents, cx))
                        },
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .h_full()
                            .flex()
                            .child(self.render_notebook(window, cx)),
                    )
                    .when(self.show_right, |d| {
                        d.child(self.splitter(Split::Right, cx))
                            .child(self.render_right(cx))
                    }),
            )
            .child(self.render_status(cx))
    }
}

/// Where the title bar's first capsule starts; on a Mac the window buttons
/// come first.
const TITLE_BAR_LEFT: f32 = if cfg!(target_os = "macos") { 78. } else { 6. };

impl Workbench {
    /// A gap between panes that can be dragged; an accent line shows under
    /// the pointer.
    fn splitter(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let across = split == Split::Variables;
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
        let right = if self.show_right {
            self.right_width
        } else {
            0.
        };
        match split {
            Split::Files => {
                let most = (width - right - agents - middle).max(160.);
                self.files_width = x.clamp(160., most.min(560.));
            }
            Split::Agents => {
                let most = (width - files - right - middle).max(300.);
                self.agents_width = (x - files - GAP).clamp(300., most.min(900.));
            }
            Split::Right => {
                let most = (width - files - agents - middle).max(280.);
                self.right_width = (width - x - GAP).clamp(280., most);
            }
            Split::Variables => {
                let bottom = height - look::STATUS_BAR;
                let most = (bottom - look::TITLE_BAR - 160.).max(80.);
                self.variables_height = (bottom - y).clamp(80., most);
            }
        }
        cx.notify();
    }

    /// The title bar: a capsule of window buttons after the Mac's own, the
    /// machine's tab in the middle, and the edit capsule at the right.
    fn render_top_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let local = self.target == Target::Local;
        let (machine, device) = match &self.target {
            Target::Local => ("Local".to_string(), "device.laptop"),
            Target::Ssh(destination) => (destination.to_string(), "device.server"),
        };
        let link_color = match &self.link {
            Link::Connecting => p.busy,
            Link::Failed(_) => p.err,
            Link::Ready(_) => p.ok,
        };
        let path = self.project.clone().unwrap_or_else(|| "No project".into());
        let edit = |id: &'static str, name: &str, action: Box<dyn Action>| {
            icon_button(id, name, &p, true)
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        let machine_tab: AnyElement = if self.editing_host {
            div()
                .w(px(300.))
                .child(Input::new(&self.host_input).xsmall())
                .into_any_element()
        } else {
            raised(
                h_flex()
                    .id("machine-tab")
                    .gap(px(6.))
                    .h(px(21.))
                    .max_w(px(420.))
                    .px(px(12.))
                    .rounded_full()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.editing_host = true;
                        let input = this.host_input.clone();
                        input.update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })),
                &p,
            )
            .child(icon(device).size(px(14.)).text_color(p.muted))
            .child(div().font_weight(FontWeight::MEDIUM).child(machine))
            .child(
                div()
                    .text_color(p.faint)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .min_w_0()
                    .child(path),
            )
            .child(dot(link_color).size(px(6.)))
            .into_any_element()
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
                            .child(
                                icon_button("toggle-right", "sidebar.right", &p, true)
                                    .when(self.show_right, |d| d.text_color(p.text))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_right = !this.show_right;
                                        cx.notify();
                                    })),
                            )
                            .child(capsule_divider(&p))
                            .child(
                                icon_button("open-project", "folder", &p, true)
                                    .when(self.daemon().is_none() || !local, |d| d.opacity(0.32))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.daemon().is_some() && this.target == Target::Local {
                                            this.choose_project(cx)
                                        }
                                    })),
                            )
                            .when(self.project.is_some(), |d| {
                                d.child(
                                    icon_button("close-project", "close", &p, true).on_click(
                                        cx.listener(|this, _, _, cx| this.close_project(cx)),
                                    ),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        capsule(&p, 25.)
                            .child(edit("undo", "edit.undo", Box::new(input::Undo)))
                            .child(edit("redo", "edit.redo", Box::new(input::Redo)))
                            .child(capsule_divider(&p))
                            .child(edit("cut", "edit.cut", Box::new(input::Cut)))
                            .child(edit("copy", "edit.copy", Box::new(input::Copy)))
                            .child(edit("paste", "edit.paste", Box::new(input::Paste))),
                    ),
            )
            .child(
                // The machine's tab, centred in the window.
                h_flex()
                    .absolute()
                    .top(px(6.))
                    .left_0()
                    .right_0()
                    .h(px(25.))
                    .justify_center()
                    .child(
                        capsule(&p, 25.)
                            .child(machine_tab)
                            .when(!self.editing_host, |d| {
                                d.child(icon_button("new-machine", "add", &p, true).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.editing_host = true;
                                        let input = this.host_input.clone();
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx);
                                            input.focus(window, cx)
                                        });
                                        cx.notify();
                                    }),
                                ))
                            }),
                    ),
            )
    }

    fn connection_text(&self) -> String {
        match (&self.link, &self.about) {
            (Link::Connecting, _) => "Connecting…".into(),
            (Link::Failed(_), _) => "Not connected".into(),
            (Link::Ready(_), Some(about)) => {
                format!("{} · sandhilld {}", about.hostname, about.daemon_version)
            }
            (Link::Ready(_), None) => "Connected".into(),
        }
    }

    /// The left pane, directly on the wall: the project's name, then its
    /// folders and files as a tree.
    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let count = self.tree.rows().len();
        let project = self.project.clone().unwrap_or_default();
        let open_path = self.open.as_ref().map(|o| o.notebook.path.clone());
        let list = uniform_list(
            "files",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let rows = this.tree.rows();
                range
                    .filter_map(|ix| rows.get(ix).map(|row| (ix, row)))
                    .map(|(ix, row)| {
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
                        let name = div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(row.name.clone());
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
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_folder(rel.clone(), cx)
                                }))
                                .into_any_element();
                        }
                        let openable = notebook::format_of(&row.name).is_some();
                        let is_open = open_path.as_deref() == Some(&format!("{project}/{rel}"));
                        line.pl(px(indent + 20.))
                            .when(!openable, |d| d.text_color(p.muted))
                            .when(is_open, |d| d.bg(p.accent.opacity(0.2)))
                            .when(!is_open, |d| d.hover(move |d| d.bg(hover)))
                            .cursor_pointer()
                            .child(picture(&look::file_icon(&row.name)))
                            .child(name)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_file(rel.clone(), cx)),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full();
        let name = self
            .project
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string());
        v_flex()
            .w(px(self.files_width))
            .h_full()
            .flex_none()
            .child(
                h_flex()
                    .h(px(30.))
                    .flex_none()
                    .pl(px(14.))
                    .pr(px(8.))
                    .gap(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(icon("folder").size(px(15.)).text_color(p.muted))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(name.clone().unwrap_or_else(|| "No project".into())),
                    )
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
                        d.child(
                            icon_button("refresh-files", "refresh", &p, false)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx))),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().px(px(6.)).pb(px(10.)).child(list))
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
        let cells = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| match cell.kind {
                CellKind::Text => h_flex()
                    .items_start()
                    .child(div().w(px(GUTTER)).flex_none())
                    .child(div().flex_1().min_w_0().child(
                        // As in the Tauri app, a notebook's Markdown shows
                        // formatted; a Quarto document's prose stays text.
                        if open.notebook.format != notebook::Format::Ipynb
                            || open.editing.contains(&ix)
                            || cell_text(open, ix, cx).trim().is_empty()
                        {
                            self.render_cell_editor(open, ix, false, window, cx)
                        } else {
                            self.render_markdown(open, ix, cx)
                        },
                    ))
                    .into_any_element(),
                CellKind::Code => self.render_code_cell(open, ix, window, cx),
            })
            .collect::<Vec<_>>();
        pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx))
            .child(self.render_notebook_bar(open, cx))
            .child(
                div()
                    .id("cells")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.notebook_scroll)
                    .child(
                        v_flex()
                            .max_w(px(1100.))
                            .pt(px(10.))
                            .pb(px(200.))
                            .pl(px(6.))
                            .pr(px(24.))
                            .gap(px(8.))
                            .children(cells),
                    ),
            )
            .into_any_element()
    }

    /// The middle pane's tabs: the open notebooks, in the order they were
    /// opened, then the Data Explorer.
    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let shown = match (&self.open, self.explorer_shown) {
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
                    self.explorer_shown,
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
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
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
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.close_tab(&path, cx)
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
        let reload = explorer.clone();
        pane(&p)
            .flex_1()
            .h_full()
            .child(
                self.render_tabs(cx).child(div().flex_1()).child(
                    bar_button("explorer-refresh", "Refresh", Some("refresh"), &p)
                        .on_click(move |_, _, cx| reload.update(cx, |e, cx| e.reload(cx))),
                ),
            )
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
                    "Click the machine's name at the top to type another machine, and press Enter.",
                ))
                .into_any_element(),
            Link::Ready(_) if self.project.is_some() => div()
                .child("Pick a notebook (.ipynb, .qmd, .Rmd) or a script (.R, .py) on the left, or use the Console.")
                .into_any_element(),
            Link::Ready(_) => {
                let hover = p.ink(7.);
                let recent = self
                    .recent
                    .iter()
                    .filter(|p| p.exists)
                    .take(8)
                    .enumerate()
                    .map(|(ix, project)| {
                        let path = project.path.clone();
                        h_flex()
                            .id(("recent", ix))
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(RADIUS))
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_project(path.clone(), cx)
                            }))
                            .child(picture(&look::folder_icon(&project.name)))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_color(p.text).child(project.name.clone()))
                                    .child(
                                        div()
                                            .text_color(p.faint)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(project.path.clone()),
                                    ),
                            )
                    });
                v_flex()
                    .gap_3()
                    .w(px(480.))
                    .child(div().text_color(p.text).font_weight(FontWeight::SEMIBOLD).child("Open a project"))
                    .child(Input::new(&self.path_input))
                    .child(div().child(
                        "Type a folder and press Enter. On this Mac, the folder button at the top picks one.",
                    ))
                    .when(!self.recent.is_empty(), |d| {
                        d.child(
                            div()
                                .pt_2()
                                .text_color(p.text)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Recent"),
                        )
                        .child(v_flex().children(recent))
                    })
                    .into_any_element()
            }
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
                    open.editing.insert(ix);
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
                    .child(Self::session_text(&open.session)),
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
            .contains_focused(window, cx);
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
            || open.waiting.contains(&ix)
            || run.is_some_and(|r| !r.finished);
        let count = match run {
            Some(run) => run.execution_count,
            None => cell.execution_count,
        };
        let label = match count {
            _ if busy => "[*]".to_string(),
            Some(n) => format!("[{n}]"),
            None => String::new(),
        };
        let outputs: &[Value] = match run {
            Some(run) => &run.outputs,
            None => &cell.outputs,
        };
        let outputs: Vec<AnyElement> = outputs
            .iter()
            .map(|o| self.render_output(show(o), PLOT_MAX_WIDTH, true, cx))
            .collect();
        let failed = run.and_then(failed_status);
        let errored = run.and_then(|r| r.status.as_deref()) == Some("error");
        let group: SharedString = format!("cell-{ix}").into();
        let number = ix + 1;
        h_flex()
            .id(("cell", ix))
            .group(group.clone())
            .items_start()
            .w_full()
            .child(
                // The gutter: the cell's number (#3, as the Agents pane names
                // it), its run count, and Run under the pointer.
                v_flex()
                    .w(px(GUTTER))
                    .flex_none()
                    .items_end()
                    .pt(px(5.))
                    .pr(px(10.))
                    .gap(px(2.))
                    .font_family(look::MONO)
                    .text_color(p.faint)
                    .child(div().opacity(0.8).child(format!("#{number}")))
                    .when(!label.is_empty(), |d| {
                        d.child(div().when(busy, |d| d.text_color(p.busy)).child(label))
                    })
                    .child(
                        div()
                            .id(("run", ix))
                            .px(px(2.))
                            .rounded(px(RADIUS))
                            .text_color(p.accent)
                            .cursor_pointer()
                            .invisible()
                            .group_hover(group, |d| d.visible())
                            .child(icon("run").size(px(15.)))
                            .on_click(cx.listener(move |this, _, _, cx| this.run_cell(ix, cx))),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_cell_editor(open, ix, true, window, cx))
                    .when(!outputs.is_empty() || failed.is_some() || busy, |d| {
                        d.child(
                            v_flex()
                                .mt(px(4.))
                                .my(px(2.))
                                .pl(px(12.))
                                .py(px(2.))
                                .gap_1()
                                .border_l_2()
                                .border_color(if busy {
                                    p.busy
                                } else if errored || failed.is_some() {
                                    p.err
                                } else {
                                    p.line
                                })
                                .children(outputs)
                                .when_some(failed, |d, s| {
                                    d.child(
                                        div().text_color(p.err).child(format!("Run ended: {s}")),
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    /// `pages` says web pages are drawn here; the Console leaves them to
    /// the Viewer.
    fn render_output(&self, shown: Shown, max_width: f32, pages: bool, cx: &App) -> AnyElement {
        let p = *Palette::get(cx);
        let text = |s: String| {
            div()
                .font_family(look::MONO)
                .text_size(px(look::CODE))
                .line_height(px(LINE_HEIGHT))
                .child(s.trim_end_matches('\n').to_string())
        };
        let note = |s: String| div().text_color(p.faint).child(s).into_any_element();
        match shown {
            Shown::Text(s) => text(s).into_any_element(),
            Shown::Warning(s) => text(s).text_color(p.warn).into_any_element(),
            Shown::Error(s) => text(s).text_color(p.err).into_any_element(),
            Shown::Note(s) => note(s),
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
                Some(Plot::Ready(image)) => div()
                    .child(img(image.clone()).max_w(px(max_width)))
                    .into_any_element(),
                Some(Plot::Failed(err)) => note(format!("The plot could not be shown: {err}")),
                _ => note("Loading the plot…".into()),
            },
        }
    }

    /// The right column: the Console, Terminal or Viewer above Variables,
    /// each a pane.
    fn render_right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let tab =
            |id: &'static str, label: &'static str, which: RightTab, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.right_tab == which, &p).on_click(
                    cx.listener(move |this, _, window, cx| this.show_tab(which, window, cx)),
                )
            };
        let top: AnyElement = match self.right_tab {
            RightTab::Console => self.render_console(cx),
            RightTab::Terminal => self.render_terminal(cx),
            RightTab::Viewer => self.render_viewer(cx),
        };
        v_flex()
            .w(px(self.right_width))
            .h_full()
            // Gives way to the middle in a narrow window.
            .flex_shrink(1.)
            .min_w(px(280.))
            .child(
                pane(&p)
                    .flex_1()
                    .child(
                        Self::strip(&p).child(
                            capsule(&p, 22.)
                                .gap(px(1.))
                                .child(tab("tab-console", "Console", RightTab::Console, cx))
                                .child(tab("tab-terminal", "Terminal", RightTab::Terminal, cx))
                                .child(tab("tab-viewer", "Viewer", RightTab::Viewer, cx)),
                        ),
                    )
                    .child(div().flex_1().min_h_0().child(top)),
            )
            .child(self.splitter(Split::Variables, cx))
            .child(self.render_variables(cx))
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let entries = self
            .console
            .iter()
            .filter_map(|id| self.runs.get(id).map(|run| (id, run)))
            .map(|(_, run)| {
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
                    .children(run.outputs.iter().map(|o| match show(o) {
                        Shown::Page(_) if run.cell.as_ref().is_some_and(|(f, _)| !is_script(f)) => {
                            self.render_output(
                                Shown::Note("A web page: shown under its cell.".into()),
                                0.,
                                false,
                                cx,
                            )
                        }
                        shown => self.render_output(shown, self.right_width - 40., false, cx),
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
                    .child(Self::session_text(state)),
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

    fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
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

    fn render_viewer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some((page, title, ix, count)) = self.viewer_page() else {
            return div()
                .px(px(14.))
                .py(px(6.))
                .text_color(p.muted)
                .child("Web pages shown from the Console (plotly, DT, leaflet…) appear here.")
                .into_any_element();
        };
        let body = match self.pages.get(&page.key()) {
            Some(view) => view.clone().into_any_element(),
            None => div().child("Loading the page…").into_any_element(),
        };
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
                            .pl_2()
                            .text_color(p.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn render_variables(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let mut rows = Vec::new();
        self.variable_rows(&self.vars.list, &[], &mut rows, cx);
        let status = if self.active_session().is_none() {
            Some("No kernel running.".to_string())
        } else if let Some(err) = &self.vars.error {
            Some(err.clone())
        } else if self.vars.list.is_empty() && !self.vars.loading {
            Some("No variables yet.".into())
        } else {
            None
        };
        pane(&p)
            .h(px(self.variables_height))
            .flex_none()
            .child(
                Self::strip(&p)
                    .child(capsule(&p, 22.).child(Self::tab(
                        "tab-variables",
                        None,
                        "Variables",
                        true,
                        &p,
                    )))
                    .when(self.vars.loading, |d| {
                        d.child(
                            div()
                                .pl_1()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child("updating…"),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        icon_button("refresh-variables", "refresh", &p, false)
                            .when(self.active_session().is_none(), |d| d.opacity(0.35))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_variables(cx))),
                    ),
            )
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
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_explorer(table_path.clone(), cx)
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

    /// The status bar: the connection, then the latest problem or a hint.
    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let link_color = match &self.link {
            Link::Connecting => p.busy,
            Link::Failed(_) => p.err,
            Link::Ready(_) => p.ok,
        };
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
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(dot(link_color))
                    .child(self.connection_text()),
            )
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(self.problem.is_some(), |d| d.text_color(p.err))
                    .child(self.problem.clone().unwrap_or_else(|| {
                        if self.open.as_ref().is_some_and(|o| o.is_script()) {
                            "⌘↵ runs the line · ⇧⌘↵ runs the file · ⌘S saves".into()
                        } else {
                            "Shift+Enter runs a cell · ⌘↵ runs it in place · ⌘S saves".into()
                        }
                    })),
            )
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
