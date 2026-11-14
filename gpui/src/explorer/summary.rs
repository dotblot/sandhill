//! The explorer's column summaries: the side panel, sparklines, stats, and the filter bar.

use super::*;

impl Explorer {
    /// Reads the columns' summaries, with the filters on.
    pub(super) fn read_summary(&mut self, cx: &mut Context<Self>) {
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

    /// The column list: each column with its sparkline and missing share,
    /// and its statistics folded under it like a folder's files. Clicking a
    /// column scrolls the table to it.
    pub(super) fn render_side(&self, cx: &mut Context<Self>) -> AnyElement {
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

    /// A column's numbers, folded open under its row: its range and middle,
    /// or its commonest values, each of which filters to it.
    pub(super) fn render_stats(
        &self,
        ix: u32,
        column: &TableColumn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
    pub(super) fn render_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
}

/// A category value's colour, as a small square.
pub(super) fn swatch(color: Hsla) -> Div {
    div().size(px(8.)).flex_none().rounded(px(2.)).bg(color)
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
    use super::missing_share;

    #[test]
    fn missing_shares_round_without_hiding_rare_or_total_gaps() {
        assert_eq!(missing_share(0, 375), "0%");
        assert_eq!(missing_share(12, 375), "3.2%");
        assert_eq!(missing_share(1, 100_000), "<0.1%");
        assert_eq!(missing_share(10, 100), "10%");
        assert_eq!(missing_share(99_999, 100_000), "100%");
        assert_eq!(missing_share(5, 0), "0%");
    }
}
