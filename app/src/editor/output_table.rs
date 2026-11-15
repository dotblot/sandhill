//! Notebook tables own their selection and rendering invalidation boundary.

use gpui_kit::component::input;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::editor::html_table::Table;
use crate::editor::table_selection::{Selection, Target, encode_tsv};
use crate::look::{self, Palette};

const ROW_HEIGHT: f32 = 24.;
const COLUMN_CHARS: (usize, usize) = (3, 28);

pub(crate) struct OutputTable {
    id: ElementId,
    table: Option<Table>,
}

impl OutputTable {
    pub(crate) fn new(id: impl Into<ElementId>, table: Table) -> Self {
        Self {
            id: id.into(),
            table: Some(table),
        }
    }
}

struct SelectionOwner {
    columns: Vec<SharedString>,
    rows: Vec<Vec<SharedString>>,
    widths: Vec<f32>,
    selection: Selection,
    drag_origin: Option<Target>,
    focus: FocusHandle,
}

impl SelectionOwner {
    fn new(table: Table, cx: &mut Context<Self>) -> Self {
        let mut owner = Self {
            columns: Vec::new(),
            rows: Vec::new(),
            widths: Vec::new(),
            selection: Selection::default(),
            drag_origin: None,
            focus: cx.focus_handle(),
        };
        owner.replace_table(table);
        owner
    }

    fn matches(&self, table: &Table) -> bool {
        self.columns
            .iter()
            .map(|s| s.as_ref())
            .eq(table.columns.iter().map(String::as_str))
            && self.rows.len() == table.rows.len()
            && self.rows.iter().zip(&table.rows).all(|(a, b)| {
                a.iter()
                    .map(|s| s.as_ref())
                    .eq(b.iter().map(String::as_str))
            })
    }

    fn replace_table(&mut self, table: Table) {
        self.widths = (0..table.columns.len())
            .map(|column| {
                let chars = table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .chain(std::iter::once(&table.columns[column]))
                    .map(|text| text.chars().count())
                    .max()
                    .unwrap_or(0)
                    .clamp(COLUMN_CHARS.0, COLUMN_CHARS.1);
                chars as f32 * look::CODE * 0.6 + 16.
            })
            .collect();
        self.columns = table.columns.into_iter().map(SharedString::from).collect();
        self.rows = table
            .rows
            .into_iter()
            .map(|row| row.into_iter().map(SharedString::from).collect())
            .collect();
        self.selection.clear();
        self.drag_origin = None;
    }

    fn dimensions(&self) -> (u64, usize) {
        // The kernel's first printed column is its row index, not data.
        (self.rows.len() as u64, self.columns.len().saturating_sub(1))
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        let changed = self.selection.range().is_some();
        self.selection.clear();
        self.drag_origin = None;
        if changed {
            cx.notify();
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let (rows, columns) = self.dimensions();
        self.selection.all(rows, columns);
        self.drag_origin = None;
        cx.notify();
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let Some(range) = self.selection.range() else {
            return;
        };
        let rows = &self.rows[range.first_row as usize..=range.last_row as usize];
        let text = encode_tsv(rows.iter().map(|row| {
            (range.first_column..=range.last_column)
                .map(|column| row.get(column + 1).map_or("", |text| text.as_ref()))
        }));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn begin(
        &mut self,
        target: Option<Target>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.focus.focus(window, cx);
        if let Some(target) = target {
            let (rows, columns) = self.dimensions();
            self.selection
                .begin(target, event.modifiers.shift, rows, columns);
            self.drag_origin = Some(target);
            cx.notify();
        } else {
            self.select_all(cx);
        }
    }

    fn extend(&mut self, target: Option<Target>, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selection.dragging() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.end();
            return;
        }
        // Keep the gesture's kind while crossing from the row-number/header
        // into the body. A whole-row drag must never collapse to a cell drag.
        let target = match (self.drag_origin, target) {
            (Some(Target::Row(_)), Some(Target::Row(row) | Target::Cell { row, .. })) => {
                Some(Target::Row(row))
            }
            (
                Some(Target::Column(_)),
                Some(Target::Column(column) | Target::Cell { column, .. }),
            ) => Some(Target::Column(column)),
            (Some(Target::Cell { .. }), Some(target @ Target::Cell { .. })) => Some(target),
            _ => None,
        };
        if let Some(target) = target {
            let previous = self.selection.range();
            let (rows, columns) = self.dimensions();
            self.selection.extend(target, rows, columns);
            if self.selection.range() != previous {
                cx.notify();
            }
        }
        cx.stop_propagation();
    }

    fn end(&mut self) {
        self.selection.end();
        self.drag_origin = None;
    }

    fn cell(
        &self,
        row: Option<usize>,
        column: usize,
        text: SharedString,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let target = match (row, column) {
            (Some(row), 0) => Some(Target::Row(row as u64)),
            (Some(row), column) => Some(Target::Cell {
                row: row as u64,
                column: column - 1,
            }),
            (None, 0) => None,
            (None, column) => Some(Target::Column(column - 1)),
        };
        let selected = self.selection.range().is_some_and(|range| match target {
            Some(Target::Cell { row, column }) => range.contains(row, column),
            Some(Target::Row(row)) => (range.first_row..=range.last_row).contains(&row),
            Some(Target::Column(column)) => {
                (range.first_column..=range.last_column).contains(&column)
            }
            None => {
                range.row_count() == self.rows.len() as u64
                    && range.column_count() == self.columns.len().saturating_sub(1)
            }
        });
        div()
            .id((
                "cell",
                row.map_or(0, |row| row + 1) * self.columns.len() + column,
            ))
            .flex()
            .items_center()
            .flex_none()
            .w(px(self.widths[column]))
            .h(px(ROW_HEIGHT))
            .px(px(8.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .cursor_text()
            .when(column == 0 && row.is_some(), |d| d.text_color(p.muted))
            .when(selected, |d| {
                d.bg(if row.is_none() || column == 0 {
                    p.accent_soft
                } else {
                    p.sel
                })
            })
            .child(text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.begin(target, event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, event, _, cx| {
                this.extend(target, event, cx);
            }))
    }
}

impl Render for SelectionOwner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let width: f32 = self.widths.iter().sum();
        div()
            .id("grid")
            .track_focus(&self.focus)
            .w_full()
            .max_w_full()
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .rounded(px(6.))
            .border(px(0.5))
            .border_color(p.line)
            .font_family(look::MONO)
            .text_size(px(look::SMALL))
            .text_color(p.text)
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    this.clear(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.clear(cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.end()))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.end()))
            .on_action(cx.listener(|this, _: &input::Copy, _, cx| {
                this.copy(cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &input::SelectAll, _, cx| {
                this.select_all(cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let key = &event.keystroke;
                if key.key == "escape" {
                    this.clear(cx);
                } else if key.modifiers.secondary(cx) && !key.modifiers.alt && !key.modifiers.shift
                {
                    match key.key.as_str() {
                        "a" => this.select_all(cx),
                        "c" => this.copy(cx),
                        _ => return,
                    }
                } else {
                    return;
                }
                cx.stop_propagation();
            }))
            .child(
                v_flex()
                    .w(px(width))
                    .min_w_full()
                    .child(
                        h_flex()
                            .h(px(ROW_HEIGHT))
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(p.chunk)
                            .border_b(px(0.5))
                            .border_color(p.line)
                            .children(self.columns.iter().enumerate().map(|(column, text)| {
                                let text = if column == 0 && text.is_empty() {
                                    SharedString::from("#")
                                } else {
                                    text.clone()
                                };
                                self.cell(None, column, text, &p, cx)
                            })),
                    )
                    .children(self.rows.iter().enumerate().map(|(row, values)| {
                        h_flex()
                            .h(px(ROW_HEIGHT))
                            .flex_none()
                            .when(row % 2 == 1, |d| d.bg(p.ink(3.)))
                            .border_b(px(0.5))
                            .border_color(p.line)
                            .children((0..self.columns.len()).map(|column| {
                                self.cell(
                                    Some(row),
                                    column,
                                    values.get(column).cloned().unwrap_or_default(),
                                    &p,
                                    cx,
                                )
                            }))
                    })),
            )
    }
}

impl IntoElement for OutputTable {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OutputTable {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let table = self
            .table
            .take()
            .expect("OutputTable lays out once per render");
        // Retain the owner without subscribing Workbench to its notifications.
        // Mouse drags repaint only this output, just like OutputText.
        let owner = window.with_element_state(
            global_id.expect("OutputTable must have a stable element id"),
            |retained: Option<Entity<SelectionOwner>>, _| {
                let owner = if let Some(owner) = retained {
                    owner.update(cx, |owner, cx| {
                        if !owner.matches(&table) {
                            owner.replace_table(table);
                            cx.notify();
                        }
                    });
                    owner
                } else {
                    cx.new(|cx| SelectionOwner::new(table, cx))
                };
                (owner.clone(), owner)
            },
        );
        let mut element = owner.into_any_element();
        let layout = element.request_layout(window, cx);
        (layout, element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}
