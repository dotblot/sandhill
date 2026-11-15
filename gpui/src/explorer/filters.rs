//! The explorer's filter editing: opening, applying, and the filter popup.

use super::summary::swatch;
use super::*;

impl Explorer {
    // Filters.

    pub(super) fn open_filter(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn kind_of(&self, column: u32) -> &str {
        self.shape
            .as_ref()
            .and_then(|s| s.columns.get(column as usize))
            .map_or("other", |c| c.kind.as_str())
    }

    pub(super) fn takes(&self, column: u32, op: &str) -> Takes {
        let ops = ops_for(self.kind_of(column));
        ops.iter().find(|o| o.op == op).unwrap_or(&ops[0]).takes
    }

    /// A category's values to tick, when all of them are known.
    pub(super) fn choices(&self, column: u32) -> Option<Vec<String>> {
        let s = self
            .overall
            .get(&column)
            .or_else(|| self.summary.get(&column))?;
        (!s.top.is_empty() && s.distinct.is_some_and(|d| d as usize <= s.top.len()))
            .then(|| s.top.iter().map(|t| t.value.clone()).collect())
    }

    /// The filter as the popup has it now.
    pub(super) fn edited_filter(&self, cx: &App) -> Option<TableFilter> {
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

    pub(super) fn apply_filter(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn pick_filter_column(&mut self, column: u32, cx: &mut Context<Self>) {
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
    pub(super) fn filter_to(&mut self, column: u32, value: String, cx: &mut Context<Self>) {
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

    /// Adding or changing a filter: a column, a condition, and its values.
    pub(super) fn render_filter_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        // Over the table's top: a chip's filter at the left, under its chip;
        // a new one at the right, under the action row's Filter button.
        let new = edit.index.is_none();
        Some(
            div()
                .absolute()
                .top(px(4.))
                .when(new, |d| d.right(px(8.)))
                .when(!new, |d| d.left(px(8.)))
                // Anchored so a narrow pane neither squeezes nor clips it.
                .child(
                    deferred(
                        anchored()
                            .when(new, |a| a.anchor(Anchor::TopRight))
                            .snap_to_window_with_margin(px(8.))
                            .child(popup),
                    )
                    .with_priority(3),
                )
                .into_any_element(),
        )
    }
}
