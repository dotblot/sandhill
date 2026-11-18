//! The side panes: Files, Viewer and Variables.

use super::*;

fn variables_filter_height() -> f32 {
    look::control_card() + 2. * look::PANE_PAD_Y
}

/// The variables whose names hold what was typed in the filter box,
/// ignoring case (all of them when nothing was typed).
pub(crate) fn filter_variables(list: &[Variable], filter: &str) -> Vec<Variable> {
    let wanted = filter.trim().to_lowercase();
    list.iter()
        .filter(|v| wanted.is_empty() || v.name.to_lowercase().contains(&wanted))
        .cloned()
        .collect()
}

impl Workbench {
    /// The Files tab: the project's folders and files as a tree, right
    /// under the tab strip (its buttons are in the strip: `files_tools`).
    pub(crate) fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let count = self.file_lines().len();
        let project = self.project.clone().unwrap_or_default();
        let open_path = self.active_document_path().map(str::to_owned);
        let list = uniform_list(
            "files",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let lines = this.file_lines();
                let selected = this.files_ui.selected.as_ref().map(|(r, _)| r.clone());
                range
                    .filter_map(|ix| lines.get(ix).map(|line| (ix, line)))
                    .map(|(ix, line)| {
                        let row = match line {
                            crate::panes::tree_actions::Line::Field { depth } => {
                                return this.render_name_field(*depth, cx);
                            }
                            crate::panes::tree_actions::Line::Row(row) => row,
                        };
                        let indent = crate::panes::tree_actions::file_row_indent(row.depth);
                        let hover = p.ink(7.);
                        let line = h_flex()
                            .id(("file", ix))
                            .relative()
                            .w_full()
                            .h(px(look::row()))
                            .pl(px(indent))
                            .pr(px(look::GAP_8))
                            .gap(px(look::GAP_4))
                            .rounded(px(look::CONTROL_RADIUS))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            // A line down from each folder above, as a guide.
                            .children((0..row.depth).map(|level| {
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(
                                        crate::panes::tree_actions::file_row_indent(level)
                                            + (look::ICON - 1.) / 2.,
                                    ))
                                    .w(px(1.))
                                    .bg(p.ink(12.))
                            }));
                        if let Some(note) = &row.note {
                            return line
                                .pl(px(indent + look::ICON + look::GAP_4))
                                .text_color(p.faint)
                                .child(note.clone())
                                .into_any_element();
                        }
                        let rel = row.rel.clone();
                        let is_selected = selected.as_deref() == Some(row.rel.as_str());
                        let menu_rel = row.rel.clone();
                        let menu_dir = row.dir;
                        let line = line.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.tree_menu(menu_rel.clone(), menu_dir, event.position, cx)
                            }),
                        );
                        let name = div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(row.name.clone());
                        let by_size = this.tree.sort == crate::panes::tree::SortBy::Size;
                        let tip = crate::panes::tree::row_tip(
                            row,
                            !row.dir || by_size,
                            chrono::Utc::now().timestamp_millis(),
                        );
                        let line = line.tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        });
                        let meta = |shown: bool| {
                            shown.then(|| {
                                div()
                                    .flex_none()
                                    .text_size(px(look::small()))
                                    .text_color(p.faint)
                                    .child(crate::panes::machine::format_bytes(row.size))
                            })
                        };
                        if row.dir {
                            return line
                                .cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .child(
                                    div()
                                        .w(px(look::ICON))
                                        .flex_none()
                                        .text_color(p.faint)
                                        .child(
                                            icon(if row.open {
                                                "chevron.down"
                                            } else {
                                                "chevron.right"
                                            })
                                            .size(px(look::ICON)),
                                        ),
                                )
                                .child(picture(&look::folder_icon(&row.name)).size(px(look::ICON)))
                                .child(name)
                                .children(meta(by_size))
                                .when(is_selected, |d| d.bg(p.ink(9.)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.files_ui.selected = Some((rel.clone(), true));
                                    this.toggle_folder(rel.clone(), cx)
                                }))
                                .into_any_element();
                        }
                        let openable = notebook::can_open(&row.name);
                        let full = format!("{project}/{rel}");
                        let is_open = open_path.as_deref() == Some(full.as_str());
                        let document = this
                            .open
                            .iter()
                            .chain(&this.others)
                            .find(|o| o.notebook.path == full);
                        let dirty = document.is_some_and(|o| o.dirty);
                        // Open in a tab: a hollow ring, or the kernel's dot
                        // (green once it runs) when the document has one.
                        let opened = this.tabs.contains(&full).then(|| match document {
                            Some(o) if !matches!(o.session, Session::None) => {
                                Self::session_dot(&o.session, &p)
                            }
                            _ => div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(p.faint),
                        });
                        line.pl(px(indent + look::ICON + look::GAP_4))
                            .when(!openable, |d| d.text_color(p.muted))
                            .when(is_open, |d| d.bg(p.accent.opacity(0.2)))
                            .when(!is_open && is_selected, |d| d.bg(p.ink(9.)))
                            .when(!is_open, |d| d.hover(move |d| d.bg(hover)))
                            .cursor_pointer()
                            .child(
                                picture(&look::file_icon_beside(&row.rel, |path| {
                                    lines.iter().any(|line| {
                                        matches!(line, crate::panes::tree_actions::Line::Row(r) if r.rel == path)
                                    })
                                }))
                                .size(px(look::ICON)),
                            )
                            .map(|d| {
                                if dirty {
                                    d.child(
                                        h_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap(px(look::GAP_4))
                                            .child(name.flex_initial())
                                            .child(dot(p.muted).size(px(4.))),
                                    )
                                } else {
                                    d.child(name)
                                }
                            })
                            .children(meta(true))
                            .children(opened)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files_ui.selected = Some((rel.clone(), false));
                                this.open_file(rel.clone(), cx)
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.render_output_file_scroll())
        // UniformList positions and reveals rows relative to its padded bounds.
        .pt(px(look::strip() + look::PANE_PAD_Y))
        .size_full();
        v_flex()
            .id("files-pane")
            .relative()
            .size_full()
            .when(self.files_ui.error.is_some(), |d| {
                d.child(
                    div()
                        .pt(px(look::strip() + look::PANE_PAD_Y))
                        .children(self.render_name_error(cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px(px(look::GAP_4))
                    .pb(px(look::PANE_PAD_Y))
                    .child(list),
            )
            .child(look::inset_scrollbar(
                &self.render_output_file_scroll(),
                look::strip(),
                cx,
            ))
            .children(self.render_new_menu(cx))
            .children(self.render_sort_menu(cx))
    }

    /// The Files tab's sort, new and refresh buttons, in its pane's strip.
    pub(crate) fn files_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.project.is_none() {
            return Vec::new();
        }
        let p = *Palette::get(cx);
        let sort = self.tree.sort;
        vec![
            icon_button("sort-files", "sort", &p, false)
                .on_prepaint(self.popup_triggers.track("files-sort"))
                .when(self.files_ui.sort_menu, |d| d.text_color(p.text))
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(format!(
                        "Sort by {}",
                        sort.label().to_lowercase()
                    ))
                    .build(w, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.files_ui.sort_menu = !this.files_ui.sort_menu;
                    this.files_ui.new_menu = false;
                    cx.notify();
                }))
                .into_any_element(),
            icon_button("new-item", "add", &p, false)
                .on_prepaint(self.popup_triggers.track("files-new"))
                .when(self.files_ui.new_menu, |d| d.text_color(p.text))
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("New file or folder").build(w, cx)
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.files_ui.new_menu = !this.files_ui.new_menu;
                    this.files_ui.sort_menu = false;
                    cx.notify();
                }))
                .into_any_element(),
            icon_button("refresh-files", "refresh", &p, false)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx)))
                .into_any_element(),
        ]
    }

    pub(crate) fn render_viewer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some((item, ix, count)) = self.viewer_page() else {
            return div()
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .text_color(p.muted)
                .child("Web pages shown from the Console (plotly, DT, leaflet, Shiny…), Quarto previews and HTML files appear here.")
                .into_any_element();
        };
        let body = match (&item.page, &item.note) {
            (_, Some(note)) => div()
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .text_color(p.muted)
                .whitespace_normal()
                .child(note.clone())
                .into_any_element(),
            (Some(page), None) => match self.pages.get(&page.key()) {
                Some(view) => view.clone().into_any_element(),
                None => div().child("Loading the page…").into_any_element(),
            },
            (None, None) => div().into_any_element(),
        };
        let extra = item.extra.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .h(px(look::row()))
                    .px(px(look::PANE_PAD_X))
                    .gap(px(look::GAP_4))
                    .border_b(px(0.5))
                    .border_color(p.glass_edge)
                    .text_color(p.muted)
                    .child(
                        icon_button("viewer-back", "previous", &p, false)
                            .when(ix == 0, |d| d.opacity(0.35))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_viewer_page(ix.saturating_sub(1), cx)
                            })),
                    )
                    .child(
                        icon_button("viewer-next", "next", &p, false)
                            .when(ix + 1 >= count, |d| d.opacity(0.35))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if ix + 1 < count {
                                    this.show_viewer_page(ix + 1, cx)
                                }
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(look::small()))
                            .child(format!("{} of {count}", ix + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(look::GAP_4))
                            .text_color(p.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(item.title.clone()),
                    )
                    .child(
                        icon_button("viewer-reload", "refresh", &p, false)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Reload the page")
                                    .build(w, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.reload_viewer(cx))),
                    )
                    .child(
                        icon_button("viewer-browser", "globe", &p, false)
                            .when(item.page.is_none(), |d| d.opacity(0.35))
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Open in browser")
                                    .build(w, cx)
                            })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_viewer_in_browser(cx)),
                            ),
                    )
                    .when_some(extra, |d, key| {
                        d.child(
                            bar_button(
                                "viewer-close",
                                if item.stoppable { "Stop" } else { "Close" },
                                Some(if item.stoppable { "stop" } else { "close" }),
                                &p,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.close_extra(&key, cx)),
                            ),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The Variables tab's body.
    pub(crate) fn render_variables(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let session = self.variables_session();
        // A list from a kernel that has gone, or from another one, is not shown.
        let live = session.is_some() && self.vars.session.as_deref() == session;
        let list: &[Variable] = if live { &self.vars.list } else { &[] };
        let mut rows = Vec::new();
        let filter = self.vars_filter.read(cx).value().to_string();
        let shown = filter_variables(list, &filter);
        self.variable_rows(&shown, &[], &mut rows, cx);
        let status = if session.is_none() {
            Some("No kernel running.".to_string())
        } else if let Some(err) = &self.vars.error {
            Some(err.clone())
        } else if list.is_empty() && !self.vars.loading {
            Some("No variables yet.".into())
        } else if shown.is_empty() && !list.is_empty() {
            Some(format!("No variables match “{}”.", filter.trim()))
        } else {
            None
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(look::PaneScroll::new(
                "variables-scroll",
                look::strip()
                    + if session.is_some() {
                        variables_filter_height()
                    } else {
                        0.
                    },
                look::GAP_4,
                div()
                    .id("variables")
                    .pb(px(look::PANE_PAD_Y))
                    .when_some(status, |d, s| {
                        d.child(div().px(px(look::PANE_PAD_X)).text_color(p.muted).child(s))
                    })
                    .children(rows),
            ))
            // The filter, Import and refresh, while a kernel runs.
            .when(session.is_some(), |d| {
                d.child(
                    h_flex()
                        .h(px(variables_filter_height()))
                        .gap(px(look::GAP_8))
                        .px(px(look::PANE_PAD_X))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(input::Input::new(&self.vars_filter).small()),
                        )
                        .child(self.import_button(cx))
                        .child(
                            icon_button("refresh-variables", "refresh", &p, false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    if let Some(session) = this.vars.session.clone() {
                                        this.refresh_object_explorers(&session, cx);
                                    }
                                    this.refresh_variables(cx);
                                }),
                            ),
                        )
                        .map(|row| look::floating_header(&p, look::strip(), false, row)),
                )
            })
            .into_any_element()
    }

    fn variable_rows(
        &self,
        list: &[Variable],
        parent: &[String],
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let p = *Palette::get(cx);
        for variable in list {
            let mut path = parent.to_vec();
            path.push(variable.name.clone());
            let open = self.vars.expanded.contains(&path);
            let toggle_path = path.clone();
            let table_path = path.clone();
            let hover = p.ink(look::ROW_HOVER);
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("var-{}", path.join("\u{1f}"))))
                    .mx(px(look::GAP_4))
                    .px(px(look::GAP_8))
                    .pl(px(look::GAP_8 + look::ICON * parent.len() as f32))
                    .gap(px(look::GAP_4))
                    .h(px(look::dense_row()))
                    .rounded(px(look::CONTROL_RADIUS))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .hover(move |d| d.bg(hover))
                    .when(variable.children, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_variable(toggle_path.clone(), cx)
                            }))
                    })
                    .child(
                        div()
                            .w(px(look::ICON_SMALL))
                            .flex_none()
                            .text_color(p.faint)
                            .when(variable.children, |d| {
                                d.child(
                                    icon(if open {
                                        "chevron.down"
                                    } else {
                                        "chevron.right"
                                    })
                                    .size(px(look::ICON_SMALL)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child(variable.name.clone()),
                    )
                    .child(div().text_color(p.faint).child(variable.class.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.muted)
                            .child(variable.summary.clone()),
                    )
                    .when(variable.table, |d| {
                        d.child(
                            icon_button(
                                SharedString::from(format!("view-{}", path.join("\u{1f}"))),
                                "table",
                                &p,
                                false,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_explorer(table_path.clone(), window, cx)
                                },
                            )),
                        )
                    })
                    .child(self.variable_object_actions(variable, &path, cx))
                    .into_any_element(),
            );
            if open && let Some(children) = self.vars.children.get(&path) {
                self.variable_rows(children, &path, rows, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::filter_variables;
    use sandhill_proto::Variable;

    #[test]
    fn filters_variables_by_name() {
        let var = |name: &str| Variable {
            name: name.into(),
            kind: "data".into(),
            class: "data.frame".into(),
            size: None,
            children: false,
            table: true,
            summary: String::new(),
            accessor: None,
        };
        let list = [var("seu"), var("Markers"), var("counts")];
        let names = |filter| {
            filter_variables(&list, filter)
                .into_iter()
                .map(|v| v.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["seu", "Markers", "counts"]);
        assert_eq!(names(" mark "), ["Markers"]);
        assert!(names("zzz").is_empty());
    }
}
