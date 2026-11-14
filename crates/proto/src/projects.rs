//! Projects and their settings: where the user is, settings, setup, found environments and trusted folders.

use super::*;

/// Where the user is in a project window.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Focus {
    /// The document in front, absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Its current cell: the chunk with the cursor, or the selected
    /// notebook cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    /// The cursor's line, 1-based (in a notebook, within the cell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The selected text, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    /// Every document open in the window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open: Vec<String>,
    /// The table currently in front; absent when focus is elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableFocus>,
    /// When the window said so; set by the daemon.
    #[serde(default)]
    pub ts_ms: i64,
}

/// Where a project's notebooks and Quarto documents run their code.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentsRunIn {
    /// The document's own folder, where `quarto render` runs it.
    #[default]
    Document,
    /// The project's folder, like the Console and scripts.
    Project,
}

/// Where a project's agent conversations are kept.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatHistory {
    /// In the project's folder (`.sandhill/agents/`), so they go with it to
    /// another machine.
    #[default]
    Project,
    /// On the machine the project is on, in the daemon's own folder.
    Machine,
}

/// A project's shared settings, kept in `.sandhill/project.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSettings {
    /// The kernel R runs in, by name. Without one, the kernel of the
    /// environment the project's files point to, else the usual one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
    /// The kernel Python runs in, as for `r`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    #[serde(default)]
    pub documents: DocumentsRunIn,
    /// Files of `NAME=value` lines, relative to the project's folder, whose
    /// variables its kernels start with. Missing files are skipped.
    #[serde(default = "default_env_files")]
    pub env_files: Vec<String>,
    #[serde(default)]
    pub chat_history: ChatHistory,
}

fn default_env_files() -> Vec<String> {
    vec![".env".into()]
}

impl Default for ProjectSettings {
    fn default() -> Self {
        ProjectSettings {
            r: None,
            python: None,
            documents: DocumentsRunIn::default(),
            env_files: default_env_files(),
            chat_history: ChatHistory::default(),
        }
    }
}

/// A project's settings and what its folder suggests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSetup {
    pub settings: ProjectSettings,
    /// Whether `.sandhill/project.toml` exists.
    pub saved: bool,
    /// Why `project.toml` couldn't be read, if it couldn't; `settings` are
    /// then the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The environments the project's files point to, best first for each
    /// language.
    pub found: Vec<FoundEnvironment>,
    /// The kernel R runs in: the one in `settings`, else the first found
    /// that is installed. Without one, the usual kernel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
    /// The kernel Python runs in, as for `r`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
}

/// An environment a project's files point to, such as the conda
/// environment its `environment.yml` names.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FoundEnvironment {
    /// `r` or `python`.
    pub language: String,
    /// The file or folder that says so: `renv.lock`, `environment.yml`,
    /// `.venv`.
    pub file: String,
    /// The environment, as shown: "renv", "conda env omicverse", ".venv".
    pub label: String,
    /// The installed kernel that runs in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// What to do to get a kernel for it, when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A folder opened as a project on the daemon's machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectInfo {
    /// Absolute path, without a trailing slash.
    pub path: String,
    /// The folder's name.
    pub name: String,
    pub opened_ms: i64,
    /// False when the folder no longer exists (moved or deleted).
    pub exists: bool,
    /// What the app saved with `save_project_state`; `null` if nothing yet,
    /// and always `null` in `list_recent_projects`.
    #[serde(default)]
    pub state: Value,
    /// Whether the user trusts the folder (`trust_folder`). Daemons from
    /// before folder trust do not say, and trust every folder.
    #[serde(default = "yes")]
    pub trusted: bool,
}

fn yes() -> bool {
    true
}

/// A folder the user trusts, with the folders inside it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustedFolder {
    pub path: String,
    pub trusted_ms: i64,
    /// False when the folder no longer exists.
    pub exists: bool,
}
