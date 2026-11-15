//! One docked source-only search session across the document's cell editors.
use crate::look::{self, Palette, bar_button, icon_button};
use crate::workbench::Workbench;
use gpui_kit::component::input::{
    EditorState, Input, InputEvent, InputState, RangeDecoration, RangeDecorationCollection,
    RangeDecorationStyle,
};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    pub cell: usize,
    pub range: Range<usize>,
}

fn word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Case conversion retains an explicit map back to original UTF-8 offsets.
/// A lowercase expansion must never produce a match for half a character.
pub(crate) fn matches(sources: &[String], query: &str, case: bool, whole: bool) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let needle = if case {
        query.to_string()
    } else {
        query.to_lowercase()
    };
    let mut result = Vec::new();
    for (cell, source) in sources.iter().enumerate() {
        let mut normalized = String::new();
        let mut boundaries = vec![None];
        for (offset, ch) in source.char_indices() {
            boundaries[normalized.len()] = Some(offset);
            if case {
                normalized.push(ch);
            } else {
                normalized.extend(ch.to_lowercase());
            }
            boundaries.resize(normalized.len() + 1, None);
            boundaries[normalized.len()] = Some(offset + ch.len_utf8());
        }
        let mut from = 0;
        while let Some(found) = normalized[from..].find(&needle) {
            let start = from + found;
            let end = start + needle.len();
            if let (Some(a), Some(b)) = (boundaries[start], boundaries[end])
                && (!whole
                    || (!source[..a].chars().next_back().is_some_and(word)
                        && !source[b..].chars().next().is_some_and(word)))
            {
                result.push(Match { cell, range: a..b });
            }
            from = end;
        }
    }
    result
}

pub(crate) struct Find {
    pub open: bool,
    pub replace: bool,
    pub case: bool,
    pub whole: bool,
    pub query: Entity<InputState>,
    pub replacement: Entity<InputState>,
    path: Option<String>,
    matches: Vec<Match>,
    current: usize,
    highlights: Vec<(Entity<EditorState>, RangeDecorationCollection)>,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    pub replaying: bool,
    navigated: bool,
}

struct Change {
    editor: Entity<EditorState>,
    before: String,
    after: String,
}
struct Transaction {
    path: String,
    changes: Vec<Change>,
}

pub(crate) fn new_find(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Find, Vec<Subscription>) {
    let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find in source"));
    let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with"));
    let change = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            this.find.current = 0;
            this.find.navigated = false;
            this.refresh_find(cx);
        }
    });
    let enter = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| {
        if let InputEvent::PressEnter { shift, .. } = event {
            this.find_next(*shift, window, cx);
        }
    });
    (
        Find {
            open: false,
            replace: false,
            case: false,
            whole: false,
            query,
            replacement,
            path: None,
            matches: Vec::new(),
            current: 0,
            highlights: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            replaying: false,
            navigated: false,
        },
        vec![change, enter],
    )
}

impl Workbench {
    pub(crate) fn sync_find_document(&mut self, cx: &mut Context<Self>) {
        if self
            .open
            .as_ref()
            .is_some_and(|open| open.notebook.truncated)
        {
            self.find.replace = false;
        }
        let path = self.open.as_ref().map(|open| open.notebook.path.clone());
        if path != self.find.path {
            self.find.path = path;
            self.find.current = 0;
            self.find.navigated = false;
            self.outline.open = false;
            self.refresh_find(cx);
        }
    }

    pub(crate) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        for (_, collection) in self.find.highlights.drain(..) {
            collection.dispose(cx);
        }
        self.find.matches.clear();
        if !self.find.open {
            cx.notify();
            return;
        }
        let Some(open) = &self.open else {
            return;
        };
        let sources: Vec<_> = open
            .editors
            .iter()
            .map(|editor| editor.read(cx).value().to_string())
            .collect();
        let query = self.find.query.read(cx).value();
        self.find.matches = matches(&sources, &query, self.find.case, self.find.whole);
        self.find.current = self
            .find
            .current
            .min(self.find.matches.len().saturating_sub(1));
        for (cell, editor) in open.editors.iter().enumerate() {
            let decorations = self
                .find
                .matches
                .iter()
                .filter(|matched| matched.cell == cell)
                .map(|matched| {
                    RangeDecoration::new(matched.range.clone())
                        .with_style(RangeDecorationStyle::Fill)
                })
                .collect();
            let collection = editor.update(cx, |editor, cx| {
                editor.create_range_decorations_collection(decorations, cx)
            });
            self.find.highlights.push((editor.clone(), collection));
        }
        cx.notify();
    }

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.open.as_ref().and_then(|open| {
            open.editors
                .iter()
                .find(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
                .map(|editor| editor.read(cx).selected_value())
        });
        if let Some(selected) = selected.filter(|value| !value.is_empty()) {
            self.find.current = 0;
            self.find.navigated = false;
            self.find
                .query
                .update(cx, |query, cx| query.set_value(selected, window, cx));
        }
        self.find.open = true;
        self.find.replace = replace
            && self
                .open
                .as_ref()
                .is_some_and(|open| !open.notebook.truncated);
        self.refresh_find(cx);
        self.find
            .query
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(crate) fn find_next(
        &mut self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.find.open && !self.find.query.read(cx).value().is_empty() {
            self.find.open = true;
            self.refresh_find(cx);
        }
        if self.find.matches.is_empty() {
            return;
        }
        let count = self.find.matches.len();
        // The selected occurrence is authoritative after a mouse selection or
        // an outline jump, not a stale per-cell engine index.
        let selected = self.open.as_ref().and_then(|open| {
            self.find.matches.iter().position(|matched| {
                let editor = open.editors[matched.cell].read(cx);
                editor.focus_handle(cx).is_focused(window)
                    && editor.selected_range() == matched.range
            })
        });
        self.find.current = match selected {
            Some(ix) if previous => (ix + count - 1) % count,
            Some(ix) => (ix + 1) % count,
            None if self.find.navigated => {
                if previous {
                    (self.find.current + count - 1) % count
                } else {
                    (self.find.current + 1) % count
                }
            }
            None if previous => count - 1,
            None => self.find.current.min(count - 1),
        };
        self.reveal_find(window, cx);
    }

    fn reveal_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(matched) = self.find.matches.get(self.find.current).cloned() else {
            return;
        };
        let line = self
            .open
            .as_ref()
            .map(|open| {
                open.editors[matched.cell].read(cx).value()[..matched.range.start]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
            })
            .unwrap_or(0);
        self.jump_to_source(matched.cell, line, Some(matched.range), window, cx);
        self.find.navigated = true;
    }

    fn replace_find(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.replaying {
            return;
        }
        self.refresh_find(cx);
        let Some(open) = &self.open else {
            return;
        };
        if open.notebook.truncated {
            return;
        }
        let replacement = self.find.replacement.read(cx).value();
        let selected: Vec<_> = if all {
            self.find.matches.clone()
        } else {
            self.find
                .matches
                .get(self.find.current)
                .cloned()
                .into_iter()
                .collect()
        };
        let mut changes = Vec::new();
        for (cell, editor) in open.editors.iter().enumerate() {
            let before = editor.read(cx).value().to_string();
            let mut after = before.clone();
            for matched in selected.iter().rev().filter(|matched| matched.cell == cell) {
                after.replace_range(matched.range.clone(), &replacement);
            }
            if after != before {
                changes.push(Change {
                    editor: editor.clone(),
                    before,
                    after,
                });
            }
        }
        let path = open.notebook.path.clone();
        if changes.is_empty() {
            return;
        }
        for change in &changes {
            change.editor.update(cx, |editor, cx| {
                editor.replace_all(change.after.clone(), window, cx)
            });
        }
        // Every editor records exactly one atomic native edit. Notebook Undo
        // replays those same native entries together, preserving earlier edits.
        if !open.single_editor() {
            self.find.undo.push(Transaction { path, changes });
            self.find.redo.clear();
        }
        if let Some(open) = self.open.as_mut() {
            open.dirty = true;
        }
        self.refresh_find(cx);
        if all && let Some(first) = selected.first() {
            self.jump_to_source(first.cell, 0, None, window, cx);
        }
        if !all {
            self.reveal_find(window, cx);
        }
    }

    pub(crate) fn find_keys(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        if self.objects.is_shown()
            || self.explorer_shown
            || self.diff_shown
            || self.media_shown.is_some()
        {
            return false;
        }
        let focused = self.focus.is_focused(window)
            || self.cells_focus.is_focused(window)
            || open
                .editors
                .iter()
                .any(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
            || self.find.query.read(cx).focus_handle(cx).is_focused(window)
            || self
                .find
                .replacement
                .read(cx)
                .focus_handle(cx)
                .is_focused(window);
        let key = &event.keystroke;
        if key.key == "escape" && self.find.open {
            self.find.open = false;
            self.refresh_find(cx);
            if let Some(open) = &self.open
                && let Some(editor) = open.editors.get(open.current.unwrap_or(0))
            {
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
            return true;
        }
        if !focused || !key.modifiers.platform || key.modifiers.control {
            return false;
        }
        if key.key.eq_ignore_ascii_case("f") && !key.modifiers.shift {
            self.open_find(key.modifiers.alt, window, cx);
            return true;
        }
        if key.key.eq_ignore_ascii_case("g") && !key.modifiers.alt {
            self.find_next(key.modifiers.shift, window, cx);
            return true;
        }
        if key.key.eq_ignore_ascii_case("z") && !key.modifiers.alt {
            return self.notebook_find_history(key.modifiers.shift, window, cx);
        }
        false
    }

    pub(crate) fn notebook_find_history(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.find.replaying {
            return false;
        }
        if !self.cells_focus.is_focused(window)
            && !self.open.as_ref().is_some_and(|open| {
                open.editors
                    .iter()
                    .any(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
            })
        {
            return false;
        }
        let Some(open) = &self.open else {
            return false;
        };
        if open.notebook.truncated {
            return false;
        }
        let stack = if redo {
            &self.find.redo
        } else {
            &self.find.undo
        };
        let Some(index) = stack
            .iter()
            .rposition(|transaction| transaction.path == open.notebook.path)
        else {
            return false;
        };
        let transaction = &stack[index];
        if transaction.changes.iter().any(|change| {
            !open.editors.contains(&change.editor)
                || &*change.editor.read(cx).value()
                    != if redo {
                        change.before.as_str()
                    } else {
                        change.after.as_str()
                    }
        }) {
            return false;
        }
        self.find.replaying = true;
        cx.notify();
        let this = cx.entity().downgrade();
        // Virtualized and rendered-Markdown cells must mount their editors
        // before native Undo can be sent to their focus handles.
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let stack = if redo {
                        &this.find.redo
                    } else {
                        &this.find.undo
                    };
                    let ready = stack.get(index).is_some_and(|transaction| {
                        this.open
                            .as_ref()
                            .is_some_and(|open| open.notebook.path == transaction.path)
                            && transaction.changes.iter().all(|change| {
                                this.open
                                    .as_ref()
                                    .is_some_and(|open| open.editors.contains(&change.editor))
                                    && &*change.editor.read(cx).value()
                                        == if redo {
                                            change.before.as_str()
                                        } else {
                                            change.after.as_str()
                                        }
                            })
                    });
                    if !ready {
                        this.find.replaying = false;
                        cx.notify();
                        return;
                    }
                    let transaction = if redo {
                        this.find.redo.remove(index)
                    } else {
                        this.find.undo.remove(index)
                    };
                    for change in &transaction.changes {
                        let focus = change.editor.read(cx).focus_handle(cx);
                        if redo {
                            focus.dispatch_action(&gpui_kit::component::input::Redo, window, cx);
                        } else {
                            focus.dispatch_action(&gpui_kit::component::input::Undo, window, cx);
                        }
                    }
                    if redo {
                        this.find.undo.push(transaction);
                    } else {
                        this.find.redo.push(transaction);
                    }
                    this.find.replaying = false;
                    this.refresh_find(cx);
                });
            });
        });
        true
    }

    pub(crate) fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.find.open {
            return None;
        }
        let p = *Palette::get(cx);
        let count = self.find.matches.len();
        let row = h_flex()
            .flex_wrap()
            .gap(px(4.))
            .w_full()
            .child(
                div()
                    .flex_1()
                    .min_w(px(60.))
                    .child(Input::new(&self.find.query).small()),
            )
            .child(div().flex_none().text_color(p.muted).child(format!(
                "{} of {}",
                if count == 0 { 0 } else { self.find.current + 1 },
                count
            )))
            .child(
                bar_button("find-case", "Aa", None, &p)
                    .when(self.find.case, |d| d.bg(p.ink(10.)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.find.case = !this.find.case;
                        this.refresh_find(cx);
                    })),
            )
            .child(
                bar_button("find-word", "Word", None, &p)
                    .when(self.find.whole, |d| d.bg(p.ink(10.)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.find.whole = !this.find.whole;
                        this.refresh_find(cx);
                    })),
            )
            .child(
                icon_button("find-prev", "arrow.up", &p, false)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Previous match (⇧⌘G)")
                            .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.find_next(true, window, cx))),
            )
            .child(
                icon_button("find-next", "chevron.down", &p, false)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Next match (⌘G)").build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.find_next(false, window, cx))),
            )
            .child(
                icon_button("find-close", "close", &p, false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.find.open = false;
                        this.refresh_find(cx);
                    },
                )),
            );
        Some(
            v_flex()
                .flex_none()
                .gap(px(4.))
                .p(px(6.))
                .border_b_1()
                .border_color(p.line)
                .text_size(px(look::SMALL))
                .child(row)
                .when(
                    self.find.replace
                        && self
                            .open
                            .as_ref()
                            .is_some_and(|open| !open.notebook.truncated),
                    |d| {
                        d.child(
                            h_flex()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&self.find.replacement).small()),
                                )
                                .child(bar_button("find-replace", "Replace", None, &p).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.replace_find(false, window, cx)
                                    }),
                                ))
                                .child(
                                    bar_button("find-replace-all", "Replace all", None, &p)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.replace_find(true, window, cx)
                                        })),
                                ),
                        )
                    },
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Match, matches};
    #[test]
    fn whole_word_unicode_and_case() {
        let source = vec!["cat cats Cat _cat cat_ 猫cat cat猫 café CAFÉ".into()];
        assert_eq!(
            matches(&source, "cat", false, true)
                .iter()
                .map(|m| m.range.clone())
                .collect::<Vec<_>>(),
            vec![0..3, 9..12]
        );
        assert_eq!(matches(&source, "cat", true, true).len(), 1);
        assert_eq!(matches(&source, "café", false, true).len(), 2);
        assert!(matches(&["İ".into()], "i", false, false).is_empty());
    }
    #[test]
    fn global_index_preserves_cell_and_byte_offsets() {
        let sources = vec!["é x x".into(), "".into(), "x".into()];
        assert_eq!(
            matches(&sources, "x", true, false),
            vec![
                Match {
                    cell: 0,
                    range: 3..4
                },
                Match {
                    cell: 0,
                    range: 5..6
                },
                Match {
                    cell: 2,
                    range: 0..1
                }
            ]
        );
        assert!(matches(&sources, "", true, false).is_empty());
    }
}
