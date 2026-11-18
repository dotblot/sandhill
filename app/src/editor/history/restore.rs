use std::collections::{HashMap, HashSet};
use std::mem;

use gpui_kit::component::input::EditorState;
use gpui_kit::*;

use crate::editor::notebook::{CellKind, Format, Notebook, format_of};
use crate::workbench::{Open, Workbench};

#[derive(Default)]
pub(crate) struct NotebookHistoryEdits {
    undo: Vec<Restore>,
    redo: Vec<Restore>,
    pub clear_outputs: HashSet<String>,
}

/// Whole-notebook edits can insert, remove and reorder cells. Swap the
/// editors as a group, retaining native edit history on either side.
struct Shape {
    notebook: Notebook,
    editors: Vec<Entity<EditorState>>,
    subscriptions: Vec<Subscription>,
    cell_runs: HashMap<String, String>,
    clear_outputs: HashSet<String>,
    mtime_ms: i64,
    dirty: bool,
}

impl Shape {
    fn swap(self, open: &mut Open) -> Self {
        open.current = None;
        open.editing.clear();
        open.agent_changes.clear();
        open.agent_deleted.clear();
        open.deleted.clear();
        open.lit = None;
        open.ghost = None;
        open.ghost_after = None;
        let ids: HashSet<_> = self.notebook.cells.iter().map(|c| c.id.as_str()).collect();
        open.sending.retain(|id| ids.contains(id.as_str()));
        open.waiting.retain(|id| ids.contains(id.as_str()));
        let dirty = self.dirty || self.mtime_ms != open.mtime_ms;
        Self {
            notebook: mem::replace(&mut open.notebook, self.notebook),
            editors: mem::replace(&mut open.editors, self.editors),
            subscriptions: mem::replace(&mut open._subscriptions, self.subscriptions),
            cell_runs: mem::replace(&mut open.cell_runs, self.cell_runs),
            clear_outputs: mem::replace(&mut open.history_edits.clear_outputs, self.clear_outputs),
            mtime_ms: open.mtime_ms,
            dirty: mem::replace(&mut open.dirty, dirty),
        }
    }
}

struct Restore {
    alternate: Shape,
    expected: Vec<(String, CellKind, String)>,
}

fn document_key(open: &Open, cx: &App) -> Vec<(String, CellKind, String)> {
    open.notebook
        .cells
        .iter()
        .zip(&open.editors)
        .map(|(cell, editor)| {
            (
                cell.id.clone(),
                cell.kind,
                editor.read(cx).value().to_string(),
            )
        })
        .collect()
}

impl Workbench {
    pub(super) fn restore_file_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.history.as_ref() else {
            return;
        };
        let Some(version) = tab.details.as_ref() else {
            return;
        };
        let (path, text) = (tab.path.clone(), version.text.clone());
        if self.open_at(&path).is_some() {
            self.show_file_tab(&path, cx);
            self.apply_file_version(text, window, cx);
            return;
        }
        let Some(rel) = self
            .project
            .as_deref()
            .and_then(|project| path.strip_prefix(&format!("{project}/")))
            .map(str::to_owned)
        else {
            self.problem =
                Some("Could not open this history's file in the current project.".into());
            cx.notify();
            return;
        };
        self.pending_history = Some((path.clone(), text));
        if format_of(&path) == Some(Format::Delimited) && !self.document_loads.contains_key(&path) {
            // Restore is already an explicit choice to edit this as text.
            self.open_shell_or_external(path, rel, cx);
        } else {
            self.open_file(rel, cx);
        }
    }

    pub(crate) fn finish_history_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_history.as_ref().is_some_and(|(path, _)| {
            self.open
                .as_ref()
                .is_some_and(|open| open.notebook.path == *path)
        }) {
            let (_, text) = self
                .pending_history
                .take()
                .expect("matched pending restore");
            self.apply_file_version(text, window, cx);
        }
    }

    fn apply_file_version(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(err) = self.replace_with_version(text, window, cx) {
            self.problem = Some(format!("Could not restore this version: {err:#}"));
        }
        cx.notify();
    }

    fn replace_with_version(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Some(open) = self.open.as_ref() else {
            return Ok(());
        };
        anyhow::ensure!(
            !open.notebook.truncated,
            "only a preview of this file is open"
        );
        // A Quarto restore can change the set of runnable cells. Apply the
        // whole document as one notebook edit, even when Source was showing.
        if open.notebook.quarto_source() {
            self.toggle_quarto_view(window, cx);
        }
        let open = self.open.as_ref().expect("document remains open");
        let sources = open.sources(cx);
        if open.notebook.history_text(&sources)? == text {
            return Ok(());
        }
        if open.single_editor() {
            let editor = open.editors[0].clone();
            self.open.as_mut().unwrap().markdown_preview = false;
            editor.update(cx, |editor, cx| {
                editor.replace_all(text, window, cx);
                editor.focus(window, cx);
            });
            return Ok(());
        }

        let (notebook, matched) = open.notebook.history_edit(&sources, &text)?;
        let mut cell_runs = HashMap::new();
        let unchanged: HashSet<_> = matched
            .into_iter()
            .map(|(old, new)| {
                let old = &open.notebook.cells[old].id;
                let new = &notebook.cells[new].id;
                if let Some(run) = open.cell_runs.get(old) {
                    cell_runs.insert(new.clone(), run.clone());
                }
                old.clone()
            })
            .collect();
        let mut clear_outputs = open.history_edits.clear_outputs.clone();
        clear_outputs.extend(
            open.notebook
                .cells
                .iter()
                .chain(&notebook.cells)
                .filter(|c| c.kind == CellKind::Code && !unchanged.contains(&c.id))
                .map(|c| c.id.clone()),
        );
        let shape = Shape {
            notebook,
            editors: Vec::new(),
            subscriptions: Vec::new(),
            cell_runs,
            clear_outputs,
            mtime_ms: open.mtime_ms,
            dirty: true,
        };
        let (editors, subscriptions) = self.cell_editors(&shape.notebook, window, cx);
        let shape = Shape {
            editors,
            subscriptions,
            ..shape
        };
        let open = self.open.as_mut().expect("document remains open");
        let alternate = shape.swap(open);
        let expected = document_key(open, cx);
        open.history_edits.undo.push(Restore {
            alternate,
            expected,
        });
        open.history_edits.redo.clear();
        let path = open.notebook.path.clone();
        self.sync_language(&path, false, cx);
        self.refresh_find(cx);
        window.focus(&self.cells_focus, cx);
        Ok(())
    }

    pub(crate) fn undo_file_history(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.history_shown || self.diff_shown || self.explorer_shown || self.objects.is_shown() {
            return false;
        }
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        if !self.cells_focus.is_focused(window)
            && !open
                .editors
                .iter()
                .any(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
        {
            return false;
        }
        let stack = if redo {
            &mut open.history_edits.redo
        } else {
            &mut open.history_edits.undo
        };
        let Some(restore) = stack.pop() else {
            return false;
        };
        let current = document_key(open, cx);
        if restore.expected != current {
            let stack = if redo {
                &mut open.history_edits.redo
            } else {
                &mut open.history_edits.undo
            };
            stack.push(restore);
            return false;
        }
        let alternate = restore.alternate.swap(open);
        let swap_back = Restore {
            alternate,
            expected: document_key(open, cx),
        };
        if redo {
            open.history_edits.undo.push(swap_back)
        } else {
            open.history_edits.redo.push(swap_back)
        }
        let path = open.notebook.path.clone();
        self.sync_language(&path, false, cx);
        self.refresh_find(cx);
        window.focus(&self.cells_focus, cx);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::workbench::Workbench;
    use gpui_kit::component::input::Undo;
    use gpui_kit::{AppContext, Focusable, TestAppContext, WeakEntity, WindowOptions};
    use std::sync::Arc;

    #[gpui_kit::test]
    fn restoring_text_is_one_undoable_edit_and_does_not_write(cx: &mut TestAppContext) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("script.py");
        let current = "new = True\n";
        std::fs::write(&path, current).unwrap();
        let nb = crate::editor::notebook::parse(path.to_str().unwrap(), current).unwrap();
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            crate::welcome::resource_manager::block_host(sandhill_link::LOCAL, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        rt,
                        sandhill_link::Target::Local,
                        None,
                        WeakEntity::new_invalid(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| this.show_notebook(nb, 0, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        let restored = "old = True\n";
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| {
                this.replace_with_version(restored.into(), window, cx)
                    .unwrap();
                assert_eq!(this.open.as_ref().unwrap().sources(cx), [restored]);
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        cx.update_window(handle, |_, window, cx| {
            let focus = workbench.read(cx).open.as_ref().unwrap().editors[0]
                .read(cx)
                .focus_handle(cx);
            focus.dispatch_action(&Undo, window, cx);
            assert_eq!(
                workbench.read(cx).open.as_ref().unwrap().sources(cx),
                [current]
            );
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
    }
}
