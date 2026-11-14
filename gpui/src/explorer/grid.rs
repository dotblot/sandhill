//! The explorer's table grid, its column resizer, and its column and settings menus.

use super::summary::swatch;
use super::*;

impl Explorer {
    /// A menu row: an icon, a label and a shortcut; dimmed when it can't
    /// be used.
    pub(super) fn menu_item(
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
    pub(super) fn render_column_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
    pub(super) fn render_settings_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    pub(super) fn render_grid(&self, cx: &mut Context<Self>) -> AnyElement {
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
    pub(super) fn column_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                this.save_view();
                            } else if let Some(w) = this.widths.get_mut(column) {
                                *w = (width + f32::from(event.position.x - start))
                                    .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
                                    .round();
                                if !this.resized.contains(&(column as u32)) {
                                    this.resized.push(column as u32);
                                }
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
                                    this.save_view();
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
}
