//! The Data Explorer, as the Tauri app has it (`DataExplorer.tsx`): a data
//! frame or matrix from a kernel as a table that only draws the rows in
//! view and reads them from the kernel in blocks as it scrolls. The columns
//! are listed at the side with a small histogram or bar chart each, and
//! the picked column's numbers under them. Filters and sorting run in the
//! kernel, so a table of a million cells scrolls like one of a hundred. The
//! table is read again after each run in its session, so it stays live.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, ColumnSummary, FileStat, TableColumn, TableFilter, TableRows, TableSort, TableSummary,
};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::look::{self, Palette, bar_button, icon};
use crate::table::{
    COLORS, Takes, column_note, column_width, export_name, filter_error, filter_text, format_cell,
    format_number, missing_text, new_filter, next_sort, ops_for, sort_text, table_code,
};

/// Rows read at a time.
const BLOCK: u64 = 200;
/// Columns read; wider tables show their first ones.
const COLUMNS: u32 = 200;
/// The Tauri app's sizes, a little taller for text 2px larger.
const ROW_HEIGHT: f32 = 26.;
const HEADER_HEIGHT: f32 = 40.;
const SIDE_WIDTH: f32 = 270.;
const SIDE_ROW: f32 = 42.;
const SPARK_WIDTH: f32 = 70.;
const SPARK_HEIGHT: f32 = 22.;
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
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Named<T> {
    name: String,
    #[serde(flatten)]
    key: T,
}

impl SavedView {
    fn of(sort: &[TableSort], filters: &[TableFilter], columns: &[TableColumn]) -> SavedView {
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
}

struct Shape {
    code: String,
    matrix: bool,
    columns: Vec<TableColumn>,
    width: u64,
    total: u64,
    rows: u64,
}

enum Block {
    Loading,
    Ready(Vec<(String, Vec<Value>)>),
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
}

/// Exporting as CSV: the name asked for, then whether to replace a file.
enum Export {
    Naming,
    Replace(String),
    Writing,
}

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
    /// The sorting and filters kept from last time, until the table's
    /// columns are known and they are moved onto them.
    saved: Option<SavedView>,
    blocks: HashMap<u64, Block>,
    widths: Vec<f32>,
    label_width: f32,
    /// The columns' summaries with the filters on, and with none: every
    /// value a filter can pick from.
    summary: HashMap<u32, ColumnSummary>,
    overall: HashMap<u32, ColumnSummary>,
    /// Each category column's values' colours, kept as filters change.
    colors: HashMap<u32, Vec<(String, u32)>>,
    picked: usize,
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
    _subscriptions: Vec<Subscription>,
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
            saved,
            blocks: HashMap::new(),
            widths: Vec::new(),
            label_width: 60.,
            summary: HashMap::new(),
            overall: HashMap::new(),
            colors: HashMap::new(),
            picked: 0,
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
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    pub fn name(&self) -> String {
        self.path.join(" › ")
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Code started or stopped running in the table's session: once it
    /// stops the table is read again, as the kernel has it now.
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy == busy {
            return;
        }
        self.busy = busy;
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
        cx.notify();
    }

    fn load(&mut self, block: u64, cx: &mut Context<Self>) {
        if self.blocks.contains_key(&block) {
            return;
        }
        self.blocks.insert(block, Block::Loading);
        let generation = self.generation;
        let first = block == 0;
        let rows = self.daemon.call::<TableRows>(Call::TableRows {
            session: self.session.clone(),
            path: self.path.clone(),
            offset: block * BLOCK,
            limit: Some(BLOCK as u32),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            column_offset: 0,
            column_limit: Some(COLUMNS),
            cells_only: !first,
        });
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
                        let cells = rows.labels.into_iter().zip(rows.values).collect();
                        this.blocks.insert(block, Block::Ready(cells));
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
        let Some(saved) = self.saved.take() else {
            return false;
        };
        let (sort, filters) = saved.on(&rows.columns);
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
        let view = (!self.sort.is_empty() || !self.filters.is_empty())
            .then(|| SavedView::of(&self.sort, &self.filters, &shape.columns))
            .and_then(|v| serde_json::to_value(v).ok());
        crate::prefs::save_table_view(project, &key, view);
    }

    /// Reads the columns' summaries, with the filters on.
    fn read_summary(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        let filtered = !self.filters.is_empty();
        let read = self.daemon.call::<TableSummary>(Call::TableSummary {
            session: self.session.clone(),
            path: self.path.clone(),
            filters: self.filters.clone(),
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
            if s.distinct.is_none_or(|d| d > COLORS.len() as u64) {
                continue;
            }
            let map = self.colors.entry(s.column).or_default();
            for t in &s.top {
                if map.len() < COLORS.len() && !map.iter().any(|(v, _)| v == &t.value) {
                    map.push((t.value.clone(), COLORS[map.len()]));
                }
            }
        }
    }

    fn color_of(&self, column: u32, value: &str) -> Option<u32> {
        self.colors
            .get(&column)?
            .iter()
            .find(|(v, _)| v == value)
            .map(|(_, c)| *c)
    }

    fn change_sort(&mut self, sort: Vec<TableSort>, cx: &mut Context<Self>) {
        self.sort = sort;
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
        self.saved = None;
        self.save_view();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<(String, Vec<Value>)> {
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

    fn show_side(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.side_shown = shown;
        crate::prefs::save_column_stats(shown);
        cx.notify();
    }

    // Filters.

    fn open_filter(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
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

    fn copy_code(&mut self, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        let code = table_code(
            &self.language,
            &shape.code,
            shape.matrix,
            &shape.columns,
            &self.sort,
            &self.filters,
        );
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.note = Some((
            format!("Copied {} code for this view.", self.language_name()),
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
            .child(self.render_top(cx))
            .children(self.render_export(cx))
            .child(
                body.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(self.render_bar(cx))
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
                                .child(div().flex_none().child(sort_text(
                                    &self.sort,
                                    self.shape.as_ref().map_or(&[][..], |s| &s.columns),
                                )))
                                .child(div().min_w_0().overflow_hidden().text_ellipsis().child(format!(
                                    "Sorting, filters and stats run in the {} session; only the rows on screen are read.",
                                    self.language_name()
                                ))),
                        ),
                ),
            )
            .children(self.render_filter_popup(cx))
    }
}

impl Explorer {
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
            .flex_none()
            .h(px(look::STRIP))
            .px(px(6.))
            .gap(px(4.))
            .border_b_1()
            .border_color(p.line)
            .child(
                bar_button(
                    "de-side",
                    if side {
                        "Hide column stats"
                    } else {
                        "Show column stats"
                    },
                    Some("sidebar.left"),
                    &p,
                )
                .when(side, |d| d.text_color(p.text))
                .on_click(cx.listener(move |this, _, _, cx| this.show_side(!side, cx))),
            )
            .child(
                h_flex()
                    .min_w_0()
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
            .child(div().flex_1())
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
                    .on_click(cx.listener(|this, _, _, cx| this.copy_code(cx))),
            )
            .child(
                bar_button("de-export", "Export CSV", Some("doc.save"), &p)
                    .on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx))),
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

    /// The column list with a sparkline each, and the picked column's
    /// numbers under it.
    fn render_side(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(shape) = &self.shape else {
            return div().into_any_element();
        };
        let rows = shape.columns.iter().enumerate().map(|(i, c)| {
            let summary = self.summary.get(&(i as u32));
            let on = self.picked == i;
            let hover = p.hover;
            h_flex()
                .id(("de-col", i))
                .flex_none()
                .h(px(SIDE_ROW))
                .px(px(6.))
                .gap(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(p.sel))
                .when(!on, |d| d.hover(move |d| d.bg(hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picked = i;
                    cx.notify();
                }))
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
                .child(self.spark(i as u32, summary, &p))
        });
        v_flex()
            .flex_none()
            .w(px(SIDE_WIDTH))
            .h_full()
            .p(px(6.))
            .border_r_1()
            .border_color(p.line)
            .child(
                div()
                    .id("de-cols")
                    .flex_shrink(1.)
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.side_scroll)
                    .child(v_flex().gap(px(1.)).children(rows)),
            )
            .children(
                shape
                    .columns
                    .get(self.picked)
                    .map(|c| self.render_stats(c, cx)),
            )
            .into_any_element()
    }

    /// A column's sparkline: a histogram of numbers, or bars for its
    /// commonest values.
    fn spark(&self, column: u32, summary: Option<&ColumnSummary>, p: &Palette) -> AnyElement {
        let bars: Vec<(u64, Option<u32>)> = match summary {
            Some(s) if !s.histogram.is_empty() => s.histogram.iter().map(|&n| (n, None)).collect(),
            Some(s) => s
                .top
                .iter()
                .map(|t| (t.count, self.color_of(column, &t.value)))
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
                    .child(
                        div()
                            .size_full()
                            .bg(color.map_or(accent, |c| rgb(c).into())),
                    )
            }))
            .into_any_element()
    }

    /// The picked column's numbers: its range and middle, or its commonest
    /// values, each of which filters to it.
    fn render_stats(&self, column: &TableColumn, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let ix = self.picked as u32;
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
                    let color = self.color_of(ix, &t.value);
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
            .id("de-stats")
            .flex_none()
            .max_h(relative(0.6))
            .overflow_y_scroll()
            .mt(px(8.))
            .pt(px(10.))
            .px(px(6.))
            .pb(px(6.))
            .border_t_1()
            .border_color(p.line)
            .child(
                v_flex()
                    .gap(px(3.))
                    .child(
                        div()
                            .mb(px(4.))
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(column.name.clone()),
                    )
                    .children(rows),
            )
            .into_any_element()
    }

    /// The filters as chips, Add filter, and the rows' count.
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
            .flex_none()
            .flex_wrap()
            .gap(px(6.))
            .px(px(8.))
            .py(px(5.))
            .min_h(px(35.))
            .border_b_1()
            .border_color(p.line)
            .children(chips)
            .child(
                h_flex()
                    .id("de-add-filter")
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
            .child(div().flex_1())
            .children(self.shape.as_ref().map(|s| {
                let rows = if s.rows == s.total {
                    format!("{} rows", format_number(s.total as f64))
                } else {
                    format!(
                        "{} of {} rows",
                        format_number(s.rows as f64),
                        format_number(s.total as f64)
                    )
                };
                let columns = format!(
                    "{} {}{}",
                    format_number(s.width as f64),
                    if s.width == 1 { "column" } else { "columns" },
                    if s.width > COLUMNS as u64 {
                        format!(", the first {COLUMNS} shown")
                    } else {
                        String::new()
                    }
                );
                div()
                    .flex_none()
                    .text_color(p.muted)
                    .whitespace_nowrap()
                    .child(format!("{rows} · {columns}"))
            }))
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
                .child(match &self.error {
                    Some(_) => "",
                    None => "Reading the table…",
                })
                .into_any_element();
        };
        let shown_columns = shape.columns.len().min(COLUMNS as usize);
        let widths = self.widths.clone();
        let label_width = self.label_width;
        let width = label_width + widths.iter().take(shown_columns).sum::<f32>();
        let picked = self.picked;
        let header =
            h_flex()
                .h(px(HEADER_HEIGHT))
                .border_b_1()
                .border_color(p.line)
                .bg(p.panel)
                .child(
                    div()
                        .w(px(label_width))
                        .h_full()
                        .flex_none()
                        .border_r_1()
                        .border_color(p.line),
                )
                .children(shape.columns.iter().take(shown_columns).enumerate().map(
                    |(ix, column)| {
                        let key = self.sort.iter().position(|s| s.column == ix as u32);
                        let arrow = key.map(|k| {
                            format!(
                                "{}{}",
                                if self.sort[k].descending {
                                    "▼"
                                } else {
                                    "▲"
                                },
                                if self.sort.len() > 1 {
                                    (k + 1).to_string()
                                } else {
                                    String::new()
                                }
                            )
                        });
                        let number = column.kind == "number";
                        let hover = p.hover;
                        v_flex()
                            .id(("column", ix))
                            .w(px(widths.get(ix).copied().unwrap_or(140.)))
                            .flex_none()
                            .h_full()
                            .px(px(10.))
                            .pt(px(4.))
                            .border_r_1()
                            .border_color(p.line)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .when(picked == ix, |d| d.border_b_2().border_color(p.accent))
                            .when(number, |d| d.items_end())
                            .tooltip({
                                let tip = format!(
                                    "Sort by {} (shift-click to sort by it next)",
                                    column.name
                                );
                                move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                        .build(w, cx)
                                }
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.picked = ix;
                                let sort =
                                    next_sort(&this.sort, ix as u32, event.modifiers().shift);
                                this.change_sort(sort, cx);
                            }))
                            .child(
                                h_flex()
                                    .max_w_full()
                                    .gap(px(4.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(column.name.clone()),
                                    )
                                    .when_some(arrow, |d, arrow| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_size(px(12.))
                                                .text_color(p.accent)
                                                .child(arrow),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(column.kind_detail.clone()),
                            )
                    },
                ));
        let numbers: Vec<bool> = shape.columns.iter().map(|c| c.kind == "number").collect();
        let language = self.language.clone();
        let list = uniform_list(
            "table-rows",
            shape.rows as usize,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let p = *Palette::get(cx);
                let cell_line = p.line.opacity(0.55);
                let on_bg = p.accent_soft.opacity(0.4);
                range
                    .map(|ix| {
                        let row = this.row(ix, cx);
                        let line = h_flex().h(px(ROW_HEIGHT)).hover(|d| d.bg(p.hover));
                        let Some((label, cells)) = row else {
                            return line
                                .child(
                                    div()
                                        .w(px(label_width))
                                        .px_2()
                                        .text_color(p.faint)
                                        .child("…"),
                                )
                                .into_any_element();
                        };
                        line.child(
                            div()
                                .w(px(label_width))
                                .h_full()
                                .flex()
                                .items_center()
                                .justify_end()
                                .flex_none()
                                .px(px(8.))
                                .bg(p.panel)
                                .border_r_1()
                                .border_b_1()
                                .border_color(cell_line)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(p.faint)
                                .child(label),
                        )
                        .children(cells.iter().take(shown_columns).enumerate().map(
                            |(col, value)| {
                                let (text, missing) = format_cell(value, &language);
                                let dot = (!missing)
                                    .then(|| this.color_of(col as u32, &text))
                                    .flatten();
                                div()
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
                                    .when(col == this.picked, |d| d.bg(on_bg))
                                    .when(numbers.get(col).copied().unwrap_or(false), |d| {
                                        d.justify_end()
                                    })
                                    .when(missing, |d| d.text_color(p.faint))
                                    .when_some(dot, |d, c| d.child(swatch(c)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(text),
                                    )
                            },
                        ))
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
            // One gesture moves the table one way: down, or across.
            .restrict_scroll_to_axis()
            .child(
                v_flex()
                    .w(px(width))
                    .min_w_full()
                    .h_full()
                    .child(header)
                    .child(list),
            )
            .into_any_element()
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
                                                        .text_color(gpui_kit::white()),
                                                )
                                            }),
                                    )
                                    .when_some(self.color_of(edit.column, &v), |d, c| {
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
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.edit = None;
                cx.notify();
            }));
        let top = look::STRIP + 40. + if self.export.is_some() { 40. } else { 0. };
        let left = if self.side_shown { SIDE_WIDTH + 8. } else { 8. };
        Some(
            div()
                .absolute()
                .top(px(top))
                .left(px(left))
                .child(deferred(popup).with_priority(3))
                .into_any_element(),
        )
    }
}

/// A table's name among those a project keeps: its language and path.
fn view_key(language: &str, path: &[String]) -> String {
    format!("{}:{}", language.to_lowercase(), path.join("\u{1f}"))
}

/// A category value's colour, as a small square.
fn swatch(color: u32) -> Div {
    div()
        .size(px(8.))
        .flex_none()
        .rounded(px(2.))
        .bg(rgb(color))
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
        let saved = SavedView::of(&sort, &filters, &columns(&["cell", "n_genes", "pct_mt"]));
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["sort"][0]["name"], "n_genes");
        assert_eq!(json["filters"][0]["op"], ">");
        let back: SavedView = serde_json::from_value(json).unwrap();
        assert_eq!(back, saved);
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
}
