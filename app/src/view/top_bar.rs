//! The title bar and the document and pane tab strips.

use super::*;

/// Use the actual source icon key, including special names and compressed types.
fn collapsed_file_groups(paths: &[String]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (ix, path) in paths.iter().enumerate() {
        let key = look::file_icon(path);
        if let Some((_, files)) = groups.iter_mut().find(|(icon, _)| *icon == key) {
            files.push(ix);
        } else {
            groups.push((key, vec![ix]));
        }
    }
    groups
}

impl Workbench {
    /// The title bar: window buttons directly on the glass after the Mac's
    /// own, and the theme button at the right; the window draws the project
    /// tabs between them.
    pub(super) fn render_top_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        div()
            .relative()
            .flex_none()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                // Behind the controls: dragging here moves the window, and a
                // double click zooms it, as a title bar does.
                div()
                    .id("title-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            if cfg!(target_os = "macos") {
                                window.titlebar_double_click();
                            } else {
                                window.zoom_window();
                            }
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                h_flex()
                    .absolute()
                    .size_full()
                    .pt(px(6.))
                    .pl(px(if window.is_fullscreen() {
                        6.
                    } else {
                        TITLE_BAR_LEFT
                    }))
                    .pr(px(6.))
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(25.))
                            .px(px(2.))
                            .child(
                                icon_button("toggle-files", "sidebar.left", &p, true)
                                    .when(self.show_files, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the left panes",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::LeftTop, window, cx);
                                    })),
                            )
                            .child(
                                icon_button("toggle-editor", "sidebar.middle", &p, true)
                                    .when(self.show_middle, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the editor",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::Editor, window, cx);
                                    })),
                            )
                            .child({
                                let waiting = self
                                    .agents
                                    .as_ref()
                                    .is_some_and(|a| a.read(cx).any_waiting());
                                icon_button("toggle-agents", "chat", &p, true)
                                    .relative()
                                    .when(self.show_agents, |d| d.text_color(p.text))
                                    .when(waiting, |d| {
                                        d.child(
                                            dot(p.accent)
                                                .size(px(6.))
                                                .absolute()
                                                .top(px(2.))
                                                .right(px(5.)),
                                        )
                                    })
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Show or hide the AI pane",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_column(Panel::Agents, window, cx);
                                    }))
                            })
                            .child(capsule_divider(&p))
                            .child(
                                icon_button("palette", "search", &p, true)
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Go to a file or run a command ⌘K",
                                        )
                                        .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_palette(window, cx)
                                    })),
                            )
                            .child(
                                icon_button("main-menu", "menu", &p, true)
                                    .on_prepaint(self.popup_triggers.track("main-menu"))
                                    .when(self.main_menu, |d| d.text_color(p.text))
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Menu")
                                            .build(w, cx)
                                    })
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.main_menu = !this.main_menu;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .flex_none()
                            .h(px(25.))
                            .px(px(2.))
                            .child(
                                icon_button(
                                    "theme",
                                    if p.dark { "theme.light" } else { "theme.dark" },
                                    &p,
                                    true,
                                )
                                .tooltip(move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(if p.dark {
                                        "Switch to the light theme"
                                    } else {
                                        "Switch to the dark theme"
                                    })
                                    .build(w, cx)
                                })
                                .on_click(|_, window, cx| look::toggle_appearance(window, cx)),
                            )
                            .child(
                                icon_button("welcome", "welcome", &p, true)
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Welcome")
                                            .build(w, cx)
                                    })
                                    .on_click(|_, _, cx| {
                                        crate::shell::app_window::show_welcome(None, cx)
                                    }),
                            ),
                    ),
            )
    }

    /// A pane's tab strip: its tabs in a capsule, then anything else.
    pub(crate) fn strip(p: &Palette) -> Div {
        h_flex()
            .flex_none()
            .h(px(look::STRIP))
            .px(px(4.))
            .gap(px(4.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .overflow_hidden()
    }

    /// One tab in a strip's capsule: a pill, raised when picked.
    pub(crate) fn tab(
        id: impl Into<ElementId>,
        leading: Option<AnyElement>,
        label: impl Into<SharedString>,
        active: bool,
        p: &Palette,
    ) -> Stateful<Div> {
        let hover = p.ink(6.);
        let text = p.text;
        let tab = h_flex()
            .id(id)
            .group(look::TAB_GROUP)
            .flex_none()
            .h(px(look::TAB_H))
            .gap(px(5.))
            .px(px(look::TAB_PAD))
            .rounded_full()
            .text_size(px(look::TAB_TEXT))
            .whitespace_nowrap()
            .text_color(p.muted)
            .cursor_pointer()
            .children(leading)
            .child(label.into());
        if active {
            raised(tab, p)
        } else {
            tab.hover(move |d| d.bg(hover).text_color(text))
        }
    }

    /// The middle pane's tabs: the open notebooks, in the order they were
    /// opened, then the Data Explorer.
    pub(crate) fn render_document_tabs(
        &self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let shown = if self.objects.is_shown() || self.explorer_shown || self.diff_shown {
            None
        } else {
            self.active_document_path()
        };
        let groups = if expanded {
            self.tabs
                .iter()
                .enumerate()
                .map(|(ix, path)| (look::file_icon(path), vec![ix]))
                .collect()
        } else {
            collapsed_file_groups(&self.tabs)
        };
        let mut tabs: Vec<AnyElement> = groups
            .into_iter()
            .map(|(file_icon, files)| {
                let ix = files[0];
                let path = &self.tabs[ix];
                let name = if expanded {
                    crate::files::base_name(path).to_string()
                } else if files.len() > 1 {
                    files.len().to_string()
                } else {
                    String::new()
                };
                let show = path.clone();
                let tip = files
                    .iter()
                    .map(|ix| self.tabs[*ix].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let dirty = files.iter().any(|ix| self.tab_dirty(&self.tabs[*ix]));
                let picked = expanded && shown == Some(path.as_str());
                let lead = picture(&file_icon).size(px(14.)).into_any_element();
                let (lead, trailing) = if expanded {
                    look::place_close(
                        lead,
                        self.close_button(ix, path.clone(), dirty, picked, cx),
                        picked || dirty,
                    )
                } else {
                    (lead, None)
                };
                Self::tab(("file-tab", ix), Some(lead), name, picked, &p)
                    .flex_none()
                    .when(!expanded, |d| d.px(px(5.)).gap(px(3.)))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                    })
                    .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
                    .when(!expanded && dirty, |d| d.child(dot(p.muted).size(px(4.))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_file_tab(&show, cx);
                        if this.panel_collapsed(crate::shell::pane_sizes::Panel::Editor) {
                            this.middle_vertical = crate::shell::pane_sizes::Vertical::Split;
                        }
                    }))
                    .into_any_element()
            })
            .collect();
        if let Some(explorer) = &self.explorer {
            let picked =
                expanded && self.explorer_shown && !self.diff_shown && !self.objects.is_shown();
            let lead = icon("table").size(px(14.)).into_any_element();
            let (lead, trailing) = if expanded {
                look::place_close(
                    lead,
                    look::tab_close("explorer-close", picked, &p).on_click(cx.listener(
                        |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_explorer(cx)
                        },
                    )),
                    picked,
                )
            } else {
                (lead, None)
            };
            tabs.push(
                Self::tab(
                    "explorer-tab",
                    Some(lead),
                    if expanded {
                        explorer.read(cx).name()
                    } else {
                        String::new()
                    },
                    picked,
                    &p,
                )
                .flex_none()
                .when(!expanded, |d| d.px(px(5.)).gap_0())
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Data Explorer").build(w, cx)
                })
                .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.reveal_middle();
                    this.hide_object_explorer();
                    this.explorer_shown = true;
                    this.diff_shown = false;
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        tabs.extend(self.diff_tab_element(expanded, cx));
        tabs.extend(self.object_tab_elements(expanded, cx));
        tabs
    }

    /// The editor's actions and pane controls share one row. File labels
    /// remain in the project's titlebar capsule.
    pub(crate) fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let document = self.open.as_ref().filter(|_| {
            !self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor)
                && !self.diff_shown
                && !self.explorer_shown
                && !self.objects.is_shown()
                && self.media_shown.is_none()
        });
        let media = self
            .media_shown
            .as_ref()
            .filter(|_| {
                !self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor)
                    && !self.diff_shown
                    && !self.explorer_shown
                    && !self.objects.is_shown()
            })
            .and_then(|path| self.media.get(path));
        let explorer = self.explorer.as_ref().filter(|_| {
            self.explorer_shown
                && !self.diff_shown
                && !self.objects.is_shown()
                && !self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor)
        });
        Self::strip(&p)
            .child(
                h_flex()
                    .id("editor-actions")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .when_some(document, |d, open| {
                        d.child(self.render_notebook_bar(open, cx))
                    })
                    .when_some(media, |d, media| {
                        d.child(media.update(cx, |media, cx| media.render_toolbar(cx)))
                    })
                    .when_some(explorer, |d, explorer| d.child(explorer.read(cx).toolbar())),
            )
            .when_some(
                document.and_then(|open| self.document_size(open, &p, cx)),
                |d, size| d.child(size),
            )
            .when(document.is_some(), |d| d.child(self.outline_button(cx)))
            .child(self.panel_buttons(crate::shell::pane_sizes::Panel::Editor, cx))
    }

    /// A document tab's close button. Unsaved, a dot that stays after the
    /// label and turns into the cross under the pointer.
    fn close_button(
        &self,
        ix: usize,
        path: String,
        dirty: bool,
        picked: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = *Palette::get(cx);
        let close = if dirty {
            let group: SharedString = format!("close-tab-{ix}").into();
            look::close_circle(("close-tab", ix), &p)
                .group(group.clone())
                .child(
                    dot(p.muted)
                        .group_hover(group.clone(), |d| d.invisible())
                        .absolute(),
                )
                .child(
                    div()
                        .invisible()
                        .group_hover(group, |d| d.visible())
                        .child(icon("close").size(px(12.))),
                )
        } else {
            look::tab_close(("close-tab", ix), picked, &p)
        };
        close.on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.ask_close(path.clone(), window, cx)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::collapsed_file_groups;
    #[test]
    fn collapsed_file_icons_group_by_source_type_in_first_occurrence_order() {
        let paths = [
            "first.R",
            "a.py",
            "second.r",
            "reads.fastq.gz",
            "reads.fastq",
            "notes.md",
        ]
        .map(str::to_string);
        let groups = collapsed_file_groups(&paths);
        assert_eq!(
            groups
                .iter()
                .map(|(_, files)| files.clone())
                .collect::<Vec<_>>(),
            vec![vec![0, 2], vec![1], vec![3, 4], vec![5]]
        );
        assert_eq!(groups[0].0, crate::look::file_icon("first.R"));
        assert_eq!(groups[2].0, crate::look::file_icon("reads.fastq.gz"));
    }
}
