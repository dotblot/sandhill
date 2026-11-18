//! A notebook working with the agent in the AI pane: the Fix and Explain
//! buttons under a cell's error, the agent's cell changes shown as a diff
//! (Keep, or Undo), cells it deleted kept as a line with Undo, and the
//! notebook following the agent to the cells it adds or changes.

use std::time::{Duration, Instant};

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::editor::notebook;
use crate::look::{self, ButtonSize::Row, Palette, icon};
use crate::workbench::{Open, Workbench};

/// How long a cell the agent added or changed stays lit.
const LIT_FOR: Duration = Duration::from_millis(1600);
/// Unchanged lines shown around a change in a diff.
const CONTEXT_LINES: usize = 2;
/// Cells longer than this (in lines) are not compared line by line: all of
/// the old text shows as removed and the new as added.
const MAX_DIFF_LINES: usize = 2000;

/// A cell the agent changed, until the user keeps the change or undoes it.
pub(crate) struct AgentChange {
    /// The cell's text before the agent's first change.
    pub before: String,
    /// The agent, as its tools name it ("Claude").
    pub by: String,
}

/// A cell the agent deleted, shown where it was until brought back or
/// dismissed.
pub(crate) struct AgentDeleted {
    pub ix: usize,
    pub cell: notebook::Cell,
    pub source: String,
    pub by: String,
}

/// One line of a cell's diff.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
    /// Unchanged lines left out.
    Gap(usize),
}

/// The lines of `before` and `after`, removed, added or the same, with
/// long runs of unchanged lines left out.
pub(crate) fn diff(before: &str, after: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let mut lines = Vec::new();
    if old.len() > MAX_DIFF_LINES || new.len() > MAX_DIFF_LINES {
        lines.extend(old.iter().map(|l| DiffLine::Removed(l.to_string())));
        lines.extend(new.iter().map(|l| DiffLine::Added(l.to_string())));
        return lines;
    }
    // The longest common subsequence, from the end, then walked forward.
    let (n, m) = (old.len(), new.len());
    let mut common = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            lines.push(DiffLine::Same(old[i].to_string()));
            i += 1;
            j += 1;
        } else if i < n && (j == m || common[i + 1][j] >= common[i][j + 1]) {
            // Removed lines go before the lines that replace them.
            lines.push(DiffLine::Removed(old[i].to_string()));
            i += 1;
        } else {
            lines.push(DiffLine::Added(new[j].to_string()));
            j += 1;
        }
    }
    fold_unchanged(lines)
}

/// Leaves out unchanged lines further than [`CONTEXT_LINES`] from a change.
fn fold_unchanged(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !matches!(l, DiffLine::Same(_)))
        .map(|(i, _)| i)
        .collect();
    let near = |i: usize| changed.iter().any(|&c| c.abs_diff(i) <= CONTEXT_LINES);
    let mut folded = Vec::new();
    let mut gap = 0;
    for (i, line) in lines.into_iter().enumerate() {
        if matches!(line, DiffLine::Same(_)) && !near(i) {
            gap += 1;
            continue;
        }
        if gap > 0 {
            folded.push(DiffLine::Gap(gap));
            gap = 0;
        }
        folded.push(line);
    }
    if gap > 0 {
        folded.push(DiffLine::Gap(gap));
    }
    folded
}

impl Workbench {
    /// Cell `ix` as a # chip (which brings its latest output to the agent),
    /// with the cell's and the file's names.
    fn cell_mention(&self, ix: usize, cx: &App) -> Option<(Value, String, String)> {
        let open = self.open.as_ref()?;
        let cell = open.notebook.cells.get(ix)?;
        let name = open
            .code_cells(cx)
            .into_iter()
            .find(|c| c.id == cell.id)
            .map(|c| c.name)?;
        let rel = self.project_relative(&open.notebook.path);
        let file = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let mention = json!({
            "kind": "chunk",
            "name": format!("{file} › {name}"),
            "path": rel,
            "cell": cell.id,
        });
        Some((mention, name, file))
    }

    /// `path` relative to the project, when it is inside it.
    fn project_relative(&self, path: &str) -> String {
        self.project
            .as_deref()
            .and_then(|p| path.strip_prefix(&format!("{}/", p.trim_end_matches('/'))))
            .unwrap_or(path)
            .to_string()
    }

    /// Asks the agent in the AI pane to fix cell `ix`'s error, or to
    /// explain it. The message names the cell with #, which brings its
    /// latest output (the error) to the agent.
    pub(crate) fn ask_agent_about_cell(
        &mut self,
        ix: usize,
        fix: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((mention, name, file)) = self.cell_mention(ix, cx) else {
            return;
        };
        let text = if fix {
            format!(
                "Fix the error in {name} of {file}. Change that cell (edit_cell) rather than adding a new one, and run it."
            )
        } else {
            format!("Explain the error in {name} of {file}. Don't change any code yet.")
        };
        self.show_agents = true;
        if let Some(agents) = self.agents.clone() {
            // After this update, as the AI pane reads this window.
            window.defer(cx, move |window, cx| {
                agents.update(cx, |a, cx| a.ask_about_cell(text, mention, window, cx));
            });
        }
        cx.notify();
    }

    /// The cell toolbar's Ask AI: cell `ix` goes in the message box as a #
    /// chip, with the caret there for the question (Huan, 2026-11-16).
    pub(crate) fn ask_ai_about_cell(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((mention, _, _)) = self.cell_mention(ix, cx) {
            self.offer_to_agent(vec![mention], window, cx);
        }
    }

    /// The editor's "Ask AI About Selection": the code selected in the
    /// editor of cell `ix` (0 for a script) goes in the message box as a
    /// chip named by its file and lines (Huan, 2026-11-16).
    pub(crate) fn ask_ai_about_selection(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Some(editor) = open.editors.get(ix) else {
            return;
        };
        let (text, first, last) = {
            let e = editor.read(cx);
            let range = e.selected_range();
            let text = e.value();
            let text = text.get(range.clone()).unwrap_or_default().to_string();
            let line_of =
                |at: usize| e.value()[..at.min(e.value().len())].matches('\n').count() + 1;
            // A selection ending at a line's start doesn't take that line.
            let end = if text.ends_with('\n') {
                range.end - 1
            } else {
                range.end
            };
            (text, line_of(range.start), line_of(end.max(range.start)))
        };
        if text.trim().is_empty() {
            return;
        }
        let rel = self.project_relative(&open.notebook.path);
        let file = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let lines = if first == last {
            format!("{first}")
        } else {
            format!("{first}–{last}")
        };
        let mut chip = json!({"kind": "selection", "path": rel, "text": text});
        if open.single_editor() {
            chip["name"] = json!(format!("{file}:{lines}"));
        } else {
            let Some((_, name, _)) = self.cell_mention(ix, cx) else {
                return;
            };
            chip["name"] = json!(format!("{file} › {name}:{lines}"));
            chip["cell"] = json!(open.notebook.cells[ix].id);
        }
        self.offer_to_agent(vec![chip], window, cx);
    }

    /// Shows the AI pane and puts `refs` in its message box.
    fn offer_to_agent(&mut self, refs: Vec<Value>, window: &mut Window, cx: &mut Context<Self>) {
        self.show_agents = true;
        if let Some(agents) = self.agents.clone() {
            // After this update, as the AI pane reads this window.
            window.defer(cx, move |window, cx| {
                agents.update(cx, |a, cx| a.offer_refs(refs, window, cx));
            });
        }
        cx.notify();
    }

    /// After the agent added or changed cell `ix`: scrolls to it and
    /// lights it up for a moment, unless the user turned following off.
    pub(crate) fn follow_agent(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !crate::prefs::follow_agent() {
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(cell) = open.notebook.cells.get(ix) else {
            return;
        };
        open.lit = Some((cell.id.clone(), Instant::now()));
        self.scroll_to_cell(ix);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LIT_FOR).await;
            this.update(cx, |this, cx| {
                if let Some(open) = this.open.as_mut()
                    && open
                        .lit
                        .as_ref()
                        .is_some_and(|(_, at)| at.elapsed() >= LIT_FOR)
                {
                    open.lit = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps the agent's change to cell `id`: the diff goes away.
    pub(crate) fn keep_agent_change(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            open.agent_changes.remove(id);
        }
        cx.notify();
    }

    /// Puts back cell `id`'s text from before the agent's change, and saves.
    pub(crate) fn undo_agent_change(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(change) = open.agent_changes.remove(id) else {
            return;
        };
        let Some(ix) = open.notebook.cells.iter().position(|c| c.id == id) else {
            return;
        };
        open.editors[ix].update(cx, |e, cx| {
            let end = e.value().len();
            e.set_selected_range(0..end, cx);
            e.replace(change.before, window, cx);
        });
        open.dirty = true;
        open.current = Some(ix);
        self.save(cx);
        cx.notify();
    }

    /// Brings back the cell the agent deleted, `n`th in the list, and saves.
    pub(crate) fn undo_agent_delete(
        &mut self,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if n >= open.agent_deleted.len() {
            return;
        }
        let gone = open.agent_deleted.remove(n);
        for other in open.agent_deleted.iter_mut().filter(|d| d.ix > gone.ix) {
            other.ix += 1;
        }
        self.bring_back_cell(gone.ix, gone.cell, gone.source, window, cx);
        self.save(cx);
    }

    /// Forgets the line where the agent deleted a cell, `n`th in the list.
    pub(crate) fn dismiss_agent_delete(&mut self, n: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut()
            && n < open.agent_deleted.len()
        {
            open.agent_deleted.remove(n);
        }
        cx.notify();
    }

    /// The lines where the agent deleted cells, above the cell now at `at`
    /// (or, at the end, after the last).
    pub(crate) fn render_agent_deleted(
        &self,
        open: &Open,
        at: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let last = open.notebook.cells.len();
        open.agent_deleted
            .iter()
            .enumerate()
            .filter(|(_, d)| d.ix.min(last) == at)
            .map(|(n, d)| {
                let first = d
                    .source
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default()
                    .to_string();
                h_flex()
                    .id(("agent-deleted", n))
                    .flex_none()
                    .w_full()
                    .pl(px(crate::editor::cell_view::GUTTER))
                    .my(px(2.))
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(8.))
                            .px(px(10.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .border_1()
                            .border_dashed()
                            .border_color(p.line)
                            .text_size(px(look::SMALL))
                            .text_color(p.faint)
                            .child(icon("delete").size(px(12.)))
                            .child(div().flex_none().child(format!("Deleted by {}", d.by)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_family(look::MONO)
                                    .child(first),
                            )
                            .child(
                                look::button(
                                    ("agent-deleted-undo", n),
                                    "Undo",
                                    Some("edit.undo"),
                                    Row,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.undo_agent_delete(n, window, cx)
                                    },
                                )),
                            )
                            .child(
                                look::button(("agent-deleted-ok", n), "OK", Some("check"), Row)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.dismiss_agent_delete(n, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// What the agent changed in cell `ix`, removed lines red and added
    /// ones green, with Keep and Undo; none when it changed nothing.
    pub(crate) fn render_agent_change(
        &self,
        open: &Open,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let id = open.notebook.cells.get(ix)?.id.clone();
        let change = open.agent_changes.get(&id)?;
        let now = open.editors.get(ix)?.read(cx).value().to_string();
        if now.trim_end() == change.before.trim_end() {
            return None;
        }
        let rows = diff(&change.before, &now).into_iter().map(|line| {
            let (mark, text, bg, color) = match line {
                DiffLine::Same(t) => (" ", t, None, p.muted),
                DiffLine::Removed(t) => ("-", t, Some(p.err.opacity(0.12)), p.text),
                DiffLine::Added(t) => ("+", t, Some(p.ok.opacity(0.14)), p.text),
                DiffLine::Gap(n) => (
                    " ",
                    if n == 1 {
                        "⋯ 1 line".to_string()
                    } else {
                        format!("⋯ {n} lines")
                    },
                    None,
                    p.faint,
                ),
            };
            h_flex()
                .w_full()
                .px(px(8.))
                .when_some(bg, |d, bg| d.bg(bg))
                .text_color(color)
                .child(div().w(px(14.)).flex_none().child(mark))
                .child(div().flex_1().min_w_0().whitespace_nowrap().child(text))
        });
        let keep_id = id.clone();
        let undo_id = id.clone();
        Some(
            v_flex()
                .w_full()
                .mt(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(p.line)
                .overflow_hidden()
                .child(
                    h_flex()
                        .w_full()
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(3.))
                        .bg(p.chunk)
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(icon("sparkle.ai").size(px(12.)))
                        .child(div().flex_1().child(format!("Changed by {}", change.by)))
                        .child(
                            look::button(("agent-keep", ix), "Keep", Some("check"), Row).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.keep_agent_change(&keep_id, cx)
                                }),
                            ),
                        )
                        .child(
                            look::button(("agent-undo", ix), "Undo", Some("edit.undo"), Row)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.undo_agent_change(&undo_id, window, cx)
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .py(px(2.))
                        .font_family(look::MONO)
                        .text_size(px(look::CODE))
                        .overflow_x_hidden()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// The Fix and Explain buttons under a cell that ended in an error,
    /// when the project has an AI pane.
    pub(crate) fn render_error_actions(
        &self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.agents.as_ref()?;
        Some(
            h_flex()
                .mt(px(4.))
                .gap(px(6.))
                .child(
                    look::button(("cell-fix", ix), "Fix", Some("sparkle.ai"), Row)
                        .tooltip("Ask the agent to fix this error")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_agent_about_cell(ix, true, window, cx)
                        })),
                )
                .child(
                    look::button(("cell-explain", ix), "Explain", Some("help"), Row)
                        .tooltip("Ask the agent what went wrong")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_agent_about_cell(ix, false, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DiffLine, diff};

    #[test]
    fn shows_what_the_agent_changed() {
        use DiffLine::*;
        assert_eq!(
            diff("a\nb\nc", "a\nB\nc\nd"),
            vec![
                Same("a".into()),
                Removed("b".into()),
                Added("B".into()),
                Same("c".into()),
                Added("d".into()),
            ]
        );
        // Unchanged lines far from a change are left out.
        let before = "1\n2\n3\n4\n5\n6\n7\n8";
        let after = "1\n2\n3\n4\n5\n6\n7\nX";
        assert_eq!(
            diff(before, after),
            vec![
                Gap(5),
                Same("6".into()),
                Same("7".into()),
                Removed("8".into()),
                Added("X".into()),
            ]
        );
    }
}
