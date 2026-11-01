//! The Data Explorer: a data frame or matrix from a kernel as a table that
//! only draws the rows in view and fetches them from the kernel in blocks
//! as it scrolls (`TableRows`). Clicking a column's name sorts by it, in the
//! kernel.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, TableColumn, TableRows, TableSort};
use serde_json::Value;

use crate::daemon::Daemon;

/// Rows fetched at a time.
const BLOCK: u64 = 200;
/// Columns fetched; wider tables show their first ones.
const COLUMNS: u32 = 200;
const ROW_HEIGHT: f32 = 24.;
const COLUMN_WIDTH: f32 = 130.;
const LABEL_WIDTH: f32 = 90.;

/// What the table's first answer said about it.
struct Shape {
    code: String,
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

pub struct Explorer {
    daemon: Rc<Daemon>,
    session: String,
    path: Vec<String>,
    shape: Option<Shape>,
    error: Option<String>,
    sort: Option<TableSort>,
    blocks: HashMap<u64, Block>,
    /// Counts reloads, so a block for an old sort is dropped.
    generation: u64,
    scroll: UniformListScrollHandle,
    across: ScrollHandle,
}

impl Explorer {
    pub fn new(
        daemon: Rc<Daemon>,
        session: String,
        path: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            daemon,
            session,
            path,
            shape: None,
            error: None,
            sort: None,
            blocks: HashMap::new(),
            generation: 0,
            scroll: UniformListScrollHandle::new(),
            across: ScrollHandle::new(),
        };
        this.load(0, cx);
        this
    }

    pub fn name(&self) -> String {
        self.path.join(" › ")
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Fetches everything again, as the kernel has it now.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.blocks.clear();
        self.error = None;
        self.load(0, cx);
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
            sort: self.sort.clone().into_iter().collect(),
            filters: Vec::new(),
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
                        if first {
                            this.shape = Some(Shape {
                                code: rows.code.clone(),
                                columns: rows.columns.clone(),
                                width: rows.width,
                                total: rows.total,
                                rows: rows.rows,
                            });
                        }
                        let cells = rows.labels.into_iter().zip(rows.values).collect();
                        this.blocks.insert(block, Block::Ready(cells));
                    }
                    Err(err) => {
                        // A busy kernel can't answer; scrolling there again retries.
                        this.blocks.insert(block, Block::Failed);
                        this.error = Some(format!("{err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// None, then ascending, then descending.
    fn sort_by(&mut self, column: u32, cx: &mut Context<Self>) {
        self.sort = match &self.sort {
            Some(s) if s.column == column && !s.descending => Some(TableSort {
                column,
                descending: true,
            }),
            Some(s) if s.column == column => None,
            _ => Some(TableSort {
                column,
                descending: false,
            }),
        };
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<(String, Vec<Value>)> {
        let block = ix as u64 / BLOCK;
        match self.blocks.get(&block) {
            Some(Block::Ready(rows)) => rows.get(ix % BLOCK as usize).cloned(),
            Some(Block::Loading) => None,
            Some(Block::Failed) => {
                self.blocks.remove(&block);
                None
            }
            None => {
                self.load(block, cx);
                None
            }
        }
    }
}

impl Render for Explorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(shape) = &self.shape else {
            return div()
                .p_4()
                .text_color(theme.muted_foreground)
                .child(match &self.error {
                    Some(err) => format!("The table could not be read: {err}"),
                    None => "Reading the table…".into(),
                })
                .into_any_element();
        };
        let shown_columns = shape.columns.len().min(COLUMNS as usize);
        let numbers: Vec<bool> = shape.columns.iter().map(|c| c.kind == "number").collect();
        let width = LABEL_WIDTH + COLUMN_WIDTH * shown_columns as f32;
        let summary = format!(
            "{} · {} rows × {} columns{}{}",
            shape.code,
            thousands(shape.total),
            thousands(shape.width),
            if shape.rows != shape.total {
                format!(" · {} shown", thousands(shape.rows))
            } else {
                String::new()
            },
            if shape.width > COLUMNS as u64 {
                format!(" · showing the first {COLUMNS} columns")
            } else {
                String::new()
            },
        );
        let header =
            h_flex()
                .h(px(ROW_HEIGHT + 8.))
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .text_xs()
                .child(div().w(px(LABEL_WIDTH)).flex_none())
                .children(shape.columns.iter().take(shown_columns).enumerate().map(
                    |(ix, column)| {
                        let arrow = match &self.sort {
                            Some(s) if s.column == ix as u32 && s.descending => " ▼",
                            Some(s) if s.column == ix as u32 => " ▲",
                            _ => "",
                        };
                        v_flex()
                            .id(("column", ix))
                            .w(px(COLUMN_WIDTH))
                            .flex_none()
                            .px_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.list_hover))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.sort_by(ix as u32, cx)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_ellipsis()
                                    .child(format!("{}{arrow}", column.name)),
                            )
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(column.kind_detail.clone()),
                            )
                    },
                ));
        let mono = theme.mono_font_family.clone();
        let list = uniform_list(
            "table-rows",
            shape.rows as usize,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let theme = cx.theme().clone();
                range
                    .map(|ix| {
                        let row = this.row(ix, cx);
                        let line = h_flex()
                            .h(px(ROW_HEIGHT))
                            .border_b_1()
                            .border_color(theme.border.opacity(0.5))
                            .font_family(mono.clone())
                            .text_size(px(12.));
                        let Some((label, cells)) = row else {
                            return line
                                .child(
                                    div()
                                        .w(px(LABEL_WIDTH))
                                        .px_2()
                                        .text_color(theme.muted_foreground)
                                        .child("…"),
                                )
                                .into_any_element();
                        };
                        line.child(
                            div()
                                .w(px(LABEL_WIDTH))
                                .flex_none()
                                .px_2()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(theme.muted_foreground)
                                .child(label),
                        )
                        .children(cells.iter().take(shown_columns).enumerate().map(
                            |(col, value)| {
                                let (text, missing) = cell_text(value);
                                div()
                                    .w(px(COLUMN_WIDTH))
                                    .flex_none()
                                    .px_2()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .when(numbers.get(col).copied().unwrap_or(false), |d| {
                                        d.flex().justify_end()
                                    })
                                    .when(missing, |d| d.text_color(theme.muted_foreground))
                                    .child(text)
                            },
                        ))
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1();
        v_flex()
            .size_full()
            .child(
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(summary)
                    .when_some(self.error.clone(), |d, err| {
                        d.child(div().text_color(theme.danger).child(err))
                    }),
            )
            .child(
                div()
                    .id("table")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.across)
                    .child(v_flex().w(px(width)).h_full().child(header).child(list)),
            )
            .into_any_element()
    }
}

/// A cell as text, and whether it's missing.
fn cell_text(value: &Value) -> (String, bool) {
    match value {
        Value::Null => ("NA".into(), true),
        Value::String(s) => (s.clone(), false),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => (i.to_string(), false),
            (None, Some(f)) => (number(f), false),
            _ => (n.to_string(), false),
        },
        Value::Bool(b) => (if *b { "TRUE" } else { "FALSE" }.into(), false),
        other => (other.to_string(), false),
    }
}

/// Up to six decimals, without trailing zeros; whole numbers plainly.
fn number(f: f64) -> String {
    if f.fract() == 0. && f.abs() < 1e15 {
        return format!("{f:.0}");
    }
    if f != 0. && (f.abs() < 1e-4 || f.abs() >= 1e15) {
        return format!("{f:.3e}");
    }
    let s = format!("{f:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{cell_text, number, thousands};
    use serde_json::json;

    #[test]
    fn formats_cells() {
        assert_eq!(number(2.0), "2");
        assert_eq!(number(0.125), "0.125");
        assert_eq!(number(1.0 / 3.0), "0.333333");
        assert_eq!(number(0.00001234), "1.234e-5");
        assert_eq!(cell_text(&json!(null)), ("NA".into(), true));
        assert_eq!(cell_text(&json!(42)), ("42".into(), false));
        assert_eq!(cell_text(&json!("ACTB")), ("ACTB".into(), false));
        assert_eq!(thousands(1_000_000), "1,000,000");
        assert_eq!(thousands(999), "999");
    }
}
