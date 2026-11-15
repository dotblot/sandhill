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
use gpui_kit::component::checkbox::Checkbox;
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
use crate::editor::table::{
    TableCodeView, Takes, column_note, column_width, export_name, filter_error, filter_text,
    format_cell, format_number, missing_text, new_filter, ops_for, sorted_by, table_code,
};
use crate::editor::table_selection::{Selection, Target, encode_tsv};
use crate::look::{self, Palette, bar_button, icon};

mod export;
mod filters;
mod grid;
mod summary;

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
const SIDE_WIDTH: f32 = 320.;
const SIDE_ROW: f32 = 32.;
const SPARK_WIDTH: f32 = 56.;
const SPARK_HEIGHT: f32 = 22.;
/// The missing share after each sparkline.
const MISSING_WIDTH: f32 = 36.;
/// A column's type badge in the column panel.
const BADGE_WIDTH: f32 = 34.;
/// A common value's name in folded statistics, before its bar.
const VALUE_WIDTH: f32 = 96.;
/// The widest a column starts, from its measured contents.
const FIT_WIDTH: f32 = 320.;
/// Kit base families, then their brighter variants for larger categories.
const CATEGORY_ANSI: [usize; 10] = [4, 3, 1, 6, 2, 5, 12, 11, 9, 14];
/// What the daemon says when the session runs other code.
const BUSY: &str = "the session is busy running code";

/// What the table's first answer said about it.
/// A table's sorting, filters, hidden and pinned columns and the widths the
/// user gave columns, as a project keeps them between launches, each with
/// its column's name, so they follow their columns if the table changed
/// meanwhile.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct SavedView {
    #[serde(default)]
    sort: Vec<Named<TableSort>>,
    #[serde(default)]
    filters: Vec<Named<TableFilter>>,
    #[serde(default)]
    hidden: Vec<HiddenColumn>,
    #[serde(default)]
    pinned: Vec<HiddenColumn>,
    #[serde(default)]
    widths: Vec<ColumnWidth>,
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

/// A width the user dragged a column to.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ColumnWidth {
    name: String,
    column: u32,
    width: f32,
}

/// Where the column called `name`, once at `was`, is now.
fn column_now(columns: &[TableColumn], name: &str, was: u32) -> Option<u32> {
    if columns.get(was as usize).is_some_and(|c| c.name == name) {
        return Some(was);
    }
    columns
        .iter()
        .position(|c| c.name == name)
        .map(|i| i as u32)
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
            pinned: Vec::new(),
            widths: Vec::new(),
        }
    }

    /// With the pinned columns, in their order, and the widths given.
    fn with_layout(
        mut self,
        columns: &[TableColumn],
        pinned: &[u32],
        widths: &[(u32, f32)],
    ) -> SavedView {
        let name = |column: u32| columns.get(column as usize).map(|c| c.name.clone());
        self.pinned = pinned
            .iter()
            .filter_map(|&column| {
                Some(HiddenColumn {
                    name: name(column)?,
                    column,
                })
            })
            .collect();
        self.widths = widths
            .iter()
            .filter_map(|&(column, width)| {
                Some(ColumnWidth {
                    name: name(column)?,
                    column,
                    width,
                })
            })
            .collect();
        self
    }

    fn is_empty(&self) -> bool {
        self.sort.is_empty()
            && self.filters.is_empty()
            && self.hidden.is_empty()
            && self.pinned.is_empty()
            && self.widths.is_empty()
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
            .filter_map(|h| column_now(columns, &h.name, h.column))
            .collect()
    }

    fn pinned_on(&self, columns: &[TableColumn]) -> Vec<u32> {
        self.pinned
            .iter()
            .filter_map(|p| column_now(columns, &p.name, p.column))
            .collect()
    }

    fn widths_on(&self, columns: &[TableColumn]) -> Vec<(u32, f32)> {
        self.widths
            .iter()
            .filter_map(|w| Some((column_now(columns, &w.name, w.column)?, w.width)))
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
    /// The columns whose width the user set, which the project keeps.
    resized: Vec<u32>,
    /// A column being resized by its header's right edge: the column, where
    /// the drag began and the width then.
    resizing: Option<(usize, Pixels, f32)>,
    label_width: f32,
    /// The columns' summaries with the filters on, and with none: every
    /// value a filter can pick from.
    summary: HashMap<u32, ColumnSummary>,
    overall: HashMap<u32, ColumnSummary>,
    /// The read (`generation`) the summary belongs to.
    summary_generation: u64,
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
    /// The Export menu, where it opened.
    export_menu: Option<Point<Pixels>>,
    /// The column panel's ⋯ menu, where it opened.
    columns_menu: Option<Point<Pixels>>,
    /// The column panel's Find column field.
    column_find: Entity<InputState>,
    /// A new table's widths, still to be measured from its first rows.
    fit_widths: bool,
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
        let column_find = cx.new(|cx| InputState::new(window, cx).placeholder("Find column"));
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
            cx.subscribe_in(&column_find, window, |_, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
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
            resized: Vec::new(),
            resizing: None,
            label_width: 60.,
            summary: HashMap::new(),
            overall: HashMap::new(),
            summary_generation: 0,
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
            export_menu: None,
            columns_menu: None,
            column_find,
            fit_widths: false,
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
        self.save_view();
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
        self.resized.clear();
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
            .then(|| self.shape.as_ref().map(|s| self.view_of(&s.columns)))
            .flatten();
        if !same {
            self.resized.clear();
            // A first guess from character counts, measured properly once
            // the window can lay the first rows out (`fit_column_widths`).
            self.fit_widths = true;
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
        // The summary of this same read can arrive before its first rows:
        // its category colours wait until the columns are known.
        if !same && self.summary_generation == self.generation {
            let summaries: Vec<ColumnSummary> = self.summary.values().cloned().collect();
            self.color_values(&summaries);
            if self.filters.is_empty() && self.search.is_empty() {
                self.overall = self.summary.clone();
            }
        }
        let Some(saved) = self.saved.take().or(previous) else {
            return false;
        };
        let (sort, filters) = saved.on(&rows.columns);
        self.hidden = saved.hidden_on(&rows.columns);
        self.pinned = saved.pinned_on(&rows.columns);
        for (column, width) in saved.widths_on(&rows.columns) {
            if let Some(slot) = self.widths.get_mut(column as usize) {
                *slot = width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
                self.resized.push(column);
            }
        }
        if sort == self.sort && filters == self.filters {
            return false;
        }
        self.sort = sort;
        self.filters = filters;
        self.save_view();
        true
    }

    /// The view as it is now, on `columns`.
    fn view_of(&self, columns: &[TableColumn]) -> SavedView {
        let widths: Vec<(u32, f32)> = self
            .resized
            .iter()
            .filter_map(|&c| Some((c, *self.widths.get(c as usize)?)))
            .collect();
        SavedView::of(&self.sort, &self.filters, columns, &self.hidden).with_layout(
            columns,
            &self.pinned,
            &widths,
        )
    }

    /// Keeps the view for the next time this table opens in the project.
    fn save_view(&self) {
        let (Some(project), Some(shape)) = (self.project.as_deref(), self.shape.as_ref()) else {
            return;
        };
        let key = view_key(&self.language, &self.path);
        let view = Some(self.view_of(&shape.columns))
            .filter(|v| !v.is_empty())
            .and_then(|v| serde_json::to_value(v).ok());
        crate::prefs::save_table_view(project, &key, view);
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
}

impl Render for Explorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.fit_widths {
            self.fit_column_widths(window);
        }
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
                    if this.menu_open() {
                        this.close_menus();
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
            .children(self.render_filter_row(cx))
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
                        .child(
                            v_flex()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(self.render_grid(cx))
                                .children(self.render_filter_popup(cx)),
                        )
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
                                        "{} × {} selected",
                                        counted(range.row_count(), "row"),
                                        counted(range.column_count() as u64, "column"),
                                    ))
                                }))
                                .child(div().flex_1())
                                .children(self.shape.as_ref().map(|s| {
                                    let rows = if s.rows == s.total {
                                        counted(s.rows, "row")
                                    } else {
                                        format!(
                                            "{} of {}",
                                            format_number(s.rows as f64),
                                            counted(s.total, "row")
                                        )
                                    };
                                    div()
                                        .flex_none()
                                        .child(format!("{rows} · {}", counted(s.width, "column")))
                                })),
                        ),
                ),
            )
            .children(self.render_column_menu(cx))
            .children(self.render_settings_menu(cx))
            .children(self.render_export_menu(cx))
            .children(self.render_columns_menu(cx))
    }
}

impl Explorer {
    /// The action row: what the table is on the left (the column panel's
    /// toggle, the table's name and a status dot), what to do with it on
    /// the right (Search, Filter, Export, read again, a window of its own,
    /// settings), in an order that never moves.
    fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let what = self
            .shape
            .as_ref()
            .map_or_else(|| self.name(), |s| s.code.clone());
        let code = format!("{} · {what}", self.language_name());
        let side = self.side_shown;
        let (dot, status) = if let Some(err) = &self.error {
            (p.err, err.clone())
        } else if self.busy {
            (
                p.warn,
                format!(
                    "{} is running code; the table updates when it finishes",
                    self.language_name()
                ),
            )
        } else {
            (
                p.ok,
                "Live: read again after each run in its session".to_string(),
            )
        };
        let filters = self.filters.len();
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
                    .tooltip(tip(if side {
                        "Hide the column panel"
                    } else {
                        "Show the column panel"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_side(!side, cx))),
            )
            .child(
                h_flex()
                    .id("de-name")
                    // The name gives way first, so the actions stay in reach.
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
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.name()),
                    )
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(code.clone()).build(w, cx)
                    }),
            )
            .child(
                div()
                    .id("de-status")
                    .flex_none()
                    .p(px(3.))
                    .child(look::dot(dot))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(status.clone()).build(w, cx)
                    }),
            )
            .child(div().flex_1())
            .child(
                div().flex_none().w(px(150.)).child(
                    Input::new(&self.search_input)
                        .small()
                        .prefix(icon("search").size(px(13.)).text_color(p.muted)),
                ),
            )
            .child(
                bar_button(
                    "de-add-filter",
                    if filters == 0 {
                        "Filter".to_string()
                    } else {
                        format!("Filter · {filters}")
                    },
                    Some("filter"),
                    &p,
                )
                .on_prepaint(self.popup_triggers.track("add-filter"))
                .when(filters > 0, |d| d.text_color(p.accent))
                .tooltip(tip("Add a filter"))
                .on_click(cx.listener(|this, _, window, cx| this.open_filter(None, window, cx))),
            )
            .child(
                bar_button("de-export", "Export", Some("doc.save"), &p)
                    .on_prepaint(self.popup_triggers.track("export"))
                    .when(self.export_menu.is_some(), |d| d.bg(p.ink(10.)))
                    .child(icon("chevron.down").size(px(10.)))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        let open = this.export_menu.is_none();
                        this.close_menus();
                        if open {
                            this.export_menu = Some(event.position());
                        }
                        cx.notify();
                    })),
            )
            .child(
                look::icon_button("de-refresh", "refresh", &p, false)
                    .tooltip(tip("Read the table again"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .when(!self.popped, |d| {
                d.child(
                    look::icon_button("de-pop-out", "window.pop-out", &p, false)
                        .tooltip(tip("Open in a new window"))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PopOut))),
                )
            })
            .child(
                look::icon_button("de-settings", "settings", &p, false)
                    .on_prepaint(self.popup_triggers.track("settings"))
                    .tooltip(tip("Settings"))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        let open = this.settings_menu.is_none();
                        this.close_menus();
                        if open {
                            this.settings_menu = Some(event.position());
                        }
                        cx.notify();
                    })),
            )
    }

    fn menu_open(&self) -> bool {
        self.column_menu.is_some()
            || self.settings_menu.is_some()
            || self.export_menu.is_some()
            || self.columns_menu.is_some()
    }

    fn close_menus(&mut self) {
        self.column_menu = None;
        self.settings_menu = None;
        self.export_menu = None;
        self.columns_menu = None;
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
}

/// A table's name among those a project keeps: its language and path.
fn view_key(language: &str, path: &[String]) -> String {
    format!("{}:{}", language.to_lowercase(), path.join("\u{1f}"))
}

/// A plain tooltip.
fn tip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |w, cx| gpui_kit::component::tooltip::Tooltip::new(text).build(w, cx)
}

/// "1 row", "2,000 rows".
fn counted(n: u64, word: &str) -> String {
    format!(
        "{} {word}{}",
        format_number(n as f64),
        if n == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod view_tests {
    use super::SavedView;
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

    #[test]
    fn kept_pins_and_widths_follow_their_columns() {
        let was = columns(&["cell", "n_genes", "pct_mt"]);
        let saved =
            SavedView::of(&[], &[], &was, &[]).with_layout(&was, &[2, 0], &[(1, 220.), (2, 90.)]);
        assert!(!saved.is_empty());
        let back: SavedView =
            serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
        // Same table: the pins in the order given, the widths as dragged.
        assert_eq!(back.pinned_on(&was), vec![2, 0]);
        assert_eq!(back.widths_on(&was), vec![(1, 220.), (2, 90.)]);
        // A column added at the front and one gone: moved along, dropped.
        let now = columns(&["batch", "cell", "n_genes"]);
        assert_eq!(back.pinned_on(&now), vec![1]);
        assert_eq!(back.widths_on(&now), vec![(2, 220.)]);
        // A view saved before pins and widths were kept has none.
        let old: SavedView =
            serde_json::from_value(serde_json::json!({"sort": [], "filters": []})).unwrap();
        assert!(old.pinned_on(&was).is_empty() && old.widths_on(&was).is_empty());
        assert!(old.is_empty());
    }
}
