//! Compare a notebook cell with the code that produced its latest output.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{EditorState, Rope, RopeLines};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::editor::cell_diff;
use crate::editor::cells::CellStatus;
use crate::look::{self, Palette};
use crate::workbench::Run;

pub(super) fn changed_source<'a>(
    run: Option<&'a Run>,
    status: &CellStatus,
    current: &Rope,
) -> Option<&'a SharedString> {
    if matches!(status, CellStatus::Queued | CellStatus::Running(_)) {
        return None;
    }
    let before = run?.code.as_ref()?;
    // Run lines are built only for cells near the viewport. Compare their
    // rope to the journaled text without allocating a copy of the editor.
    (!same_code(current, before)).then_some(before)
}

fn same_code(current: &Rope, before: &str) -> bool {
    if current == before {
        return true;
    }
    let mut current = RopeLines::new(current);
    for old in before.trim_end().lines().map(str::trim_end) {
        let Some(line) = current.next() else {
            return false;
        };
        let last = line
            .chars_at(line.len())
            .reversed()
            .take_while(|c| c.is_whitespace())
            .fold(line.len(), |len, c| len - c.len_utf8());
        if line.slice(..last) != old {
            return false;
        }
    }
    current.all(|line| line.chars().all(char::is_whitespace))
}

pub(super) fn trigger(editor: Entity<EditorState>, before: SharedString, p: &Palette) -> Popover {
    let button = Button::new("edited-run-button")
        .text()
        .tooltip("The code changed after this output was made. Run the cell to update it.")
        .accessibility_label("Edited since run")
        .child(
            div()
                .text_size(px(look::small()))
                .text_color(p.warn)
                .child("· edited since run"),
        );
    Popover::new("edited-run")
        .appearance(false)
        .trigger(button)
        .content(move |_, window, cx| {
            // Keep this comparison for the lifetime of the popup. The closed
            // run line only checks equality, so scrolling never builds diffs.
            let rows = window.use_keyed_state("edited-run-rows", cx, |_, cx| {
                cell_diff::rows(&before, &editor.read(cx).value()).collect::<Vec<_>>()
            });
            let p = *Palette::get(cx);
            crate::agents::composer_view::popup(&p)
                .w(px(400.).min(window.viewport_size().width - px(2. * look::GAP_8)))
                .font_family(look::ui_font())
                .child(
                    h_flex()
                        .gap(px(look::GAP_12))
                        .px(px(look::GAP_8))
                        .text_color(p.muted)
                        .child("- Code that ran")
                        .child("+ Current code"),
                )
                .child(
                    div()
                        .id("edited-run-scroll")
                        .max_h(px(240.).min(window.viewport_size().height / 2.))
                        .overflow_scroll()
                        .restrict_scroll_to_axis()
                        .font_family(look::mono())
                        .child(
                            v_flex()
                                .min_w_full()
                                .children(rows.read(cx).iter().map(|row| row.render(&p))),
                        ),
                )
        })
}

#[cfg(test)]
mod tests {
    use super::{CellStatus, Rope, Run, changed_source};
    use crate::editor::notebook;

    #[test]
    fn reports_edits_but_not_undo_or_trailing_whitespace() {
        let mut run = Run::default();
        run.code = Some("if True: \n    print('é')\t\n\n".into());
        run.finished = true;
        let status = CellStatus::Done(None);
        let label =
            |source: &str| changed_source(Some(&run), &status, &Rope::from(source)).is_some();

        assert!(!label("if True: \n    print('é')\t\n\n"));
        assert!(!label("if True:\r\n    print('é')"));
        assert!(!label("if True:\n    print('é')\u{2003}\n  \n\n"));
        assert!(label("if True:\n    print('edited')"));
        assert!(label("if True:\n  print('é')"));
        assert!(label("if True:\n\n    print('é')"));
        assert!(!label("if True:\n    print('é')"));
    }

    #[test]
    fn unknown_code_does_not_mark_a_notebooks_saved_jupyter_output() {
        let notebook = notebook::parse("outside.ipynb", r#"{
            "nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": [{
                "id": "a", "cell_type": "code", "metadata": {}, "source": "print(7)",
                "execution_count": 1, "outputs": [{"output_type": "stream", "name": "stdout", "text": "3\n"}]
            }]
        }"#).unwrap();
        let cell = &notebook.cells[0];
        assert_eq!(cell.outputs[0]["text"], "3\n");
        let current = Rope::from(cell.source.as_str());
        assert!(changed_source(None, &CellStatus::Idle, &current).is_none());
        let mut missing_code = Run::default();
        missing_code.finished = true;
        assert!(changed_source(Some(&missing_code), &CellStatus::Done(None), &current).is_none());
    }

    #[test]
    fn a_pending_run_hides_the_label() {
        let mut run = Run::default();
        run.code = Some("print(1)".into());
        let source = Rope::from("print(2)");
        for status in [CellStatus::Queued, CellStatus::Running(None)] {
            assert!(changed_source(Some(&run), &status, &source).is_none());
        }
    }
}
