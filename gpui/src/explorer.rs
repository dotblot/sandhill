//! The Data Explorer, as the Tauri app has it (`DataExplorer.tsx`): a data
//! frame or matrix from a kernel as a table that only draws the rows in
//! view and reads them from the kernel in blocks as it scrolls. The columns
//! are listed at the side with a small histogram or bar chart each, and
//! the picked column's numbers under them. Filters and sorting run in the
//! kernel, so a table of a million cells scrolls like one of a hundred. The
//! table is read again after each run in its session, so it stays live.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, ColumnSummary, FileStat, TableCell, TableColumn, TableFilter, TableFocus, TableRows,
    TableSort, TableSummary,
};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::look::{self, Palette, bar_button, icon};
use crate::table::{
    TableCodeView, Takes, column_note, column_width, export_name, filter_error, filter_text,
    format_cell, format_number, missing_text, new_filter, ops_for, sorted_by, table_code,
};
use crate::table_selection::{Selection, Target, encode_tsv};

/// Rows read at a time.
const BLOCK: u64 = 200;
/// How narrow and wide a column can be dragged.
const MIN_COLUMN_WIDTH: f32 = 40.;
const MAX_COLUMN_WIDTH: f32 = 1200.;
/// Columns read; wider tables show their first ones.
const COLUMNS: u32 = 200;
/// The Tauri app's sizes, a little taller for text 2px larger.
const ROW_HEIGHT: f32 = 26.;
/// The header holds the column's name and its ⋮ menu.
const HEADER_HEIGHT: f32 = 32.;
const SIDE_WIDTH: f32 = 310.;
const SIDE_ROW: f32 = 42.;
const SPARK_WIDTH: f32 = 70.;
const SPARK_HEIGHT: f32 = 22.;
/// The missing share after each sparkline.
const MISSING_WIDTH: f32 = 40.;
/// Kit base families, then their brighter variants for larger categories.
const CATEGORY_ANSI: [usize; 10] = [4, 3, 1, 6, 2, 5, 12, 11, 9, 14];
/// What the daemon says when the session runs other code.
const BUSY: &str = "the session is busy running code";

/// What the table's first answer said about it.
/// A table's sorting and filters as a project keeps them between launches,
/// each with its column's name, so they follow their columns if the table
/// changed meanwhile.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct SavedView {
    #[serde(default)]
    sort: Vec<Named<TableSort>>,
    #[serde(default)]
    filters: Vec<Named<TableFilter>>,
    #[serde(default)]
    hidden: Vec<HiddenColumn>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Named<T> {
    name: String,
    #[serde(flatten)]
    key: T,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct HiddenColumn {
    name: String,
    column: u32,
}

impl SavedView {
    fn of(
        sort: &[TableSort],
        filters: &[TableFilter],
        columns: &[TableColumn],
        hidden: &[u32],
    ) -> SavedView {
        let name = |column: u32| {
            columns
                .get(column as usize)
                .map(|c| c.name.clone())
                .unwrap_or_default()
        };
        SavedView {
            sort: sort
                .iter()
                .map(|k| Named {
                    name: name(k.column),
                    key: k.clone(),
                })
                .collect(),
            filters: filters
                .iter()
                .map(|f| Named {
                    name: name(f.column),
                    key: f.clone(),
                })
                .collect(),
            hidden: hidden
                .iter()
                .map(|&column| HiddenColumn {
                    name: name(column),
                    column,
                })
                .collect(),
        }
    }

    /// The sorting and filters on the table's columns now: each moved to
    /// where its column is, and dropped if its column is gone.
    fn on(&self, columns: &[TableColumn]) -> (Vec<TableSort>, Vec<TableFilter>) {
        let place = |name: &str, was: u32| {
            if columns.get(was as usize).is_some_and(|c| c.name == name) {
                return Some(was);
            }
            columns
                .iter()
                .position(|c| c.name == name)
                .map(|i| i as u32)
        };
        let sort = self
            .sort
            .iter()
            .filter_map(|k| {
                place(&k.name, k.key.column).map(|column| TableSort {
                    column,
                    ..k.key.clone()
                })
            })
            .collect();
        let filters = self
            .filters
            .iter()
            .filter_map(|f| {
                place(&f.name, f.key.column).map(|column| TableFilter {
                    column,
                    ..f.key.clone()
                })
            })
            .collect();
        (sort, filters)
    }

    fn hidden_on(&self, columns: &[TableColumn]) -> Vec<u32> {
        self.hidden
            .iter()
            .filter_map(|h| {
                if columns
                    .get(h.column as usize)
                    .is_some_and(|c| c.name == h.name)
                {
                    Some(h.column)
                } else {
                    columns
                        .iter()
                        .position(|c| c.name == h.name)
                        .map(|i| i as u32)
                }
            })
            .collect()
    }
}

struct Shape {
    code: String,
    matrix: bool,
    columns: Vec<TableColumn>,
    width: u64,
    total: u64,
    rows: u64,
}

#[derive(Clone)]
struct CachedRow {
    label: String,
    cells: Vec<Value>,
    /// A projected reply's original column index to its cell position.
    slots: Option<Rc<HashMap<u32, usize>>>,
}

enum Block {
    Loading,
    Ready(Vec<CachedRow>),
    Failed,
}

/// The filter being added or changed, in its popup.
struct FilterEdit {
    /// The filter changed; none for a new one.
    index: Option<usize>,
    column: u32,
    op: String,
    /// A category's values ticked.
    ticked: Vec<String>,
    /// Whether Add was pressed, so a problem shows.
    tried: bool,
    /// Whether the column list is open.
    columns_open: bool,
    popup_triggers: crate::popup::PopupTriggers,
}

/// Exporting as CSV: the name asked for, then whether to replace a file.
enum Export {
    Naming,
    Replace(String),
    Writing,
}

/// State used by the agent's current-table context changed.
pub(crate) struct FocusChanged;

pub struct Explorer {
    daemon: Rc<Daemon>,
    session: String,
    language: String,
    path: Vec<String>,
    /// The project's folder, where an export goes.
    project: Option<String>,
    shape: Option<Shape>,
    error: Option<String>,
    note: Option<(String, bool)>,
    sort: Vec<TableSort>,
    filters: Vec<TableFilter>,
    /// Original column indices, never compacted when a column is hidden.
    hidden: Vec<u32>,
    /// The sorting and filters kept from last time, until the table's
    /// columns are known and they are moved onto them.
    saved: Option<SavedView>,
    blocks: HashMap<u64, Block>,
    widths: Vec<f32>,
    /// A column being resized by its header's right edge: the column, where
    /// the drag began and the width then.
    resizing: Option<(usize, Pixels, f32)>,
    label_width: f32,
    /// The columns' summaries with the filters on, and with none: every
    /// value a filter can pick from.
    summary: HashMap<u32, ColumnSummary>,
    overall: HashMap<u32, ColumnSummary>,
    /// Each category value's theme colour index, kept as filters change.
    colors: HashMap<u32, Vec<(String, usize)>>,
    picked: usize,
    selection: Selection,
    focus: FocusHandle,
    toolbar: Entity<ExplorerToolbar>,
    popup_triggers: crate::popup::PopupTriggers,
    search_input: Entity<InputState>,
    search: String,
    search_generation: u64,
    side_shown: bool,
    /// Code runs in the session: the table is read again when it ends.
    busy: bool,
    stale: bool,
    edit: Option<FilterEdit>,
    value_a: Entity<InputState>,
    value_b: Entity<InputState>,
    typed: Entity<InputState>,
    export: Option<Export>,
    export_input: Entity<InputState>,
    /// Counts reloads, so a block for an old sort or filter is dropped.
    generation: u64,
    scroll: UniformListScrollHandle,
    across: ScrollHandle,
    side_scroll: ScrollHandle,
    /// Pinned columns (original indices), shown first and kept at the left
    /// while the table scrolls sideways.
    pinned: Vec<u32>,
    /// Columns whose statistics are open in the summary panel.
    expanded: std::collections::HashSet<u32>,
    /// A column's ⋮ menu: the column and where it opened.
    column_menu: Option<(u32, Point<Pixels>)>,
    /// The gear menu, where it opened.
    settings_menu: Option<Point<Pixels>>,
    /// Shown in a window of its own.
    popped: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FocusChanged> for Explorer {}

struct ToolbarChanged;
impl EventEmitter<ToolbarChanged> for Explorer {}

/// The explorer asks to move into a window of its own.
pub(crate) struct PopOut;
impl EventEmitter<PopOut> for Explorer {}

/// Retains the action row independently of the editor and notebook.
pub(crate) struct ExplorerToolbar {
    explorer: WeakEntity<Explorer>,
    _subscription: Subscription,
}

impl Render for ExplorerToolbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.explorer
            .update(cx, |explorer, cx| {
                explorer.render_top(cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// A Data Explorer moved into a window of its own: its toolbar along the
/// top, beside the window buttons, and the table under it.
pub(crate) struct ExplorerWindow {
    explorer: Entity<Explorer>,
}

impl ExplorerWindow {
    pub(crate) fn new(explorer: Entity<Explorer>) -> Self {
        Self { explorer }
    }
}

impl Render for ExplorerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let toolbar = self.explorer.read(cx).toolbar();
        look::wall(&p)
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text)
            .text_size(px(look::CHROME))
            .line_height(rems(1.25))
            .child(
                h_flex()
                    .flex_none()
                    .h(px(look::TITLE_BAR + 4.))
                    // Clear of the window buttons, which sit on this row.
                    .pl(px(if window.is_fullscreen() {
                        6.
                    } else {
                        crate::view::TITLE_BAR_LEFT
                    }))
                    .pr(px(6.))
                    .child(toolbar),
            )
            .child(
                look::pane(&p)
                    .flex_1()
                    .min_h_0()
                    .mx(px(look::EDGE))
                    .mb(px(look::EDGE))
                    .child(self.explorer.clone()),
            )
    }
}

impl Explorer {
    pub fn new(
        daemon: Rc<Daemon>,
        session: String,
        language: String,
        path: Vec<String>,
        project: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let value_a = cx.new(|cx| InputState::new(window, cx));
        let value_b = cx.new(|cx| InputState::new(window, cx));
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder("a, b, c"));
        let export_input = cx.new(|cx| InputState::new(window, cx));
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let explorer = cx.entity();
        let toolbar = cx.new(|cx| ExplorerToolbar {
            explorer: explorer.downgrade(),
            _subscription: cx.subscribe(&explorer, |_, _, _: &ToolbarChanged, cx| cx.notify()),
        });
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     event: &InputEvent,
                     _: &mut Window,
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                this.apply_filter(cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&value_a, window, enter),
            cx.subscribe_in(&value_b, window, enter),
            cx.subscribe_in(&typed, window, enter),
            cx.subscribe_in(&search_input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.debounce_search(cx);
                }
            }),
            cx.subscribe_in(
                &export_input,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.export_named(cx);
                    }
                },
            ),
        ];
        let saved: Option<SavedView> = project
            .as_deref()
            .and_then(|p| crate::prefs::table_view(p, &view_key(&language, &path)))
            .and_then(|v| serde_json::from_value(v).ok());
        let (sort, filters) = saved
            .as_ref()
            .map(|v| {
                (
                    v.sort.iter().map(|k| k.key.clone()).collect(),
                    v.filters.iter().map(|f| f.key.clone()).collect(),
                )
            })
            .unwrap_or_default();
        let mut this = Self {
            daemon,
            session,
            language,
            path,
            project,
            shape: None,
            error: None,
            note: None,
            sort,
            filters,
            hidden: Vec::new(),
            saved,
            blocks: HashMap::new(),
            widths: Vec::new(),
            resizing: None,
            label_width: 60.,
            summary: HashMap::new(),
            overall: HashMap::new(),
            colors: HashMap::new(),
            picked: 0,
            selection: Selection::default(),
            focus: cx.focus_handle(),
            toolbar,
            popup_triggers: crate::popup::PopupTriggers::default(),
            search_input,
            search: String::new(),
            search_generation: 0,
            side_shown: crate::prefs::column_stats(),
            busy: false,
            stale: false,
            edit: None,
            value_a,
            value_b,
            typed,
            export: None,
            export_input,
            generation: 0,
            scroll: UniformListScrollHandle::new(),
            across: ScrollHandle::new(),
            side_scroll: ScrollHandle::new(),
            pinned: Vec::new(),
            expanded: std::collections::HashSet::new(),
            column_menu: None,
            settings_menu: None,
            popped: false,
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// Marks the explorer as shown in its own window, where it offers no
    /// way to open another.
    pub(crate) fn set_popped(&mut self, cx: &mut Context<Self>) {
        self.popped = true;
        cx.emit(ToolbarChanged);
        cx.notify();
    }

    pub fn name(&self) -> String {
        self.path.join(" › ")
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Snapshot of this view, using original column positions and row
    /// positions in the current filtered/sorted view.
    pub(crate) fn table_focus(&self) -> Option<TableFocus> {
        let shape = self.shape.as_ref()?;
        Some(TableFocus {
            session: self.session.clone(),
            path: self.path.clone(),
            name: self.name(),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            hidden_columns: self.hidden.clone(),
            selected_column: (self.picked < shape.columns.len()).then_some(self.picked as u32),
            selected_cells: self.selected_cells(),
            shown_rows: shape.rows,
            total_rows: shape.total,
        })
    }

    pub(crate) fn toolbar(&self) -> Entity<ExplorerToolbar> {
        self.toolbar.clone()
    }

    fn selected_cells(&self) -> Vec<TableCell> {
        let Some(range) = self.selection.range() else {
            return Vec::new();
        };
        let visible = self.visible_columns();
        // Agent focus is a bounded snapshot, not the selection's storage.
        (range.first_row..=range.last_row)
            .flat_map(|row| {
                (range.first_column..=range.last_column)
                    .filter_map(|column| visible.get(column).copied())
                    .map(move |column| TableCell { row, column })
            })
            .take(10_000)
            .collect()
    }

    fn begin_selection(
        &mut self,
        target: Target,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.shape.as_ref().map_or(0, |s| s.rows);
        self.selection
            .begin(target, shift, rows, self.visible_columns().len());
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn extend_selection(&mut self, target: Target, cx: &mut Context<Self>) {
        if !self.selection.dragging() {
            return;
        }
        let before = self.selection.range();
        let rows = self.shape.as_ref().map_or(0, |s| s.rows);
        self.selection
            .extend(target, rows, self.visible_columns().len());
        if self.selection.range() != before {
            cx.notify();
        }
    }

    fn debounce_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.search_generation {
                    return;
                }
                let search = this.search_input.read(cx).value().to_string();
                if search == this.search {
                    return;
                }
                this.search = search;
                this.selection.clear();
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                this.reload(cx);
                cx.emit(FocusChanged);
            });
        })
        .detach();
    }

    /// Code started or stopped running in the table's session: once it
    /// stops the table is read again, as the kernel has it now.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy == busy {
            return;
        }
        self.busy = busy;
        cx.emit(ToolbarChanged);
        if !busy {
            self.reload(cx);
        }
        cx.notify();
    }

    /// Reads everything again: the rows from the top, and the columns'
    /// summaries.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.stale = true;
            cx.notify();
            return;
        }
        self.stale = false;
        self.generation += 1;
        self.blocks.clear();
        self.error = None;
        self.note = None;
        self.load(0, cx);
        self.read_summary(cx);
        cx.emit(ToolbarChanged);
        cx.notify();
    }

    /// The original indices in display order: pinned columns first. Keep row
    /// requests and the grid on this same projection when the daemon supports
    /// selected columns.
    fn visible_columns(&self) -> Vec<u32> {
        let count = self.shape.as_ref().map_or(0, |s| s.columns.len()) as u32;
        let shown = |i: &u32| *i < count && !self.hidden.contains(i);
        self.pinned
            .iter()
            .copied()
            .filter(shown)
            .chain((0..count).filter(|i| shown(i) && !self.pinned.contains(i)))
            .collect()
    }

    /// How many shown columns are pinned.
    fn pinned_shown(&self) -> usize {
        let count = self.shape.as_ref().map_or(0, |s| s.columns.len()) as u32;
        self.pinned
            .iter()
            .filter(|i| **i < count && !self.hidden.contains(i))
            .count()
    }

    fn toggle_pin(&mut self, column: u32, cx: &mut Context<Self>) {
        if self.pinned.contains(&column) {
            self.pinned.retain(|&c| c != column);
        } else {
            self.pinned.push(column);
        }
        self.selection.clear();
        cx.emit(FocusChanged);
        cx.notify();
    }

    /// Selects a whole column, by its original index.
    fn select_column(&mut self, column: u32, cx: &mut Context<Self>) {
        let Some(display) = self.visible_columns().iter().position(|&c| c == column) else {
            return;
        };
        let rows = self.shape.as_ref().map_or(0, |s| s.rows);
        let count = self.visible_columns().len();
        self.selection
            .begin(Target::Column(display), false, rows, count);
        self.selection.end();
        self.picked = column as usize;
        cx.emit(FocusChanged);
        cx.notify();
    }

    /// Scrolls the table sideways so a column shows, and selects it.
    fn reveal_column(&mut self, column: u32, cx: &mut Context<Self>) {
        let visible = self.visible_columns();
        let Some(display) = visible.iter().position(|&c| c == column) else {
            self.picked = column as usize;
            cx.notify();
            return;
        };
        let pinned = self.pinned_shown();
        if display >= pinned {
            let before: f32 = visible[pinned..display]
                .iter()
                .filter_map(|&c| self.widths.get(c as usize))
                .sum();
            let offset = self.across.offset();
            let most = self.across.max_offset().x;
            self.across
                .set_offset(point((-px(before)).max(-most), offset.y));
        }
        self.select_column(column, cx);
    }

    /// Back to the table as it first opens: no filters, sorting, hidden or
    /// pinned columns, and the first widths.
    fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.settings_menu = None;
        self.pinned.clear();
        self.hidden.clear();
        self.expanded.clear();
        self.filters.clear();
        self.sort.clear();
        // Forgets the view kept for the project.
        self.save_view();
        // No shape makes the next read lay out new widths.
        self.shape = None;
        self.change_sort(Vec::new(), cx);
        self.read_summary(cx);
    }

    fn rows_request(&self, block: u64) -> Call {
        Call::TableRows {
            session: self.session.clone(),
            path: self.path.clone(),
            offset: block * BLOCK,
            limit: Some(BLOCK as u32),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            column_offset: 0,
            column_limit: Some(COLUMNS),
            cells_only: block != 0,
            // The first block keeps complete column metadata for the sidebar.
            columns: (block != 0).then(|| self.visible_columns()),
        }
    }

    fn show_column(&mut self, column: u32, shown: bool, cx: &mut Context<Self>) {
        self.hidden.retain(|&i| i != column);
        if !shown {
            self.hidden.push(column);
        }
        self.save_view();
        self.selection.clear();
        cx.emit(FocusChanged);
        // Picked stays an original index, so hidden columns' stats stay usable.
        self.invalidate_rows(cx);
        cx.notify();
    }

    fn show_all_columns(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.hidden = if shown {
            Vec::new()
        } else {
            (0..self.shape.as_ref().map_or(0, |s| s.columns.len()) as u32).collect()
        };
        self.save_view();
        self.selection.clear();
        cx.emit(FocusChanged);
        self.invalidate_rows(cx);
        cx.notify();
    }

    fn invalidate_rows(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.blocks.clear();
        if self.busy {
            self.stale = true;
        } else {
            self.load(0, cx);
            if self.summary.is_empty() {
                self.read_summary(cx);
            }
        }
    }

    fn load(&mut self, block: u64, cx: &mut Context<Self>) {
        if self.blocks.contains_key(&block) {
            return;
        }
        self.blocks.insert(block, Block::Loading);
        let generation = self.generation;
        let first = block == 0;
        let rows = self.daemon.call::<TableRows>(self.rows_request(block));
        cx.spawn(async move |this, cx| {
            let rows = rows.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match rows {
                    Ok(rows) => {
                        if first && this.took_shape(&rows) {
                            // The kept filters moved to other columns.
                            this.reload(cx);
                            return;
                        }
                        let slots = rows.column_indices.map(|indices| {
                            Rc::new(
                                indices
                                    .into_iter()
                                    .enumerate()
                                    .map(|(slot, column)| (column, slot))
                                    .collect(),
                            )
                        });
                        let cells = rows
                            .labels
                            .into_iter()
                            .zip(rows.values)
                            .map(|(label, cells)| CachedRow {
                                label,
                                cells,
                                slots: slots.clone(),
                            })
                            .collect();
                        this.blocks.insert(block, Block::Ready(cells));
                        if first {
                            this.selection.clear();
                            cx.emit(FocusChanged);
                            cx.emit(ToolbarChanged);
                        }
                    }
                    Err(err) => {
                        let err = format!("{err:#}");
                        // A busy kernel can't answer: scrolling there again
                        // retries, and the end of the run reads it all again.
                        this.blocks.insert(block, Block::Failed);
                        if err.contains(BUSY) {
                            this.stale = true;
                        } else {
                            this.error = Some(err);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The first rows of a read: its columns, counts and code. A table of
    /// new columns gets new widths, colours and a picked column. Says
    /// whether the sorting and filters kept from last time had to be moved
    /// to other columns, so the rows must be read again.
    fn took_shape(&mut self, rows: &TableRows) -> bool {
        let same = self.shape.as_ref().is_some_and(|s| {
            s.columns.len() == rows.columns.len()
                && s.columns
                    .iter()
                    .zip(&rows.columns)
                    .all(|(a, b)| a.name == b.name && a.kind == b.kind)
        });
        let previous = (!same)
            .then(|| {
                self.shape
                    .as_ref()
                    .map(|s| SavedView::of(&self.sort, &self.filters, &s.columns, &self.hidden))
            })
            .flatten();
        if !same {
            self.widths = rows
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let cells: Vec<String> = rows
                        .values
                        .iter()
                        .take(50)
                        .filter_map(|row| row.get(i))
                        .map(|v| format_cell(v, &self.language).0)
                        .collect();
                    column_width(c, &cells)
                })
                .collect();
            let longest = rows
                .labels
                .iter()
                .map(|l| l.chars().count())
                .chain([2, rows.total.to_string().len()])
                .max()
                .unwrap_or(2);
            self.label_width = (longest as f32 * 8.4 + 24.).clamp(44., 240.).round();
            self.colors.clear();
            self.overall.clear();
            if self.picked >= rows.columns.len() {
                self.picked = 0;
            }
        }
        self.shape = Some(Shape {
            code: rows.code.clone(),
            matrix: rows.matrix,
            columns: rows.columns.clone(),
            width: rows.width,
            total: rows.total,
            rows: rows.rows,
        });
        let Some(saved) = self.saved.take().or(previous) else {
            return false;
        };
        let (sort, filters) = saved.on(&rows.columns);
        self.hidden = saved.hidden_on(&rows.columns);
        if sort == self.sort && filters == self.filters {
            return false;
        }
        self.sort = sort;
        self.filters = filters;
        self.save_view();
        true
    }

    /// Keeps the sorting and filters for the next time this table opens in
    /// the project.
    fn save_view(&self) {
        let (Some(project), Some(shape)) = (self.project.as_deref(), self.shape.as_ref()) else {
            return;
        };
        let key = view_key(&self.language, &self.path);
        let view = (!self.sort.is_empty() || !self.filters.is_empty() || !self.hidden.is_empty())
            .then(|| SavedView::of(&self.sort, &self.filters, &shape.columns, &self.hidden))
            .and_then(|v| serde_json::to_value(v).ok());
        crate::prefs::save_table_view(project, &key, view);
    }

    /// Reads the columns' summaries, with the filters on.
    fn read_summary(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        let filtered = !self.filters.is_empty() || !self.search.is_empty();
        let read = self.daemon.call::<TableSummary>(Call::TableSummary {
            session: self.session.clone(),
            path: self.path.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            column_offset: 0,
            column_limit: Some(COLUMNS),
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok(read) => {
                        this.summary = read.columns.iter().map(|c| (c.column, c.clone())).collect();
                        if !filtered {
                            this.overall = this.summary.clone();
                        }
                        this.color_values(&read.columns);
                    }
                    Err(err) => {
                        if !format!("{err:#}").contains(BUSY) {
                            this.summary.clear();
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Colours for the values of category columns with at most 10 of them,
    /// keeping those given before.
    fn color_values(&mut self, summaries: &[ColumnSummary]) {
        let Some(shape) = &self.shape else { return };
        for s in summaries {
            let Some(column) = shape.columns.get(s.column as usize) else {
                continue;
            };
            if (column.kind != "category" && column.kind != "logical") || s.top.is_empty() {
                continue;
            }
            if s.distinct.is_none_or(|d| d > CATEGORY_ANSI.len() as u64) {
                continue;
            }
            let map = self.colors.entry(s.column).or_default();
            for t in &s.top {
                if map.len() < CATEGORY_ANSI.len() && !map.iter().any(|(v, _)| v == &t.value) {
                    map.push((t.value.clone(), map.len()));
                }
            }
        }
    }

    fn color_of(&self, column: u32, value: &str, p: &Palette) -> Option<Hsla> {
        self.colors
            .get(&column)?
            .iter()
            .find(|(v, _)| v == value)
            .map(|(_, c)| p.ansi[CATEGORY_ANSI[*c]])
    }

    fn change_sort(&mut self, sort: Vec<TableSort>, cx: &mut Context<Self>) {
        self.sort = sort;
        self.selection.clear();
        cx.emit(FocusChanged);
        self.saved = None;
        self.save_view();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.generation += 1;
        self.blocks.clear();
        self.error = None;
        self.load(0, cx);
        cx.notify();
    }

    fn change_filters(&mut self, filters: Vec<TableFilter>, cx: &mut Context<Self>) {
        self.filters = filters;
        self.selection.clear();
        cx.emit(FocusChanged);
        self.saved = None;
        self.save_view();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<CachedRow> {
        let block = ix as u64 / BLOCK;
        match self.blocks.get(&block) {
            Some(Block::Ready(rows)) => rows.get(ix % BLOCK as usize).cloned(),
            Some(Block::Loading) => None,
            Some(Block::Failed) => {
                if !self.busy {
                    self.blocks.remove(&block);
                }
                None
            }
            None => {
                if !self.busy {
                    self.load(block, cx);
                }
                None
            }
        }
    }

    /// Shows or hides the summary panel here; whether it shows at first is
    /// the gear menu's setting.
    fn show_side(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.side_shown = shown;
        cx.emit(ToolbarChanged);
        cx.notify();
    }

    fn toggle_side_default(&mut self, cx: &mut Context<Self>) {
        crate::prefs::save_column_stats(!crate::prefs::column_stats());
        self.settings_menu = None;
        cx.emit(ToolbarChanged);
        cx.notify();
    }

    // Filters.

    fn open_filter(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit.as_ref().is_some_and(|edit| edit.index == index) {
            self.edit = None;
            cx.notify();
            return;
        }
        let Some(shape) = &self.shape else { return };
        let filter = match index.and_then(|i| self.filters.get(i)) {
            Some(f) => f.clone(),
            None => new_filter(
                self.picked as u32,
                &shape.columns,
                self.summary.get(&(self.picked as u32)),
            ),
        };
        let takes = self.takes(filter.column, &filter.op);
        let first = filter.values.first().cloned().unwrap_or_default();
        let second = filter.values.get(1).cloned().unwrap_or_default();
        let list = filter.values.join(", ");
        self.value_a.update(cx, |i, cx| {
            i.set_value(first, window, cx);
            if takes == Takes::One || takes == Takes::Two {
                i.focus(window, cx);
            }
        });
        self.value_b
            .update(cx, |i, cx| i.set_value(second, window, cx));
        self.typed.update(cx, |i, cx| {
            i.set_value(list, window, cx);
            if takes == Takes::Many {
                i.focus(window, cx);
            }
        });
        self.edit = Some(FilterEdit {
            index,
            column: filter.column,
            op: filter.op,
            ticked: filter.values,
            tried: false,
            columns_open: false,
            popup_triggers: crate::popup::PopupTriggers::default(),
        });
        cx.notify();
    }

    fn kind_of(&self, column: u32) -> &str {
        self.shape
            .as_ref()
            .and_then(|s| s.columns.get(column as usize))
            .map_or("other", |c| c.kind.as_str())
    }

    fn takes(&self, column: u32, op: &str) -> Takes {
        let ops = ops_for(self.kind_of(column));
        ops.iter().find(|o| o.op == op).unwrap_or(&ops[0]).takes
    }

    /// A category's values to tick, when all of them are known.
    fn choices(&self, column: u32) -> Option<Vec<String>> {
        let s = self
            .overall
            .get(&column)
            .or_else(|| self.summary.get(&column))?;
        (!s.top.is_empty() && s.distinct.is_some_and(|d| d as usize <= s.top.len()))
            .then(|| s.top.iter().map(|t| t.value.clone()).collect())
    }

    /// The filter as the popup has it now.
    fn edited_filter(&self, cx: &App) -> Option<TableFilter> {
        let edit = self.edit.as_ref()?;
        let ops = ops_for(self.kind_of(edit.column));
        let op = ops.iter().find(|o| o.op == edit.op).unwrap_or(&ops[0]);
        let a = self.value_a.read(cx).value().trim().to_string();
        let b = self.value_b.read(cx).value().trim().to_string();
        let values = match op.takes {
            Takes::None => Vec::new(),
            Takes::One => vec![a],
            Takes::Two => vec![a, b],
            Takes::Many => match self.choices(edit.column) {
                Some(_) => edit.ticked.clone(),
                None => self
                    .typed
                    .read(cx)
                    .value()
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect(),
            },
        };
        Some(TableFilter {
            column: edit.column,
            op: op.op.into(),
            values,
        })
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let Some(filter) = self.edited_filter(cx) else {
            return;
        };
        let column = self
            .shape
            .as_ref()
            .and_then(|s| s.columns.get(filter.column as usize));
        if filter_error(&filter, column).is_some() {
            if let Some(edit) = &mut self.edit {
                edit.tried = true;
            }
            cx.notify();
            return;
        }
        let Some(edit) = self.edit.take() else { return };
        let mut filters = self.filters.clone();
        match edit.index {
            Some(i) if i < filters.len() => filters[i] = filter,
            _ => filters.push(filter),
        }
        self.change_filters(filters, cx);
    }

    fn pick_filter_column(&mut self, column: u32, cx: &mut Context<Self>) {
        let ops = ops_for(self.kind_of(column));
        let Some(edit) = &mut self.edit else { return };
        edit.column = column;
        edit.columns_open = false;
        if !ops.iter().any(|o| o.op == edit.op) {
            edit.op = ops[0].op.into();
        }
        // Ticked values belong to their column.
        edit.ticked.clear();
        cx.notify();
    }

    /// A value clicked in the stats becomes a filter, or joins the
    /// column's "is one of".
    fn filter_to(&mut self, column: u32, value: String, cx: &mut Context<Self>) {
        let mut filters = self.filters.clone();
        match filters
            .iter_mut()
            .find(|f| f.column == column && f.op == "in")
        {
            Some(f) if f.values.contains(&value) => return,
            Some(f) => f.values.push(value),
            None => filters.push(TableFilter {
                column,
                op: "in".into(),
                values: vec![value],
            }),
        }
        self.change_filters(filters, cx);
    }

    // Copy and export.

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(range) = self.selection.range() else {
            return;
        };
        if range
            .row_count()
            .saturating_mul(range.column_count() as u64)
            > 1_000_000
        {
            self.note = Some((
                "Copy is limited to 1,000,000 cells. Select a smaller range.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let columns = self.visible_columns()[range.first_column..=range.last_column].to_vec();
        let mut values: Vec<Option<Vec<String>>> = (range.first_row..=range.last_row)
            .map(|row| match self.blocks.get(&(row / BLOCK)) {
                Some(Block::Ready(rows)) => rows.get((row % BLOCK) as usize).and_then(|row| {
                    columns
                        .iter()
                        .map(|column| {
                            let slot =
                                row.slots.as_ref().map_or(Some(*column as usize), |slots| {
                                    slots.get(column).copied()
                                })?;
                            row.cells.get(slot).map(clipboard_value)
                        })
                        .collect()
                }),
                _ => None,
            })
            .collect();
        let requests: Vec<_> = (range.first_row / BLOCK..=range.last_row / BLOCK)
            .filter(|block| {
                let start = (block * BLOCK).max(range.first_row);
                let end = ((block + 1) * BLOCK - 1).min(range.last_row);
                (start..=end).any(|row| values[(row - range.first_row) as usize].is_none())
            })
            .map(|block| {
                let mut request = self.rows_request(block);
                if let Call::TableRows {
                    columns: projection,
                    cells_only,
                    ..
                } = &mut request
                {
                    *projection = Some(columns.clone());
                    *cells_only = true;
                }
                request
            })
            .collect();
        let daemon = self.daemon.clone();
        let generation = self.generation;
        self.note = Some(("Copying selected values…".into(), false));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Result<String, String> = async {
                for request in requests {
                    let offset = match &request {
                        Call::TableRows { offset, .. } => *offset,
                        _ => unreachable!(),
                    };
                    let rows = daemon
                        .call::<TableRows>(request)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                    let indices = rows.column_indices.as_deref().unwrap_or(&columns);
                    for (i, cells) in rows.values.iter().enumerate() {
                        let row = offset + i as u64;
                        if row < range.first_row || row > range.last_row {
                            continue;
                        }
                        values[(row - range.first_row) as usize] = columns
                            .iter()
                            .map(|column| {
                                let slot = indices.iter().position(|i| i == column)?;
                                cells.get(slot).map(clipboard_value)
                            })
                            .collect();
                    }
                }
                let values: Option<Vec<Vec<String>>> = values.into_iter().collect();
                values.map(encode_tsv).ok_or_else(|| {
                    "The table changed while copying. Refresh and select the range again.".into()
                })
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.note = Some(if generation != this.generation {
                    (
                        "The table changed while copying. Select the range again.".into(),
                        true,
                    )
                } else {
                    match result {
                        Ok(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            ("Copied selected values.".into(), false)
                        }
                        Err(error) => (format!("Could not copy selected values: {error}"), true),
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        let code = table_code(
            &self.language,
            &shape.code,
            shape.matrix,
            &shape.columns,
            TableCodeView {
                sort: &self.sort,
                filters: &self.filters,
                hidden: &self.hidden,
                search: &self.search,
            },
        );
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.note = Some((
            format!("Copied {} code for this view.", self.language_name()),
            false,
        ));
        cx.notify();
    }

    /// Copies code that applies just the filters, in the session's language.
    fn copy_filter_code(&mut self, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        if self.filters.is_empty() {
            return;
        }
        let code = table_code(
            &self.language,
            &shape.code,
            shape.matrix,
            &shape.columns,
            TableCodeView {
                sort: &[],
                filters: &self.filters,
                hidden: &[],
                search: "",
            },
        );
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.note = Some((
            format!("Copied {} code for the filters.", self.language_name()),
            false,
        ));
        cx.notify();
    }

    fn language_name(&self) -> &'static str {
        if self.language.eq_ignore_ascii_case("r") {
            "R"
        } else {
            "Python"
        }
    }

    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = crate::prefs::export_folder(self.project.as_deref().unwrap_or(""))
            .filter(|f| !f.is_empty())
            .map_or_else(
                || export_name(&self.path),
                |f| format!("{f}/{}", export_name(&self.path)),
            );
        self.export_input.update(cx, |i, cx| {
            i.set_value(name, window, cx);
            i.focus(window, cx);
        });
        self.export = Some(Export::Naming);
        cx.notify();
    }

    fn export_file(&self, typed: &str) -> String {
        match &self.project {
            Some(project) if !typed.starts_with('/') => {
                format!("{}/{typed}", project.trim_end_matches('/'))
            }
            _ => typed.to_string(),
        }
    }

    /// The name was typed: asks before replacing a file that's there.
    fn export_named(&mut self, cx: &mut Context<Self>) {
        let typed = self.export_input.read(cx).value().trim().to_string();
        if typed.is_empty() || !matches!(self.export, Some(Export::Naming)) {
            return;
        }
        let file = self.export_file(&typed);
        self.export = Some(Export::Writing);
        let stat = self
            .daemon
            .call::<Vec<FileStat>>(Call::StatFiles { paths: vec![file] });
        cx.spawn(async move |this, cx| {
            let stat = stat.await;
            this.update(cx, |this, cx| {
                let there = stat
                    .ok()
                    .and_then(|s| s.into_iter().next())
                    .is_some_and(|s| s.mtime_ms.is_some());
                if there {
                    this.export = Some(Export::Replace(typed));
                    cx.notify();
                } else {
                    this.write_export(typed, cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn write_export(&mut self, typed: String, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        let rows = shape.rows;
        let file = self.export_file(&typed);
        self.export = Some(Export::Writing);
        let write = self.daemon.call::<Value>(Call::TableExport {
            session: self.session.clone(),
            path: self.path.clone(),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            file,
        });
        let project = self.project.clone().unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let done = write.await;
            this.update(cx, |this, cx| {
                this.export = None;
                this.note = Some(match done {
                    Ok(_) => {
                        if !typed.starts_with('/') {
                            let folder = typed.rsplit_once('/').map_or("", |(f, _)| f);
                            crate::prefs::save_export_folder(&project, folder);
                        }
                        (
                            format!("Exported {} rows to {typed}.", format_number(rows as f64)),
                            false,
                        )
                    }
                    Err(err) if format!("{err:#}").contains(BUSY) => (
                        format!(
                            "{} is running code; export once it finishes.",
                            this.language_name()
                        ),
                        true,
                    ),
                    Err(err) => (format!("Could not export the table: {err:#}"), true),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for Explorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let mut body = h_flex().flex_1().min_h_0().items_stretch();
        if self.side_shown && self.shape.is_some() {
            body = body.child(self.render_side(cx));
        }
        v_flex()
            .size_full()
            .relative()
            .id("data-explorer")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Copy, _, cx| {
                    this.copy_selection(cx);
                    cx.stop_propagation();
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::SelectAll, _, cx| {
                    let rows = this.shape.as_ref().map_or(0, |s| s.rows);
                    this.selection.all(rows, this.visible_columns().len());
                    cx.emit(FocusChanged);
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    if this.column_menu.is_some() || this.settings_menu.is_some() {
                        this.column_menu = None;
                        this.settings_menu = None;
                    } else if let Some(edit) = &mut this.edit {
                        if edit.columns_open {
                            edit.columns_open = false;
                        } else {
                            this.edit = None;
                        }
                    } else {
                        this.selection.clear();
                        cx.emit(FocusChanged);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if !event.keystroke.modifiers.platform {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "a" => {
                        let rows = this.shape.as_ref().map_or(0, |s| s.rows);
                        this.selection.all(rows, this.visible_columns().len());
                        cx.stop_propagation();
                        cx.emit(FocusChanged);
                        cx.notify();
                    }
                    "c" => {
                        this.copy_selection(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selection.dragging() {
                        this.selection.end();
                        cx.emit(FocusChanged);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selection.dragging() {
                        this.selection.end();
                        cx.emit(FocusChanged);
                    }
                }),
            )
            .children(self.render_export(cx))
            .child(
                body.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.selection.clear();
                                cx.emit(FocusChanged);
                                cx.notify();
                            }),
                        )
                        .children(self.render_note(&p))
                        .child(self.render_grid(cx))
                        .child(
                            h_flex()
                                .flex_none()
                                .gap(px(12.))
                                .px(px(10.))
                                .py(px(4.))
                                .border_t_1()
                                .border_color(p.line)
                                .text_color(p.faint)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .children(self.selection.range().map(|range| {
                                    div().flex_none().child(format!(
                                        "{} rows × {} columns selected",
                                        format_number(range.row_count() as f64),
                                        format_number(range.column_count() as f64),
                                    ))
                                }))
                                .child(div().flex_1())
                                .children(self.shape.as_ref().map(|s| {
                                    let rows = if s.rows == s.total {
                                        format!("{} rows", format_number(s.rows as f64))
                                    } else {
                                        format!(
                                            "{} of {} rows",
                                            format_number(s.rows as f64),
                                            format_number(s.total as f64)
                                        )
                                    };
                                    div().flex_none().child(format!(
                                        "{rows} · {} columns",
                                        format_number(s.width as f64)
                                    ))
                                })),
                        ),
                ),
            )
            .children(self.render_filter_popup(cx))
            .children(self.render_column_menu(cx))
            .children(self.render_settings_menu(cx))
    }
}

impl Explorer {
    /// A menu row: an icon, a label and a shortcut; dimmed when it can't
    /// be used.
    fn menu_item(
        id: &'static str,
        icon_name: &str,
        label: &'static str,
        shortcut: Option<&'static str>,
        enabled: bool,
        p: &Palette,
    ) -> Stateful<Div> {
        let hover = p.ink(8.);
        h_flex()
            .id(id)
            .h(px(28.))
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(6.))
            .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
            .when(!enabled, |d| d.opacity(0.4))
            .child(
                div()
                    .w(px(16.))
                    .flex_none()
                    .text_color(p.muted)
                    .child(icon(icon_name).size(px(14.))),
            )
            .child(div().flex_1().child(label))
            .children(shortcut.map(|s| div().pl(px(16.)).text_color(p.faint).child(s)))
    }

    /// A column's ⋮ menu, as Positron's data explorer has it.
    fn render_column_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (column, at) = self.column_menu?;
        let p = *Palette::get(cx);
        let sorted = self.sort.iter().any(|s| s.column == column);
        let pinned = self.pinned.contains(&column);
        let close = |this: &mut Self, cx: &mut Context<Self>| {
            this.column_menu = None;
            cx.notify();
        };
        let separator = || div().h(px(1.)).my(px(4.)).mx(px(6.)).bg(p.line);
        let shift_tip = |w: &mut Window, cx: &mut App| {
            gpui_kit::component::tooltip::Tooltip::new("Hold ⇧ to add it to the current sort")
                .build(w, cx)
        };
        let body = crate::composer_view::popup(&p)
            .id("de-column-menu-popup")
            .occlude()
            .min_w(px(220.))
            .text_size(px(look::SMALL))
            .child(
                Self::menu_item(
                    "de-cm-copy",
                    "edit.copy",
                    "Copy Column",
                    Some("⌘C"),
                    true,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    this.select_column(column, cx);
                    this.copy_selection(cx);
                })),
            )
            .child(
                Self::menu_item(
                    "de-cm-select",
                    "column.select",
                    "Select Column",
                    None,
                    true,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    this.select_column(column, cx);
                })),
            )
            .child(
                Self::menu_item(
                    "de-cm-pin",
                    if pinned { "pin.off" } else { "pin" },
                    if pinned { "Unpin Column" } else { "Pin Column" },
                    None,
                    true,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    this.toggle_pin(column, cx);
                })),
            )
            .child(separator())
            .child(
                Self::menu_item("de-cm-asc", "arrow.up", "Sort Ascending", None, true, &p)
                    .tooltip(shift_tip)
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        close(this, cx);
                        let sort = sorted_by(&this.sort, column, false, event.modifiers().shift);
                        this.change_sort(sort, cx);
                    })),
            )
            .child(
                Self::menu_item(
                    "de-cm-desc",
                    "arrow.down",
                    "Sort Descending",
                    None,
                    true,
                    &p,
                )
                .tooltip(shift_tip)
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    close(this, cx);
                    let sort = sorted_by(&this.sort, column, true, event.modifiers().shift);
                    this.change_sort(sort, cx);
                })),
            )
            .child(
                Self::menu_item(
                    "de-cm-clear",
                    "sort.clear",
                    "Clear Sorting",
                    None,
                    sorted,
                    &p,
                )
                .when(sorted, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        close(this, cx);
                        let sort = this
                            .sort
                            .iter()
                            .filter(|s| s.column != column)
                            .cloned()
                            .collect();
                        this.change_sort(sort, cx);
                    }))
                }),
            )
            .child(separator())
            .child(
                Self::menu_item("de-cm-filter", "add", "Add Filter", None, true, &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        close(this, cx);
                        this.picked = column as usize;
                        this.edit = None;
                        this.open_filter(None, window, cx);
                    }),
                ),
            )
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)));
        Some(
            deferred(anchored().position(at).snap_to_window().child(body))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// The gear's menu: the summary panel's default, and resetting the view.
    fn render_settings_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.settings_menu?;
        let p = *Palette::get(cx);
        let side_default = crate::prefs::column_stats();
        let body = crate::composer_view::popup(&p)
            .id("de-settings-popup")
            .occlude()
            .min_w(px(250.))
            .text_size(px(look::SMALL))
            .child(
                Self::menu_item(
                    "de-settings-side",
                    if side_default {
                        "check"
                    } else {
                        "sidebar.left"
                    },
                    "Show Summary Panel by Default",
                    None,
                    true,
                    &p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_side_default(cx))),
            )
            .child(div().h(px(1.)).my(px(4.)).mx(px(6.)).bg(p.line))
            .child(
                Self::menu_item("de-settings-reset", "refresh", "Reset View", None, true, &p)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "Clear filters, sorting, hidden and pinned columns, and widths",
                        )
                        .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.reset_view(cx))),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.settings_menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(at).snap_to_window().child(body))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// The bar over the table: the column list's toggle, the table and its
    /// session, read again, copy as code and export.
    fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let state = if self.busy { "running code…" } else { "live" };
        let what = self
            .shape
            .as_ref()
            .map_or_else(|| self.name(), |s| s.code.clone());
        let side = self.side_shown;
        h_flex()
            .flex_1()
            .w_full()
            .h_full()
            .px(px(6.))
            .gap(px(4.))
            .whitespace_nowrap()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.edit.is_some() {
                    this.edit = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                look::icon_button("de-side", "sidebar.left", &p, side)
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if side {
                            "Hide column stats"
                        } else {
                            "Show column stats"
                        })
                        .build(w, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show_side(!side, cx))),
            )
            .child(
                h_flex()
                    // The label gives way first, so Export stays in reach.
                    .flex_shrink(1.)
                    .min_w(px(40.))
                    .gap(px(6.))
                    .px(px(4.))
                    .children(
                        look::language_icon(&self.language)
                            .map(|path| look::picture(path).size(px(14.))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if self.busy { p.busy } else { p.muted })
                            .child(format!("{} · {what}, {state}", self.language_name())),
                    ),
            )
            .child(
                look::icon_button("de-refresh", "refresh", &p, false)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Read the table again")
                            .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                bar_button("de-copy", "Copy as code", Some("edit.copy"), &p)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "Copy code for this view: filters, sorting and shown columns",
                        )
                        .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.copy_code(cx))),
            )
            .child({
                let filtered = !self.filters.is_empty();
                bar_button(
                    "de-copy-filters",
                    "Copy filters as code",
                    Some("filter"),
                    &p,
                )
                .when(!filtered, |d| d.opacity(0.4).cursor_default())
                .when(filtered, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.copy_filter_code(cx)))
                })
            })
            .child(div().flex_1())
            .child(self.render_bar(cx))
            .child(
                look::icon_button("de-export", "doc.save", &p, false)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Export CSV").build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx))),
            )
            .when(!self.popped, |d| {
                d.child(
                    look::icon_button("de-pop-out", "window.pop-out", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Open in a new window")
                                .build(w, cx)
                        })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PopOut))),
                )
            })
            .child(
                look::icon_button("de-settings", "settings", &p, false)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Settings").build(w, cx)
                    })
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        this.settings_menu = match this.settings_menu {
                            Some(_) => None,
                            None => Some(event.position()),
                        };
                        this.column_menu = None;
                        cx.notify();
                    })),
            )
    }

    /// Asks for the export's file name, or whether to replace a file.
    fn render_export(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let export = self.export.as_ref()?;
        let rows = self.shape.as_ref().map_or(0, |s| s.rows);
        let bar = h_flex()
            .flex_none()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .bg(p.accent_soft)
            .border_b_1()
            .border_color(p.line);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            let mut b = gpui_kit::component::button::Button::new(id)
                .small()
                .label(label);
            if primary {
                b = gpui_kit::component::button::ButtonVariants::primary(b);
            }
            b
        };
        Some(match export {
            Export::Naming | Export::Writing => bar
                .child(div().flex_none().child("Export the table as"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.export_input).small()),
                )
                .child(div().flex_none().text_color(p.muted).child(format!(
                    "{} rows, filtered and sorted as shown",
                    format_number(rows as f64)
                )))
                .child(
                    button("de-export-go", "Export", true)
                        .on_click(cx.listener(|this, _, _, cx| this.export_named(cx))),
                )
                .child(
                    button("de-export-cancel", "Cancel", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.export = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
            Export::Replace(name) => {
                let name = name.clone();
                bar.child(div().flex_1().child(format!(
                    "Replace {name}? A file with this name is already there."
                )))
                .child(button("de-replace", "Replace", true).on_click(
                    cx.listener(move |this, _, _, cx| this.write_export(name.clone(), cx)),
                ))
                .child(
                    button("de-replace-cancel", "Cancel", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.export = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element()
            }
        })
    }

    /// The column list: each column with its sparkline and missing share,
    /// and its statistics folded under it like a folder's files. Clicking a
    /// column scrolls the table to it.
    fn render_side(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(shape) = &self.shape else {
            return div().into_any_element();
        };
        let shown_rows = shape.rows;
        let rows: Vec<AnyElement> = shape
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let column = i as u32;
                let summary = self.summary.get(&column);
                let on = self.picked == i;
                let shown = !self.hidden.contains(&column);
                let open = self.expanded.contains(&column);
                let hover = p.hover;
                let missing = summary.map(|s| s.missing);
                let share = missing.map(|m| missing_share(m, shown_rows));
                let tip = missing.map(|m| {
                    format!(
                        "{} of {} values missing ({})",
                        format_number(m as f64),
                        format_number(shown_rows as f64),
                        missing_text(&self.language)
                    )
                });
                let row = h_flex()
                    .id(("de-col", i))
                    .flex_none()
                    .h(px(SIDE_ROW))
                    .pl(px(2.))
                    .pr(px(6.))
                    .gap(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.sel))
                    .when(!on, |d| d.hover(move |d| d.bg(hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.reveal_column(column, cx)))
                    .child(
                        div()
                            .id(("de-fold", i))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_color(p.muted)
                            .hover(move |d| d.bg(hover))
                            .child(
                                icon(if open {
                                    "chevron.down"
                                } else {
                                    "chevron.right"
                                })
                                .size(px(12.)),
                            )
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if open {
                                    "Hide statistics"
                                } else {
                                    "Show statistics"
                                })
                                .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if !this.expanded.remove(&column) {
                                    this.expanded.insert(column);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        bar_button(SharedString::from(format!("de-visible-{i}")), "", None, &p)
                            .px(px(4.))
                            .child(if shown { "☑" } else { "☐" })
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if shown {
                                    "Hide column"
                                } else {
                                    "Show column"
                                })
                                .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.show_column(column, !shown, cx);
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(c.name.clone()),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(column_note(c, summary)),
                            ),
                    )
                    .child(self.spark(column, summary, &p))
                    .child(
                        div()
                            .id(("de-missing", i))
                            .w(px(MISSING_WIDTH))
                            .flex_none()
                            .text_right()
                            .text_size(px(look::SMALL))
                            .text_color(if missing.unwrap_or(0) > 0 {
                                p.muted
                            } else {
                                p.faint
                            })
                            .children(share)
                            .when_some(tip, |d, tip| {
                                d.tooltip(move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                        .build(w, cx)
                                })
                            }),
                    );
                v_flex()
                    .flex_none()
                    .child(row)
                    .when(open, |d| d.child(self.render_stats(column, c, cx)))
                    .into_any_element()
            })
            .collect();
        v_flex()
            .flex_none()
            .w(px(SIDE_WIDTH))
            .h_full()
            .p(px(6.))
            .border_r_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(4.))
                    .pb(px(4.))
                    .child(
                        bar_button("de-show-all", "Show all", None, &p).on_click(
                            cx.listener(|this, _, _, cx| this.show_all_columns(true, cx)),
                        ),
                    )
                    .child(
                        bar_button("de-hide-all", "Hide all", None, &p).on_click(
                            cx.listener(|this, _, _, cx| this.show_all_columns(false, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("de-cols")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.side_scroll)
                    .child(v_flex().gap(px(1.)).children(rows)),
            )
            .into_any_element()
    }

    /// A column's sparkline: a histogram of numbers, or bars for its
    /// commonest values.
    fn spark(&self, column: u32, summary: Option<&ColumnSummary>, p: &Palette) -> AnyElement {
        let bars: Vec<(u64, Option<Hsla>)> = match summary {
            Some(s) if !s.histogram.is_empty() => s.histogram.iter().map(|&n| (n, None)).collect(),
            Some(s) => s
                .top
                .iter()
                .map(|t| (t.count, self.color_of(column, &t.value, p)))
                .collect(),
            None => Vec::new(),
        };
        let top = bars.iter().map(|b| b.0).max().unwrap_or(1).max(1);
        let step = SPARK_WIDTH / bars.len().max(1) as f32;
        let accent = p.accent.opacity(0.7);
        h_flex()
            .flex_none()
            .w(px(SPARK_WIDTH))
            .h(px(SPARK_HEIGHT))
            .items_end()
            .children(bars.into_iter().map(move |(count, color)| {
                let height = if count == 0 {
                    0.
                } else {
                    (count as f32 / top as f32 * 20.).max(1.)
                };
                div()
                    .flex_none()
                    .w(px(step))
                    .h(px(height))
                    .pr(px(if step > 2. { 1. } else { 0. }))
                    .child(div().size_full().bg(color.unwrap_or(accent)))
            }))
            .into_any_element()
    }

    /// A column's numbers, folded open under its row: its range and middle,
    /// or its commonest values, each of which filters to it.
    fn render_stats(&self, ix: u32, column: &TableColumn, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let line = |label: AnyElement, value: String| {
            h_flex()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(p.muted)
                        .child(label),
                )
                .child(div().flex_none().child(value))
        };
        let text = |s: &str| div().child(s.to_string()).into_any_element();
        let mut rows: Vec<AnyElement> = Vec::new();
        match self.summary.get(&ix) {
            None => rows.push(
                div()
                    .text_color(p.faint)
                    .child("Reading…")
                    .into_any_element(),
            ),
            Some(s) => {
                if column.kind == "number"
                    && let (Some(min), Some(max)) = (s.min, s.max)
                {
                    let n = |v: Option<f64>| v.map(format_number).unwrap_or_default();
                    rows.push(line(text("Min"), format_number(min)).into_any_element());
                    rows.push(line(text("Median"), n(s.median)).into_any_element());
                    rows.push(line(text("Mean"), n(s.mean)).into_any_element());
                    rows.push(line(text("Max"), format_number(max)).into_any_element());
                }
                for (n, t) in s.top.iter().enumerate() {
                    let value = t.value.clone();
                    let color = self.color_of(ix, &t.value, &p);
                    let accent = p.accent;
                    let label = h_flex()
                        .id(("de-val", n))
                        .gap(px(5.))
                        .min_w_0()
                        .cursor_pointer()
                        .hover(move |d| d.text_color(accent))
                        .when_some(color, |d, c| d.child(swatch(c)))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(t.value.clone()),
                        )
                        .tooltip({
                            let tip =
                                format!("Show only rows where {} is {}", column.name, t.value);
                            move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                            }
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.filter_to(ix, value.clone(), cx)
                            }),
                        )
                        .into_any_element();
                    rows.push(line(label, format_number(t.count as f64)).into_any_element());
                }
                if let Some(distinct) = s.distinct
                    && !s.top.is_empty()
                    && distinct > s.top.len() as u64
                {
                    rows.push(
                        div()
                            .text_color(p.faint)
                            .child(format!(
                                "… {} more values",
                                format_number((distinct - s.top.len() as u64) as f64)
                            ))
                            .into_any_element(),
                    );
                }
                if column.kind != "number"
                    && let Some(distinct) = s.distinct
                {
                    rows.push(
                        line(text("Distinct"), format_number(distinct as f64)).into_any_element(),
                    );
                }
                rows.push(
                    line(
                        text(&format!("Missing ({})", missing_text(&self.language))),
                        format_number(s.missing as f64),
                    )
                    .into_any_element(),
                );
            }
        }
        div()
            .id(("de-stats", ix as usize))
            .flex_none()
            .ml(px(22.))
            .mr(px(6.))
            .mb(px(6.))
            .pl(px(8.))
            .py(px(4.))
            .border_l_1()
            .border_color(p.line)
            .child(v_flex().gap(px(3.)).children(rows))
            .into_any_element()
    }

    /// Filter chips and chip-less search, embedded in the single action row.
    fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let columns = self.shape.as_ref().map_or(&[][..], |s| &s.columns);
        let chips = self.filters.iter().enumerate().map(|(i, f)| {
            let soft = p.ink(8.);
            let text = p.text;
            h_flex()
                .flex_none()
                .h(px(24.))
                .pl(px(10.))
                .pr(px(3.))
                .gap(px(2.))
                .rounded(px(12.))
                .bg(p.accent_soft)
                .whitespace_nowrap()
                .child(
                    div()
                        .id(("de-chip", i))
                        .cursor_pointer()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(filter_text(f, columns))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_filter(Some(i), window, cx)
                        })),
                )
                .child(
                    div()
                        .id(("de-chip-x", i))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_color(p.muted)
                        .cursor_pointer()
                        .hover(move |d| d.bg(soft).text_color(text))
                        .child(icon("close").size(px(11.)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut filters = this.filters.clone();
                            if i < filters.len() {
                                filters.remove(i);
                            }
                            this.change_filters(filters, cx);
                        })),
                )
        });
        let text = p.text;
        let faint = p.faint;
        h_flex()
            .flex_shrink(1.)
            .min_w_0()
            .gap(px(6.))
            // Many chips are cut short before the buttons after them.
            .child(
                h_flex()
                    .flex_shrink(1.)
                    .min_w_0()
                    .overflow_hidden()
                    .gap(px(6.))
                    .children(chips),
            )
            .child(
                h_flex()
                    .id("de-add-filter")
                    .on_prepaint(self.popup_triggers.track("add-filter"))
                    .flex_none()
                    .h(px(24.))
                    .px(px(10.))
                    .gap(px(5.))
                    .rounded(px(12.))
                    .border_1()
                    .border_dashed()
                    .border_color(p.line)
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(move |d| d.text_color(text).border_color(faint))
                    .child(icon("filter").size(px(13.)))
                    .child("Add filter")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_filter(None, window, cx)),
                    ),
            )
            .child(
                div().flex_none().w(px(140.)).child(
                    Input::new(&self.search_input)
                        .small()
                        .prefix(icon("search").size(px(13.)).text_color(p.muted)),
                ),
            )
    }

    fn render_note(&self, p: &Palette) -> Option<AnyElement> {
        let (text, failed) = if self.busy {
            (
                format!(
                    "{} is running code; the table updates when it finishes.",
                    self.language_name()
                ),
                false,
            )
        } else if let Some(err) = &self.error {
            (err.clone(), true)
        } else {
            self.note.clone()?
        };
        let busy_note = self.busy;
        Some(
            div()
                .flex_none()
                .px(px(10.))
                .py(px(4.))
                .border_b_1()
                .border_color(p.line)
                .when(failed, |d| d.text_color(p.err).bg(p.err_soft))
                .when(!failed && busy_note, |d| {
                    d.text_color(p.busy).bg(p.busy_soft)
                })
                .when(!failed && !busy_note, |d| d.text_color(p.muted))
                .child(text)
                .into_any_element(),
        )
    }

    fn render_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(shape) = &self.shape else {
            return div()
                .flex_1()
                .p_4()
                .text_color(p.muted)
                .child(if self.error.is_some() {
                    ""
                } else {
                    "Reading the table…"
                })
                .into_any_element();
        };
        let visible = self.visible_columns();
        let pinned = self.pinned_shown();
        let widths = self.widths.clone();
        let label_width = self.label_width;
        let width = label_width
            + visible
                .iter()
                .filter_map(|&i| widths.get(i as usize))
                .sum::<f32>();
        // The row labels and pinned columns stay put while the rest
        // scrolls sideways: they are drawn over it, moved back by the scroll.
        let fixed_width = label_width
            + visible[..pinned]
                .iter()
                .filter_map(|&i| widths.get(i as usize))
                .sum::<f32>();
        // As the table is painted: a wheel turn can overshoot until layout
        // clamps it.
        let scrolled = (-self.across.offset().x)
            .min(self.across.max_offset().x)
            .max(px(0.));
        let selected = self.selection.range();
        let hover_ink = p.ink(8.);
        let mut header_cells: Vec<AnyElement> = visible
            .iter()
            .enumerate()
            .map(|(display, &original)| {
                let ix = original as usize;
                let column = &shape.columns[ix];
                let key = self.sort.iter().position(|s| s.column == original);
                let descending = key.is_some_and(|k| self.sort[k].descending);
                let group = SharedString::from(format!("de-header-{ix}"));
                let column_width = widths.get(ix).copied().unwrap_or(140.);
                let menu_open = self.column_menu.is_some_and(|(c, _)| c == original);
                h_flex()
                    .id(("column", ix))
                    .group(group.clone())
                    .relative()
                    .w(px(column_width))
                    .flex_none()
                    .h_full()
                    .px(px(8.))
                    .gap(px(2.))
                    .border_r_1()
                    .border_color(p.line)
                    .cursor_text()
                    .when(
                        selected
                            .is_some_and(|r| display >= r.first_column && display <= r.last_column),
                        |d| d.bg(p.sel),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.picked = ix;
                            this.begin_selection(
                                Target::Column(display),
                                event.modifiers.shift,
                                window,
                                cx,
                            );
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.extend_selection(Target::Column(display), cx);
                        }
                    }))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(
                                div()
                                    .min_w_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(column.name.clone()),
                            )
                            // A sorted column says which way, and its place
                            // among several sort keys.
                            .when_some(key, |d, k| {
                                d.child(
                                    h_flex()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(p.accent)
                                        .child(
                                            icon(if descending {
                                                "arrow.down"
                                            } else {
                                                "arrow.up"
                                            })
                                            .size(px(12.)),
                                        )
                                        .when(self.sort.len() > 1, |d| {
                                            d.child((k + 1).to_string())
                                        }),
                                )
                            }),
                    )
                    // Over the name's end, shown on hover: the name keeps
                    // the column's width.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(5.))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id(("de-column-menu", ix))
                                    .size(px(20.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .bg(p.panel)
                                    .text_color(p.muted)
                                    .hover(|d| d.bg(hover_ink))
                                    .when(!menu_open, |d| {
                                        d.opacity(0.).group_hover(group, |d| d.opacity(1.))
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(
                                        move |this, event: &ClickEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.column_menu = match this.column_menu {
                                                Some((c, _)) if c == original => None,
                                                _ => Some((original, event.position())),
                                            };
                                            this.settings_menu = None;
                                            cx.notify();
                                        },
                                    ))
                                    .child(icon("more.vertical").size(px(14.))),
                            ),
                    )
                    // The right edge drags to change the column's width.
                    .child(
                        div()
                            .id(("de-resize", ix))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(-4.))
                            .w(px(7.))
                            .cursor(CursorStyle::ResizeLeftRight)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.resizing = Some((ix, event.position.x, column_width));
                                    cx.notify();
                                }),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let header_pinned: Vec<AnyElement> = header_cells.drain(..pinned).collect();
        let header = h_flex()
            .flex_none()
            .relative()
            .h(px(HEADER_HEIGHT))
            .border_b_1()
            .border_color(p.line)
            .bg(p.panel)
            .child(div().w(px(fixed_width)).h_full().flex_none())
            .children(header_cells)
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(scrolled)
                    .bg(p.panel)
                    .child(
                        div()
                            .w(px(label_width))
                            .h_full()
                            .flex_none()
                            .border_r_1()
                            .border_color(p.line),
                    )
                    .children(header_pinned)
                    .when(pinned > 0, |d| {
                        d.border_r_1().border_color(p.accent.opacity(0.5))
                    }),
            );
        let numbers: Vec<bool> = shape.columns.iter().map(|c| c.kind == "number").collect();
        let language = self.language.clone();
        let list = uniform_list(
            "table-rows",
            shape.rows as usize,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let p = *Palette::get(cx);
                let cell_line = p.line.opacity(0.55);
                let selected = this.selection.range();
                range
                    .map(|ix| {
                        let row = this.row(ix, cx);
                        let selected_row = selected
                            .is_some_and(|r| ix as u64 >= r.first_row && ix as u64 <= r.last_row);
                        let line = h_flex()
                            .relative()
                            .h(px(ROW_HEIGHT))
                            .when(ix % 2 == 1, |d| d.bg(p.ink(3.)));
                        let label = row
                            .as_ref()
                            .map_or_else(|| "…".to_string(), |r| r.label.clone());
                        let label_el = div()
                            .id(("de-row-label", ix))
                            .w(px(label_width))
                            .h_full()
                            .flex()
                            .items_center()
                            .justify_end()
                            .flex_none()
                            .px(px(8.))
                            .border_r_1()
                            .border_b_1()
                            .border_color(cell_line)
                            .text_color(p.faint)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .cursor_text()
                            .when(selected_row, |d| d.bg(p.sel))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.begin_selection(
                                        Target::Row(ix as u64),
                                        event.modifiers.shift,
                                        window,
                                        cx,
                                    );
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _, cx| {
                                    if event.pressed_button == Some(MouseButton::Left) {
                                        this.extend_selection(Target::Row(ix as u64), cx);
                                    }
                                },
                            ))
                            .child(label);
                        let mut cells: Vec<AnyElement> = visible
                            .iter()
                            .enumerate()
                            .map(|(display, &original)| {
                                let col = original as usize;
                                let value = row.as_ref().and_then(|r| {
                                    let slot = r
                                        .slots
                                        .as_ref()
                                        .map_or(Some(col), |slots| slots.get(&original).copied())?;
                                    r.cells.get(slot)
                                });
                                let (text, missing) = value.map_or_else(
                                    || ("…".into(), true),
                                    |v| format_cell(v, &language),
                                );
                                let color = (!missing)
                                    .then(|| this.color_of(original, &text, &p))
                                    .flatten();
                                div()
                                    .id(("table-cell", ix * widths.len() + col))
                                    .w(px(widths.get(col).copied().unwrap_or(140.)))
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .flex_none()
                                    .gap(px(6.))
                                    .px(px(10.))
                                    .border_r_1()
                                    .border_b_1()
                                    .border_color(cell_line)
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .cursor_text()
                                    .when(
                                        selected.is_some_and(|r| r.contains(ix as u64, display)),
                                        |d| d.bg(p.sel),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                cx.stop_propagation();
                                                this.picked = col;
                                                this.begin_selection(
                                                    Target::Cell {
                                                        row: ix as u64,
                                                        column: display,
                                                    },
                                                    event.modifiers.shift,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
                                    .on_mouse_move(cx.listener(
                                        move |this, event: &MouseMoveEvent, _, cx| {
                                            if event.pressed_button == Some(MouseButton::Left) {
                                                this.extend_selection(
                                                    Target::Cell {
                                                        row: ix as u64,
                                                        column: display,
                                                    },
                                                    cx,
                                                );
                                            }
                                        },
                                    ))
                                    .when(numbers.get(col).copied().unwrap_or(false), |d| {
                                        d.justify_end()
                                    })
                                    .when(missing, |d| d.text_color(p.faint))
                                    .when_some(color, |d, c| d.child(swatch(c)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(text),
                                    )
                                    .into_any_element()
                            })
                            .collect();
                        let fixed: Vec<AnyElement> = cells.drain(..pinned).collect();
                        line.child(div().w(px(fixed_width)).h_full().flex_none())
                            .children(cells)
                            .child(
                                h_flex()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(scrolled)
                                    .bg(p.bg)
                                    .when(pinned > 0, |d| {
                                        d.border_r_1().border_color(p.accent.opacity(0.5))
                                    })
                                    .child(
                                        h_flex()
                                            .h_full()
                                            .when(ix % 2 == 1, |d| d.bg(p.ink(3.)))
                                            .child(label_el)
                                            .children(fixed),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .map(|mut list| {
            list.style().restrict_scroll_to_axis = Some(true);
            list
        })
        .flex_1();
        div()
            .id("table")
            .flex_1()
            .min_h_0()
            .overflow_x_scroll()
            .track_scroll(&self.across)
            .restrict_scroll_to_axis()
            .child(
                v_flex()
                    .w(px(width))
                    .min_w_full()
                    .h_full()
                    .child(header)
                    .child(list),
            )
            .when(self.resizing.is_some(), |d| {
                d.cursor(CursorStyle::ResizeLeftRight)
                    .child(self.column_resizer(cx))
            })
            .into_any_element()
    }

    /// While a column is resized: follows the pointer anywhere in the window
    /// until the button is let go.
    fn column_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let moved = this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    moved
                        .update(cx, |this, cx| {
                            let Some((column, start, width)) = this.resizing else {
                                return;
                            };
                            if !event.dragging() {
                                this.resizing = None;
                            } else if let Some(w) = this.widths.get_mut(column) {
                                *w = (width + f32::from(event.position.x - start))
                                    .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
                                    .round();
                            }
                            cx.stop_propagation();
                            cx.notify();
                        })
                        .ok();
                });
                let released = this.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        released
                            .update(cx, |this, cx| {
                                if this.resizing.take().is_some() {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            })
                            .ok();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    /// Adding or changing a filter: a column, a condition, and its values.
    fn render_filter_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let edit = self.edit.as_ref()?;
        let shape = self.shape.as_ref()?;
        let column = shape.columns.get(edit.column as usize);
        let ops = ops_for(self.kind_of(edit.column));
        let current = *ops.iter().find(|o| o.op == edit.op).unwrap_or(&ops[0]);
        let label =
            |text: &'static str| div().w(px(70.)).flex_none().text_color(p.muted).child(text);
        let row = || h_flex().gap(px(8.)).items_center();
        let hover = p.ink(8.);
        let column_button = h_flex()
            .id("df-column")
            .on_prepaint(edit.popup_triggers.track("filter-column"))
            .flex_1()
            .min_w_0()
            .h(px(26.))
            .px(px(8.))
            .gap(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(p.line)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(column.map_or_else(String::new, |c| c.name.clone())),
            )
            .child(icon("chevron.down").size(px(12.)).text_color(p.muted))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(edit) = &mut this.edit {
                    edit.columns_open = !edit.columns_open;
                }
                cx.notify();
            }));
        let column_list =
            edit.columns_open.then(|| {
                div()
                    .id("df-columns")
                    .ml(px(78.))
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(p.line)
                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if let Some(edit) = &mut this.edit
                            && !edit
                                .popup_triggers
                                .contains("filter-column", event.position)
                        {
                            edit.columns_open = false;
                            cx.notify();
                        }
                    }))
                    .children(shape.columns.iter().enumerate().map(|(i, c)| {
                        let on = i as u32 == edit.column;
                        h_flex()
                            .id(("df-col", i))
                            .h(px(24.))
                            .px(px(8.))
                            .gap(px(6.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(p.sel))
                            .hover(move |d| d.bg(hover))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(c.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(c.kind_detail.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pick_filter_column(i as u32, cx)
                            }))
                    }))
            });
        let conditions =
            h_flex()
                .flex_1()
                .flex_wrap()
                .gap(px(4.))
                .children(ops.iter().enumerate().map(|(i, o)| {
                    let on = o.op == current.op;
                    let name = o.op;
                    div()
                        .id(("df-op", i))
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(if on { p.accent } else { p.line })
                        .when(on, |d| d.bg(p.accent_soft))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(o.label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(edit) = &mut this.edit {
                                edit.op = name.into();
                            }
                            cx.notify();
                        }))
                }));
        let values: Option<AnyElement> = match current.takes {
            Takes::None => None,
            Takes::One | Takes::Two => Some(
                row()
                    .child(label(if current.takes == Takes::Two {
                        "From"
                    } else {
                        "Value"
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.value_a).small()),
                    )
                    .when(current.takes == Takes::Two, |d| {
                        d.child(div().flex_none().text_color(p.muted).child("and"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&self.value_b).small()),
                            )
                    })
                    .into_any_element(),
            ),
            Takes::Many => Some(match self.choices(edit.column) {
                Some(list) => div()
                    .id("df-choices")
                    .ml(px(78.))
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .gap(px(2.))
                            .children(list.into_iter().enumerate().map(|(i, v)| {
                                let ticked = edit.ticked.contains(&v);
                                let value = v.clone();
                                h_flex()
                                    .id(("df-choice", i))
                                    .gap(px(6.))
                                    .cursor_pointer()
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(if ticked { p.accent } else { p.faint })
                                            .when(ticked, |d| {
                                                d.bg(p.accent).child(
                                                    icon("check")
                                                        .size(px(11.))
                                                        .text_color(p.primary_text),
                                                )
                                            }),
                                    )
                                    .when_some(self.color_of(edit.column, &v, &p), |d, c| {
                                        d.child(swatch(c))
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(v),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(edit) = &mut this.edit {
                                            if let Some(at) =
                                                edit.ticked.iter().position(|t| t == &value)
                                            {
                                                edit.ticked.remove(at);
                                            } else {
                                                edit.ticked.push(value.clone());
                                            }
                                        }
                                        cx.notify();
                                    }))
                            })),
                    )
                    .into_any_element(),
                None => row()
                    .child(label("Values"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.typed).small()),
                    )
                    .into_any_element(),
            }),
        };
        let problem = edit
            .tried
            .then(|| self.edited_filter(cx))
            .flatten()
            .and_then(|f| filter_error(&f, column));
        let button = |id: &'static str, text: &'static str| {
            gpui_kit::component::button::Button::new(id)
                .small()
                .label(text)
        };
        let popup = crate::composer_view::popup(&p)
            .id("de-filter")
            .occlude()
            .w(px(340.))
            .p(px(10.))
            .text_size(px(look::SMALL))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                v_flex()
                    .gap(px(8.))
                    .child(row().child(label("Column")).child(column_button))
                    .children(column_list)
                    .child(
                        row()
                            .items_start()
                            .child(label("Condition").pt(px(3.)))
                            .child(conditions),
                    )
                    .children(values)
                    .when_some(problem, |d, problem| {
                        d.child(div().ml(px(78.)).text_color(p.err).child(problem))
                    })
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(6.))
                            .mt(px(2.))
                            .child(button("df-cancel", "Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.edit = None;
                                    cx.notify();
                                },
                            )))
                            .child(
                                gpui_kit::component::button::ButtonVariants::primary(button(
                                    "df-apply",
                                    if edit.index.is_some() {
                                        "Change filter"
                                    } else {
                                        "Add filter"
                                    },
                                ))
                                .on_click(cx.listener(|this, _, _, cx| this.apply_filter(cx))),
                            ),
                    ),
            )
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this.popup_triggers.contains("add-filter", event.position) {
                    this.edit = None;
                    cx.notify();
                }
            }));
        let top = if self.export.is_some() { 40. } else { 0. };
        let left = if self.side_shown { SIDE_WIDTH + 8. } else { 8. };
        Some(
            div()
                .absolute()
                .top(px(top))
                .left(px(left))
                // Anchored so a narrow pane neither squeezes nor clips it.
                .child(
                    deferred(anchored().snap_to_window_with_margin(px(8.)).child(popup))
                        .with_priority(3),
                )
                .into_any_element(),
        )
    }
}

fn clipboard_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

/// A table's name among those a project keeps: its language and path.
fn view_key(language: &str, path: &[String]) -> String {
    format!("{}:{}", language.to_lowercase(), path.join("\u{1f}"))
}

/// A category value's colour, as a small square.
fn swatch(color: Hsla) -> Div {
    div().size(px(8.)).flex_none().rounded(px(2.)).bg(color)
}

/// A column's missing share of `rows`, as the summary panel shows it:
/// "0%", "<0.1%", "3.2%", "100%".
fn missing_share(missing: u64, rows: u64) -> String {
    if missing == 0 || rows == 0 {
        return "0%".into();
    }
    let share = missing as f64 / rows as f64 * 100.;
    if share < 0.1 {
        "<0.1%".into()
    } else if share >= 99.95 {
        "100%".into()
    } else {
        format!("{:.1}%", share).replace(".0%", "%")
    }
}

#[cfg(test)]
mod view_tests {
    use super::{SavedView, missing_share};
    use sandhill_proto::{TableColumn, TableFilter, TableSort};

    fn columns(names: &[&str]) -> Vec<TableColumn> {
        names
            .iter()
            .map(|n| {
                serde_json::from_value(
                    serde_json::json!({"name": n, "kind": "number", "type": "dbl"}),
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn missing_shares_round_without_hiding_rare_or_total_gaps() {
        assert_eq!(missing_share(0, 375), "0%");
        assert_eq!(missing_share(12, 375), "3.2%");
        assert_eq!(missing_share(1, 100_000), "<0.1%");
        assert_eq!(missing_share(10, 100), "10%");
        assert_eq!(missing_share(99_999, 100_000), "100%");
        assert_eq!(missing_share(5, 0), "0%");
    }

    #[test]
    fn kept_views_follow_their_columns() {
        let sort = vec![TableSort {
            column: 1,
            descending: true,
        }];
        let filters = vec![TableFilter {
            column: 2,
            op: ">".into(),
            values: vec!["200".into()],
        }];
        let saved = SavedView::of(
            &sort,
            &filters,
            &columns(&["cell", "n_genes", "pct_mt"]),
            &[1],
        );
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["sort"][0]["name"], "n_genes");
        assert_eq!(json["filters"][0]["op"], ">");
        let back: SavedView = serde_json::from_value(json).unwrap();
        assert_eq!(back, saved);
        assert_eq!(
            back.hidden_on(&columns(&["batch", "cell", "n_genes"])),
            vec![2]
        );
        // Same table: as it was.
        assert_eq!(
            back.on(&columns(&["cell", "n_genes", "pct_mt"])),
            (sort.clone(), filters.clone())
        );
        // A column added at the front: they move along; one gone: dropped.
        let (s, f) = back.on(&columns(&["batch", "cell", "n_genes"]));
        assert_eq!(s[0].column, 2);
        assert!(f.is_empty());
    }

    #[test]
    fn old_saved_views_show_every_column() {
        let saved: SavedView =
            serde_json::from_value(serde_json::json!({"sort": [], "filters": []})).unwrap();
        assert!(saved.hidden_on(&columns(&["a", "b"])).is_empty());
    }
}
