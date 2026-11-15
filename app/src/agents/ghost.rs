//! Ghost cells: after the user runs a cell, an agent suggests the cell to
//! come next, shown faded under it until the user takes it (Accept, or
//! Accept and Run) or dismisses it. The switch in the notebook's bar turns
//! it on; it is off at first, as each suggestion runs the agent once. The
//! daemon asks the agent (`SuggestCell`): Claude Code or Codex, as the AI
//! pane's agent, run once with no tools.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use sandhill_proto::{Call, CellSuggestion};
use serde_json::Value;

use crate::agents::cell_ai::pill;
use crate::editor::notebook::CellKind;
use crate::editor::shown::{Shown, show};
use crate::look::{self, Palette, icon};
use crate::workbench::{Open, Workbench};

/// The suggestion for the cell after `after` (a cell id).
pub(crate) struct Ghost {
    pub after: String,
    pub state: GhostState,
}

pub(crate) enum GhostState {
    Thinking,
    Ready { code: String, agent: String },
    Failed(String),
}

/// How much of a cell's output goes with the question.
const MAX_OUTPUT: usize = 4000;

/// What a cell showed, as text for the agent: plots and pages are named,
/// not sent.
pub(crate) fn outputs_text(outputs: &[Value]) -> String {
    let mut text = String::new();
    for output in outputs {
        let part = match show(output) {
            Shown::Text(t) | Shown::Warning(t) | Shown::Error(t) | Shown::Note(t) => t,
            Shown::Plot { .. } => "[a plot]".into(),
            Shown::Page(_) => "[a web page]".into(),
            Shown::Table(_) => output["data"]["text/plain"]
                .as_str()
                .map_or("[a table]".into(), str::to_string),
        };
        text.push_str(part.trim_end());
        text.push('\n');
        if text.len() > MAX_OUTPUT {
            let mut end = MAX_OUTPUT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            break;
        }
    }
    text
}

impl Workbench {
    /// Turns ghost cells on or off; off drops the one shown.
    pub(crate) fn toggle_ghost_cells(&mut self, cx: &mut Context<Self>) {
        self.ghost_cells = !self.ghost_cells;
        crate::prefs::save_ghost_cells(self.ghost_cells);
        if !self.ghost_cells
            && let Some(open) = self.open.as_mut()
        {
            open.ghost = None;
        }
        cx.notify();
    }

    /// A run of a cell the user ran finished: when it went well and nothing
    /// else waits to run, asks for the cell to come after it.
    pub(crate) fn suggest_after_run(&mut self, run_id: &str, cx: &mut Context<Self>) {
        if !self.ghost_cells {
            return;
        }
        let Some(run) = self.runs.get(run_id) else {
            return;
        };
        let Some((file, cell_id)) = run.cell.clone() else {
            return;
        };
        if run.by.is_some() || run.quiet || run.status.as_deref() != Some("ok") {
            return;
        }
        let session = run.session.clone();
        let output = outputs_text(&run.outputs);
        // Run All: only after the last cell.
        let more = self
            .runs
            .values()
            .any(|r| r.session == session && !r.finished && !r.quiet);
        let agent = self
            .agents
            .as_ref()
            .map(|a| a.read(cx).new_kind.to_string());
        let language = self.active_language();
        let cwd = self.project.clone();
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if more
            || open.notebook.path != file
            || !open.notebook.can_restructure()
            || open.ghost_after.as_deref() != Some(cell_id.as_str())
            || !open.sending.is_empty()
            || !open.waiting.is_empty()
        {
            return;
        }
        let Some(ix) = open.notebook.cells.iter().position(|c| c.id == cell_id) else {
            return;
        };
        let cells: Vec<String> = open.editors[..=ix]
            .iter()
            .map(|e| e.read(cx).value().to_string())
            .filter(|s| !s.trim().is_empty())
            .collect();
        let language = open
            .notebook
            .language
            .clone()
            .or(language)
            .unwrap_or_else(|| "python".into());
        let cwd = cwd.unwrap_or_else(|| {
            std::path::Path::new(&file)
                .parent()
                .map_or(".".into(), |d| d.to_string_lossy().into_owned())
        });
        open.ghost_after = None;
        open.ghost = Some(Ghost {
            after: cell_id.clone(),
            state: GhostState::Thinking,
        });
        let asked = daemon.call::<CellSuggestion>(Call::SuggestCell {
            agent,
            cwd,
            language,
            cells,
            output,
        });
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            this.update(cx, |this, cx| {
                let Some(open) = this.open_at(&file) else {
                    return;
                };
                // Another run or a dismissal came first.
                if !open
                    .ghost
                    .as_ref()
                    .is_some_and(|g| g.after == cell_id && matches!(g.state, GhostState::Thinking))
                {
                    return;
                }
                open.ghost = match answer {
                    Ok(CellSuggestion {
                        code: Some(code),
                        agent,
                    }) => Some(Ghost {
                        after: cell_id,
                        state: GhostState::Ready { code, agent },
                    }),
                    Ok(_) => None,
                    Err(err) => Some(Ghost {
                        after: cell_id,
                        state: GhostState::Failed(
                            format!("{err:#}")
                                .trim_start_matches("daemon error: ")
                                .to_string(),
                        ),
                    }),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Takes the suggestion: a new cell with its code under the cell it
    /// follows, run too when asked.
    fn accept_ghost(&mut self, run: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(Ghost {
            after,
            state: GhostState::Ready { code, .. },
        }) = open.ghost.take()
        else {
            return;
        };
        let Some(ix) = open.notebook.cells.iter().position(|c| c.id == after) else {
            return;
        };
        self.add_cell_with(ix + 1, CellKind::Code, code, window, cx);
        if run {
            self.run_cell(ix + 1, cx);
        }
    }

    fn dismiss_ghost(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            open.ghost = None;
            cx.notify();
        }
    }

    /// The suggestion under cell `at - 1`, if there is one there.
    pub(crate) fn render_ghost(
        &self,
        open: &Open,
        at: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ghost = open.ghost.as_ref()?;
        let before = open.notebook.cells.get(at.checked_sub(1)?)?;
        if before.id != ghost.after {
            return None;
        }
        let p = *Palette::get(cx);
        let close = || {
            div()
                .id("ghost-dismiss")
                .flex_none()
                .cursor_pointer()
                .text_color(p.faint)
                .hover(move |d| d.text_color(p.text))
                .child(icon("close").size(px(12.)))
                .on_click(cx.listener(|this, _, _, cx| this.dismiss_ghost(cx)))
        };
        let head = |text: String| {
            h_flex()
                .gap(px(6.))
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(icon("sparkle.ai").size(px(12.)))
                .child(div().flex_1().min_w_0().child(text))
        };
        let body = match &ghost.state {
            GhostState::Thinking => head("Thinking of a next cell…".into()).child(close()),
            GhostState::Failed(err) => {
                head(format!("Couldn't suggest a next cell: {err}")).child(close())
            }
            GhostState::Ready { code, agent } => {
                let who = crate::agents::kind_of(agent).map_or("The agent", |k| k.short);
                v_flex()
                    .gap(px(6.))
                    .child(
                        head(format!("{who} suggests a next cell"))
                            .child(
                                pill("ghost-accept", "add", "Accept", &p).on_click(cx.listener(
                                    |this, _, window, cx| this.accept_ghost(false, window, cx),
                                )),
                            )
                            .child(pill("ghost-run", "run", "Accept and Run", &p).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.accept_ghost(true, window, cx)
                                }),
                            ))
                            .child(close()),
                    )
                    .child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(look::RADIUS))
                            .border_1()
                            .border_dashed()
                            .border_color(p.line)
                            .font_family(look::MONO)
                            .text_color(p.muted)
                            .opacity(0.75)
                            .whitespace_normal()
                            .child(code.clone()),
                    )
            }
        };
        Some(
            div()
                .w_full()
                .pl(px(crate::editor::cell_view::GUTTER + 8.))
                .pr(px(6.))
                .pt(px(4.))
                .child(body)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::outputs_text;
    use serde_json::json;

    #[test]
    fn tells_the_agent_what_a_cell_showed() {
        let outputs = [
            json!({"output_type": "stream", "name": "stdout", "text": "2700 cells\n"}),
            json!({"output_type": "display_data", "data": {"image/png": "iVBOR"}}),
            json!({"output_type": "error", "ename": "KeyError", "evalue": "'x'", "traceback": []}),
        ];
        let text = outputs_text(&outputs);
        assert!(text.starts_with("2700 cells\n[a plot]\n"), "{text}");
        assert!(text.contains("KeyError"), "{text}");
        let long = [json!({"output_type": "stream", "name": "stdout", "text": "é".repeat(5000)})];
        assert!(outputs_text(&long).len() <= 4000);
    }
}
