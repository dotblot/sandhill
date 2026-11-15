//! What the notebook's cells can do besides being edited: be added,
//! removed, moved and turned from code to Markdown and back (in a Jupyter
//! notebook), run with the cells above or below them, have their outputs
//! folded or cleared, and say how their run is going.

use std::time::{Duration, Instant};

use gpui_kit::*;
use sandhill_proto::Call;

use crate::editor::notebook::{self, CellKind};
use crate::workbench::{DocKind, Run, Workbench};

/// How a cell's latest run is going.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CellStatus {
    /// Not run since the notebook was opened, and nothing saved.
    Idle,
    /// Waiting for the kernel to start or for the cells before it.
    Queued,
    /// Running for this long.
    Running(Option<Duration>),
    /// Finished, in this long.
    Done(Option<Duration>),
    Failed(Option<Duration>),
}

impl CellStatus {
    /// How long, as the cell's corner shows it: `0.4s`, `12s`, `3m 05s`.
    pub(crate) fn time_text(&self) -> Option<String> {
        let took = match self {
            CellStatus::Running(t) | CellStatus::Done(t) | CellStatus::Failed(t) => (*t)?,
            _ => return None,
        };
        Some(duration_text(took))
    }
}

pub(crate) fn duration_text(took: Duration) -> String {
    let secs = took.as_secs_f64();
    if secs < 10. {
        format!("{secs:.1}s")
    } else if secs < 60. {
        format!("{}s", secs.round() as u64)
    } else {
        let whole = secs.round() as u64;
        format!("{}m {:02}s", whole / 60, whole % 60)
    }
}

/// How a run is going, from what the daemon told about it.
pub(crate) fn run_status(run: &Run) -> CellStatus {
    let took = |end: i64| {
        let start = run.began_ms.or(run.queued_ms)?;
        Some(Duration::from_millis((end - start).max(0) as u64))
    };
    if run.finished {
        let took = run.finished_ms.and_then(took);
        return match run.status.as_deref() {
            Some("ok") | None => CellStatus::Done(took),
            _ => CellStatus::Failed(took),
        };
    }
    match run.began_at {
        Some(at) => CellStatus::Running(Some(at.elapsed())),
        None if run.began_ms.is_some() => CellStatus::Running(None),
        None => CellStatus::Queued,
    }
}

impl Workbench {
    /// How the open notebook's cell `ix` is doing.
    pub(crate) fn cell_status(&self, ix: usize) -> CellStatus {
        let Some(open) = &self.open else {
            return CellStatus::Idle;
        };
        let Some(cell) = open.notebook.cells.get(ix) else {
            return CellStatus::Idle;
        };
        if open.sending.contains(&cell.id) || open.waiting.contains(&cell.id) {
            return CellStatus::Queued;
        }
        match open.cell_runs.get(&cell.id).and_then(|r| self.runs.get(r)) {
            Some(run) => run_status(run),
            None => CellStatus::Idle,
        }
    }

    /// A run started or finished in `session`: the kernel runs one thing at
    /// a time, so the oldest run not finished is the one running.
    pub(crate) fn advance_queue(&mut self, session: &str, ts_ms: i64, cx: &mut Context<Self>) {
        let waiting = self
            .runs
            .values_mut()
            .filter(|r| r.session == session && !r.finished);
        let mut next: Option<&mut Run> = None;
        for run in waiting {
            if run.began_ms.is_some() {
                return;
            }
            if next
                .as_ref()
                .is_none_or(|n| run.queued_ms.unwrap_or(i64::MAX) < n.queued_ms.unwrap_or(i64::MAX))
            {
                next = Some(run);
            }
        }
        if let Some(run) = next {
            run.began_ms = Some(ts_ms);
            run.began_at = Some(Instant::now());
            self.tick(cx);
        }
    }

    /// Redraws once a second while a run is going, for the time it shows.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let going = this
                    .update(cx, |this, cx| {
                        let going = this
                            .runs
                            .values()
                            .any(|r| !r.finished && r.began_at.is_some());
                        if going {
                            cx.notify();
                        } else {
                            this.ticking = false;
                        }
                        going
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Runs the cells above `ix`, not `ix` itself.
    pub(crate) fn run_above(&mut self, ix: usize, cx: &mut Context<Self>) {
        for i in 0..ix {
            self.run_cell(i, cx);
        }
    }

    /// Runs `ix` and every cell below it.
    pub(crate) fn run_below(&mut self, ix: usize, cx: &mut Context<Self>) {
        let count = self.open.as_ref().map_or(0, |o| o.notebook.cells.len());
        for i in ix..count {
            self.run_cell(i, cx);
        }
    }

    /// Adds an empty cell at `at` and puts the cursor in it.
    pub(crate) fn add_cell(
        &mut self,
        at: usize,
        kind: CellKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_cell_with(at, kind, String::new(), window, cx);
    }

    /// Adds a cell holding `source` at `at` and puts the cursor in it.
    pub(crate) fn add_cell_with(
        &mut self,
        at: usize,
        kind: CellKind,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self
            .open
            .as_ref()
            .and_then(|open| open.session.id())
            .and_then(|session| self.session_language(session))
            .or_else(|| {
                self.active_kernel()
                    .and_then(|kernel| self.kernels.iter().find(|spec| spec.name == kernel))
                    .map(|spec| spec.language.as_str())
            })
            .map(str::to_string);
        let Some(open) = self.open.as_mut().filter(|o| o.notebook.can_add_cells()) else {
            return;
        };
        let requested = at.min(open.notebook.cells.len());
        let quarto = open.notebook.format == notebook::Format::Quarto;
        let old_ids: Vec<_> = if quarto {
            open.notebook
                .cells
                .iter()
                .map(|cell| cell.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        let (at, split) = if quarto {
            let sources: Vec<_> = open
                .editors
                .iter()
                .map(|editor| editor.read(cx).value().to_string())
                .collect();
            match open
                .notebook
                .insert_quarto_cell(requested, kind, language.as_deref(), &sources)
            {
                Ok(inserted) => inserted,
                Err(err) => {
                    self.problem = Some(format!("Adding cell failed: {err:#}"));
                    cx.notify();
                    return;
                }
            }
        } else {
            open.notebook.insert_cell(requested, kind);
            (requested, false)
        };
        let id = open.notebook.cells[at].id.clone();
        let renames: std::collections::HashMap<_, _> = old_ids
            .into_iter()
            .zip(
                open.notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| *ix != at && !(split && *ix == at + 1))
                    .map(|(_, cell)| cell.id.clone()),
            )
            .filter(|(old, new)| old != new)
            .collect();
        if !renames.is_empty() {
            let rename = |id: String| renames.get(&id).cloned().unwrap_or(id);
            open.editing = std::mem::take(&mut open.editing)
                .into_iter()
                .map(rename)
                .collect();
            open.collapsed = std::mem::take(&mut open.collapsed)
                .into_iter()
                .map(rename)
                .collect();
            open.sending = std::mem::take(&mut open.sending)
                .into_iter()
                .map(rename)
                .collect();
            open.waiting = std::mem::take(&mut open.waiting)
                .into_iter()
                .map(rename)
                .collect();
            open.cell_runs = std::mem::take(&mut open.cell_runs)
                .into_iter()
                .map(|(id, run)| (rename(id), run))
                .collect();
            open.agent_changes = std::mem::take(&mut open.agent_changes)
                .into_iter()
                .map(|(id, change)| (rename(id), change))
                .collect();
            if let Some((id, _)) = &mut open.lit {
                *id = rename(std::mem::take(id));
            }
            if let Some(id) = &mut open.ghost_after {
                *id = rename(std::mem::take(id));
            }
            if let Some(ghost) = &mut open.ghost {
                ghost.after = rename(std::mem::take(&mut ghost.after));
            }
            for run in self.runs.values_mut() {
                if let Some((path, id)) = &mut run.cell
                    && *path == open.notebook.path
                {
                    *id = rename(std::mem::take(id));
                }
            }
        }
        let mut nb = DocKind::of(&open.notebook);
        nb.language = open.notebook.cell_language(at).map(str::to_string);
        let header = (requested == 0 && at == 1).then(|| open.notebook.cells[0].source.clone());
        let tail = split.then(|| open.notebook.cells[at + 1].source.clone());
        if let Some(header) = header {
            let (editor, subscription) = self.cell_editor(&nb, CellKind::Text, header, window, cx);
            let open = self.open.as_mut().expect("checked above");
            open.editors[0] = editor;
            open._subscriptions.push(subscription);
        }
        if let Some(tail) = tail {
            let (editor, subscription) = self.cell_editor(&nb, CellKind::Text, tail, window, cx);
            let open = self.open.as_mut().expect("checked above");
            open.editors.insert(at, editor);
            open._subscriptions.push(subscription);
        }
        let (editor, subscription) = self.cell_editor(&nb, kind, source, window, cx);
        let open = self.open.as_mut().expect("checked above");
        open.editors.insert(at, editor.clone());
        open._subscriptions.push(subscription);
        if kind == CellKind::Text {
            open.editing.insert(id);
        }
        open.current = Some(at);
        open.dirty = true;
        let path = open.notebook.path.clone();
        self.language_event(&path, &gpui_kit::component::input::InputEvent::Change, cx);
        editor.update(cx, |e, cx| e.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn rename_quarto_cells(
        &mut self,
        renames: &std::collections::HashMap<String, String>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if renames.is_empty() {
            return;
        }
        let rename = |id: String| renames.get(&id).cloned().unwrap_or(id);
        open.editing = std::mem::take(&mut open.editing)
            .into_iter()
            .map(rename)
            .collect();
        open.collapsed = std::mem::take(&mut open.collapsed)
            .into_iter()
            .map(rename)
            .collect();
        open.sending = std::mem::take(&mut open.sending)
            .into_iter()
            .map(rename)
            .collect();
        open.waiting = std::mem::take(&mut open.waiting)
            .into_iter()
            .map(rename)
            .collect();
        open.cell_runs = std::mem::take(&mut open.cell_runs)
            .into_iter()
            .map(|(id, run)| (rename(id), run))
            .collect();
        open.agent_changes = std::mem::take(&mut open.agent_changes)
            .into_iter()
            .map(|(id, change)| (rename(id), change))
            .collect();
        if let Some((id, _)) = &mut open.lit {
            *id = rename(std::mem::take(id));
        }
        if let Some(id) = &mut open.ghost_after {
            *id = rename(std::mem::take(id));
        }
        if let Some(ghost) = &mut open.ghost {
            ghost.after = rename(std::mem::take(&mut ghost.after));
        }
        for run in self.runs.values_mut() {
            if let Some((path, id)) = &mut run.cell
                && *path == open.notebook.path
            {
                *id = rename(std::mem::take(id));
            }
        }
    }

    /// Removes cell `ix`; the cell after it, or else before it, becomes the
    /// current one.
    pub(crate) fn delete_cell(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| o.notebook.can_delete_cells()) else {
            return;
        };
        if ix >= open.editors.len() {
            return;
        }
        let quarto = open.notebook.format == notebook::Format::Quarto;
        let old_ids: Vec<_> = open
            .notebook
            .cells
            .iter()
            .map(|cell| cell.id.clone())
            .collect();
        let mut cell = if quarto {
            let sources: Vec<_> = open
                .editors
                .iter()
                .map(|editor| editor.read(cx).value().to_string())
                .collect();
            match open.notebook.remove_quarto_cell(ix, &sources) {
                Ok(cell) => cell,
                Err(error) => {
                    self.problem = Some(format!("Deleting cell failed: {error:#}"));
                    cx.notify();
                    return;
                }
            }
        } else {
            open.notebook.remove_cell(ix).expect("checked above")
        };
        let mut renames = std::collections::HashMap::new();
        if quarto {
            let deleted_id = format!("deleted-{}-{}", open.deleted.len(), cell.id);
            renames.insert(cell.id.clone(), deleted_id.clone());
            cell.id = deleted_id;
            renames.extend(
                old_ids
                    .into_iter()
                    .enumerate()
                    .filter(|(at, _)| *at != ix)
                    .map(|(_, id)| id)
                    .zip(open.notebook.cells.iter().map(|cell| cell.id.clone())),
            );
        }
        let editor = open.editors.remove(ix);
        let source = editor.read(cx).value().to_string();
        open.editing.remove(&cell.id);
        open.collapsed.remove(&cell.id);
        open.waiting.retain(|id| *id != cell.id);
        open.deleted.push((ix, cell, source));
        let count = open.notebook.cells.len();
        open.current = (count > 0).then(|| ix.min(count - 1));
        open.dirty = true;
        let path = open.notebook.path.clone();
        self.rename_quarto_cells(&renames);
        self.language_event(&path, &gpui_kit::component::input::InputEvent::Change, cx);
        cx.notify();
    }

    /// Brings back the cell deleted last, where it was.
    pub(crate) fn undo_delete_cell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| !open.notebook.quarto_source())
        else {
            return;
        };
        let Some((ix, cell, source)) = open.deleted.pop() else {
            return;
        };
        self.bring_back_cell(ix, cell, source, window, cx);
    }

    /// Puts a deleted cell back at `ix`, with its text.
    pub(crate) fn bring_back_cell(
        &mut self,
        ix: usize,
        cell: notebook::Cell,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let kind = cell.kind;
        let quarto = open.notebook.format == notebook::Format::Quarto;
        let old_ids: Vec<_> = open
            .notebook
            .cells
            .iter()
            .map(|cell| cell.id.clone())
            .collect();
        let deleted_id = cell.id.clone();
        let ix = ix.min(open.notebook.cells.len());
        if quarto {
            let sources: Vec<_> = open
                .editors
                .iter()
                .map(|editor| editor.read(cx).value().to_string())
                .collect();
            if let Err(error) = open
                .notebook
                .restore_quarto_cell(ix, cell.clone(), &sources)
            {
                open.deleted.push((ix, cell, source));
                self.problem = Some(format!("Restoring cell failed: {error:#}"));
                cx.notify();
                return;
            }
        } else {
            open.notebook.restore_cell(ix, cell);
        }
        let mut renames: std::collections::HashMap<_, _> = old_ids
            .into_iter()
            .zip(
                open.notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| *at != ix)
                    .map(|(_, cell)| cell.id.clone()),
            )
            .collect();
        renames.insert(deleted_id, open.notebook.cells[ix].id.clone());
        let mut doc = DocKind::of(&open.notebook);
        doc.language = open.notebook.cell_language(ix).map(str::to_string);
        let (editor, subscription) = self.cell_editor(&doc, kind, source, window, cx);
        let open = self.open.as_mut().expect("checked above");
        open.editors.insert(ix, editor);
        open._subscriptions.push(subscription);
        open.current = Some(ix);
        open.dirty = true;
        let path = open.notebook.path.clone();
        self.rename_quarto_cells(&renames);
        self.language_event(&path, &gpui_kit::component::input::InputEvent::Change, cx);
        cx.notify();
    }

    /// Moves cell `from` to `to`.
    pub(crate) fn move_cell(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| o.notebook.can_restructure()) else {
            return;
        };
        let count = open.notebook.cells.len();
        if from >= count || to >= count || from == to {
            return;
        }
        open.notebook.move_cell(from, to);
        let editor = open.editors.remove(from);
        open.editors.insert(to, editor);
        open.current = Some(to);
        open.dirty = true;
        let path = open.notebook.path.clone();
        self.language_event(&path, &gpui_kit::component::input::InputEvent::Change, cx);
        cx.notify();
    }

    /// Makes cell `ix` code or Markdown, keeping its text.
    pub(crate) fn set_cell_kind(
        &mut self,
        ix: usize,
        kind: CellKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut().filter(|o| o.notebook.can_restructure()) else {
            return;
        };
        let Some(cell) = open.notebook.cells.get(ix) else {
            return;
        };
        if cell.kind == kind {
            return;
        }
        let id = cell.id.clone();
        let source = open.editors[ix].read(cx).value().to_string();
        open.notebook.set_kind(ix, kind);
        open.cell_runs.remove(&id);
        open.collapsed.remove(&id);
        open.editing.remove(&id);
        let mut nb = DocKind::of(&open.notebook);
        nb.language = open.notebook.cell_language(ix).map(str::to_string);
        let (editor, subscription) = self.cell_editor(&nb, kind, source, window, cx);
        let open = self.open.as_mut().expect("checked above");
        open.editors[ix] = editor;
        open._subscriptions.push(subscription);
        open.current = Some(ix);
        open.dirty = true;
        let path = open.notebook.path.clone();
        self.language_event(&path, &gpui_kit::component::input::InputEvent::Change, cx);
        cx.notify();
    }

    /// Folds cell `ix`'s outputs away, or shows them again.
    pub(crate) fn toggle_outputs(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(id) = open.notebook.cells.get(ix).map(|c| c.id.clone()) else {
            return;
        };
        if !open.collapsed.remove(&id) {
            open.collapsed.insert(id);
        }
        cx.notify();
    }

    /// Clears the outputs of these cells (all of them when `None`): from
    /// the window, from the file when it is saved, and from what the
    /// daemon shows when the notebook is opened again. Cells still running
    /// keep theirs.
    pub(crate) fn clear_outputs(&mut self, cells: Option<&[usize]>, cx: &mut Context<Self>) {
        let daemon = self.daemon();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let all: Vec<usize> = (0..open.notebook.cells.len()).collect();
        let mut cleared = Vec::new();
        for &ix in cells.unwrap_or(&all) {
            let Some(cell) = open.notebook.cells.get(ix) else {
                continue;
            };
            if cell.kind != CellKind::Code {
                continue;
            }
            let id = cell.id.clone();
            let running = open
                .cell_runs
                .get(&id)
                .and_then(|r| self.runs.get(r))
                .is_some_and(|r| !r.finished);
            if running {
                continue;
            }
            if !open.notebook.cells[ix].outputs.is_empty() {
                open.notebook.clear_outputs(ix);
                open.dirty = true;
            }
            open.cell_runs.remove(&id);
            open.collapsed.remove(&id);
            cleared.push(id);
        }
        if let Some(daemon) = daemon
            && !cleared.is_empty()
        {
            daemon.fire(Call::ClearCellOutputs {
                file: open.notebook.path.clone(),
                cell_ids: cleared,
            });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CellStatus, Run, duration_text, run_status};

    #[test]
    fn says_how_long_cells_took() {
        assert_eq!(duration_text(Duration::from_millis(420)), "0.4s");
        assert_eq!(duration_text(Duration::from_millis(12_400)), "12s");
        assert_eq!(duration_text(Duration::from_secs(185)), "3m 05s");
    }

    #[test]
    fn a_run_is_queued_then_running_then_done() {
        let mut run = Run::default();
        run.queued_ms = Some(1_000);
        assert_eq!(run_status(&run), CellStatus::Queued);
        run.began_ms = Some(1_500);
        assert_eq!(run_status(&run), CellStatus::Running(None));
        run.finished = true;
        run.finished_ms = Some(2_000);
        run.status = Some("ok".into());
        assert_eq!(
            run_status(&run),
            CellStatus::Done(Some(Duration::from_millis(500)))
        );
        run.status = Some("error".into());
        assert_eq!(run_status(&run).time_text().as_deref(), Some("0.5s"));
        assert!(matches!(run_status(&run), CellStatus::Failed(_)));
    }
}
