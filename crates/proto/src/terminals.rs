//! Terminals, jobs and agents: terminal info and events, agent accounts and options, and what agents ask of windows.

use super::*;

/// A cell an agent added or changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellEdited {
    pub file: String,
    /// The cell's id now: a notebook cell id, a chunk label or
    /// `chunk-<n>`. For a script, empty, or the console entry the window
    /// runs the code as.
    pub cell_id: String,
    /// Lower case: `r`, `python`.
    pub language: String,
    /// True when an open window made the edit; it then also runs the cell
    /// if asked to, as if the user pressed Run.
    #[serde(default)]
    pub in_window: bool,
    /// The kernel the document asks for (a notebook's kernelspec, a Quarto
    /// document's `jupyter:`), for whoever runs the cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
}

/// Something the daemon asks a window to do (see [`Call::AddCell`]). The
/// window that works on `project` answers with [`Call::Answer`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppRequest {
    pub id: u64,
    pub project: String,
    /// `add_cell` or `edit_cell`, with that call's params.
    pub method: String,
    pub params: Value,
}

/// The `kind` of a plain shell terminal.
pub const TERMINAL_SHELL: &str = "shell";
/// The kind of terminal a document's preview runs in (`quarto preview`).
pub const TERMINAL_PREVIEW: &str = "preview";
/// The kind of terminal a job runs in (see [`Call::StartJob`]).
pub const TERMINAL_JOB: &str = "job";
/// The kind of terminal an agent's own sign-in runs in (see
/// [`Call::SignInAgentAccount`]).
pub const TERMINAL_LOGIN: &str = "login";

/// What a job runs: a script, with the program for its kind, or a command
/// line, which the user's shell runs as their Terminal would.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    /// A script: R runs with `Rscript` and Python with `python`, each from
    /// the environment of the project's kernel for it; shell scripts with
    /// `bash`, Nextflow pipelines with `nextflow run`, Snakefiles with
    /// `snakemake`; `.qmd`/`.Rmd` with `quarto render`; any other file
    /// runs as a program. Quarto uses a fresh process, not live variables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A command line; with `path`, the arguments that follow the script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// A document's preview (see [`Call::PreviewDocument`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreviewInfo {
    /// Where its page is on the daemon's machine: `http://127.0.0.1:{port}/`.
    pub url: String,
    /// The terminal `quarto preview` runs in; closing it ends the preview.
    pub terminal: TerminalInfo,
}

/// A coding agent the daemon can start in a terminal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentInfo {
    /// What `start_terminal` takes: `claude`, `codex`.
    pub kind: String,
    /// "Claude Code", "Codex".
    pub name: String,
    /// The CLI found on this machine; `None` when it isn't installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A shell command that installs the CLI, for when it is missing.
    pub install: String,
}

/// An account an agent can run a conversation on (see
/// [`Call::ListAgentAccounts`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentAccount {
    /// The agent's kind (`claude`) for its own login; `<kind>:<slug>` for
    /// one added in Sandhill. What [`AgentOptions::account`] takes.
    pub id: String,
    pub kind: String,
    /// "Default" for the agent's own login, else the name it was given.
    pub name: String,
    /// `login` (the agent's own sign-in) or `api_key` (Antigravity).
    pub method: String,
    /// The agent's own login: `~/.claude`, `~/.codex`, Antigravity's.
    #[serde(default)]
    pub default: bool,
    /// Whether it is signed in; `None` when that could not be checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signed_in: Option<bool>,
    /// The email or account name the agent reports, when signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Its last known use, as conversations and status queries report it:
    /// `{rate_limits: {<window>: {used_percentage, resets_at?}}, observed_at_ms,
    /// reset_credits?}`. `reset_credits` is Codex's banked resets, `{available,
    /// expires_at}` (the first expiry, Unix seconds, or null); Claude Code has
    /// no official way to read its limit resets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

/// How to start an agent. Empty fields leave the CLI's own defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentOptions {
    /// A model name or alias the CLI accepts (`opus`, `gpt-5-codex`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Thinking effort: `low`, `medium`, `high`...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The agent's native mode: `supervised` (its default), `readonly`,
    /// `edits`, `auto`, `full` or `plan`. Unsupported or unknown values
    /// use its default; pi ignores this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// Documents the session is about, named to the agent at start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<String>,
    /// Where the agent's code goes: `console` runs what `add_cell` is given
    /// in the Console instead of adding it to the document. Unset is the
    /// document. A `set` command (`{kind: "set", code}`) changes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The file the user pinned the conversation to, relative to the
    /// project unless outside it. Unset, it works in the editor's front
    /// document. A `set` command (`{kind: "set", pin}`) changes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// What the user added to the first message with the @ menu (files,
    /// chunks, variables, plots), which that message takes. A message
    /// later takes them from a `set` or `say` command's `refs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<Value>,
    /// The account it runs on (an [`AgentAccount::id`]); unset is the
    /// agent's own login. `restart_terminal` with another account switches
    /// a conversation, which goes on where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Codex only: start a new thread from this source terminal's conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<String>,
}

impl AgentOptions {
    pub fn is_empty(&self) -> bool {
        *self == AgentOptions::default()
    }
}

/// A terminal the daemon keeps: a shell or an agent CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalInfo {
    pub id: String,
    /// [`TERMINAL_SHELL`] or an agent's kind.
    pub kind: String,
    /// "Terminal", "Claude Code", "Codex".
    pub title: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub created_ms: i64,
    /// When its program last started: later than `created_ms` once it
    /// has been restarted. An exit from before this is the old program's.
    #[serde(default)]
    pub started_ms: i64,
    /// True while its program runs.
    pub running: bool,
    /// How the program ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<TerminalExit>,
    /// Bytes of output so far: where new output will start.
    pub end: u64,
    #[serde(default, skip_serializing_if = "AgentOptions::is_empty")]
    pub options: AgentOptions,
    /// What the agent last reported about itself (see [`TerminalEvent::agent`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Value>,
    /// An agent started by another build of sandhilld than the one running,
    /// such as before an update. It keeps that build's tools and hooks
    /// until it is started again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
    /// The name the user gave it (see [`Call::RenameTerminal`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For a job: what it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobSpec>,
    /// Successful Quarto render outputs, absolute paths, retained on reconnect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalExit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub ended_ms: i64,
    /// The daemon stopped this run after prolonged inactivity.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalSubscribed {
    pub terminal: String,
    /// Where the replayed output starts. It can be later than the `from`
    /// asked for when that output is no longer kept.
    pub from: u64,
}

/// News from a terminal. Each carries one of `data`, `agent` or `exit`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TerminalEvent {
    pub terminal: String,
    /// Where `data` starts in the terminal's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Output bytes, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Historical output reconstructs the display, but must never answer
    /// terminal queries back to the running program.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replay: bool,
    /// What an agent reported through its hooks and status line:
    /// `{state: "busy" | "waiting" | "idle", mode, status: {model, context,
    /// effort, rate_limits...}}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Value>,
    /// An entry of the agent's conversation, for subscriptions that asked
    /// for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<ChatEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<TerminalExit>,
    /// Successful Quarto render outputs, also retained in TerminalInfo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_paths: Vec<String>,
}

/// One entry of an agent's conversation, as its hooks reported it. `seq`
/// counts from 0 per terminal. `event` has a `kind`: `prompt`, `text`,
/// `step`, `step_done`, `step_failed`, `ask`, `answered`, `stop` or
/// `session` (see `docs/protocol.md`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatEntry {
    pub seq: u64,
    pub event: Value,
}

/// What [`Call::SuggestCell`] got: the next cell's code, or none when the
/// agent saw no useful next step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CellSuggestion {
    /// The agent that answered (its kind).
    pub agent: String,
    pub code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::AgentOptions;
    use serde_json::json;

    #[test]
    fn saved_options_ignore_the_removed_web_choice() {
        let options: AgentOptions = serde_json::from_value(json!({
            "web": false,
            "permission": "ask",
            "model": "opus"
        }))
        .unwrap();
        assert_eq!(options.permission.as_deref(), Some("ask"));
        assert_eq!(options.model.as_deref(), Some("opus"));
        assert!(serde_json::to_value(options).unwrap().get("web").is_none());
        let options: AgentOptions =
            serde_json::from_value(json!({"fork": "source-terminal"})).unwrap();
        assert_eq!(options.fork.as_deref(), Some("source-terminal"));
        assert_eq!(
            serde_json::to_value(options).unwrap(),
            json!({"fork": "source-terminal"})
        );
        assert!(
            serde_json::to_value(AgentOptions::default())
                .unwrap()
                .get("fork")
                .is_none()
        );
    }
}
