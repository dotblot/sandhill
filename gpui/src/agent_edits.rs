//! The window's side of agents' work on documents, as the Tauri app's
//! `agentEdits.ts` has it: the daemon is told where the user is (the
//! document in front, its current cell, the selection, every document
//! open), and the edits agents ask for in documents open here are made
//! here, as the user would make them: in the editor, saved, and run as
//! with the cell's Run button. Documents no window has open are edited by
//! the daemon itself.

use std::path::Path;
use std::time::Duration;

use gpui_kit::*;
use sandhill_daemon::cells;
use sandhill_proto::{AppRequest, Call, Focus};
use serde_json::json;

use crate::notebook::{self, CellKind, Format};
use crate::workbench::Workbench;

/// Cursor moves are told the daemon once they settle.
const FOCUS_DELAY: Duration = Duration::from_millis(300);

impl Workbench {
    /// Where the user is now.
    fn focus_now(&self, cx: &App) -> Focus {
        let mut focus = Focus {
            open: self
                .open
                .iter()
                .map(|o| o.notebook.path.clone())
                .chain(self.tabs.iter().cloned())
                .fold(Vec::new(), |mut all, p| {
                    if !all.contains(&p) {
                        all.push(p);
                    }
                    all
                }),
            ..Focus::default()
        };
        if let Some(open) = &self.open {
            focus.file = Some(open.notebook.path.clone());
            if let Some(ix) = open.current
                && let Some(editor) = open.editors.get(ix)
            {
                let editor = editor.read(cx);
                if !open.is_script() {
                    focus.cell = open.notebook.cells.get(ix).map(|c| c.id.clone());
                }
                focus.line = Some(editor.cursor_position().line + 1);
                let selected = editor.selected_value().to_string();
                focus.selection = (!selected.is_empty()).then_some(selected);
            }
        }
        focus
    }

    /// Tells the daemon where the user is, once it has settled. Called as
    /// the window draws.
    pub(crate) fn report_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        // The cell with the keyboard focus is the current one.
        if let Some(open) = self.open.as_mut()
            && let Some(ix) = open
                .editors
                .iter()
                .position(|e| e.read(cx).focus_handle(cx).is_focused(window))
        {
            open.current = Some(ix);
        }
        let focus = self.focus_now(cx);
        if self.focus_sent.as_ref() == Some(&focus) {
            return;
        }
        self.focus_sent = Some(focus);
        self.focus_generation += 1;
        let generation = self.focus_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOCUS_DELAY).await;
            this.update(cx, |this, _| {
                if this.focus_generation != generation {
                    return;
                }
                if let (Some(daemon), Some(project), Some(focus)) =
                    (this.daemon(), this.project.clone(), this.focus_sent.clone())
                {
                    daemon.fire(Call::SetFocus { project, focus });
                }
            })
            .ok();
        })
        .detach();
    }

    /// An agent asks to add a cell to, or change one in, a document open
    /// here.
    pub(crate) fn on_request(
        &mut self,
        request: AppRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.as_deref() == Some(&request.project) && request.method == "git_credential" {
            return self.on_git_credential(request.id, &request.params, window, cx);
        }
        if self.project.as_deref() != Some(&request.project)
            || !matches!(
                request.method.as_str(),
                "add_cell" | "edit_cell" | "delete_cell"
            )
        {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        let outcome = self.make_edit(&request, window, cx);
        let (result, error) = match &outcome {
            Ok((cell_id, language, _)) => (
                Some(json!({"cell_id": cell_id, "language": language})),
                None,
            ),
            Err(err) => (None, Some(format!("{err:#}"))),
        };
        daemon.fire(Call::Answer {
            id: request.id,
            result,
            error,
        });
        if let Ok((_, _, Some(run))) = outcome
            && request.params["run"] == true
        {
            match run {
                Run::Cell(ix) => self.run_cell(ix, cx),
                Run::Script(code) => self.run_script(code, cx),
            }
        }
        cx.notify();
    }

    /// Makes the edit in the editor and saves: the cell's id, its language
    /// and what to run.
    fn make_edit(
        &mut self,
        request: &AppRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<(String, String, Option<Run>)> {
        let params = &request.params;
        let file = params["file"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no file given"))?
            .to_string();
        let code = params["code"].as_str().unwrap_or_default().to_string();
        // Brings the document to the front, so the user sees the edit.
        if self.open.as_ref().map(|o| o.notebook.path.as_str()) != Some(&file) {
            self.show_file_tab(&file, cx);
        }
        let open = self
            .open
            .as_mut()
            .filter(|o| o.notebook.path == file)
            .ok_or_else(|| anyhow::anyhow!("Sandhill could not open {file}"))?;
        let language = open.notebook.language.clone().unwrap_or_default();
        let outcome = if open.is_script() {
            if request.method != "add_cell" {
                anyhow::bail!("a script has no cells; edit {file} as a file instead");
            }
            let editor = open.editors[0].clone();
            editor.update(cx, |e, cx| {
                let text = e.value().to_string();
                let gap = if text.is_empty() || text.ends_with('\n') {
                    ""
                } else {
                    "\n"
                };
                let end = text.len();
                e.set_selected_range(end..end, cx);
                e.replace(format!("{gap}{}\n", code.trim_end()), window, cx);
            });
            open.dirty = true;
            let language = cells::normalize_language(&language);
            (String::new(), language, Some(Run::Script(code)))
        } else if request.method == "delete_cell" {
            let id = params["cell_id"].as_str().unwrap_or_default().to_string();
            if !open.notebook.can_restructure() {
                anyhow::bail!(
                    "only a Jupyter notebook's cells can be deleted; edit {file} as a file instead"
                );
            }
            let ix = open
                .notebook
                .cells
                .iter()
                .position(|c| c.id == id)
                .ok_or_else(|| anyhow::anyhow!("{file} has no cell {id}"))?;
            self.delete_cell(ix, cx);
            (id, language, None)
        } else if request.method == "edit_cell" {
            let id = params["cell_id"].as_str().unwrap_or_default();
            let ix = open
                .notebook
                .cells
                .iter()
                .position(|c| c.id == id)
                .ok_or_else(|| anyhow::anyhow!("{file} has no cell {id}"))?;
            if open.notebook.cells[ix].kind != CellKind::Code {
                anyhow::bail!("cell {id} of {file} is not a code cell");
            }
            open.editors[ix].update(cx, |e, cx| {
                let end = e.value().len();
                e.set_selected_range(0..end, cx);
                e.replace(code.trim_end_matches('\n').to_string(), window, cx);
            });
            open.dirty = true;
            open.current = Some(ix);
            (id.to_string(), language, Some(Run::Cell(ix)))
        } else {
            // The document as edited here, the cell added as the daemon
            // would add it, then its editors made again.
            let sources: Vec<String> = open
                .editors
                .iter()
                .map(|e| e.read(cx).value().to_string())
                .collect();
            let text = open.notebook.render(&sources)?;
            let (text, edited) = cells::add_cell_to_text(
                Path::new(&file),
                &text,
                &code,
                params["language"].as_str(),
                params["after"].as_str(),
                &file,
            )?;
            let nb = notebook::parse(&file, &text)?;
            let ix = nb
                .cells
                .iter()
                .position(|c| c.id == edited.cell_id)
                .unwrap_or(nb.cells.len().saturating_sub(1));
            let (editors, subscriptions) = self.cell_editors(&nb, window, cx);
            let open = self.open.as_mut().expect("shown above");
            open.notebook = nb;
            open.editors = editors;
            open._subscriptions = subscriptions;
            open.editing.clear();
            open.deleted.clear();
            open.waiting.clear();
            open.dirty = true;
            open.current = Some(ix);
            (edited.cell_id, edited.language, Some(Run::Cell(ix)))
        };
        let format = self.open.as_ref().map(|o| o.notebook.format);
        if format != Some(Format::Script) || request.method == "add_cell" {
            self.save(cx);
        }
        Ok(outcome)
    }
}

/// What an agent's edit runs once made.
enum Run {
    Cell(usize),
    Script(String),
}
