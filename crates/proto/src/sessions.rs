//! Kernel sessions and their runs: kernelspecs, session info, history, outputs and plots.

use super::*;

/// Which notebook cell a piece of code came from, so clients can put outputs
/// back under the right cell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CellRef {
    pub file: String,
    pub cell_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionReply {
    pub matches: Vec<String>,
    pub cursor_start: usize,
    pub cursor_end: usize,
    pub metadata: Value,
}

/// A kernel's `inspect_reply`: `data` is its MIME bundle (`text/plain`…).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InspectReply {
    pub found: bool,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KernelspecInfo {
    pub name: String,
    /// Sandhill's name for it, made by [`kernel_label`].
    pub display_name: String,
    pub language: String,
    /// The environment the kernel runs in (a conda env, a venv, a pyenv
    /// version), so a notebook that names its environment finds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// The R or Python version it runs, as that program says it (`4.6.2`,
    /// `3.12.4`); none when it couldn't be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// For R, which kernel it is: `Ark` or `IRkernel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
}

/// How Sandhill names a kernel everywhere (Huan, 2026-11-15): the
/// language, its version, the environment in brackets and, for R, the
/// kernel after a dot. "R 4.6.2 · Ark", "R 4.4.1 (omicverse) · IRkernel",
/// "Python 3.12 (omicverse)", "Python 3.13". Python shows two parts of its
/// version, R three. An environment named for its version (a pyenv
/// version such as `3.12.4`) adds nothing and is left out.
pub fn kernel_label(
    language: &str,
    version: Option<&str>,
    env: Option<&str>,
    engine: Option<&str>,
) -> String {
    let mut label = language_label(language);
    if let Some(version) = version
        .map(|said| version_number(&label, said))
        .filter(|v| !v.is_empty())
    {
        label = format!("{label} {version}");
    }
    if let Some(env) = env.filter(|env| env.chars().next().is_some_and(|c| !c.is_ascii_digit())) {
        label = format!("{label} ({env})");
    }
    if let Some(engine) = engine.filter(|engine| !engine.is_empty()) {
        label = format!("{label} · {engine}");
    }
    label
}

/// "R", "Python", or the language as the kernel names it, capitalised.
pub fn language_label(language: &str) -> String {
    match language.to_lowercase().as_str() {
        "r" => "R".into(),
        "python" | "python3" => "Python".into(),
        _ => {
            let mut chars = language.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// The version number in what a program says of itself ("R version 4.6.2
/// (2025-10-31)", "Python 3.12.4", "3.12.4 | packaged by conda-forge").
pub fn version_in(said: &str) -> Option<String> {
    said.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|w| w.contains('.') && w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|w| w.trim_end_matches('.').to_string())
}

/// The version as shown: two parts of it for Python, all of it for R.
fn version_number(language: &str, said: &str) -> String {
    let number = version_in(said).unwrap_or_default();
    if language == "Python" {
        number.split('.').take(2).collect::<Vec<_>>().join(".")
    } else {
        number
    }
}

/// An R installation without IRkernel (see [`Call::ListRWithoutIrkernel`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RWithoutIrkernel {
    /// The R program, as found: `/opt/homebrew/bin/R`.
    pub r: String,
    /// Its version, like `4.6.2`; empty when R didn't say.
    pub version: String,
    /// The environment it belongs to, like `conda env omicverse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// What installing does, in a sentence, for the user to confirm.
    pub plan: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Running,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub kernel: String,
    pub language: String,
    pub cwd: String,
    pub pid: Option<u32>,
    pub status: SessionStatus,
    pub created_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    /// Highest event sequence number journaled for this session (0 if none).
    pub last_seq: i64,
    /// The project folder the session was started for, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Runs sent to the kernel that have not finished: the one running and
    /// any queued behind it. Always 0 once the session has ended.
    #[serde(default)]
    pub pending_runs: u32,
    /// The language's version as the kernel reports it, like "4.4.1" for R.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunStarted {
    pub run: String,
    pub msg_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Subscribed {
    pub session: String,
    /// Events with a higher sequence number than this will be delivered.
    pub from_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryPage {
    pub events: Vec<Event>,
    /// Pass as `since_seq` to fetch the next page; equal to the request's
    /// `since_seq` when there is nothing more.
    pub next_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Blob {
    pub hash: String,
    /// The original JSON string value (base64 for binary MIME types).
    pub data: String,
}

/// Outputs of the latest run of each cell of one file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileOutputs {
    pub file: String,
    pub cells: Vec<CellOutputs>,
    /// The journal position these outputs reflect. Subscribe with this as
    /// `since_seq` to receive what happens next without gaps or repeats.
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellOutputs {
    pub cell_id: String,
    pub run: String,
    /// Empty when the outputs were recovered from a sidecar, not the journal.
    pub session: String,
    pub language: String,
    /// The source that produced these outputs, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub started_ms: i64,
    /// `None` while the run is queued or executing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u64>,
    /// nbformat 4 output objects. Large values are blob references.
    pub outputs: Vec<Value>,
}

/// A session's runs, for its Console.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRuns {
    pub session: String,
    pub runs: Vec<SessionRun>,
    /// The journal position these runs reflect. Subscribe with this as
    /// `since_seq` to receive what happens next without gaps or repeats.
    pub seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRun {
    pub run: String,
    /// The document cell the code came from; `None` for code typed in the
    /// Console or run by an agent's `run_code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellRef>,
    /// The agent that ran it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub code: String,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u64>,
    /// nbformat 4 output objects. Large values are blob references.
    pub outputs: Vec<Value>,
}

/// A session's plots, for the Plots pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionPlots {
    pub session: String,
    pub plots: Vec<Plot>,
    /// The journal position these plots reflect.
    pub seq: i64,
}

/// Whether a plot can be drawn again, and why not.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlotRedraw {
    pub can: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// With `show`: the PNG drawn, in base64. It isn't kept, so resizing
    /// a pane leaves nothing behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub png: Option<String>,
}

/// An image a run showed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Plot {
    pub run: String,
    /// Where the image is in the journal.
    pub seq: i64,
    pub ts_ms: i64,
    /// The document cell the run came from; `None` for the Console or an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<CellRef>,
    /// The agent that ran it, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The run's code.
    pub code: String,
    /// The output's MIME bundle: images as blob references (SVG inline when
    /// small), and its text.
    pub data: Value,
    #[serde(default)]
    pub metadata: Value,
}

/// Content of a [`EVENT_RUN_FINISHED`] event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunFinished {
    /// `ok`, `error` or `aborted`, as reported by the kernel.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u64>,
}
