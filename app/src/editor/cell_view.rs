//! Drawing a notebook's cells around their editors and outputs: the
//! gutter (the cell's number, how its run went, and a handle to drag it
//! by), the tools that show when the pointer is over a cell, the rows
//! between cells that add one, and the keys that work on cells when none
//! is being edited (Esc leaves a cell's editor), as in Jupyter. Only the
//! cells near the view are drawn (see [`Placed`]).

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::component::input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::editor::cells::CellStatus;
use crate::editor::html_table::{Table, shown_name};
use crate::editor::notebook::{CellKind, Format};
use crate::editor::output_table::OutputTable;
use crate::editor::output_text::OutputText;
use crate::look::{self, Palette, icon, icon_button};
use crate::workbench::{Open, Workbench};
use crate::{
    AddCellAbove, AddCellBelow, CellToCode, CellToMarkdown, DeleteCell, EditCell, RunCell, RunLine,
    SelectNextCell, SelectPrevCell, UndoDeleteCell,
};

/// The notebook's gutter, the handle cells are picked and dragged by. With
/// the list's 6px padding it matches the 24px on the right.
pub(crate) const GUTTER: f32 = 18.;
/// The cell tools' height; the rows between cells leave room for them.
pub(crate) const TOOLS_HEIGHT: f32 = 24.;

/// A cell being dragged to a new place.
#[derive(Clone)]
pub(crate) struct DraggedCell {
    ix: usize,
    label: SharedString,
}

impl Render for DraggedCell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(p.bg)
            .border_1()
            .border_color(p.accent)
            .text_size(px(look::CHROME))
            .child(self.label.clone())
    }
}

/// Where the open notebook's cells were at the last layout, and which to
/// draw. Only the cells within a screen of the view are drawn; the others
/// are space of their last height. Every cell laid out and painted each
/// frame made scrolling a notebook of 136 cells take 28 ms a frame (221 ms
/// unoptimised), however few showed.
#[derive(Default)]
pub(crate) struct Placed {
    /// The notebook last drawn, its cells' ids in order, and whether add
    /// rows sat between them: what the scroll handle's children were.
    drawn: RefCell<(String, Vec<String>, bool)>,
    /// Each cell's top (from the top of the notebook) and height, by id.
    places: HashMap<String, (Pixels, Pixels)>,
    /// Whether each cell is drawn this frame.
    pub(crate) near: Vec<bool>,
}

impl Placed {
    /// Whether cell `ix` is drawn this frame.
    pub(crate) fn shows(&self, ix: usize) -> bool {
        self.near.get(ix).copied().unwrap_or(true)
    }
}

/// A reservation, not a rendered cell. Keep the full document's scroll extent
/// from the first frame without decoding offscreen outputs or formatting prose.
fn estimated_cell_height(cell: &crate::editor::notebook::Cell) -> Pixels {
    let source = cell.source.split('\n').count().max(1) as f32 * 20. + 22.;
    let mut outputs = 0.;
    for output in &cell.outputs {
        let data = &output["data"];
        outputs += if data.get("image/png").is_some()
            || data.get("image/jpeg").is_some()
            || data.get("image/svg+xml").is_some()
        {
            240.
        } else if data.get("text/html").is_some() {
            420.
        } else {
            let text = output["text"]
                .as_str()
                .or_else(|| data["text/plain"].as_str())
                .unwrap_or("");
            text.lines().count().max(1) as f32 * 20.
        };
        if outputs >= 480. {
            break;
        }
    }
    px(source + outputs.min(480.) + if cell.outputs.is_empty() { 0. } else { 24. })
}

impl Workbench {
    /// Draw only cells near the viewport, including on the first frame.
    /// Unmeasured cells reserve estimated space until scrolled into view;
    /// constructing their editors' elements just to measure them blocks opening.
    pub(crate) fn place_cells(&mut self, window: &Window) {
        let Some(open) = &self.open else {
            self.placed.near.clear();
            return;
        };
        let scroll = &self.notebook_scroll;
        let view = scroll.bounds();
        let placed = &mut self.placed;
        {
            let (path, ids, adds) = &*placed.drawn.borrow();
            if *path != open.notebook.path {
                placed.places.clear();
            } else {
                for (ix, id) in ids.iter().enumerate() {
                    let child = if *adds { ix * 2 + 1 } else { ix };
                    if let Some(b) = scroll.bounds_for_item(child) {
                        placed
                            .places
                            .insert(id.clone(), (b.top() - view.top(), b.size.height));
                    }
                }
            }
        }
        let gap = TOOLS_HEIGHT + 4.;
        let mut next_y = px(10.
            + if open.notebook.can_add_cells() {
                gap
            } else {
                0.
            });
        for cell in &open.notebook.cells {
            let (y, height) = *placed
                .places
                .entry(cell.id.clone())
                .or_insert_with(|| (next_y, estimated_cell_height(cell)));
            next_y = y + height + px(gap);
        }
        let top = -scroll.offset().y;
        let screen = if view.size.height > px(0.) {
            view.size.height
        } else {
            window.viewport_size().height.max(px(1.))
        };
        let (from, to) = (top - screen, top + screen * 2.);
        placed.near = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| {
                self.find.replaying
                    || open.current == Some(ix)
                    || open.editing.contains(&cell.id)
                    || open.lit.as_ref().is_some_and(|(id, _)| *id == cell.id)
                    || placed
                        .places
                        .get(&cell.id)
                        .is_none_or(|(y, height)| *y + *height >= from && *y <= to)
            })
            .collect();
    }

    /// The notebook's cells, each `bodies[ix]` in its frame (or, for a
    /// cell not drawn, space of its last height), with the rows that add
    /// cells between them when cells can be added.
    pub(crate) fn render_cells(
        &self,
        open: &Open,
        bodies: Vec<Option<AnyElement>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let adds = open.notebook.can_add_cells();
        let count = bodies.len();
        let mut children = Vec::with_capacity(count * 2 + 1);
        for (ix, body) in bodies.into_iter().enumerate() {
            if adds {
                children.push(self.add_row_after_deleted(open, ix, false, cx));
            }
            children.push(match body {
                Some(body) => self.cell_frame(open, ix, body, window, cx),
                None => {
                    let id = &open.notebook.cells[ix].id;
                    let height = self.placed.places.get(id).map_or(px(0.), |(_, h)| *h);
                    div().flex_none().w_full().h(height).into_any_element()
                }
            });
        }
        if adds {
            children.push(self.add_row_after_deleted(open, count, true, cx));
        }
        *self.placed.drawn.borrow_mut() = (
            open.notebook.path.clone(),
            open.notebook.cells.iter().map(|c| c.id.clone()).collect(),
            adds,
        );
        children
    }

    /// The row that adds a cell at `at`, under the lines where an agent
    /// deleted cells there: one child, so cells keep their places among
    /// the children.
    fn add_row_after_deleted(
        &self,
        open: &Open,
        at: usize,
        always: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ghost = self.render_ghost(open, at, cx);
        let deleted = self.render_agent_deleted(open, at, cx);
        if deleted.is_empty() && ghost.is_none() {
            let last = open.notebook.cells.len().saturating_sub(1);
            if !self.placed.shows(at.min(last)) && !self.placed.shows(at.saturating_sub(1)) {
                return div()
                    .flex_none()
                    .w_full()
                    .h(px(if always { 36. } else { TOOLS_HEIGHT + 4. }))
                    .into_any_element();
            }
            return self.add_row(at, always, cx);
        }
        v_flex()
            .flex_none()
            .w_full()
            .children(ghost)
            .children(deleted)
            .child(self.add_row(at, always, cx))
            .into_any_element()
    }

    /// Scrolls the notebook so cell `ix` shows.
    pub(crate) fn scroll_to_cell(&mut self, ix: usize) {
        if let Some(open) = &self.open {
            let child = Self::child_of_cell(open, ix);
            if open.single_editor() {
                self.notebook_scroll.scroll_to_item(child);
            } else {
                self.notebook_scroll_target.set(Some(child));
            }
        }
    }

    /// Where cell `ix` is among the children `render_cells` makes.
    pub(crate) fn child_of_cell(open: &Open, ix: usize) -> usize {
        if open.notebook.can_add_cells() {
            ix * 2 + 1
        } else {
            ix
        }
    }

    /// A cell's frame: its gutter, its body, and its tools.
    fn cell_frame(
        &self,
        open: &Open,
        ix: usize,
        body: AnyElement,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let cell = &open.notebook.cells[ix];
        let group: SharedString = format!("cell-{ix}").into();
        let commanding = self.cells_focus.is_focused(window);
        let selected = open.current == Some(ix);
        let lit = open.lit.as_ref().is_some_and(|(id, _)| *id == cell.id);
        let accent = p.accent;
        h_flex()
            .id(("cell", ix))
            .group(group.clone())
            .flex_none()
            .relative()
            .items_start()
            .w_full()
            // A cell the agent just added or changed lights up.
            .when(lit, |d| d.rounded(px(8.)).bg(p.accent_soft))
            // Observe the press without taking focus or consuming it: editor
            // caret placement and output drag-selection keep their own events.
            .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left
                    && let Some(open) = this.open.as_mut()
                    && open.current != Some(ix)
                {
                    open.current = Some(ix);
                    cx.notify();
                }
            }))
            .when(open.notebook.can_restructure(), |d| {
                // The line shows where the cell lands: above this one when
                // it comes from below, under it when from above.
                d.drag_over::<DraggedCell>(move |d, dragged, _, _| {
                    if dragged.ix > ix {
                        d.border_t_2().border_color(accent)
                    } else {
                        d.border_b_2().border_color(accent)
                    }
                })
                .on_drop(cx.listener(
                    move |this, dragged: &DraggedCell, _, cx| this.move_cell(dragged.ix, ix, cx),
                ))
            })
            // Leaving a cell's editor with Esc picks the cell, for the keys.
            .on_action(cx.listener(move |this, _: &input::Escape, window, cx| {
                this.pick_cell(ix, window, cx)
            }))
            .child(self.cell_gutter(open, ix, cx))
            .child(div().flex_1().min_w_0().child(body))
            .child(self.cell_tools(open, ix, group, selected && commanding, cx))
            .when(cell.kind == CellKind::Text, |d| d.min_h(px(26.)))
            .into_any_element()
    }

    /// The gutter beside a cell: clicking it picks the cell, and it is the
    /// handle to drag the cell by. The number shows on the run line.
    fn cell_gutter(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = &open.notebook.cells[ix];
        let code = cell.kind == CellKind::Code;
        let label: SharedString = if code {
            format!("Cell #{}", ix + 1).into()
        } else {
            format!("Markdown cell #{}", ix + 1).into()
        };
        v_flex()
            .id(("gutter", ix))
            .w(px(GUTTER))
            .flex_none()
            .self_stretch()
            .when(open.notebook.can_restructure(), |d| {
                d.cursor(CursorStyle::PointingHand)
                    .on_drag(DraggedCell { ix, label }, |d, _, _, cx| {
                        cx.new(|_| d.clone())
                    })
            })
            .on_click(cx.listener(move |this, _, window, cx| this.pick_cell(ix, window, cx)))
            .when(code, |d| d.min_h(px(20.)))
    }

    /// The line between a code cell and its outputs: the cell's number
    /// (`#3`, as the Agents pane names cells; Huan, 2026-11-14), its run's number as Jupyter
    /// counts it (`Output [5]`), and how long it took (or has been running,
    /// or that it waits).
    pub(crate) fn run_line(
        &self,
        open: &Open,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let cell = &open.notebook.cells[ix];
        if cell.kind != CellKind::Code {
            return None;
        }
        let status = self.cell_status(ix);
        let (text, color) = match &status {
            CellStatus::Idle => (None, p.faint),
            CellStatus::Queued => (Some("waiting".to_string()), p.faint),
            CellStatus::Running(_) => (
                Some(
                    status
                        .time_text()
                        .map_or("running".into(), |t| format!("running {t}")),
                ),
                p.busy,
            ),
            CellStatus::Done(_) => (status.time_text(), p.faint),
            CellStatus::Failed(_) => (
                Some(
                    status
                        .time_text()
                        .map_or("failed".into(), |t| format!("failed after {t}")),
                ),
                p.err,
            ),
        };
        let count = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id))
            .and_then(|r| r.execution_count)
            .or(cell.execution_count);
        Some(
            h_flex()
                .id(("run-line", ix))
                .mt(px(3.))
                .pl(px(14.))
                .gap(px(10.))
                .font_family(look::MONO)
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(format!("#{}", ix + 1))
                .when_some(count, |d, n| d.child(format!("Output [{n}]")))
                .when_some(text, |d, t| d.child(div().text_color(color).child(t)))
                .into_any_element(),
        )
    }

    /// The cell's tools, in its top right corner: Run (always there for
    /// code), and the rest while the pointer is over the cell or it is
    /// picked.
    fn cell_tools(
        &self,
        open: &Open,
        ix: usize,
        group: SharedString,
        picked: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        let cell = &open.notebook.cells[ix];
        let code = cell.kind == CellKind::Code;
        let moves = open.notebook.can_restructure();
        let count = open.notebook.cells.len();
        let busy = matches!(
            self.cell_status(ix),
            CellStatus::Queued | CellStatus::Running(_)
        );
        let has_outputs =
            code && (open.cell_runs.contains_key(&cell.id) || !cell.outputs.is_empty());
        let folded = open.collapsed.contains(&cell.id);
        let tool = |id: &str, name: &str, tip: &'static str| {
            icon_button(SharedString::from(format!("{id}-{ix}")), name, &p, false)
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
        };
        let tools = h_flex()
            .gap(px(1.))
            .when(code, |d| {
                d.child(
                    tool("run-above", "run.above", "Run the cells above")
                        .on_click(cx.listener(move |this, _, _, cx| this.run_above(ix, cx))),
                )
                .child(
                    tool("run-below", "run.below", "Run this cell and those below")
                        .on_click(cx.listener(move |this, _, _, cx| this.run_below(ix, cx))),
                )
                .child(tool("ask-ai", "agent", "Ask AI about this cell").on_click(
                    cx.listener(move |this, _, window, cx| this.ask_ai_about_cell(ix, window, cx)),
                ))
            })
            .when(has_outputs, |d| {
                d.child(
                    tool(
                        "fold",
                        if folded {
                            "chevron.right"
                        } else {
                            "chevron.down"
                        },
                        if folded {
                            "Show the outputs"
                        } else {
                            "Hide the outputs"
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_outputs(ix, cx))),
                )
                .child(
                    tool("clear", "outputs.clear", "Clear the outputs").on_click(
                        cx.listener(move |this, _, _, cx| this.clear_outputs(Some(&[ix]), cx)),
                    ),
                )
            })
            .when(moves, |d| {
                d.when(ix > 0, |d| {
                    d.child(tool("up", "arrow.up", "Move up").on_click(
                        cx.listener(move |this, _, _, cx| this.move_cell(ix, ix - 1, cx)),
                    ))
                })
                .when(ix + 1 < count, |d| {
                    d.child(tool("down", "arrow.down", "Move down").on_click(
                        cx.listener(move |this, _, _, cx| this.move_cell(ix, ix + 1, cx)),
                    ))
                })
            })
            .when(open.notebook.can_delete_cells(), |d| {
                d.child(
                    tool("delete", "delete", "Delete the cell (Z brings it back)").on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.delete_cell(ix, cx);
                            window.focus(&this.cells_focus, cx);
                        }),
                    ),
                )
            });
        // Above the cell's top right corner, so it covers none of the code;
        // it shows while the pointer is over the cell or over the tools
        // (they stay where the pointer can reach them), or the cell is
        // picked.
        h_flex()
            .id(("cell-tools", ix))
            .absolute()
            .bottom_full()
            .right(px(6.))
            .h(px(TOOLS_HEIGHT))
            .gap(px(2.))
            .px(px(2.))
            .rounded(px(6.))
            .border(px(0.5))
            .border_color(p.line)
            .bg(p.bg)
            .when(!picked, |d| {
                d.opacity(0.)
                    .group_hover(group, |d| d.opacity(1.))
                    .hover(|d| d.opacity(1.))
            })
            // Run first, then Run the cells above and the other tools.
            .when(code, |d| {
                d.child(
                    tool("run", "run", "Run this cell")
                        .when(busy, |d| d.text_color(p.busy))
                        .on_click(cx.listener(move |this, _, _, cx| this.run_cell(ix, cx))),
                )
            })
            .child(tools)
    }

    /// The centred row before cell `at` (or after the last) with buttons
    /// that add a code or Markdown cell there; they show when the pointer
    /// is over the row, and always after the last cell.
    fn add_row(&self, at: usize, always: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let group: SharedString = format!("add-{at}").into();
        let button = |id: &str, label: &'static str, kind: CellKind| {
            div()
                .id(SharedString::from(format!("{id}-{at}")))
                .flex()
                .items_center()
                .gap(px(3.))
                .px(px(8.))
                .h(px(20.))
                .rounded(px(10.))
                .border(px(0.5))
                .border_color(p.line)
                .bg(p.bg)
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .cursor_pointer()
                .hover(|d| d.text_color(p.accent).border_color(p.accent))
                .child(icon("add").size(px(12.)))
                .child(label)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.add_cell(at, kind, window, cx)),
                )
        };
        h_flex()
            .id(("add-row", at))
            .group(group.clone())
            .flex_none()
            .w_full()
            .h(px(if always { 36. } else { TOOLS_HEIGHT + 4. }))
            .justify_center()
            .child(
                h_flex()
                    .gap(px(8.))
                    .when(!always, |d| {
                        d.invisible().group_hover(group, |d| d.visible())
                    })
                    .child(button("add-code", "Code", CellKind::Code))
                    .child(button("add-text", "Markdown", CellKind::Text)),
            )
            .into_any_element()
    }

    /// Picks cell `ix` for the keys: its editor lets go of the keyboard.
    pub(crate) fn pick_cell(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if ix >= open.notebook.cells.len() {
            return;
        }
        open.current = Some(ix);
        let child = Self::child_of_cell(open, ix);
        self.notebook_scroll_target.set(Some(child));
        window.focus(&self.cells_focus, cx);
        cx.notify();
    }

    /// The keys that work on the picked cell, on the notebook's cells.
    pub(crate) fn cell_keys(&self, cells: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let current = |this: &Workbench| this.open.as_ref().and_then(|o| o.current);
        cells
            .on_action(cx.listener(move |this, _: &AddCellAbove, window, cx| {
                let at = current(this).unwrap_or(0);
                this.add_cell_picked(at, window, cx);
            }))
            .on_action(cx.listener(move |this, _: &AddCellBelow, window, cx| {
                let at = current(this).map_or(0, |ix| ix + 1);
                this.add_cell_picked(at, window, cx);
            }))
            .on_action(cx.listener(move |this, _: &DeleteCell, _, cx| {
                if let Some(ix) = current(this) {
                    this.delete_cell(ix, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &UndoDeleteCell, window, cx| {
                this.undo_delete_cell(window, cx);
            }))
            .on_action(cx.listener(move |this, _: &CellToCode, window, cx| {
                if let Some(ix) = current(this) {
                    this.set_cell_kind(ix, CellKind::Code, window, cx);
                    window.focus(&this.cells_focus, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &CellToMarkdown, window, cx| {
                if let Some(ix) = current(this) {
                    this.set_cell_kind(ix, CellKind::Text, window, cx);
                    window.focus(&this.cells_focus, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevCell, window, cx| {
                let ix = current(this).map_or(0, |ix| ix.saturating_sub(1));
                this.pick_cell(ix, window, cx);
            }))
            .on_action(cx.listener(move |this, _: &SelectNextCell, window, cx| {
                let count = this.open.as_ref().map_or(0, |o| o.notebook.cells.len());
                let ix = current(this).map_or(0, |ix| (ix + 1).min(count.saturating_sub(1)));
                this.pick_cell(ix, window, cx);
            }))
            .on_action(cx.listener(move |this, _: &EditCell, window, cx| {
                if let Some(ix) = current(this) {
                    this.edit_cell(ix, window, cx);
                }
            }))
            // Shift+Enter runs the picked cell and picks the next.
            .on_action(cx.listener(move |this, _: &RunCell, window, cx| {
                if let Some(ix) = current(this) {
                    this.run_cell(ix, cx);
                    let count = this.open.as_ref().map_or(0, |o| o.notebook.cells.len());
                    if ix + 1 < count {
                        this.pick_cell(ix + 1, window, cx);
                    }
                }
            }))
            .on_action(cx.listener(move |this, _: &RunLine, _, cx| {
                if let Some(ix) = current(this) {
                    this.run_cell(ix, cx);
                }
            }))
    }

    /// A data frame under a cell, as a grid that scrolls sideways, with
    /// its size and a button that opens it in the Data Explorer when the
    /// cell shows it by name.
    pub(crate) fn render_table(
        &self,
        mut table: Table,
        id: SharedString,
        cell: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let caption = table.caption.take();
        let explorer = cell
            .and_then(|ix| self.open.as_ref()?.editors.get(ix))
            .and_then(|e| shown_name(&e.read(cx).value()))
            .filter(|_| self.vars.session.is_some());
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(OutputTable::new(id.clone(), table))
            .child(
                h_flex()
                    .gap(px(10.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .when_some(caption, |d, caption| {
                        d.child(div().when(cell.is_some(), |d| d.cursor_text()).child(
                            OutputText::new(SharedString::from(format!("{id}-caption")), caption),
                        ))
                    })
                    .when_some(explorer, |d, name| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("{id}-explore")))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_color(p.accent)
                                .cursor_pointer()
                                .hover(|d| d.underline())
                                .child(icon("table").size(px(12.)))
                                .child(format!("Open {name} in the Data Explorer"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_explorer(vec![name.clone()], window, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Adds a code cell at `at` and keeps the keys on the cells, picking it.
    fn add_cell_picked(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self
            .open
            .as_ref()
            .map_or(0, |open| open.notebook.cells.len());
        self.add_cell(at, CellKind::Code, window, cx);
        if let Some(open) = &self.open
            && open.notebook.cells.len() > count
            && let Some(inserted) = open.current
        {
            self.pick_cell(inserted, window, cx);
        }
    }

    /// Puts the cursor in cell `ix`; a Markdown cell shows its text.
    pub(crate) fn edit_cell(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (Some(cell), Some(editor)) = (open.notebook.cells.get(ix), open.editors.get(ix)) else {
            return;
        };
        if cell.kind == CellKind::Text && open.notebook.format == Format::Ipynb {
            open.editing.insert(cell.id.clone());
        }
        open.current = Some(ix);
        editor.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }
}
