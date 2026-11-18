//! CSV/TSV opening choices and lossless rainbow text editing.
//! Parsing supplies decoration ranges only. The Editor remains the sole source
//! of saved text; separators, quotes, BOMs and line endings are never rewritten.
use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, RopeExt, TextDecoration};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, FileBytes, SessionInfo, SessionStatus};

use crate::look::{self, Palette};
use crate::workbench::{Open, Workbench};

pub(crate) const TEXT_LIMIT: u64 = 5_000_000;

pub(crate) struct DelimitedDialog {
    request: u64,
    path: String,
    rel: String,
    size: Option<u64>,
    sessions: Vec<SessionInfo>,
    selected: Option<String>,
    choice: Choice,
    loading: bool,
    error: Option<String>,
}

/// What the dialog's main button does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Text,
    Import,
}

/// Up to 5 MB the text opens editable, so it comes first; above that it
/// is a read-only start, and a live session can read the whole file.
fn first_choice(size: Option<u64>, sessions: usize) -> Choice {
    if size.is_some_and(|size| size > TEXT_LIMIT) && sessions > 0 {
        Choice::Import
    } else {
        Choice::Text
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Field {
    range: Range<usize>,
    column: usize,
    header: bool,
}

fn delimiter(path: &str) -> u8 {
    let path = path.to_ascii_lowercase();
    if path.ends_with(".tsv") || path.ends_with(".tab") {
        b'\t'
    } else {
        b','
    }
}

/// Quote-aware logical records with byte ranges into the original UTF-8 text.
/// A quote only opens at the start of a field; doubled quotes stay in that field.
fn fields(text: &str, sep: u8) -> (Vec<Field>, usize) {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = if text.starts_with('\u{feff}') { 3 } else { 0 };
    let mut field_start = i;
    let mut column = 0;
    let mut quoted = false;
    let mut header = true;
    let mut header_end = text.len();
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            if quoted && bytes.get(i + 1) == Some(&b'"') {
                i += 2;
                continue;
            }
            if quoted || i == field_start {
                quoted = !quoted;
            }
        } else if !quoted && (b == sep || b == b'\r' || b == b'\n') {
            out.push(Field {
                range: start..i,
                column,
                header,
            });
            if b == sep {
                column += 1;
            } else {
                if header {
                    header_end = i;
                    header = false;
                }
                column = 0;
                if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            start = i + 1;
            field_start = start;
        }
        i += 1;
    }
    out.push(Field {
        range: start..text.len(),
        column,
        header,
    });
    (out, header_end)
}

fn column_color(p: &Palette, column: usize) -> Hsla {
    p.ansi[[4, 2, 5, 6, 3, 1][column % 6]]
}

fn decorations(text: &str, sep: u8, p: &Palette) -> (Vec<TextDecoration>, usize) {
    let (fields, header_end) = fields(text, sep);
    let marks = fields
        .into_iter()
        .filter(|f| !f.range.is_empty())
        .map(|f| {
            TextDecoration::new(
                f.range,
                HighlightStyle {
                    color: Some(column_color(p, f.column)),
                    background_color: f.header.then_some(p.chrome),
                    ..Default::default()
                },
            )
        })
        .collect();
    (marks, header_end)
}

/// A delimited document's separator, by its colouring (`notebook::COLOURINGS`).
pub(crate) fn separator(language: Option<&str>) -> u8 {
    if language == Some("csv") { b',' } else { b'\t' }
}

/// How many columns a delimited document has: the fields of its first
/// logical record (the pinned header).
pub(crate) fn column_count(header: &str, sep: u8) -> usize {
    let (fields, _) = fields(header, sep);
    fields
        .iter()
        .filter(|field| field.header)
        .map(|field| field.column + 1)
        .max()
        .unwrap_or(0)
}

/// How many fields of `line` the bytes `start..end` touch: separators
/// outside quotes inside the selection, plus one; a separator it ends on
/// starts no new field. Quotes are followed from the line's start.
pub(crate) fn fields_spanned(line: &str, start: usize, end: usize, sep: u8) -> usize {
    let mut quoted = false;
    let mut spanned = 1;
    for (at, &byte) in line.as_bytes().iter().enumerate().take(end) {
        if byte == b'"' {
            quoted = !quoted;
        } else if byte == sep && !quoted && at >= start && at + 1 < end {
            spanned += 1;
        }
    }
    spanned
}

pub(crate) fn decorate_editor(
    editor: &Entity<EditorState>,
    sep: u8,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<EditorState>, Vec<Subscription>) {
    let p = *Palette::get(cx);
    let text = editor.read(cx).value().to_string();
    let (marks, header_end) = decorations(&text, sep, &p);
    let header = cx.new(|cx| {
        EditorState::new(window, cx)
            .language("plain")
            .line_number(false)
            .folding(false)
            .soft_wrap(false)
            .scroll_beyond_last_line(Some(0))
            .default_value(text[..header_end].to_string())
    });
    let collection = editor.update(cx, |state, cx| {
        state.create_decorations_collection(marks, cx)
    });
    let header_marks = header.update(cx, |state, cx| {
        state.create_decorations_collection(decorations(&text[..header_end], sep, &p).0, cx)
    });
    let theme_body = editor.clone();
    let theme_header = header.clone();
    let theme_collection = collection.clone();
    let theme_header_marks = header_marks.clone();
    let theme = cx.observe_global::<Palette>(move |_, cx| {
        let p = *Palette::get(cx);
        theme_collection.set(
            decorations(theme_body.read(cx).value().as_ref(), sep, &p).0,
            cx,
        );
        theme_header_marks.set(
            decorations(theme_header.read(cx).value().as_ref(), sep, &p).0,
            cx,
        );
    });
    let pinned = header.clone();
    let changes = cx.subscribe_in(
        editor,
        window,
        move |_, editor, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                let p = *Palette::get(cx);
                let text = editor.read(cx).value().to_string();
                let (marks, header_end) = decorations(&text, sep, &p);
                collection.set(marks, cx);
                pinned.update(cx, |state, cx| {
                    state.set_value(text[..header_end].to_string(), window, cx)
                });
                header_marks.set(decorations(&text[..header_end], sep, &p).0, cx);
            }
        },
    );
    // Scrolling emits an entity notification, not InputEvent::Change.
    let scrolling = cx.observe(editor, |_, _, cx| cx.notify());
    (header, vec![changes, scrolling, theme])
}

/// Keep machine IDs exclusively in selection state, never in visible labels.
fn import_session_label(
    session: &SessionInfo,
    sessions: &[SessionInfo],
    specs: &[sandhill_proto::KernelspecInfo],
) -> String {
    let mut label = crate::panes::machine::session_label(session, specs);
    let mut total = 0;
    let mut ordinal = 0;
    for candidate in sessions {
        if candidate.language.eq_ignore_ascii_case(&session.language) {
            total += 1;
            if candidate.id == session.id {
                ordinal = total;
            }
        }
    }
    if total > 1 {
        label.push_str(&format!(" · Session {ordinal}"));
    }
    let context = session
        .project
        .as_deref()
        .filter(|project| !project.is_empty())
        .and_then(|project| {
            let name = crate::panes::files::base_name(project);
            if session.cwd == project {
                Some(name.to_string())
            } else {
                session
                    .cwd
                    .strip_prefix(&format!("{project}/"))
                    .map(|rel| format!("{name}/{rel}"))
            }
        })
        .unwrap_or_else(|| session.cwd.clone());
    if !context.is_empty() {
        label.push_str(" · ");
        label.push_str(&context);
    }
    if session.pending_runs > 0 {
        label.push_str(" (busy)");
    }
    label
}

impl Workbench {
    pub(crate) fn offer_delimited(&mut self, rel: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let path = format!("{project}/{rel}");
        let request = self.next_document_load;
        self.next_document_load += 1;
        self.delimited = Some(DelimitedDialog {
            request,
            path: path.clone(),
            rel,
            size: None,
            sessions: Vec::new(),
            selected: None,
            choice: Choice::Text,
            loading: true,
            error: None,
        });
        let generation = self.generation;
        // The original byte API reports full size even on daemons whose
        // StatFiles response predates its optional size field.
        let stat = daemon.call::<FileBytes>(Call::ReadBytes {
            path: path.clone(),
            offset: 0,
            limit: Some(0),
        });
        let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let stat = stat.await;
            let sessions = sessions.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let Some(dialog) = &mut this.delimited else {
                    return;
                };
                if dialog.request != request {
                    return;
                }
                dialog.loading = false;
                match stat {
                    Ok(stat) if stat.offset == 0 && stat.data.is_empty() => {
                        dialog.size = Some(stat.size);
                    }
                    Ok(_) => {
                        dialog.error = Some("Invalid file metadata response".into());
                    }
                    Err(err) => dialog.error = Some(format!("Could not inspect file: {err:#}")),
                }
                match sessions {
                    Ok(sessions) => {
                        dialog.sessions = sessions
                            .into_iter()
                            .filter(|s| {
                                s.status == SessionStatus::Running
                                    && matches!(
                                        s.language.to_ascii_lowercase().as_str(),
                                        "r" | "python"
                                    )
                            })
                            .collect();
                        dialog.selected = dialog.sessions.first().map(|s| s.id.clone());
                    }
                    Err(err) => {
                        dialog.error = Some(format!("Could not list live sessions: {err:#}"))
                    }
                }
                dialog.choice = first_choice(dialog.size, dialog.sessions.len());
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn open_delimited_text(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.delimited else {
            return;
        };
        if dialog.loading || dialog.size.is_none() {
            return;
        }
        let Some(daemon) = self.daemon() else { return };
        let path = dialog.path.clone();
        let rel = dialog.rel.clone();
        let request = dialog.request;
        let project = self.project.clone();
        let generation = self.generation;
        self.delimited.as_mut().unwrap().loading = true;
        let read =
            crate::editor::documents::read_text_preview(daemon, path.clone(), project.clone());
        cx.spawn(async move |this, cx| {
            let read = read.await.and_then(|(file, truncated)| {
                let mut nb = crate::editor::notebook::parse(&file.path, &file.text)?;
                nb.truncated = truncated;
                Ok((nb, file.mtime_ms))
            });
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation || this.project != project {
                    return;
                }
                if this.delimited.as_ref().is_none_or(|d| d.request != request) {
                    return;
                }
                match read {
                    Ok((nb, mtime)) => {
                        this.delimited = None;
                        if this.open_at(&nb.path).is_some() {
                            this.show_file_tab(&nb.path, cx);
                        } else {
                            this.put_away();
                            this.explorer_shown = false;
                            this.diff_shown = false;
                            this.tabs.push(nb.path.clone());
                            this.notebook_scroll.set_offset(Point::default());
                            this.show_notebook(nb, mtime, window, cx);
                            this.session_changed(cx);
                        }
                    }
                    Err(err) => {
                        if let Some(dialog) = &mut this.delimited {
                            dialog.loading = false;
                            dialog.error = Some(format!("Opening {rel} failed: {err:#}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_delimited(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(editor) = open.editors.first() else {
            return div().into_any_element();
        };
        let Some(header) = &open.delimited_header else {
            return div().into_any_element();
        };
        let offset = editor.read(cx).scroll_offset();
        let rows = header.read(cx).text().lines_len();
        // Lay out the pinned editor without a horizontal viewport of its own.
        // Its wrapper applies the body's offset, even past a short header's end.
        let width = header.read(cx).text().len() as f32 * look::code() + 100.;
        div()
            .size_full()
            .min_h_0()
            .relative()
            .when(offset.y >= px(0.), |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(rows as f32 * look::code() * 1.5 + 8.))
                        .bg(p.chrome),
                )
            })
            .child(
                Editor::new(editor)
                    .readonly(open.notebook.truncated)
                    .h(relative(1.))
                    .appearance(false)
                    .bordered(false)
                    .p(px(8.))
                    .font_family(look::mono())
                    .text_size(px(look::code())),
            )
            // At the top, the real first record is editable and visible once.
            // After vertical scrolling, its read view covers the scrolled body
            // rather than consuming another row or shifting its viewport.
            .when(offset.y < px(0.), |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(rows as f32 * look::code() * 1.5 + 16.))
                        .overflow_hidden()
                        .bg(p.chrome)
                        .border_b_1()
                        .border_color(p.line)
                        .child(
                            div().relative().left(offset.x).h_full().w(px(width)).child(
                                Editor::new(header)
                                    .h(relative(1.))
                                    .readonly(true)
                                    .appearance(false)
                                    .bordered(false)
                                    .p(px(8.))
                                    .bg(p.chrome)
                                    .font_family(look::mono())
                                    .text_size(px(look::code())),
                            ),
                        ),
                )
            })
            .into_any_element()
    }

    pub(crate) fn select_delimited_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.delimited
            && dialog.sessions.iter().any(|session| session.id == id)
        {
            dialog.selected = Some(id.to_string());
            cx.notify();
        }
    }

    pub(crate) fn import_delimited_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.delimited.as_ref().and_then(|d| {
            if d.loading {
                return None;
            }
            let session = d
                .sessions
                .iter()
                .find(|session| Some(session.id.as_str()) == d.selected.as_deref())?;
            Some((d.path.clone(), session.clone()))
        });
        if let Some((path, session)) = chosen {
            self.delimited = None;
            self.open_preselected_import(path, session, window, cx);
        }
    }

    fn delimited_import_code(&self) -> Option<String> {
        let dialog = self.delimited.as_ref()?;
        let session = dialog
            .sessions
            .iter()
            .find(|s| Some(s.id.as_str()) == dialog.selected.as_deref())?;
        let language = session.language.to_ascii_lowercase();
        let kind = crate::editor::imports::imports_for(&language).first()?;
        Some(crate::editor::imports::generated_import_code(
            &language,
            kind,
            &dialog.path,
            Some(&session.cwd),
            &crate::editor::imports::variable_name(&dialog.path),
        ))
    }

    fn copy_delimited_import_code(&mut self, cx: &mut Context<Self>) {
        let Some(code) = self.delimited_import_code() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.notice = Some("Copied import code.".into());
        cx.notify();
    }

    fn insert_delimited_import_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(code) = self.delimited_import_code() else {
            return;
        };
        if self.insert_into_active_document(&code, window, cx) {
            self.delimited = None;
            cx.notify();
        }
    }

    fn choose_delimited(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.delimited
            && (choice == Choice::Text || !dialog.sessions.is_empty())
        {
            dialog.choice = choice;
            cx.notify();
        }
    }

    pub(crate) fn render_delimited_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.delimited.as_ref()?;
        let p = *Palette::get(cx);
        let text_ok = !dialog.loading && dialog.size.is_some();
        let import_ok = !dialog.loading && dialog.selected.is_some();
        let hover = p.ink(8.);
        let name = crate::panes::files::base_name(&dialog.rel).to_string();
        let mut facts = Vec::new();
        if let Some((folder, _)) = dialog.rel.rsplit_once('/') {
            facts.push(folder.to_string());
        }
        facts.push(match dialog.size {
            Some(size) => crate::git::file_size(size),
            None if dialog.loading => "Checking…".into(),
            None => "Size unavailable".into(),
        });
        facts.push(
            if delimiter(&dialog.path) == b'\t' {
                "Tab-separated"
            } else {
                "Comma-separated"
            }
            .into(),
        );
        let large = dialog.size.is_some_and(|size| size > TEXT_LIMIT);
        let no_sessions = !dialog.loading && dialog.sessions.is_empty();
        // One selectable row per choice: a radio mark, a title, one line.
        let option = |id: &'static str, choice: Choice, title: &'static str, detail: String| {
            let picked = dialog.choice == choice;
            let enabled = choice == Choice::Text || !no_sessions;
            v_flex()
                .id(id)
                .w_full()
                .p(px(look::GAP_8))
                .gap(px(look::GAP_2))
                .rounded(px(look::CONTROL_RADIUS))
                .border_1()
                .border_color(if picked { p.accent } else { p.line })
                .when(!enabled, |d| d.opacity(0.5))
                .when(enabled, |d| {
                    d.cursor_pointer().hover(move |d| d.bg(hover)).on_click(
                        cx.listener(move |this, _, _, cx| this.choose_delimited(choice, cx)),
                    )
                })
                .child(
                    h_flex()
                        .gap(px(look::GAP_4))
                        .child(
                            div()
                                .size(px(12.))
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(if picked { p.accent } else { p.muted })
                                .when(picked, |d| d.bg(p.accent)),
                        )
                        .child(div().text_color(p.text).child(title)),
                )
                .child(div().pl(px(16.)).text_color(p.muted).child(detail))
        };
        let sessions = v_flex()
            .id("delimited-sessions")
            .pl(px(20.))
            .pt(px(look::GAP_6))
            .gap(px(look::GAP_2))
            .max_h(px(180.))
            .overflow_y_scroll()
            .children(dialog.sessions.iter().enumerate().map(|(i, session)| {
                let id = session.id.clone();
                let selected = dialog.selected.as_deref() == Some(id.as_str());
                let label = import_session_label(session, &dialog.sessions, &self.kernels);
                let language = if session.language.eq_ignore_ascii_case("r") {
                    "lang/r"
                } else {
                    "lang/python"
                };
                h_flex()
                    .id(("delimited-session", i))
                    .w_full()
                    .min_w_0()
                    .h(px(look::row()))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(p.sel))
                    .hover(move |d| d.bg(hover))
                    .child(look::picture(language))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.text)
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_delimited_session(&id, cx);
                    }))
            }));
        let link = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .text_color(if enabled { p.accent } else { p.faint })
                .when(enabled, |d| d.cursor_pointer())
                .child(label)
        };
        let import_detail = if no_sessions {
            "No live R or Python session. Start one in the Console first.".to_string()
        } else {
            "Reads the whole file into R or Python.".to_string()
        };
        let import = option(
            "delimited-choose-import",
            Choice::Import,
            "Import into a live session",
            import_detail,
        )
        .when(dialog.choice == Choice::Import, |d| {
            d.child(sessions).child(
                h_flex()
                    .pl(px(20.))
                    .pt(px(look::GAP_4))
                    .gap(px(look::GAP_8))
                    .child(
                        link("delimited-copy", "Copy Code", import_ok).when(import_ok, |d| {
                            d.on_click(
                                cx.listener(|this, _, _, cx| this.copy_delimited_import_code(cx)),
                            )
                        }),
                    )
                    .child({
                        let enabled = import_ok && self.can_insert_into_active_document(cx);
                        link("delimited-insert", "Insert into Document", enabled).when(
                            enabled,
                            |d| {
                                d.on_click(cx.listener(|this, _, window, cx| {
                                    this.insert_delimited_import_code(window, cx)
                                }))
                            },
                        )
                    }),
            )
        });
        let (primary, primary_ok) = match dialog.choice {
            Choice::Text => ("Open Text", text_ok),
            Choice::Import => ("Import…", import_ok),
        };
        let card = v_flex()
            .id("delimited-card")
            .occlude()
            .w(px(520.))
            .max_h(relative(0.9))
            .p(px(20.))
            .gap(px(look::GAP_12))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap(px(look::GAP_4))
                    .pb(px(look::GAP_4))
                    .child(look::picture(&look::file_icon(&dialog.path)).size(px(look::ICON_LARGE)))
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(px(look::title()))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_color(p.muted)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(facts.join(" · ")),
                            ),
                    ),
            )
            .child(option(
                "delimited-choose-text",
                Choice::Text,
                "View as text",
                if large {
                    "First 5 MB, read-only.".into()
                } else {
                    "Edit the file.".into()
                },
            ))
            .child(import)
            .when_some(dialog.error.clone(), |d, error| {
                d.child(div().text_color(p.err).child(error))
            })
            .child(
                h_flex()
                    .pt(px(look::GAP_6))
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .child(
                        Button::new("delimited-cancel")
                            .small()
                            .h(px(look::control_card()))
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delimited = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("delimited-open")
                            .small()
                            .h(px(look::control_card()))
                            .primary()
                            .label(primary)
                            .disabled(!primary_ok)
                            .on_click(cx.listener(|this, _, window, cx| {
                                match this.delimited.as_ref().map(|d| d.choice) {
                                    Some(Choice::Text) => this.open_delimited_text(cx),
                                    Some(Choice::Import) => this.import_delimited_file(window, cx),
                                    None => {}
                                }
                            })),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("delimited-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.delimited = None;
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Field, fields, fields_spanned};

    #[test]
    fn selected_columns_follow_quotes_and_ignore_a_final_separator() {
        let line = "gene,\"a,b\",count,x";
        // "gene,\"a,b\",count" spans three fields; the comma inside quotes is data.
        assert_eq!(fields_spanned(line, 0, 16, b','), 3);
        // Ending on the comma after "gene" stays in one field.
        assert_eq!(fields_spanned(line, 0, 5, b','), 1);
        // A start inside a quoted field still knows it is quoted.
        assert_eq!(fields_spanned(line, 7, 10, b','), 1);
    }

    #[test]
    fn quote_aware_ranges_preserve_separators_and_logical_header() {
        let raw = "\u{feff}\"雪,\"\"ice\"\"\",\"two\nlines\",\r\n1,,\"三\"\r\n";
        let (parsed, end) = fields(raw, b',');
        assert_eq!(&raw[..end], "\u{feff}\"雪,\"\"ice\"\"\",\"two\nlines\",");
        assert_eq!(
            parsed
                .iter()
                .map(|f| (&raw[f.range.clone()], f.column, f.header))
                .collect::<Vec<_>>(),
            vec![
                ("\u{feff}\"雪,\"\"ice\"\"\"", 0, true),
                ("\"two\nlines\"", 1, true),
                ("", 2, true),
                ("1", 0, false),
                ("", 1, false),
                ("\"三\"", 2, false),
                ("", 0, false),
            ]
        );
    }

    #[test]
    fn tsv_empty_and_unterminated_edits_are_supported() {
        assert_eq!(
            fields("", b'\t'),
            (
                vec![Field {
                    range: 0..0,
                    column: 0,
                    header: true
                }],
                0
            )
        );
        let raw = "a\tb\n\"x\ty\nunfinished";
        let (parsed, end) = fields(raw, b'\t');
        assert_eq!(end, 3);
        assert_eq!(
            &raw[parsed.last().unwrap().range.clone()],
            "\"x\ty\nunfinished"
        );
        assert_eq!(parsed.last().unwrap().column, 0);
    }
}
