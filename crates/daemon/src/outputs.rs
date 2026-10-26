//! Turns a run's journaled kernel messages into nbformat 4 output objects,
//! the form notebooks store and the app renders.
//!
//! The app's UI applies the same rules to live events
//! (`app/src/model/outputs.ts`); `tests/fixtures/outputs.json` keeps the two
//! in agreement.

use sandhill_proto::{CellOutputs, FileOutputs, Plot, SessionPlots, SessionRun, SessionRuns};
use serde_json::{Map, Value, json};

use crate::journal::{Journal, Result};

/// Outputs of one run from its journaled events up to `max_seq`.
pub fn run_outputs(journal: &Journal, run_id: &str, max_seq: i64) -> Result<Vec<Value>> {
    let mut builder = OutputBuilder::new();
    for (msg_type, content) in journal.run_events(run_id, max_seq)? {
        builder.push(&msg_type, &content);
    }
    Ok(builder.finish())
}

/// The latest run of every cell of `file`, as of one journal position.
pub fn file_outputs(journal: &Journal, file: &str) -> Result<FileOutputs> {
    // Fix the position first: anything journaled after it is left for the
    // client's subscription, so nothing arrives twice or not at all.
    let seq = journal.max_seq()?;
    let mut cells = Vec::new();
    for run in journal.latest_cell_runs(file)? {
        let outputs = run_outputs(journal, &run.run_id, seq)?;
        cells.push(CellOutputs {
            cell_id: run.cell_id,
            run: run.run_id,
            session: run.session_id,
            language: run.language,
            code: run.code,
            started_ms: run.started_ms,
            finished_ms: run.finished_ms,
            status: run.status,
            execution_count: run.execution_count,
            outputs,
        });
    }
    Ok(FileOutputs {
        file: file.to_string(),
        cells,
        seq,
    })
}

/// A session's latest `limit` runs with their outputs, for its Console, as
/// of one journal position.
pub fn session_runs(journal: &Journal, session: &str, limit: u32) -> Result<SessionRuns> {
    let seq = journal.max_seq()?;
    let mut runs = Vec::new();
    for run in journal.console_runs(session, limit)? {
        let outputs = run_outputs(journal, &run.run_id, seq)?;
        runs.push(SessionRun {
            run: run.run_id,
            cell: run.cell,
            by: run.by,
            code: run.code,
            started_ms: run.started_ms,
            finished_ms: run.finished_ms,
            status: run.status,
            execution_count: run.execution_count,
            outputs,
        });
    }
    Ok(SessionRuns {
        session: session.to_string(),
        runs,
        seq,
    })
}

/// A session's latest plots, for the Plots pane: the images its runs showed.
pub fn session_plots(journal: &Journal, session: &str, limit: u32) -> Result<SessionPlots> {
    let seq = journal.max_seq()?;
    let plots = journal
        .session_plots(session, limit)?
        .into_iter()
        .filter(|p| {
            p.content["data"]
                .as_object()
                .is_some_and(|data| data.keys().any(|mime| is_plot_mime(mime)))
        })
        .map(|p| Plot {
            run: p.run_id,
            seq: p.seq,
            ts_ms: p.ts_ms,
            cell: p.cell,
            by: p.by,
            code: p.code,
            data: p.content["data"].clone(),
            metadata: p.content.get("metadata").cloned().unwrap_or(Value::Null),
        })
        .collect();
    Ok(SessionPlots {
        session: session.to_string(),
        plots,
        seq,
    })
}

/// A session's latest web pages, for the Viewer: see [`for_viewer`].
pub fn session_pages(journal: &Journal, session: &str, limit: u32) -> Result<SessionPlots> {
    let seq = journal.max_seq()?;
    // Read more than asked for, as notebooks' outputs are left out.
    let plots = journal
        .session_html(session, limit.saturating_mul(4))?
        .into_iter()
        .filter(|p| {
            for_viewer(
                p.cell.as_ref().map(|c| c.file.as_str()),
                &p.content["data"],
                &p.content["metadata"],
            )
        })
        .map(|p| Plot {
            run: p.run_id,
            seq: p.seq,
            ts_ms: p.ts_ms,
            cell: p.cell,
            by: p.by,
            code: p.code,
            data: p.content["data"].clone(),
            metadata: p.content.get("metadata").cloned().unwrap_or(Value::Null),
        })
        .collect::<Vec<_>>();
    let skip = plots.len().saturating_sub(limit as usize);
    Ok(SessionPlots {
        session: session.to_string(),
        plots: plots.into_iter().skip(skip).collect(),
        seq,
    })
}

/// Whether the Viewer shows an output of a run from `file` (none for the
/// Console): a page from the Console or a script, or an app's address from
/// anywhere, but not a web page opened in the browser. Notebooks keep their other pages under their cells.
/// `forViewer` in `app/src/model/pages.ts` says the same.
pub fn for_viewer(file: Option<&str>, data: &Value, metadata: &Value) -> bool {
    // A web page R opened in the browser.
    if data[crate::viewer::MIME]["browser"] == Value::Bool(true) {
        return false;
    }
    if file.is_some_and(|f| !is_script(f)) {
        return data[crate::viewer::MIME]["url"].is_string();
    }
    is_page(data, metadata)
}

/// Whether an output is a web page rather than HTML for the Console (a
/// table): it names a page (see `viewer`), is a plotly figure, runs
/// scripts (htmlwidgets, plotly), says it wants a frame of its own
/// (IRkernel's `isolated`), or is large enough to be kept as a blob.
pub fn is_page(data: &Value, metadata: &Value) -> bool {
    if data.get(crate::viewer::MIME).is_some() || data.get(PLOTLY_MIME).is_some() {
        return true;
    }
    let Some(html) = data.get("text/html") else {
        return false;
    };
    if html.get(sandhill_proto::BLOB_KEY).is_some()
        || metadata["text/html"]["isolated"] == Value::Bool(true)
    {
        return true;
    }
    let text = match html {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => return false,
    };
    text.to_ascii_lowercase().contains("<script")
}

/// A plotly figure as data, which Python's plotly shows in Jupyter.
pub const PLOTLY_MIME: &str = "application/vnd.plotly.v1+json";

/// A script's file (`.R`, `.py`), whose code runs as the Console's does.
fn is_script(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".r") || lower.ends_with(".py")
}

/// The image types the Plots pane shows.
pub fn is_plot_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/svg+xml"
    )
}

#[derive(Debug, Default)]
pub struct OutputBuilder {
    outputs: Vec<Output>,
    /// A `clear_output` with `wait: true` clears when the next output arrives.
    clear_pending: bool,
}

#[derive(Debug)]
struct Output {
    value: Value,
    /// `transient.display_id`, which `update_display_data` targets.
    display_id: Option<String>,
}

impl OutputBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one journaled event. Events that aren't outputs are ignored.
    pub fn push(&mut self, msg_type: &str, content: &Value) {
        match msg_type {
            "clear_output" => {
                if content["wait"].as_bool().unwrap_or(false) {
                    self.clear_pending = true;
                } else {
                    self.outputs.clear();
                    self.clear_pending = false;
                }
            }
            "update_display_data" => {
                let Some(id) = display_id(content) else {
                    return;
                };
                for output in &mut self.outputs {
                    if output.display_id.as_deref() == Some(id) {
                        output.value["data"] = content["data"].clone();
                        output.value["metadata"] = metadata(content);
                    }
                }
            }
            "stream" => {
                self.take_pending_clear();
                let name = content["name"].as_str().unwrap_or("stdout");
                let text = content["text"].as_str().unwrap_or_default();
                if let Some(last) = self.outputs.last_mut()
                    && last.value["output_type"] == "stream"
                    && last.value["name"] == name
                {
                    let joined = format!("{}{text}", last.value["text"].as_str().unwrap_or(""));
                    last.value["text"] = Value::String(joined);
                    return;
                }
                self.add(
                    json!({"output_type": "stream", "name": name, "text": text}),
                    None,
                );
            }
            "display_data" => {
                self.take_pending_clear();
                let value = json!({
                    "output_type": "display_data",
                    "data": content["data"].clone(),
                    "metadata": metadata(content),
                });
                self.add(value, display_id(content).map(str::to_string));
            }
            "execute_result" => {
                self.take_pending_clear();
                let value = json!({
                    "output_type": "execute_result",
                    "execution_count": content["execution_count"].clone(),
                    "data": content["data"].clone(),
                    "metadata": metadata(content),
                });
                self.add(value, display_id(content).map(str::to_string));
            }
            "error" => {
                self.take_pending_clear();
                self.add(
                    json!({
                        "output_type": "error",
                        "ename": content["ename"].clone(),
                        "evalue": content["evalue"].clone(),
                        "traceback": content["traceback"].clone(),
                    }),
                    None,
                );
            }
            _ => {}
        }
    }

    pub fn finish(self) -> Vec<Value> {
        self.outputs.into_iter().map(|o| o.value).collect()
    }

    fn add(&mut self, value: Value, display_id: Option<String>) {
        self.outputs.push(Output { value, display_id });
    }

    fn take_pending_clear(&mut self) {
        if self.clear_pending {
            self.outputs.clear();
            self.clear_pending = false;
        }
    }
}

fn display_id(content: &Value) -> Option<&str> {
    content["transient"]["display_id"].as_str()
}

fn metadata(content: &Value) -> Value {
    match &content["metadata"] {
        Value::Object(m) => Value::Object(m.clone()),
        _ => Value::Object(Map::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared with the UI's tests so both sides build identical outputs.
    #[test]
    fn matches_shared_fixtures() {
        let text = include_str!("../../../tests/fixtures/outputs.json");
        let cases: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let mut builder = OutputBuilder::new();
            for event in case["events"].as_array().unwrap() {
                builder.push(event["msg_type"].as_str().unwrap(), &event["content"]);
            }
            assert_eq!(
                Value::Array(builder.finish()),
                case["outputs"],
                "case {}",
                case["name"]
            );
        }
    }

    /// Shared with the UI's tests so both sides send the same pages to the
    /// Viewer.
    #[test]
    fn the_viewer_takes_the_shared_fixtures_pages() {
        let text = include_str!("../../../tests/fixtures/pages.json");
        let cases: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            assert_eq!(
                for_viewer(case["file"].as_str(), &case["data"], &case["metadata"]),
                case["viewer"].as_bool().unwrap(),
                "case {}",
                case["name"]
            );
        }
    }
}
