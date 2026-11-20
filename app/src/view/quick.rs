//! The Explore a Table window (`crate::quick`): a title bar naming the session
//! and its tables, then the movable and resizable Explore panels.

use std::borrow::Cow;
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use sandhill_proto::{Known, QuickColumnKind, QuickPlot, QuickTableKind, QuickValues};

use super::*;
use crate::look::side_pane;
use crate::quick::{self, Edit, Item};
use crate::quick_layout::{Layout, Panel as QuickPanel, Side};

mod differential;

/// The width of a fact's name on the card.
const FACT_LABEL: f32 = 92.;

/// The table names genes by Ensembl or Entrez IDs, which symbols can
/// replace on plots.
fn ids_to_name(table: &sandhill_proto::QuickCardTable) -> bool {
    table.gene.as_ref().and_then(Known::usable).is_some()
        && matches!(
            table.gene_ids.as_ref().and_then(Known::usable),
            Some(sandhill_proto::GeneIds::Ensembl | sandhill_proto::GeneIds::Entrez)
        )
}

/// One preference for all Explore windows, loaded on first use.
struct ExploreLayout(Layout);

impl Global for ExploreLayout {}

impl Default for ExploreLayout {
    fn default() -> Self {
        Self(crate::prefs::explore_layout())
    }
}

fn save_layout(layout: Layout, cx: &mut App) {
    if layout != cx.global::<ExploreLayout>().0 {
        crate::prefs::save_explore_layout(&layout);
        cx.global_mut::<ExploreLayout>().0 = layout;
    }
}

fn visible_layout(ai_shown: bool, cx: &App) -> Cow<'_, Layout> {
    let layout = &cx.global::<ExploreLayout>().0;
    if layout.is_visible(QuickPanel::Ai) == ai_shown {
        Cow::Borrowed(layout)
    } else {
        let mut layout = layout.clone();
        layout.set_visible(QuickPanel::Ai, ai_shown);
        Cow::Owned(layout)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuickSplit {
    Column(usize),
    Stack(usize),
}

struct DraggedSplit {
    split: QuickSplit,
    grab: Cell<Pixels>,
}

impl Render for DraggedSplit {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct LayoutWindow {
    _observe: Subscription,
    // Resize locally until the mouse goes up, then save and update the other windows.
    resize: Option<(QuickSplit, Layout)>,
}

impl LayoutWindow {
    fn layout<'a>(&'a self, ai_shown: bool, cx: &'a App) -> Cow<'a, Layout> {
        self.resize.as_ref().map_or_else(
            || visible_layout(ai_shown, cx),
            |(_, layout)| Cow::Borrowed(layout),
        )
    }

    fn drag(&mut self, event: &DragMoveEvent<DraggedSplit>, cx: &mut Context<Self>) {
        let Some((split, layout)) = &mut self.resize else {
            return;
        };
        let at = event.event.position - event.bounds.origin;
        let grab = event.drag(cx).grab.get();
        let changed = match *split {
            QuickSplit::Column(i) => {
                let room =
                    f32::from(event.bounds.size.width) - (layout.columns().len() - 1) as f32 * GAP;
                layout.resize_columns(i, f32::from(at.x - grab) - i as f32 * GAP, room)
            }
            QuickSplit::Stack(i) => {
                let room = f32::from(event.bounds.size.height) - GAP;
                layout.resize_stack(i, f32::from(at.y - grab), room)
            }
        };
        if changed {
            cx.notify();
        }
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if let Some((_, layout)) = self.resize.take() {
            save_layout(layout, cx);
            cx.notify();
        }
    }
}

impl QuickPanel {
    fn title(self) -> &'static str {
        match self {
            Self::Explorer => "Data Explorer",
            Self::Plots => "Plots",
            Self::Guide => "Guide",
            Self::Ai => "AI",
        }
    }
}

#[derive(Clone, Copy)]
struct DraggedPanel {
    panel: QuickPanel,
    from: WindowId,
}

impl DraggedPanel {
    fn side(&self, bounds: Bounds<Pixels>, window: &Window) -> Option<Side> {
        if self.from != window.window_handle().window_id()
            || !bounds.contains(&window.mouse_position())
        {
            return None;
        }
        // Take the nearest edge in panel-relative coordinates, so wide or
        // tall panes still offer all four directions.
        let at = window.mouse_position() - bounds.origin;
        let x = f32::from(at.x) / f32::from(bounds.size.width) - 0.5;
        let y = f32::from(at.y) / f32::from(bounds.size.height) - 0.5;
        Some(if x.abs() >= y.abs() {
            if x < 0. { Side::Left } else { Side::Right }
        } else if y < 0. {
            Side::Top
        } else {
            Side::Bottom
        })
    }
}

impl Render for DraggedPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(cx);
        crate::agents::composer_view::popup(p).child(self.panel.title())
    }
}

fn drop_outline(mut bounds: Bounds<Pixels>, side: Side, accent: Hsla, window: &mut Window) {
    match side {
        Side::Left | Side::Right => {
            bounds.size.width /= 2.;
            if side == Side::Right {
                bounds.origin.x += bounds.size.width;
            }
        }
        Side::Top | Side::Bottom => {
            bounds.size.height /= 2.;
            if side == Side::Bottom {
                bounds.origin.y += bounds.size.height;
            }
        }
    }
    window.paint_quad(outline(bounds, accent, BorderStyle::Solid).corner_radii(px(RADIUS)));
}

fn panel_drop_target(panel: QuickPanel, ai_shown: bool, p: &Palette) -> Div {
    // The overlay sits above the pane's controls and scroll areas without
    // occluding them. Share its painted bounds with the drop handler.
    let preview = Rc::new(Cell::new((Bounds::default(), None)));
    let paint = preview.clone();
    let hover = preview.clone();
    let accent = p.accent;
    div()
        .absolute()
        .inset_0()
        .drag_over::<DraggedPanel>(move |style, dragged, window, cx| {
            let (bounds, _) = hover.get();
            let side = dragged.side(bounds, window).and_then(|side| {
                visible_layout(ai_shown, cx).drop_side(dragged.panel, panel, side)
            });
            hover.set((bounds, side));
            style
        })
        .on_drop(move |dragged: &DraggedPanel, window, cx| {
            if let Some(side) = dragged.side(preview.get().0, window) {
                let mut layout = visible_layout(ai_shown, cx).into_owned();
                if layout.move_panel(dragged.panel, panel, side) {
                    save_layout(layout, cx);
                }
            }
        })
        .child(
            canvas(
                move |bounds, _, _| {
                    paint.set((bounds, None));
                    paint
                },
                move |bounds, paint, window, _| {
                    if let Some(side) = paint.get().1 {
                        drop_outline(bounds, side, accent, window);
                    }
                },
            )
            .size_full(),
        )
}

fn layout_menu(p: &Palette) -> impl IntoElement {
    Button::new("quick-layout-menu")
        .ghost()
        .small()
        .accessibility_label("Explore layout")
        .tooltip("Explore layout")
        .child(icon("more").size(px(look::ICON)).text_color(p.muted))
        .dropdown_menu_with_anchor(Anchor::TopRight, |menu, _, _| {
            menu.item(PopupMenuItem::new("Reset Layout").on_click(|_, _, cx| {
                let mut layout = cx.global::<ExploreLayout>().0.clone();
                if layout.reset() {
                    save_layout(layout, cx);
                }
            }))
        })
}

impl Workbench {
    fn quick_ai_shown(&self, cx: &App) -> bool {
        self.quick
            .as_ref()
            .is_some_and(|q| !q.ai_hidden && cx.global::<ExploreLayout>().0.ai_visible(q.ai_ready))
    }

    pub(super) fn render_quick_top_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        cx.default_global::<ExploreLayout>();
        let ai_shown = self.quick_ai_shown(cx);
        let name = self.quick_name();
        let unkept = self.quick_unkept();
        let folder = self.quick_folder();
        let mut tables = look::tab_capsule(&p).gap(px(1.));
        let mut any = false;
        for (ix, table) in q.card.tables.iter().enumerate() {
            if !q.tables.iter().any(|t| t.name == table.name) {
                continue;
            }
            any = true;
            let label = match &table.sheet {
                Some(sheet) => format!("{} · {sheet}", table.file),
                None => table.file.clone(),
            };
            let picked = q.viewing.as_deref() == Some(&table.name);
            let name = table.name.clone();
            tables = tables.child(
                Self::tab(("quick-table", ix), None, label, picked, &p).on_click(cx.listener(
                    move |this, _, window, cx| this.quick_pick(name.clone(), window, cx),
                )),
            );
        }
        for (ix, (output, _)) in q.outputs.iter().enumerate() {
            any = true;
            let picked = q.viewing.as_deref() == Some(output);
            let name = output.clone();
            tables = tables.child(
                Self::tab(("quick-output", ix), None, output.clone(), picked, &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.quick_view_output(name.clone(), window, cx)
                    }),
                ),
            );
        }
        let ready = q.loaded_in.is_some() && q.busy.is_none();
        div()
            .relative()
            .flex_none()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                div()
                    .id("quick-title-drag")
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
                    .pr(px(look::GAP_8))
                    .gap(px(look::GAP_8))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .when(any, |d| d.child(tables.min_w_0().overflow_x_hidden()))
                    .child(div().flex_1())
                    .map(|d| match (unkept, folder) {
                        (true, _) => d.child(
                            Button::new("quick-save")
                                .small()
                                .label("Save Session…")
                                .disabled(q.keeping.is_some())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.quick_needs_folder(quick::KeepReason::Save, window, cx)
                                })),
                        ),
                        (false, Some(folder)) => d.child(
                            Button::new("quick-reveal-folder")
                                .small()
                                .label("Show in Finder")
                                .on_click(move |_, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&folder))
                                }),
                        ),
                        (false, None) => d,
                    })
                    .child(
                        Button::new("quick-export")
                            .small()
                            .label("Export…")
                            .disabled(!ready || q.keeping.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_needs_folder(quick::KeepReason::Export, window, cx)
                            })),
                    )
                    .child(
                        Button::new("quick-open-project")
                            .small()
                            .label("Open as Project")
                            .disabled(q.keeping.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_needs_folder(quick::KeepReason::OpenProject, window, cx)
                            })),
                    )
                    .child(
                        icon_button("quick-toggle-ai", "chat", &p, true)
                            .when(ai_shown, |d| d.text_color(p.text))
                            .tooltip(|window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Show or hide the AI pane",
                                )
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let visible = !this.quick_ai_shown(cx);
                                if let Some(quick) = &mut this.quick {
                                    quick.ai_hidden = false;
                                }
                                let mut layout = cx.global::<ExploreLayout>().0.clone();
                                layout.choose_ai_visibility(visible);
                                save_layout(layout, cx);
                                cx.notify();
                            })),
                    )
                    .child(layout_menu(&p)),
            )
            .into_any_element()
    }

    pub(super) fn render_quick_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let ai_shown = self.quick_ai_shown(cx);
        let state = window.use_keyed_state("explore-layout-state", cx, |window, cx| LayoutWindow {
            _observe: window.observe_global::<ExploreLayout>(cx, |window, _| window.refresh()),
            resize: None,
        });
        let layout = state.read(cx).layout(ai_shown, cx);
        let count = layout.columns().len();
        let room = (f32::from(window.viewport_size().width) - 2. * EDGE - (count - 1) as f32 * GAP)
            .max(0.);
        let stack_room = (f32::from(window.viewport_size().height)
            - look::TITLE_BAR
            - look::STATUS_BAR
            - 2. * GAP)
            .max(0.);
        let widths = layout.widths(room);
        let columns = std::array::from_fn::<_, 3, _>(|i| layout.columns().get(i).copied());
        let id = window.window_handle().window_id();
        let finish = state.clone();
        let finish_out = state.clone();
        let resize = state.clone();
        let mut workspace = h_flex()
            .size_full()
            .items_stretch()
            .on_drag_move(move |event: &DragMoveEvent<DraggedSplit>, _, cx| {
                resize.update(cx, |state, cx| state.drag(event, cx));
            })
            .capture_any_mouse_up(move |event, _, cx| {
                if event.button == MouseButton::Left {
                    finish.update(cx, LayoutWindow::finish);
                }
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                finish_out.update(cx, LayoutWindow::finish);
            })
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.bg(p.accent_soft))
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, _, cx| {
                let paths = dropped
                    .paths()
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                this.quick_add(paths, cx);
            }));
        for (i, (width, column)) in widths.into_iter().zip(columns).enumerate() {
            let Some(column) = column else { break };
            let mut body = v_flex().w(px(width)).h_full().min_w_0().flex_none();
            if let Some(bottom) = column.bottom() {
                body = body
                    .child(
                        self.render_quick_panel(column.top, ai_shown, id, cx)
                            .h(px(column.top_height(stack_room)))
                            .flex_none(),
                    )
                    .child(Self::quick_splitter(
                        QuickSplit::Stack(i),
                        ai_shown,
                        &state,
                        cx,
                    ))
                    .child(self.render_quick_panel(bottom, ai_shown, id, cx).flex_1());
            } else {
                body = body.child(
                    self.render_quick_panel(column.top, ai_shown, id, cx)
                        .h_full(),
                );
            }
            if i > 0 {
                workspace = workspace.child(Self::quick_splitter(
                    QuickSplit::Column(i - 1),
                    ai_shown,
                    &state,
                    cx,
                ));
            }
            workspace = workspace.child(body);
        }
        workspace.into_any_element()
    }

    fn quick_splitter(
        split: QuickSplit,
        ai_shown: bool,
        state: &Entity<LayoutWindow>,
        cx: &App,
    ) -> Stateful<Div> {
        let across = matches!(split, QuickSplit::Stack(_));
        let dragging = state
            .read(cx)
            .resize
            .as_ref()
            .is_some_and(|(which, _)| *which == split);
        let state = state.clone();
        Self::splitter_handle(
            format!("quick-split-{split:?}").into(),
            across,
            dragging,
            Palette::get(cx),
        )
        .on_drag(
            DraggedSplit {
                split,
                grab: Cell::default(),
            },
            move |drag, grab, _, cx| {
                let grab = if across { grab.y } else { grab.x };
                drag.grab.set(grab.clamp(px(0.), px(GAP)));
                let layout = visible_layout(ai_shown, cx).into_owned();
                state.update(cx, |state, cx| {
                    state.resize = Some((drag.split, layout));
                    cx.notify();
                });
                cx.stop_propagation();
                cx.new(|_| DraggedSplit {
                    split,
                    grab: Cell::default(),
                })
            },
        )
    }

    fn quick_header(panel: QuickPanel, id: WindowId, p: &Palette) -> Div {
        Self::strip(p).child(Self::quick_title(panel, id).flex_1())
    }

    /// A panel's name, which drags the panel.
    fn quick_title(panel: QuickPanel, id: WindowId) -> Stateful<Div> {
        h_flex()
            .id(("quick-panel-header", panel as usize))
            .self_stretch()
            .min_w_0()
            .pl(px(look::GAP_8))
            .cursor(CursorStyle::OpenHand)
            .child(panel.title())
            .on_drag(DraggedPanel { panel, from: id }, |dragged, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| *dragged)
            })
    }

    /// The table panel's header: its name, then the Data Explorer's own
    /// tools (search, filter, export, read again, a window of its own,
    /// settings), as the project window shows them above a table; they
    /// scroll sideways when the panel is narrow.
    fn quick_explorer_header(
        explorer: &Entity<crate::panes::explorer::Explorer>,
        id: WindowId,
        p: &Palette,
        cx: &App,
    ) -> Div {
        Self::strip(p)
            .child(Self::quick_title(QuickPanel::Explorer, id).flex_none())
            .child(
                h_flex()
                    .id("quick-explorer-tools")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .child(explorer.read(cx).toolbar()),
            )
    }

    fn render_quick_panel(
        &self,
        panel: QuickPanel,
        ai_shown: bool,
        id: WindowId,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *Palette::get(cx);
        let content = match panel {
            QuickPanel::Explorer => self.render_quick_explorer(id, cx),
            QuickPanel::Plots => self.render_quick_plots(id, cx),
            QuickPanel::Guide => self.render_quick_guide(id, cx),
            // The conversation's own header is the panel's top (Huan,
            // 2026-11-19): no "AI" strip above it.
            QuickPanel::Ai => div()
                .flex_1()
                .h_full()
                .min_h_0()
                .child(self.render_agents_pane(cx))
                .into_any_element(),
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_h_0()
            .child(content)
            .child(panel_drop_target(panel, ai_shown, &p))
    }

    fn render_quick_explorer(&self, id: WindowId, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        if let Some(explorer) = &self.explorer {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(Self::quick_explorer_header(explorer, id, &p, cx))
                .child(div().flex_1().min_h_0().child(explorer.clone()))
                .into_any_element();
        }
        let waiting = self
            .quick
            .as_ref()
            .and_then(|q| q.busy.clone())
            .unwrap_or_else(|| {
                "Your tables show here. Drop more tables onto this window to add them.".into()
            });
        pane(&p)
            .flex_1()
            .h_full()
            .child(Self::quick_header(QuickPanel::Explorer, id, &p))
            .child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .px(px(look::GAP_12))
                    .text_color(p.muted)
                    .child(div().max_w(px(320.)).text_center().child(waiting)),
            )
            .into_any_element()
    }

    fn render_quick_plots(&self, id: WindowId, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let empty = self.plots_now().1.is_none();
        side_pane(&p)
            .relative()
            .flex_1()
            .h_full()
            .child(
                Self::quick_header(QuickPanel::Plots, id, &p).children(self.plot_tools(cx)),
            )
            .child(div().flex_1().min_h_0().map(|d| {
                if empty {
                    d.flex()
                        .items_center()
                        .justify_center()
                        .px(px(look::GAP_12))
                        .text_color(p.muted)
                        .child(
                            div()
                                .max_w(px(300.))
                                .text_center()
                                .child("Plots you make in the Guide show here. Save one with the save button above."),
                        )
                } else {
                    d.child(self.render_plots(cx))
                }
            }))
            .into_any_element()
    }

    fn render_quick_guide(&self, id: WindowId, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let mut body = v_flex()
            .gap(px(look::GAP_8))
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .child(
                Button::new("quick-add-table")
                    .small()
                    .label("Add Table…")
                    .disabled(q.busy.is_some() || q.archive_picker.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.quick_choose_tables(cx))),
            );
        if q.not_ready {
            body = body.child(
                v_flex()
                    .gap(px(look::GAP_8))
                    .child(div().text_color(p.warn).child(look::mac_or(
                        "R isn't ready to explore tables on this Mac yet. Getting Ready in Welcome shows what is missing and installs it.",
                        "R isn't ready to explore tables on this computer yet. Getting Ready in Welcome shows what is missing and installs it.",
                    )))
                    .child(
                        Button::new("quick-setup")
                            .small()
                            .label("Open Getting Ready")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.defer(|cx| {
                                    crate::shell::app_window::show_welcome(
                                        Some(crate::welcome::start_screen::StartMode::Ready),
                                        cx,
                                    )
                                });
                            })),
                    ),
            );
        }
        if !q.ai_ready {
            body = body.child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(
                        div()
                            .text_color(p.muted)
                            .child("The AI helper isn't set up. Everything here works without it."),
                    )
                    .child(
                        Button::new("setup-explore-ai")
                            .small()
                            .label("Set Up an AI Helper…")
                            .on_click(|_, _, cx| {
                                cx.defer(|cx| {
                                    crate::shell::app_window::show_welcome(
                                        Some(crate::welcome::start_screen::StartMode::Ready),
                                        cx,
                                    )
                                });
                            }),
                    ),
            );
        }
        if let Some(busy) = &q.busy {
            body = body.child(div().text_color(p.muted).child(busy.clone()));
        }
        if let Some(error) = &q.error {
            body = body.child(div().text_color(p.err).child(error.clone()));
            // A template's packages are installed from Getting Ready.
            if error.contains("Missing R packages") {
                body = body.child(
                    Button::new("quick-open-ready")
                        .small()
                        .label("Open Getting Ready…")
                        .on_click(|_, _, cx| {
                            cx.defer(|cx| {
                                crate::shell::app_window::show_welcome(
                                    Some(crate::welcome::start_screen::StartMode::Ready),
                                    cx,
                                )
                            });
                        }),
                );
            }
        }
        if let Some(path) = &q.exported {
            let at = path.clone();
            body = body.child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(p.muted)
                            .child("Saved the workbook and methods.txt in results."),
                    )
                    .child(
                        Button::new("quick-reveal")
                            .small()
                            .label("Show in Finder")
                            .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&at))),
                    ),
            );
        }
        if self.quick_table().is_some() || q.card_read {
            body = body
                .child(heading("What we know", &p))
                .children(self.render_quick_suggestions(q, &p, cx));
        }
        if let Some(table) = self.quick_table().cloned() {
            body = body
                .child(self.render_quick_card(&table, cx))
                .when(
                    table.kind.usable() == Some(&QuickTableKind::Counts),
                    |body| {
                        body.child(heading("Find Changed Genes", &p))
                            .child(self.render_find_changed_genes(&table, cx))
                    },
                )
                .child(heading("Make Plots", &p))
                .child(self.render_quick_make(&table, cx));
            if let Some(sentence) = &q.sentence {
                body = body.child(
                    div()
                        .p(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .bg(p.ink(look::ROW_HOVER))
                        .child(sentence.clone()),
                );
            }
        }
        side_pane(&p)
            .flex_1()
            .h_full()
            .child(Self::quick_header(QuickPanel::Guide, id, &p))
            .child(
                div()
                    .id("quick-guide")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_size(px(look::small()))
                    .child(body),
            )
            .into_any_element()
    }

    fn render_quick_suggestions(
        &self,
        q: &quick::Quick,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let suggestions = crate::agents::quick::suggestions(&q.card);
        if suggestions.is_empty() {
            return None;
        }
        let mut list = v_flex().gap(px(look::GAP_4));
        for (i, suggestion) in suggestions.into_iter().enumerate() {
            let table_name = suggestion.table.map(str::to_string);
            let field = suggestion.field;
            let table = suggestion
                .table
                .and_then(|name| q.card.tables.iter().find(|t| t.name == name));
            let label = table.map_or_else(
                || suggestion.label.to_string(),
                |table| match &table.sheet {
                    Some(sheet) => format!("{} · {sheet}: {}", table.file, suggestion.label),
                    None => format!("{}: {}", table.file, suggestion.label),
                },
            );
            list = list.child(
                h_flex()
                    .items_start()
                    .gap(px(look::GAP_4))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(p.muted).child(label))
                            .child(div().child(suggestion.value)),
                    )
                    .child(
                        Button::new(("accept-quick-suggestion", i))
                            .small()
                            .label("Accept")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.quick_accept(Some((table_name.as_deref(), field)), cx);
                            })),
                    ),
            );
        }
        Some(
            v_flex()
                .gap(px(look::GAP_4))
                .p(px(look::GAP_8))
                .rounded(px(look::CONTROL_RADIUS))
                .bg(p.ink(look::ROW_HOVER))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Suggested by AI"),
                )
                .child(list)
                .child(
                    Button::new("accept-all-quick-suggestions")
                        .small()
                        .label("Accept All")
                        .on_click(cx.listener(|this, _, _, cx| this.quick_accept(None, cx))),
                )
                .into_any_element(),
        )
    }

    /// The facts about the table picked, and the session's own.
    fn render_quick_card(
        &self,
        table: &sandhill_proto::QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let kind = table
            .kind
            .usable()
            .copied()
            .unwrap_or(QuickTableKind::Unknown);
        let mut card = v_flex().gap(px(look::GAP_2));
        if table.dates > 0 {
            card = card.child(div().text_color(p.warn).child(format!(
                "{} gene names look like dates Excel made (1-Mar, 2-Sep). Those genes can't be matched by name.",
                table.dates
            )));
        }
        if let Some(question) = &table.question {
            card = card.child(
                div()
                    .text_color(p.warn)
                    .child(format!("{question} Choose below what the table is.")),
            );
        }
        card = card.child(
            self.fact(
                Item::Kind,
                "Table",
                table
                    .kind
                    .usable()
                    .map(|_| (quick::kind_label(kind).to_string(), table.kind.from)),
                cx,
            ),
        );
        let column = |known: &Option<Known<String>>| {
            known
                .as_ref()
                .and_then(|k| Some((k.usable()?.clone(), k.from)))
        };
        let gene = table
            .gene
            .as_ref()
            .filter(|g| g.usable().is_some())
            .map(|g| {
                let ids = table
                    .gene_ids
                    .as_ref()
                    .and_then(Known::usable)
                    .map(|&i| format!(" ({})", quick::ids_label(i)))
                    .unwrap_or_default();
                (format!("{}{ids}", g.value), g.from)
            });
        match kind {
            QuickTableKind::Results => {
                card = card
                    .child(self.fact(Item::Gene, "Genes", gene, cx))
                    .children(self.symbols_line(table, &p, cx))
                    .child(self.fact(Item::Fc, "Fold change", column(&table.fc), cx))
                    .child(self.fact(Item::Padj, "Adjusted p", column(&table.padj), cx))
                    .child(self.fact(Item::Mean, "Average", column(&table.mean), cx));
                if let Some(samples) = table.samples.as_ref().and_then(Known::usable) {
                    card = card
                        .child(self.fact(
                            Item::Values,
                            "Sample values",
                            table.values.as_ref().and_then(|v| {
                                Some((quick::values_label(*v.usable()?).to_string(), v.from))
                            }),
                            cx,
                        ))
                        .child(self.fact(
                            Item::Groups,
                            "Sample factors",
                            self.factor_summary(samples),
                            cx,
                        ));
                }
                let comparison = quick::comparison(&q.card, table)
                    .filter(|c| c.usable().is_some())
                    .map(|c| {
                        (
                            format!("higher in {} than in {}", c.value.higher, c.value.lower),
                            c.from,
                        )
                    });
                card = card.child(self.fact(Item::Comparison, "Positive means", comparison, cx));
                let omnibus = table.differential.as_ref().is_some_and(|info| info.omnibus);
                if omnibus {
                    card = card.child(div().text_color(p.muted)
                        .child("P-values test the full design against the reduced design. Fold changes show the chosen comparison."));
                }
                card = card.child(self.fact(
                    Item::Cutoffs,
                    "Cutoffs",
                    Some((
                        if omnibus {
                            format!("adjusted p below {}", q.card.cutoffs.padj)
                        } else {
                            format!(
                                "adjusted p below {}, |log2 fold change| at least {}",
                                q.card.cutoffs.padj, q.card.cutoffs.lfc
                            )
                        },
                        sandhill_proto::QuickFrom::Told,
                    )),
                    cx,
                ));
            }
            QuickTableKind::Counts | QuickTableKind::Normalized => {
                let samples = table.samples.as_ref().and_then(Known::usable);
                card = card
                    .child(self.fact(Item::Gene, "Genes", gene, cx))
                    .children(self.symbols_line(table, &p, cx))
                    .child(
                        div()
                            .pl(px(FACT_LABEL))
                            .text_color(p.muted)
                            .child(format!("{} samples", samples.map_or(0, Vec::len))),
                    )
                    .child(self.fact(
                        Item::Values,
                        "Values",
                        table.values.as_ref().and_then(|v| {
                            Some((quick::values_label(*v.usable()?).to_string(), v.from))
                        }),
                        cx,
                    ));
                if let Some(samples) = samples {
                    card = card.child(self.fact(
                        Item::Groups,
                        "Sample factors",
                        self.factor_summary(samples),
                        cx,
                    ));
                }
            }
            QuickTableKind::Enrichment => {
                card = card
                    .child(self.fact(Item::Term, "Terms", column(&table.term), cx))
                    .child(self.fact(Item::Padj, "Adjusted p", column(&table.padj), cx))
                    .child(self.fact(Item::Count, "Gene count", column(&table.count), cx))
                    .child(self.fact(Item::Ratio, "Gene ratio", column(&table.ratio), cx));
            }
            QuickTableKind::SampleSheet | QuickTableKind::Unknown => {}
        }
        // No Quick Plot needs the organism: it shows once known, or when
        // gene symbols need it.
        let organism = q.card.organism.as_ref().filter(|o| o.usable().is_some());
        if organism.is_some() || ids_to_name(table) {
            card = card.child(self.fact(
                Item::Organism,
                "Organism",
                organism.map(|o| (o.value.clone(), o.from)),
                cx,
            ));
        }
        card.into_any_element()
    }

    /// Under the genes: gene symbols for Ensembl or Entrez IDs (Huan,
    /// 2026-11-19; `docs/design/gene-symbols-plan.md`): Add Gene Symbols,
    /// or, once added, Use IDs Instead.
    fn symbols_line(
        &self,
        table: &sandhill_proto::QuickCardTable,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !ids_to_name(table) {
            return None;
        }
        let q = self.quick.as_ref()?;
        let busy = q.busy.is_some();
        let line = h_flex()
            .pl(px(FACT_LABEL))
            .gap(px(look::GAP_8))
            .text_color(p.muted);
        if let Some(column) = table.labels.as_ref().and_then(Known::usable) {
            return Some(
                line.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(format!("Plots name genes by the symbols in {column}.")),
                )
                .child(
                    Button::new("quick-use-ids")
                        .small()
                        .label("Use IDs Instead")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.quick_edit(Edit::Labels(None), cx)),
                        ),
                )
                .into_any_element(),
            );
        }
        let organism = q.card.organism.as_ref().and_then(Known::usable);
        let Some(package) = organism.and_then(|o| sandhill_proto::symbols_package(o)) else {
            return Some(
                line.child("Gene symbols need the organism: human or mouse.")
                    .into_any_element(),
            );
        };
        Some(
            line.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(format!("Add a symbol for each gene, from {package}.")),
            )
            .child(
                Button::new("quick-add-symbols")
                    .small()
                    .primary()
                    .label("Add Gene Symbols")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.quick_add_symbols(cx))),
            )
            .into_any_element(),
        )
    }

    fn factor_summary(&self, samples: &[String]) -> Option<(String, sandhill_proto::QuickFrom)> {
        let q = self.quick.as_ref()?;
        let known = q.card.factors.as_ref()?;
        let matching: Vec<_> = known
            .usable()?
            .iter()
            .filter(|f| {
                f.assignments
                    .iter()
                    .any(|(sample, _)| samples.contains(sample))
            })
            .map(|f| format!("{} ({})", f.name, f.levels.len()))
            .collect();
        (!matching.is_empty()).then(|| (matching.join(", "), known.from))
    }

    /// One fact: its name, value and where it came from; a click opens its
    /// choices under it.
    fn fact(
        &self,
        item: Item,
        label: &'static str,
        value: Option<(String, sandhill_proto::QuickFrom)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let open = self.quick.as_ref().is_some_and(|q| q.open == Some(item));
        let hover = p.ink(look::ROW_HOVER);
        let (text, from) = match value {
            Some((text, from)) => (text, Some(from)),
            None => ("Still needed".to_string(), None),
        };
        // Cutoffs nobody changed are Sandhill's defaults.
        let default_cutoffs = item == Item::Cutoffs
            && self
                .quick
                .as_ref()
                .is_some_and(|q| q.card.cutoffs == sandhill_proto::QuickCutoffs::default());
        let row = h_flex()
            .id(SharedString::from(format!("quick-fact-{item:?}")))
            .min_h(px(look::row()))
            .px(px(look::GAP_4))
            .py(px(look::GAP_2))
            .gap(px(look::GAP_4))
            .rounded(px(look::CONTROL_RADIUS))
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(
                div()
                    .w(px(FACT_LABEL))
                    .flex_none()
                    .text_color(p.muted)
                    .child(label),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_color(if from.is_some() { p.text } else { p.warn })
                            .child(text),
                    )
                    .children(from.map(|from| {
                        div().text_size(px(look::tiny())).text_color(p.faint).child(
                            if default_cutoffs {
                                "default"
                            } else {
                                quick::from_label(from)
                            },
                        )
                    })),
            )
            .child(
                icon(if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                })
                .size(px(look::ICON))
                .text_color(p.faint),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.quick_toggle(item, cx)));
        v_flex()
            .child(row)
            .when(open, |d| d.child(self.render_choices(item, cx)))
            .into_any_element()
    }

    /// The choices under an opened fact.
    fn render_choices(&self, item: Item, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let Some(table) = self.quick_table() else {
            return div().into_any_element();
        };
        let loaded = q.tables.iter().find(|t| t.name == table.name);
        // Each row of choices needs ids of its own: shared ids made only
        // the first sample's group choices clickable.
        let pills = |key: &str, choices: Vec<(String, bool, Edit)>, cx: &mut Context<Self>| {
            let mut row = h_flex().flex_wrap().gap(px(look::GAP_4));
            for (ix, (label, picked, edit)) in choices.into_iter().enumerate() {
                let edit = std::cell::RefCell::new(Some(edit));
                row = row.child(
                    Self::tab(
                        SharedString::from(format!("quick-choice-{item:?}-{key}-{ix}")),
                        None,
                        label,
                        picked,
                        &p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(edit) = edit.borrow_mut().take() {
                            this.quick_edit(edit, cx);
                        }
                    })),
                );
            }
            row
        };
        let body: AnyElement = match item {
            Item::Kind => pills(
                "kind",
                [
                    QuickTableKind::Results,
                    QuickTableKind::Counts,
                    QuickTableKind::Normalized,
                    QuickTableKind::Enrichment,
                    QuickTableKind::SampleSheet,
                    QuickTableKind::Unknown,
                ]
                .into_iter()
                .map(|k| {
                    (
                        quick::kind_label(k).to_string(),
                        table.kind.value == k,
                        Edit::Kind(k),
                    )
                })
                .collect(),
                cx,
            )
            .into_any_element(),
            Item::Gene
            | Item::Fc
            | Item::Padj
            | Item::Mean
            | Item::Term
            | Item::Count
            | Item::Ratio => {
                let current = match item {
                    Item::Gene => &table.gene,
                    Item::Fc => &table.fc,
                    Item::Padj => &table.padj,
                    Item::Mean => &table.mean,
                    Item::Term => &table.term,
                    Item::Count => &table.count,
                    _ => &table.ratio,
                };
                let text = matches!(item, Item::Gene | Item::Term);
                let columns: Vec<(String, bool, Edit)> = loaded
                    .map(|t| {
                        t.columns
                            .iter()
                            .filter(|c| {
                                item == Item::Ratio || (c.kind == QuickColumnKind::Number) != text
                            })
                            .map(|c| {
                                (
                                    c.name.clone(),
                                    current.as_ref().is_some_and(|k| k.value == c.name),
                                    Edit::Column(item, c.name.clone()),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                pills("column", columns, cx).into_any_element()
            }
            Item::Values => pills(
                "values",
                [
                    QuickValues::Counts,
                    QuickValues::Normalized,
                    QuickValues::Logged,
                ]
                .into_iter()
                .map(|v| {
                    (
                        quick::values_label(v).to_string(),
                        table.values.as_ref().is_some_and(|k| k.value == v),
                        Edit::Values(v),
                    )
                })
                .collect(),
                cx,
            )
            .into_any_element(),
            Item::Organism => {
                let now = q.card.organism.as_ref().map(|o| o.value.as_str());
                pills(
                    "organism",
                    vec![
                        (
                            "human".into(),
                            now == Some("human"),
                            Edit::Organism(Some("human".into())),
                        ),
                        (
                            "mouse".into(),
                            now == Some("mouse"),
                            Edit::Organism(Some("mouse".into())),
                        ),
                        (
                            "Other or not sure".into(),
                            now.is_none(),
                            Edit::Organism(None),
                        ),
                    ],
                    cx,
                )
                .into_any_element()
            }
            Item::Groups => self.render_sample_factors(table, cx),
            Item::Comparison => {
                let inputs = q
                    .inputs
                    .as_ref()
                    .map(|i| (i.higher.clone(), i.lower.clone()));
                let names = quick::colour_factor(&q.card, table, None)
                    .map_or(&[][..], |f| f.levels.as_slice());
                let now = quick::comparison(&q.card, table).map(|c| &c.value);
                let ways: Vec<(String, bool, Edit)> = names
                    .iter()
                    .flat_map(|higher| {
                        names
                            .iter()
                            .filter(move |lower| *lower != higher)
                            .map(move |lower| {
                                (
                                    format!("{higher} vs {lower}"),
                                    now.is_some_and(|c| &c.higher == higher && &c.lower == lower),
                                    Edit::Comparison(higher.clone(), lower.clone()),
                                )
                            })
                    })
                    .take(12)
                    .collect();
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(
                        div()
                            .text_color(p.muted)
                            .child("Which group is a positive fold change higher in?"),
                    )
                    .when(!ways.is_empty(), |d| d.child(pills("comparison", ways, cx)))
                    .children(inputs.map(|(higher, lower)| {
                        h_flex()
                            .gap(px(look::GAP_4))
                            .child(div().flex_none().child("Higher in"))
                            .child(div().flex_1().child(Input::new(&higher).small()))
                            .child(div().flex_none().child("than in"))
                            .child(div().flex_1().child(Input::new(&lower).small()))
                    }))
                    .child(
                        Button::new("quick-save-comparison")
                            .small()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let higher = this.quick_input(|i| &i.higher, cx);
                                let lower = this.quick_input(|i| &i.lower, cx);
                                this.quick_edit(Edit::Comparison(higher, lower), cx);
                            })),
                    )
                    .into_any_element()
            }
            Item::Cutoffs => {
                let omnibus = table.differential.as_ref().is_some_and(|info| info.omnibus);
                let inputs = q.inputs.as_ref().map(|i| (i.padj.clone(), i.lfc.clone()));
                v_flex()
                    .gap(px(look::GAP_4))
                    .children(inputs.map(|(padj, lfc)| {
                        h_flex()
                            .gap(px(look::GAP_4))
                            .child(div().flex_none().child("Adjusted p below"))
                            .child(div().flex_1().child(Input::new(&padj).small()))
                            .when(!omnibus, |row| {
                                row.child(div().flex_none().child("log2 fold change"))
                                    .child(div().flex_1().child(Input::new(&lfc).small()))
                            })
                    }))
                    .child(
                        Button::new("quick-save-cutoffs")
                            .small()
                            .label("Save")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let now = this
                                    .quick
                                    .as_ref()
                                    .map(|q| q.card.cutoffs)
                                    .unwrap_or_default();
                                let padj = this
                                    .quick_input(|i| &i.padj, cx)
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .filter(|v| *v > 0. && *v <= 1.)
                                    .unwrap_or(now.padj);
                                let lfc = if omnibus {
                                    now.lfc
                                } else {
                                    this.quick_input(|i| &i.lfc, cx)
                                        .trim()
                                        .parse::<f64>()
                                        .ok()
                                        .filter(|v| *v >= 0.)
                                        .unwrap_or(now.lfc)
                                };
                                this.quick_edit(Edit::Cutoffs(padj, lfc), cx);
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .pl(px(look::GAP_8))
            .py(px(look::GAP_4))
            .child(body)
            .into_any_element()
    }

    /// A button for each plot the table can make; one that can't be made
    /// yet says what it needs.
    fn render_quick_make(
        &self,
        table: &sandhill_proto::QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let kind = table
            .kind
            .usable()
            .copied()
            .unwrap_or(QuickTableKind::Unknown);
        let plots = quick::plots_for(kind);
        if plots.is_empty() {
            return div()
                .text_color(p.muted)
                .child(match kind {
                    QuickTableKind::SampleSheet => {
                        "A sample sheet has nothing to plot on its own. Its factors can be set on a counts table's card."
                    }
                    _ => "Choose what this table is above to see what can be made from it.",
                })
                .into_any_element();
        }
        let show_gene = self.quick_input(|i| &i.gene, cx);
        let busy = q.busy.is_some() || q.loaded_in.is_none();
        let mut list = v_flex()
            .gap(px(look::GAP_4))
            .children(self.render_colour_choice(table, cx));
        for (ix, plot) in plots.iter().copied().enumerate() {
            if plot == QuickPlot::GeneByGroup
                && let Some(input) = q.inputs.as_ref().map(|i| i.gene.clone())
            {
                list = list.child(div().child(Input::new(&input).small()));
            }
            let needs = quick::missing(table, plot, &show_gene);
            let tip = (!needs.is_empty()).then(|| format!("Needs {}.", needs.join(", ")));
            list = list.child(
                div()
                    .id(("quick-make-wrap", ix))
                    .child(
                        Button::new(("quick-make", ix))
                            .small()
                            .w_full()
                            .label(quick::plot_label(plot))
                            .disabled(busy || tip.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.quick_make(plot, cx))),
                    )
                    .when_some(tip, |d, tip| {
                        d.tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        })
                    }),
            );
        }
        list.into_any_element()
    }

    /// "Keep the plots and steps from de_results?" (Huan, 2026-11-19):
    /// where and under which name, in the trust card's style. Asked on
    /// Save Session…, Export…, Open as Project, and closing after making
    /// something; nothing is made beside the tables before this.
    pub(crate) fn render_quick_keep(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = self.quick.as_ref()?;
        let keeping = q.keeping.as_ref()?;
        let p = *Palette::get(cx);
        let reason = keeping.reason;
        let closing = reason == quick::KeepReason::Close;
        let title = format!("Keep the plots and steps from {}?", self.quick_name());
        let detail = match reason {
            quick::KeepReason::Close => {
                "Sandhill makes a folder with copies of the tables, the results and steps.R. If you don't keep them, nothing is left behind."
            }
            quick::KeepReason::Export => {
                "Exported files go in a folder of their own, with copies of the tables, the results and steps.R."
            }
            quick::KeepReason::OpenProject => {
                "A project needs a folder: Sandhill makes one with copies of the tables, the results and steps.R."
            }
            quick::KeepReason::Save => {
                "Sandhill makes a folder with copies of the tables, the results and steps.R. What you make next goes there too."
            }
        };
        let input = q.inputs.as_ref().map(|i| i.keep_name.clone());
        let busy = keeping.busy;
        let wide = |button: Button| button.w_full();
        let mut buttons = v_flex().gap(px(look::GAP_8)).child(wide(
            Button::new("quick-keep")
                .primary()
                .label(if busy { "Keeping…" } else { "Keep" })
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.quick_keep(cx))),
        ));
        if closing {
            buttons = buttons.child(wide(
                Button::new("quick-dont-keep")
                    .label("Don't Keep")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.quick_dont_keep(window, cx))),
            ));
        }
        buttons = buttons.child(wide(
            Button::new("quick-keep-cancel")
                .label("Cancel")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.quick_cancel_keep(cx))),
        ));
        let card = v_flex()
            .id("quick-keep-card")
            .occlude()
            .w(px(440.))
            .max_w(relative(0.95))
            .p(px(20.))
            .gap(px(look::GAP_12))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(look::title()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().text_color(p.muted).child(detail))
            .child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.muted).child("Folder name"))
                    .children(input.map(|i| Input::new(&i).small())),
            )
            .child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.muted).child("In"))
                    .child(
                        h_flex()
                            .gap(px(look::GAP_8))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(keeping.parent.clone()),
                            )
                            .child(
                                Button::new("quick-keep-change")
                                    .small()
                                    .label("Change…")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.quick_keep_change_folder(cx)
                                    })),
                            ),
                    ),
            )
            .when_some(keeping.error.clone(), |d, error| {
                d.child(div().text_color(p.err).child(error))
            })
            .child(buttons);
        Some(
            deferred(
                div()
                    .id("quick-keep-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

fn heading(text: &'static str, p: &Palette) -> Div {
    div()
        .pt(px(look::GAP_8))
        .text_color(p.faint)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
