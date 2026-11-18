//! The notebook, script and Markdown documents: cells, their editors and outputs.

use super::*;
use gpui_kit::component::input::RopeExt as _;

/// Whether a box of outputs took a wheel turn of `dy` (negative moves
/// down), so the notebook must not scroll too. Its own scroll listener runs
/// first: it adds a turn that can move it, unclamped, and skips one that
/// can't (at its top or bottom, or when it fits), leaving `offset` where it
/// was. `max` is how far down it can go. A turn that lands exactly on an
/// end reads as skipped, so the notebook moves once with it.
fn box_takes(offset: Pixels, max: Pixels, dy: Pixels) -> bool {
    !dy.is_zero() && (offset + dy).clamp(-max, px(0.)) != offset
}

fn cell_text(open: &Open, ix: usize, cx: &App) -> String {
    open.editors[ix].read(cx).value().to_string()
}

/// The editor's right-click item that puts the selection in the AI pane's
/// message box; dimmed without a selection (Huan, 2026-11-16).
fn ask_ai_item(
    editor: &Entity<gpui_kit::component::input::EditorState>,
) -> impl Fn(
    gpui_kit::component::native_menu::NativeMenu,
    &mut Window,
    &mut App,
) -> gpui_kit::component::native_menu::NativeMenu
+ 'static {
    let editor = editor.downgrade();
    move |menu, _, cx| {
        let selected = editor
            .upgrade()
            .is_some_and(|e| !e.read(cx).selected_range().is_empty());
        menu.separator().menu_with_disabled(
            "Ask AI About Selection",
            !selected,
            Box::new(crate::AskAiAboutSelection),
        )
    }
}

/// Closes the explanation box as soon as the pointer leaves the name it
/// explains (Huan, 2026-11-16); the editor itself keeps it until its next
/// answer, 3 seconds later. Runs after the editor's own mouse move.
fn hover_leaves(
    editor: &Entity<gpui_kit::component::input::EditorState>,
) -> impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static {
    let editor = editor.downgrade();
    move |event, _, cx| {
        let Some(editor) = editor.upgrade() else {
            return;
        };
        let left = {
            let e = editor.read(cx);
            e.hover_popover().is_some_and(|shown| {
                !e.range_to_bounds(&shown.symbol_range)
                    .is_some_and(|bounds| bounds.contains(&event.position))
            })
        };
        if left {
            editor.update(cx, |e, cx| e.clear_hover_state(cx));
        }
    }
}

impl Workbench {
    /// The notebook and source jumps share the same unobscured viewport edge.
    /// Find has a 24px input, 12px padding and 1px edge; Replace adds 28px.
    pub(crate) fn notebook_header_height(&self) -> f32 {
        look::STRIP
            + if self.find.open {
                if self.find.replace { 65. } else { 37. }
            } else {
                0.
            }
    }

    pub(super) fn render_notebook(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        if self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor) {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .into_any_element();
        }
        if let Some(path) = &self.opening_shown
            && !self.objects.is_shown()
            && !self.explorer_shown
            && !self.diff_shown
        {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .child(
                    gpui_kit::component::progress::Progress::new("document-opening")
                        .loading(true)
                        .color(p.accent)
                        .h(px(2.))
                        .accessibility_label("Opening document"),
                )
                .child(
                    h_flex()
                        .px(px(16.))
                        .py(px(14.))
                        .gap(px(8.))
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(crate::panes::files::base_name(path).to_string()),
                        )
                        .child(div().flex_none().child("Opening…")),
                )
                .into_any_element();
        }
        if let Some(browser) = self.render_object_explorer(cx) {
            return browser;
        }
        if self.diff_shown && self.diff.is_some() {
            return self.render_diff(cx);
        }
        if !self.diff_shown
            && !self.explorer_shown
            && let Some(media) = self
                .media_shown
                .as_ref()
                .and_then(|path| self.media.get(path))
        {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .child(div().flex_1().min_h_0().child(media.clone()))
                .into_any_element();
        }
        if let Some(explorer) = self
            .explorer
            .as_ref()
            .filter(|_| self.explorer_shown || self.open.is_none())
        {
            return self.render_explorer(explorer, cx);
        }
        let Some(open) = &self.open else {
            return self.render_start(cx);
        };
        if open.single_editor() {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(self.render_tabs(cx))
                .children(self.render_find(cx))
                .children(self.render_truncated_note(open, cx))
                .child(self.render_source_document(open, cx))
                .into_any_element();
        }
        let scroll = self.notebook_scroll.clone();
        let anchor = self.placed.logical_scroll_top(&scroll);
        let bodies = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| {
                if !self.placed.shows(ix) {
                    return None;
                }
                Some(match cell.kind {
                    // As in the Tauri app, a notebook's Markdown shows
                    // formatted; a Quarto document's prose stays text.
                    CellKind::Text => {
                        if open.notebook.format != notebook::Format::Ipynb
                            || open.editing.contains(&cell.id)
                            || self.find.replaying
                            || cell_text(open, ix, cx).trim().is_empty()
                        {
                            self.render_cell_editor(open, ix, false, window, cx)
                        } else {
                            self.render_markdown(open, ix, cx)
                        }
                    }
                    CellKind::Code => self.render_code_cell(open, ix, window, cx),
                })
            })
            .collect::<Vec<_>>();
        let cells = self.render_cells(open, bodies, window, cx);
        let commanding = self.cells_focus.is_focused(window);
        let header_height = self.notebook_header_height();
        let target = self
            .notebook_scroll_target
            .take()
            .and_then(|logical| self.placed.scroll_child(logical));
        let anchor =
            anchor.and_then(|(logical, within)| self.placed.scroll_anchor(logical, within));
        let original_offset = scroll.offset();
        let scroller = div()
            .on_children_prepainted(move |_, window, _| {
                if let Some(item) = target
                    && let Some(bounds) = scroll.bounds_for_item(item)
                {
                    let y = look::header_scroll_offset(
                        scroll.offset().y,
                        bounds.top() - scroll.bounds().top(),
                        bounds.bottom() - scroll.bounds().top(),
                        scroll.bounds().size.height,
                        px(header_height),
                        scroll.max_offset().y,
                    );
                    scroll.set_offset(Point {
                        x: original_offset.x,
                        y,
                    });
                    window.refresh();
                    return;
                }
                // Replacing an estimated row above the viewport must not move
                // the visible cell. Child bounds include the header padding,
                // so this anchor correction preserves it without adding it twice.
                // Explicit wheel/jump requests remain authoritative.
                if scroll.offset() != original_offset {
                    return;
                }
                if let Some((item, within)) = anchor
                    && let Some(bounds) = scroll.bounds_for_item(item)
                {
                    let y = (scroll.bounds().top() + within - bounds.top())
                        .clamp(-scroll.max_offset().y, px(0.));
                    if (y - original_offset.y).abs() > px(0.5) {
                        scroll.set_offset(Point {
                            x: original_offset.x,
                            y,
                        });
                        window.refresh();
                    }
                }
            })
            .id("cells")
            .flex()
            .flex_col()
            .on_scroll(cx.listener(|this, _, _, cx| {
                // Hover geometry belongs to the pre-scroll editor layout.
                this.clear_language_popovers(None, cx);
            }))
            .track_focus(&self.cells_focus)
            .when(commanding, |d| d.key_context(crate::CELLS))
            .pb(px(200.))
            .pl(px(6.))
            .pr(px(24.))
            // Between cells that can be added to, the rows that add them.
            .when(!open.notebook.can_add_cells(), |d| {
                d.gap(px(crate::editor::cell_view::TOOLS_HEIGHT + 4.))
            })
            .children(cells);
        pane(&p)
            .relative()
            .flex_1()
            .h_full()
            .child(
                look::PaneScroll::new(
                    "notebook-scroll",
                    header_height,
                    10.,
                    self.cell_keys(scroller, cx),
                )
                .handle(&self.notebook_scroll),
            )
            .child(look::floating_header(
                &p,
                0.,
                true,
                v_flex()
                    .child(self.render_tabs(cx))
                    .children(self.render_find(cx)),
            ))
            .into_any_element()
    }

    fn render_source_document(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        if open.notebook.format == notebook::Format::Delimited {
            return self.render_delimited(open, cx);
        }
        if open.notebook.format == notebook::Format::Markdown && open.markdown_preview {
            return div()
                .id("markdown-document-preview")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.notebook_scroll)
                .px(px(18.))
                .py(px(12.))
                .child(
                    crate::editor::md_images::markdown(open, 0, "markdown-document", cx)
                        .style(TextViewStyle::default().paragraph_gap(rems(0.5))),
                )
                .into_any_element();
        }
        self.render_script(open, cx).into_any_element()
    }

    /// A script: one editor with line numbers, filling the pane. ⌘↵ runs
    /// the line or statement; the runs show in the Console.
    fn render_script(&self, open: &Open, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .pt(px(6.))
            .on_action(cx.listener(|this, _: &RunLine, window, cx| this.run_statement(window, cx)))
            // Shift+Enter does the same, as in the Tauri app's scripts.
            .on_action(cx.listener(|this, _: &RunCell, window, cx| this.run_statement(window, cx)))
            .on_action(
                cx.listener(|this, _: &crate::AskAiAboutSelection, window, cx| {
                    this.ask_ai_about_selection(0, window, cx)
                }),
            )
            .on_mouse_move(hover_leaves(&open.editors[0]))
            .capture_action({
                let editor = open.editors[0].clone();
                move |action: &gpui_kit::component::input::Enter, window, cx| {
                    if crate::editor::indent::enter_in_brackets(&editor, action, window, cx) {
                        cx.stop_propagation();
                    }
                }
            })
            .child(
                Editor::new(&open.editors[0])
                    .extend_context_menu(ask_ai_item(&open.editors[0]))
                    .readonly(open.notebook.truncated)
                    .bordered(false)
                    .appearance(false)
                    .line_height(px(LINE_HEIGHT))
                    .h_full(),
            )
    }

    /// A notebook's Markdown cell shown formatted, as in the Tauri app; a
    /// double click edits it, and Shift+Enter or leaving it shows it formatted
    /// again.
    fn render_markdown(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let editor = open.editors[ix].clone();
        let path = open.notebook.path.clone();
        div()
            .id(("markdown", ix))
            .w_full()
            .px(px(8.))
            .py(px(2.))
            .text_size(px(look::CHROME))
            .line_height(relative(1.4))
            .cursor_text()
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() < 2 {
                    return;
                }
                if let Some(open) = this.open.as_mut().filter(|o| o.notebook.path == path) {
                    let id = open.notebook.cells[ix].id.clone();
                    open.editing.insert(id);
                    editor.update(cx, |editor, cx| editor.focus(window, cx));
                    cx.notify();
                }
            }))
            .child(
                crate::editor::md_images::markdown(open, ix, ("md", ix), cx).style(
                    TextViewStyle::default()
                        .paragraph_gap(rems(0.5))
                        .heading_font_size(|level, _| {
                            px(match level {
                                1 => look::TITLE,
                                2 => look::CHROME,
                                _ => look::TAB_TEXT,
                            })
                        })
                        .code_block(StyleRefinement::default().text_size(px(look::DOC_CODE))),
                ),
            )
            .into_any_element()
    }

    /// "30,001 ln", for CSV/TSV "· 41 col", and while text is selected
    /// "· 12 ln selected" or, on one line, "· 3 col selected" (CSV/TSV
    /// columns, else characters; Huan, 2026-11-14). Single-editor documents
    /// only, not notebooks. Sits at the row's right, before the outline.
    pub(super) fn document_size(&self, open: &Open, p: &Palette, cx: &App) -> Option<AnyElement> {
        if !open.notebook.single_editor() {
            return None;
        }
        let editor = open.editors.first()?.read(cx);
        let text = editor.text();
        let mut lines = text.lines_len();
        // A final newline ends the last line rather than starting another.
        if lines > 1 && text.len() > 0 && text.char_at(text.len() - 1) == Some('\n') {
            lines -= 1;
        }
        let mut label = format!(
            "{}{} ln",
            Self::grouped(lines),
            if open.notebook.truncated { "+" } else { "" },
        );
        let delimited = open.notebook.format == notebook::Format::Delimited;
        let sep = crate::editor::delimited::separator(open.notebook.language.as_deref());
        if delimited && let Some(header) = &open.delimited_header {
            let columns = crate::editor::delimited::column_count(&header.read(cx).value(), sep);
            label.push_str(&format!(" · {} col", Self::grouped(columns)));
        }
        let range = editor.selected_range();
        if !range.is_empty() {
            let start = text.offset_to_point(range.start);
            let end = text.offset_to_point(range.end);
            // A selection ending at a line's start doesn't take that line.
            let last = if end.row > start.row && end.column == 0 {
                end.row - 1
            } else {
                end.row
            };
            if last > start.row {
                let rows = last - start.row + 1;
                label.push_str(&format!(" · {} ln selected", Self::grouped(rows)));
            } else {
                let count = if delimited {
                    let line = text.slice_line(start.row).to_string();
                    crate::editor::delimited::fields_spanned(&line, start.column, end.column, sep)
                } else {
                    editor.selected_value().chars().count()
                };
                label.push_str(&format!(" · {} col selected", Self::grouped(count)));
            }
        }
        Some(
            div()
                .flex_none()
                .px(px(6.))
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .whitespace_nowrap()
                .child(label)
                .into_any_element(),
        )
    }

    /// 30001 as "30,001".
    fn grouped(n: usize) -> String {
        let digits = n.to_string();
        let mut out = String::new();
        for (i, digit) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(digit);
        }
        out
    }

    /// Document actions inside the editor's single header row.
    pub(super) fn render_notebook_bar(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let wrap_button = Self::editor_wrap_type(open).map(|kind| {
            icon_button("editor-wrap", "editor.wrap", &p, false)
                .when(crate::prefs::editor_soft_wrap(kind), |button| {
                    button.bg(p.accent_soft).text_color(p.accent)
                })
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Wrap long lines").build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_editor_wrap(cx)))
        });
        if open.notebook.is_text_document() {
            return h_flex()
                .flex_none()
                .gap(px(4.))
                .when(open.notebook.quarto_source(), |bar| {
                    bar.child(
                        bar_button("quarto-view", "Notebook", Some("doc"), &p)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    "Switch to Notebook view",
                                )
                                .build(w, cx)
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_quarto_view(window, cx);
                            })),
                    )
                })
                .when(open.notebook.format == notebook::Format::Markdown, |bar| {
                    bar.child(
                        bar_button(
                            "markdown-view",
                            if open.markdown_preview {
                                "Edit source"
                            } else {
                                "Rendered view"
                            },
                            Some("doc"),
                            &p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(open) = this.open.as_mut() {
                                open.markdown_preview = !open.markdown_preview;
                                cx.notify();
                            }
                        })),
                    )
                })
                .when(
                    crate::panes::jobs::is_job_script(&open.notebook.path),
                    |bar| {
                        bar.child(
                            bar_button("run-job", "Run as job", Some("terminal"), &p)
                                .on_click(cx.listener(|this, _, _, cx| this.run_open_as_job(cx))),
                        )
                    },
                )
                .children(self.colouring_button(cx))
                .children(wrap_button)
                .child(
                    bar_button("save", "Save", Some("doc.save"), &p)
                        .when(!open.dirty, |d| d.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .into_any_element();
        }
        h_flex()
            .flex_none()
            .gap(px(4.))
            .whitespace_nowrap()
            .when(open.notebook.format == notebook::Format::Quarto, |bar| {
                bar.child(
                    bar_button("quarto-view", "Source", Some("doc"), &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Switch to Source view")
                                .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_quarto_view(window, cx);
                        })),
                )
            })
            .child(self.kernel_picker(true, cx))
            .children(self.language_status(&open.notebook.path).map(|reason| {
                div()
                    .id("language-status")
                    .text_size(px(look::TINY))
                    .text_color(p.muted)
                    .child("Analysis unavailable")
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(reason.clone()).build(window, cx)
                    })
            }))
            .when(open.is_script(), |bar| {
                bar.child(
                    bar_button("run-line", "Run line", Some("run"), &p).on_click(
                        cx.listener(|this, _, window, cx| this.run_statement(window, cx)),
                    ),
                )
            })
            .child(
                bar_button(
                    "run-all",
                    if open.is_script() {
                        "Run file"
                    } else {
                        "Run All"
                    },
                    Some("run"),
                    &p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.run_all(cx))),
            )
            .when(
                crate::panes::viewer::is_previewable(&open.notebook.path),
                |bar| {
                    let path = open.notebook.path.clone();
                    bar.child(
                        bar_button("preview", "Preview", Some("globe"), &p)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(
                                    if cfg!(target_os = "macos") {
                                        "Preview with Quarto in the Viewer (⇧⌘K)"
                                    } else {
                                        "Preview with Quarto in the Viewer (Ctrl+Shift+K)"
                                    },
                                )
                                .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preview_document(path.clone(), cx)
                            })),
                    )
                    .child(self.render_document_button(cx))
                },
            )
            .when(
                crate::panes::jobs::is_job_script(&open.notebook.path),
                |bar| {
                    bar.child(
                        bar_button("run-job", "Run as job", Some("terminal"), &p)
                            .on_click(cx.listener(|this, _, _, cx| this.run_open_as_job(cx))),
                    )
                },
            )
            .when(open.notebook.can_restructure(), |bar| {
                let on = self.ghost_cells;
                bar.child(
                    bar_button("ghost-cells", "Suggest", Some("sparkle.ai"), &p)
                        .when(on, |d| d.bg(p.accent_soft).text_color(p.accent))
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if on {
                                "Suggesting the next cell after each run: click to stop"
                            } else {
                                "Suggest the next cell after each run, with Claude Code or Codex"
                            })
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_ghost_cells(cx))),
                )
            })
            .when(!open.is_script(), |bar| {
                // Icon-only so Save stays visible beside Render and AI at
                // the default window width.
                bar.child(
                    icon_button("clear-all", "outputs.clear", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Clear all outputs")
                                .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.clear_outputs(None, cx))),
                )
            })
            .when(!open.is_script(), |bar| {
                bar.child(self.notebook_ai_button(cx))
            })
            .children(wrap_button)
            .when(!open.notebook.truncated, |bar| {
                bar.child(
                    bar_button("save", "Save", Some("doc.save"), &p)
                        .when(!open.dirty, |d| d.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
            })
            .children(self.session_buttons(open.session.id().is_some(), cx))
            .into_any_element()
    }

    /// A cell's editor, as tall as its text, so the notebook scrolls and
    /// the cell doesn't. Code sits in a box; text doesn't.
    fn render_cell_editor(
        &self,
        open: &Open,
        ix: usize,
        boxed: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let editor = &open.editors[ix];
        let state = editor.read(cx);
        let line_count = state.value().split('\n').count().max(1);
        let lines = if state.display_map().folded_ranges().is_empty() {
            line_count
        } else {
            (0..line_count)
                .filter(|line| !state.display_map().is_buffer_line_hidden(*line))
                .count()
                .max(1)
        };
        let focused = open.current == Some(ix)
            || (open.current.is_none()
                && editor
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx))
            // A cell the agent just added or changed lights up.
            || open
                .lit
                .as_ref()
                .is_some_and(|(id, _)| *id == open.notebook.cells[ix].id);
        div()
            .w_full()
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                this.clear_language_popovers(Some(ix), cx);
            }))
            .when(boxed, |d| {
                d.bg(p.chunk)
                    .border_1()
                    .border_color(if focused { p.accent } else { p.line })
                    .rounded(px(RADIUS))
                    .overflow_hidden()
            })
            .on_action(cx.listener(move |this, _: &RunCell, window, cx| {
                this.run_and_advance(ix, window, cx)
            }))
            .on_action(cx.listener(move |this, _: &RunLine, _, cx| this.run_cell(ix, cx)))
            .on_action(
                cx.listener(move |this, _: &crate::AskAiAboutSelection, window, cx| {
                    this.ask_ai_about_selection(ix, window, cx)
                }),
            )
            .on_mouse_move(hover_leaves(editor))
            .capture_action({
                let editor = editor.clone();
                move |action: &gpui_kit::component::input::Enter, window, cx| {
                    if crate::editor::indent::enter_in_brackets(&editor, action, window, cx) {
                        cx.stop_propagation();
                    }
                }
            })
            .child(
                Editor::new(editor)
                    .extend_context_menu(ask_ai_item(editor))
                    .bordered(false)
                    .appearance(false)
                    .line_height(px(LINE_HEIGHT))
                    .when(!boxed, |e| {
                        e.font_family(".SystemUIFont").text_size(px(look::CHROME))
                    })
                    .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
            )
            .into_any_element()
    }

    /// A code cell's editor and outputs; `render_cells` puts the gutter
    /// and tools around them. Long outputs scroll in a box of their own;
    /// folded ones show a line that unfolds them.
    fn render_code_cell(
        &self,
        open: &Open,
        ix: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let cell = &open.notebook.cells[ix];
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id));
        let busy = open.sending.contains(&cell.id)
            || open.waiting.contains(&cell.id)
            || run.is_some_and(|r| !r.finished);
        let outputs: &[Value] = match run {
            Some(run) => &run.outputs,
            None => &cell.outputs,
        };
        let shown: Vec<Shown> = outputs.iter().map(show).collect();
        let pages = shown.iter().any(|s| matches!(s, Shown::Page(_)));
        // An error can come from a run that ended fine, as when a plot
        // fails to draw.
        let error_shown = shown.iter().any(|s| matches!(s, Shown::Error(_)));
        let count = shown.len();
        let failed = run.and_then(failed_status);
        let errored = run.and_then(|r| r.status.as_deref()) == Some("error");
        let folded = open.collapsed.contains(&cell.id);
        let outputs: Vec<AnyElement> = if folded {
            Vec::new()
        } else {
            shown
                .into_iter()
                .enumerate()
                .map(|(n, s)| {
                    let id = SharedString::from(format!("out-{}-{n}", cell.id));
                    self.render_output(s, id, PLOT_MAX_WIDTH, Some(ix), None, cx)
                })
                .collect()
        };
        let selected = open.current == Some(ix)
            || (open.current.is_none()
                && open.editors[ix]
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx))
            || open.lit.as_ref().is_some_and(|(id, _)| *id == cell.id);
        let border = if selected {
            p.accent
        } else if busy {
            p.busy
        } else if errored || failed.is_some() {
            p.err
        } else {
            p.line
        };
        let fixable = (errored || error_shown) && !busy && !folded;
        let line = self.run_line(open, ix, cx);
        let framed = line.is_some() || count > 0 || busy || failed.is_some();
        v_flex()
            .w_full()
            .child(self.render_cell_editor(open, ix, true, window, cx))
            .child(
                v_flex()
                    .when(framed, |d| {
                        // Start at the code box's lower edge, including the
                        // run metadata, so the two side lines stay connected.
                        d.mx(px(8.))
                            .mb(px(2.))
                            .border_l_1()
                            .border_r_1()
                            .border_b_1()
                            .rounded_b(px(RADIUS))
                            .border_color(border)
                    })
                    .children(line)
                    .children(self.render_agent_change(open, ix, cx))
                    .when(folded && count > 0, |d| {
                        d.child(
                            div()
                                .id(("unfold", ix))
                                .py(px(4.))
                                .px(px(12.))
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .cursor_pointer()
                                .hover(|d| d.text_color(p.accent))
                                .child(if count == 1 {
                                    "1 output hidden: show".to_string()
                                } else {
                                    format!("{count} outputs hidden: show")
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.toggle_outputs(ix, cx)),
                                ),
                        )
                    })
                    .when(
                        !outputs.is_empty() || failed.is_some() || busy && !folded,
                        |d| {
                            let scroll = self
                                .output_scrolls
                                .borrow_mut()
                                .entry(cell.id.clone())
                                .or_default()
                                .clone();
                            let inner = scroll.clone();
                            d.child(
                                // The wheel capture area exactly matches the
                                // scroll box. Metadata and borders outside it
                                // leave the wheel to the notebook.
                                div()
                                    .id(("outputs-wheel", ix))
                                    .w_full()
                                    .on_scroll(move |event, window, cx| {
                                        let dy = event.delta.pixel_delta(window.line_height()).y;
                                        if box_takes(inner.offset().y, inner.max_offset().y, dy) {
                                            cx.stop_propagation();
                                        }
                                    })
                                    .child(
                                        v_flex()
                                            .id(("outputs", ix))
                                            .track_scroll(&scroll)
                                            .w_full()
                                            .px(px(12.))
                                            .pt(px(2.))
                                            .pb(px(8.))
                                            .gap_1()
                                            // Long outputs scroll, while web pages
                                            // keep their own height.
                                            .when(!pages, |d| {
                                                d.max_h(px(OUTPUT_MAX_HEIGHT)).overflow_y_scroll()
                                            })
                                            .children(outputs)
                                            .when_some(failed, |d, s| {
                                                d.child(
                                                    div().text_color(p.err).cursor_text().child(
                                                        OutputText::new(
                                                            ("failed-output", ix),
                                                            format!("Run ended: {s}"),
                                                        ),
                                                    ),
                                                )
                                            }),
                                    ),
                            )
                        },
                    )
                    .when(fixable, |d| d.children(self.render_error_actions(ix, cx))),
            )
            .into_any_element()
    }

    /// One output. `cell` is the notebook cell it is under: web pages are
    /// drawn there (the Console leaves them to the Viewer), and its plots
    /// can be saved. Text can be selected and copied.
    pub(super) fn render_output(
        &self,
        shown: Shown,
        id: SharedString,
        max_width: f32,
        cell: Option<usize>,
        console_run: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let pages = cell.is_some();
        let text_id = id.clone();
        let text = move |s: String| {
            div()
                .w_full()
                .font_family(look::MONO)
                .text_size(px(look::CODE))
                .line_height(px(LINE_HEIGHT))
                .when(cell.is_some(), |d| d.cursor_text())
                .child(OutputText::new(
                    text_id.clone(),
                    s.trim_end_matches('\n').to_string(),
                ))
        };
        let note_id = id.clone();
        let note = move |s: String| {
            div()
                .text_color(p.faint)
                .when(cell.is_some(), |d| d.cursor_text())
                .child(OutputText::new(note_id.clone(), s))
                .into_any_element()
        };
        match shown {
            Shown::Text(s) => text(s).into_any_element(),
            Shown::Warning(s) => text(s).text_color(p.warn).into_any_element(),
            Shown::Error(s) => text(s).text_color(p.err).into_any_element(),
            Shown::Note(s) => note(s),
            Shown::Table(table) => self.render_table(table, id, cell, cx),
            Shown::Page(_) if !pages => note("A web page: see the Viewer.".into()),
            Shown::Page(page) => match self.pages.get(&page.key()) {
                Some(view) => div()
                    .w_full()
                    .h(px(PAGE_HEIGHT))
                    .rounded(px(RADIUS))
                    .overflow_hidden()
                    .child(view.clone())
                    .into_any_element(),
                None => note("Loading the page…".into()),
            },
            Shown::Plot { value, .. } => match self.plots.get(&plot_key(&value)) {
                Some(Plot::Ready(image)) => {
                    let key = plot_key(&value);
                    let group: SharedString = format!("{id}-plot").into();
                    let copied = image.clone();
                    let run = console_run.map(str::to_string);
                    let sent_run = run.clone();
                    let sent_key = key.clone();
                    let zoom_key = key.clone();
                    let zoom_run = run.clone();
                    let bounds_key = format!("{id}-image");
                    let zoom_bounds_key = bounds_key.clone();
                    let save_bounds_key = bounds_key.clone();
                    let tools = Some(
                        h_flex()
                            .absolute()
                            .top(px(6.))
                            .right(px(6.))
                            .gap(px(2.))
                            .p(px(2.))
                            .rounded(px(8.))
                            .bg(p.bg)
                            .border_1()
                            .border_color(p.line)
                            .invisible()
                            .group_hover(group.clone(), |d| d.visible())
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-zoom")),
                                    "zoom",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Enlarge the image")
                                        .build(w, cx)
                                })
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(ix) = cell {
                                            this.open_cell_plot(
                                                ix,
                                                &zoom_key,
                                                &zoom_bounds_key,
                                                window,
                                                cx,
                                            );
                                        } else if let Some(run) = &zoom_run {
                                            this.open_console_plot(
                                                run,
                                                &zoom_key,
                                                &zoom_bounds_key,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-save")),
                                    "doc.save",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Save the plot…")
                                        .build(w, cx)
                                })
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(ix) = cell {
                                            this.open_cell_plot(
                                                ix,
                                                &key,
                                                &save_bounds_key,
                                                window,
                                                cx,
                                            );
                                        } else if let Some(run) = &run {
                                            this.open_console_plot(
                                                run,
                                                &key,
                                                &save_bounds_key,
                                                window,
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("{id}-copy")),
                                    "edit.copy",
                                    &p,
                                    false,
                                )
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("Copy the plot")
                                        .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.copy_image(&copied, cx)),
                                ),
                            )
                            .when_some(sent_run, |d, run| {
                                d.child(
                                    icon_button(
                                        SharedString::from(format!("{id}-send")),
                                        "send",
                                        &p,
                                        false,
                                    )
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Send to Plots")
                                            .build(w, cx)
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.send_console_plot(&run, &sent_key, cx);
                                        },
                                    )),
                                )
                            }),
                    );
                    div()
                        .id(SharedString::from(format!("{id}-box")))
                        .group(group)
                        .relative()
                        .max_w(px(max_width))
                        .on_prepaint(self.track_plot_bounds(bounds_key, image.clone()))
                        // No wider than the notebook, however wide drawn.
                        .child(
                            img(image.clone())
                                .max_w(relative(1.))
                                .object_fit(ObjectFit::Contain),
                        )
                        .children(tools)
                        .into_any_element()
                }
                Some(Plot::Failed(err)) => note(format!("The plot could not be shown: {err}")),
                _ => note("Loading the plot…".into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::box_takes;
    use gpui_kit::px;
    #[test]
    fn outputs_hand_the_wheel_on_at_their_ends() {
        let max = px(300.);
        // At the top: up was skipped and goes to the notebook; down moved it.
        assert!(!box_takes(px(0.), max, px(10.)));
        assert!(box_takes(px(-10.), max, px(-10.)));
        // In the middle the box takes both ways.
        assert!(box_takes(px(-90.), max, px(10.)));
        assert!(box_takes(px(-110.), max, px(-10.)));
        // Past the bottom by a turn that moved it: kept.
        assert!(box_takes(px(-305.), max, px(-10.)));
        // At the bottom: down was skipped and goes on; up moved it.
        assert!(!box_takes(px(-300.), max, px(-10.)));
        assert!(box_takes(px(-290.), max, px(10.)));
        // A box that fits never takes it, nor a sideways turn.
        assert!(!box_takes(px(0.), px(0.), px(-10.)));
        assert!(!box_takes(px(-100.), max, px(0.)));
    }
}
