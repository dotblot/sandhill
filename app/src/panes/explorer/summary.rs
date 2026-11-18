//! The explorer's column summaries: the column panel, sparklines, folded statistics, and the filter row.

use super::*;

const BATCH: u32 = 32;

impl Explorer {
    /// Forget statistics on a new table/filter generation. Visible list
    /// items request small batches on demand, with one request in flight.
    pub(super) fn read_summary(&mut self, _cx: &mut Context<Self>) {
        self.summary_generation += 1;
        self.summary_pending = None;
        self.summary_blocks.clear();
        self.summary.clear();
        self.overall.clear();
        self.colors
            .retain(|c, _| self.summary_visible.contains(&(c / BATCH)));
    }

    pub(super) fn ensure_summary(&mut self, column: u32, cx: &mut Context<Self>) {
        const CACHE: usize = 8;
        let block = column / BATCH;
        self.summary_visible.insert(block);
        if self.busy
            || self.refresh_shape
            || self.summary_pending.is_some()
            || self.summary_blocks.contains(&block)
        {
            return;
        }
        self.summary_pending = Some(block);
        let generation = self.summary_generation;
        let filtered = !self.filters.is_empty() || !self.search.is_empty();
        let request = Call::TableSummary {
            session: self.session.clone(),
            path: self.path.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            column_offset: block * BATCH,
            column_limit: Some(BATCH),
        };
        let overall = filtered.then(|| Call::TableSummary {
            session: self.session.clone(),
            path: self.path.clone(),
            filters: Vec::new(),
            search: String::new(),
            column_offset: block * BATCH,
            column_limit: Some(BATCH),
        });
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            let read = daemon.call::<TableSummary>(request).await;
            let overall = match overall {
                Some(request) => daemon.call::<TableSummary>(request).await.ok(),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.summary_generation != generation {
                    return;
                }
                this.summary_pending = None;
                this.summary_blocks.push_back(block);
                while this.summary_blocks.len() > CACHE + this.summary_visible.len() {
                    let old = this
                        .summary_blocks
                        .iter()
                        .position(|b| !this.summary_visible.contains(b))
                        .and_then(|i| this.summary_blocks.remove(i));
                    if let Some(old) = old {
                        this.summary.retain(|&c, _| c / BATCH != old);
                        this.overall.retain(|&c, _| c / BATCH != old);
                        this.colors.retain(|&c, _| c / BATCH != old);
                    } else {
                        break;
                    }
                }
                if let Ok(read) = read {
                    this.color_values(&read.columns);
                    if !filtered {
                        this.overall
                            .extend(read.columns.iter().cloned().map(|c| (c.column, c)));
                    }
                    this.summary
                        .extend(read.columns.into_iter().map(|c| (c.column, c)));
                }
                if let Some(overall) = overall {
                    this.overall
                        .extend(overall.columns.into_iter().map(|c| (c.column, c)));
                }
                // Expanded statistics may change height after their reply.
                let count = this.side_list.item_count();
                this.side_list.splice(0..count, count);
                cx.notify();
            });
        })
        .detach();
    }

    /// Colours for the values of category columns with at most 10 of them,
    /// keeping those given before.
    pub(super) fn color_values(&mut self, summaries: &[ColumnSummary]) {
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

    pub(super) fn color_of(&self, column: u32, value: &str, p: &Palette) -> Option<Hsla> {
        self.colors
            .get(&column)?
            .iter()
            .find(|(v, _)| v == value)
            .map(|(_, c)| p.ansi[CATEGORY_ANSI[*c]])
    }

    /// The column panel: a heading with how many columns show and a ⋯ menu,
    /// a Find column field, then one line per column (fold arrow, type
    /// badge, name, sparkline, missing share, shown checkbox) with its
    /// statistics folded under it. Clicking a column scrolls the table to it.
    pub(super) fn render_side(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(shape) = &self.shape else {
            return div().into_any_element();
        };
        let shown_rows = shape.rows;
        let find = self.column_find.read(cx).value().trim().to_lowercase();
        let row_hover = p.ink(look::ROW_HOVER);
        let button_hover = p.ink(look::BUTTON_HOVER);
        let columns: Vec<usize> = shape
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| find.is_empty() || c.name.to_lowercase().contains(&find))
            .map(|(i, _)| i)
            .collect();
        let none_found = columns.is_empty() && !find.is_empty();
        if self.side_list.item_count() != columns.len() {
            self.side_list.reset(columns.len());
        }
        let rows = list(
            self.side_list.clone(),
            cx.processor(move |this: &mut Explorer, item: usize, _, cx| {
                let i = columns[item];
                let column = i as u32;
                this.ensure_summary(column, cx);
                let c: &TableColumn = &this.shape.as_ref().unwrap().columns[i];
                let summary = this.summary.get(&column);
                let on = this.picked == i;
                let shown = !this.hidden.contains(&column);
                let open = this.expanded.contains(&column);
                let missing = summary.map(|s| s.missing);
                let share = missing.map(|m| missing_share(m, shown_rows));
                let missing_tip = missing.map(|m| {
                    format!(
                        "{} of {} values missing ({})",
                        format_number(m as f64),
                        format_number(shown_rows as f64),
                        missing_text(&this.language)
                    )
                });
                let name_tip = format!("{} · {}", c.name, column_note(c, summary));
                let kind_tip = c.kind_detail.clone();
                let (badge, tint) = type_badge(c);
                let row = h_flex()
                    .id(("de-col", i))
                    .flex_none()
                    .h(px(look::ROW))
                    .pl(px(look::GAP_2))
                    .pr(px(look::GAP_6))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.sel))
                    .when(!on, |d| d.hover(move |d| d.bg(row_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.reveal_column(column, cx)))
                    .child(
                        div()
                            .id(("de-fold", i))
                            .size(px(look::CONTROL_ROW))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(look::CONTROL_RADIUS))
                            .text_color(p.muted)
                            .hover(move |d| d.bg(button_hover))
                            .child(
                                icon(if open {
                                    "chevron.down"
                                } else {
                                    "chevron.right"
                                })
                                .size(px(look::ICON_SMALL)),
                            )
                            .tooltip(tip(if open {
                                "Hide statistics"
                            } else {
                                "Show statistics"
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if !this.expanded.remove(&column) {
                                    this.expanded.insert(column);
                                }
                                let count = this.side_list.item_count();
                                this.side_list.splice(0..count, count);
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(look::GAP_6))
                            .when(!shown, |d| d.opacity(0.45))
                            .child(
                                div()
                                    .id(("de-kind", i))
                                    .flex_none()
                                    .w(px(BADGE_WIDTH))
                                    .h(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(look::CONTROL_RADIUS))
                                    .bg(tint.map_or(p.ink(8.), |t| p.ansi[t].opacity(0.2)))
                                    .text_size(px(look::TINY))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(badge)
                                    .tooltip(move |w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(kind_tip.clone())
                                            .build(w, cx)
                                    }),
                            )
                            .child(
                                div()
                                    .id(("de-name", i))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(c.name.clone())
                                    .tooltip(move |w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(name_tip.clone())
                                            .build(w, cx)
                                    }),
                            )
                            .child(this.spark(column, summary, &p))
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
                                    .when_some(missing_tip, |d, tip| {
                                        d.tooltip(move |w, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                                .build(w, cx)
                                        })
                                    }),
                            ),
                    )
                    .child(
                        Checkbox::new(("de-visible", i))
                            .checked(shown)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                cx.stop_propagation();
                                this.show_column(column, *on, cx);
                            })),
                    );
                column_list_item()
                    .child(row)
                    .when(open, |d| d.child(this.render_stats(column, c, cx)))
                    .into_any_element()
            }),
        )
        .flex_1();
        let total = shape.columns.len();
        let shown = total
            - self
                .hidden
                .iter()
                .filter(|&&c| (c as usize) < total)
                .count();
        v_flex()
            .flex_none()
            .w(px(SIDE_WIDTH))
            .h_full()
            .p(px(look::GAP_6))
            .gap(px(look::GAP_4))
            .border_r_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .flex_none()
                    .h(px(look::ROW))
                    .pl(px(look::GAP_6))
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .text_size(px(look::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.faint)
                            .child("COLUMNS"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.muted)
                            .child(format!("{shown} of {total}")),
                    )
                    .child(
                        look::icon_button("de-columns-menu", "more", &p, false)
                            .on_prepaint(self.popup_triggers.track("columns"))
                            .when(self.columns_menu.is_some(), |d| d.bg(p.ink(10.)))
                            .tooltip(tip("Show or hide all columns"))
                            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                                let open = this.columns_menu.is_none();
                                this.close_menus();
                                if open {
                                    this.columns_menu = Some(event.position());
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().flex_none().child(
                    Input::new(&self.column_find)
                        .small()
                        .prefix(icon("search").size(px(look::ICON)).text_color(p.muted)),
                ),
            )
            .child(
                div()
                    .id("de-cols")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(rows)
                    .when(none_found, |d| {
                        d.child(
                            div()
                                .px(px(6.))
                                .py(px(4.))
                                .text_color(p.muted)
                                .child("No column matches."),
                        )
                    }),
            )
            .into_any_element()
    }

    /// A column's sparkline: a histogram of numbers, or bars for its
    /// commonest values.
    pub(super) fn spark(
        &self,
        column: u32,
        summary: Option<&ColumnSummary>,
        p: &Palette,
    ) -> AnyElement {
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

    /// A column's statistics, folded open under its row and lined up with
    /// its badge: numbers as a wide histogram between their min and max,
    /// then mean and median; categories as each common value with a bar of
    /// its share (clicking one filters to it); last, distinct and missing.
    pub(super) fn render_stats(
        &self,
        ix: u32,
        column: &TableColumn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let mut body: Vec<AnyElement> = Vec::new();
        match self.summary.get(&ix) {
            None => body.push(
                div()
                    .text_color(p.faint)
                    .child("Reading…")
                    .into_any_element(),
            ),
            Some(s) => {
                if column.kind == "number"
                    && let (Some(min), Some(max)) = (s.min, s.max)
                {
                    let top = s.histogram.iter().copied().max().unwrap_or(1).max(1);
                    let bar = p.accent.opacity(0.7);
                    body.push(
                        h_flex()
                            .h(px(40.))
                            .items_end()
                            .children(s.histogram.iter().map(|&count| {
                                div()
                                    .flex_1()
                                    .h(relative(count as f32 / top as f32))
                                    .when(count > 0, |d| d.min_h(px(1.)))
                                    .mr(px(1.))
                                    .bg(bar)
                            }))
                            .into_any_element(),
                    );
                    body.push(
                        h_flex()
                            .justify_between()
                            .text_color(p.muted)
                            .child(format_number(min))
                            .child(format_number(max))
                            .into_any_element(),
                    );
                    let stat = |label: &'static str, value: Option<f64>| {
                        h_flex()
                            .flex_1()
                            .gap(px(6.))
                            .child(div().text_color(p.muted).child(label))
                            .child(value.map(format_number).unwrap_or_default())
                    };
                    body.push(
                        h_flex()
                            .child(stat("Mean", s.mean))
                            .child(stat("Median", s.median))
                            .into_any_element(),
                    );
                }
                let most = s.top.iter().map(|t| t.count).max().unwrap_or(1).max(1);
                for (n, t) in s.top.iter().enumerate() {
                    let value = t.value.clone();
                    let color = self.color_of(ix, &t.value, &p);
                    let accent = p.accent;
                    let tip = format!("Show only rows where {} is {}", column.name, t.value);
                    body.push(
                        h_flex()
                            .id(("de-val", n))
                            .h(px(look::DENSE_ROW))
                            .gap(px(look::GAP_6))
                            .rounded(px(look::CODE_RADIUS))
                            .cursor_pointer()
                            .hover(move |d| d.bg(p.ink(look::ROW_HOVER)).text_color(accent))
                            .child(
                                h_flex()
                                    .w(px(VALUE_WIDTH))
                                    .flex_none()
                                    .gap(px(look::GAP_4))
                                    .when_some(color, |d, c| d.child(swatch(c)))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(t.value.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(6.))
                                    .rounded(px(3.))
                                    .bg(p.ink(6.))
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(t.count as f32 / most as f32))
                                            .rounded(px(3.))
                                            .bg(color.unwrap_or(p.accent).opacity(0.6)),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(MISSING_WIDTH + 4.))
                                    .flex_none()
                                    .text_right()
                                    .text_color(p.muted)
                                    .child(format_number(t.count as f64)),
                            )
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter_to(ix, value.clone(), cx)
                            }))
                            .into_any_element(),
                    );
                }
                if let Some(distinct) = s.distinct
                    && !s.top.is_empty()
                    && distinct > s.top.len() as u64
                {
                    body.push(
                        div()
                            .text_color(p.faint)
                            .child(format!(
                                "… {} more values",
                                format_number((distinct - s.top.len() as u64) as f64)
                            ))
                            .into_any_element(),
                    );
                }
                let mut last = Vec::new();
                if column.kind != "number"
                    && let Some(distinct) = s.distinct
                {
                    last.push(format!("{} distinct", format_number(distinct as f64)));
                }
                last.push(format!(
                    "{} missing ({})",
                    format_number(s.missing as f64),
                    missing_text(&self.language)
                ));
                body.push(
                    div()
                        .text_color(p.muted)
                        .child(last.join(" · "))
                        .into_any_element(),
                );
            }
        }
        div()
            .id(("de-stats", ix as usize))
            .flex_none()
            .ml(px(look::GAP_2 + look::CONTROL_ROW + look::GAP_4))
            .mr(px(look::GAP_8))
            .mt(px(look::GAP_2))
            .mb(px(look::GAP_8))
            .text_size(px(look::SMALL))
            .child(v_flex().gap(px(look::GAP_4)).children(body))
            .into_any_element()
    }

    /// Filters and sort keys as chips over the table, only while there are
    /// some: a filter's chip changes it, a sort key's flips its direction,
    /// and × removes either. Chips wrap rather than being cut short.
    pub(super) fn render_filter_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.filters.is_empty() && self.sort.is_empty() {
            return None;
        }
        let p = *Palette::get(cx);
        let columns = self.shape.as_ref().map_or(&[][..], |s| &s.columns);
        let soft = p.ink(look::BUTTON_HOVER);
        let text = p.text;
        let remove = move |id: ElementId| {
            div()
                .id(id)
                .size(px(look::CONTROL_ROW))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_color(p.muted)
                .cursor_pointer()
                .hover(move |d| d.bg(soft).text_color(text))
                .child(icon("close").size(px(look::ICON_SMALL)))
        };
        let chip = || {
            h_flex()
                .flex_none()
                .max_w(px(360.))
                .h(px(look::CONTROL_CARD))
                .pl(px(look::GAP_8))
                .pr(px(look::GAP_4))
                .gap(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .whitespace_nowrap()
        };
        let filters = self.filters.iter().enumerate().map(|(i, f)| {
            chip()
                .bg(p.accent_soft)
                .child(
                    div()
                        .id(("de-chip", i))
                        .min_w_0()
                        .cursor_pointer()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(filter_text(f, columns))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_filter(Some(i), window, cx)
                        })),
                )
                .child(remove(("de-chip-x", i).into()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let mut filters = this.filters.clone();
                        if i < filters.len() {
                            filters.remove(i);
                        }
                        this.change_filters(filters, cx);
                    },
                )))
                .into_any_element()
        });
        let sorts = self.sort.iter().enumerate().map(|(i, key)| {
            let name = columns
                .get(key.column as usize)
                .map_or_else(|| format!("column {}", key.column + 1), |c| c.name.clone());
            let descending = key.descending;
            chip()
                .bg(p.ink(6.))
                .child(
                    h_flex()
                        .id(("de-sort", i))
                        .min_w_0()
                        .gap(px(look::GAP_4))
                        .cursor_pointer()
                        .child(
                            icon(if descending { "arrow.down" } else { "arrow.up" })
                                .size(px(look::ICON_SMALL))
                                .text_color(p.accent),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(name),
                        )
                        .tooltip(tip(if descending {
                            "Sort ascending instead"
                        } else {
                            "Sort descending instead"
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let mut sort = this.sort.clone();
                            if let Some(key) = sort.get_mut(i) {
                                key.descending = !key.descending;
                            }
                            this.change_sort(sort, cx);
                        })),
                )
                .child(remove(("de-sort-x", i).into()).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let mut sort = this.sort.clone();
                        if i < sort.len() {
                            sort.remove(i);
                        }
                        this.change_sort(sort, cx);
                    },
                )))
                .into_any_element()
        });
        Some(
            h_flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(px(look::GAP_8))
                .px(px(look::PANE_PAD_X))
                .py(px(look::GAP_4))
                .border_b_1()
                .border_color(p.line)
                .children(filters)
                .children(sorts)
                .child(div().flex_1())
                .child(
                    bar_button("de-clear-all", "Clear all", None, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.sort.clear();
                            this.change_filters(Vec::new(), cx);
                        },
                    )),
                )
                .into_any_element(),
        )
    }
}

/// List items are independent layout roots: unlike the old flex-column
/// children, auto-width items shrink to each name's intrinsic width.
fn column_list_item() -> Div {
    v_flex().flex_none().w_full()
}

/// A category value's colour, as a small square.
pub(super) fn swatch(color: Hsla) -> Div {
    div().size(px(8.)).flex_none().rounded(px(2.)).bg(color)
}

/// A column's kind as a short badge, with its terminal colour (distinct
/// hues, unlike the chart's shades): "num", "int", "str", "cat", "bool",
/// "date", or "other" in grey.
pub(super) fn type_badge(column: &TableColumn) -> (&'static str, Option<usize>) {
    match column.kind.as_str() {
        "number" if column.kind_detail.to_lowercase().contains("int") => ("int", Some(4)),
        "number" => ("num", Some(4)),
        "text" => ("str", Some(2)),
        "category" => ("cat", Some(5)),
        "logical" => ("bool", Some(3)),
        "date" => ("date", Some(6)),
        _ => ("other", None),
    }
}

/// A column's missing share of `rows`, as the summary panel shows it:
/// "0%", "<0.1%", "3.2%", "100%".
pub(super) fn missing_share(missing: u64, rows: u64) -> String {
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
mod tests {
    use super::{SPARK_HEIGHT, SPARK_WIDTH, column_list_item, missing_share};
    use crate::look::DENSE_ROW;
    use gpui_kit::component::h_flex;
    use gpui_kit::{
        AppContext, Bounds, Context, IntoElement, ListAlignment, ListState, ParentElement, Pixels,
        Render, Styled, TestAppContext, Window, canvas, div, list, point, px, size,
    };
    use std::rc::Rc;

    #[test]
    fn missing_shares_round_without_hiding_rare_or_total_gaps() {
        assert_eq!(missing_share(0, 375), "0%");
        assert_eq!(missing_share(12, 375), "3.2%");
        assert_eq!(missing_share(1, 100_000), "<0.1%");
        assert_eq!(missing_share(10, 100), "10%");
        assert_eq!(missing_share(99_999, 100_000), "100%");
        assert_eq!(missing_share(5, 0), "0%");
    }

    #[gpui_kit::test]
    fn virtual_column_rows_align_statistics_for_different_name_lengths(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let positions = Rc::new(std::cell::RefCell::new([None, None]));
        let painted = positions.clone();
        struct ColumnRows(Rc<std::cell::RefCell<[Option<Bounds<Pixels>>; 2]>>);
        impl Render for ColumnRows {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let painted = self.0.clone();
                list(
                    ListState::new(2, ListAlignment::Top, px(0.)),
                    move |i, _, _| {
                        let painted = painted.clone();
                        column_list_item()
                            .child(
                                h_flex()
                                    .h(px(DENSE_ROW))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(["FAM138A", "MIR1302-10"][i]),
                                    )
                                    .child(
                                        canvas(
                                            move |bounds, _, _| {
                                                painted.borrow_mut()[i] = Some(bounds);
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .w(px(SPARK_WIDTH))
                                        .h(px(SPARK_HEIGHT))
                                        .flex_none(),
                                    ),
                            )
                            .into_any_element()
                    },
                )
                .size_full()
            }
        }
        cx.draw(
            point(px(0.), px(0.)),
            size(px(320.), px(80.)),
            move |_, cx| cx.new(|_| ColumnRows(painted)).into_any_element(),
        );
        let positions = positions.borrow();
        let short = positions[0].expect("short-name row painted");
        let long = positions[1].expect("long-name row painted");
        assert_eq!(
            short.left(),
            long.left(),
            "statistics must share one column"
        );
        assert_eq!(short.right(), px(320.), "each row must fill the list width");
    }
}
