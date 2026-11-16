//! Skills installed on a device for every agent (Huan, 2026-11-16;
//! `docs/design/done/skills-plan.md`). A skill is a folder with a `SKILL.md`;
//! the daemon keeps its own copy and hands it to agents through the
//! `get_skill` tool, never through the agents' own folders.

use serde::{Deserialize, Serialize};

/// Where an installed skill came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillSource {
    /// A git repository (GitHub, or any address git takes). `path` is the
    /// skill's folder inside it (`""` for the top), `commit` the commit
    /// installed; `suggested` names the entry of Sandhill's list it came
    /// from.
    Git {
        url: String,
        path: String,
        commit: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggested: Option<String>,
    },
    /// Files sent from the Mac: a folder or a zip file, named by `label`.
    Upload { label: String },
}

/// A skill installed on the device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillInfo {
    /// Its id and folder name: the `SKILL.md` name, made safe.
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    /// `Apache-2.0`, `MIT`… when recognised, `LICENSE` (or the file's name)
    /// when not; none without a licence file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    /// Off: kept, but not offered to agents.
    pub enabled: bool,
    pub installed_ms: i64,
    pub files: u32,
    pub bytes: u64,
    /// Files agents could run (scripts, executables), relative paths.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// The folder on the device.
    pub path: String,
}

/// An entry of Sandhill's own list (`Suggested`), fetched only on Add.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestedSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub url: String,
    pub path: String,
    pub commit: String,
    pub licence: String,
    /// Already installed under the name its `SKILL.md` gives.
    pub installed: bool,
}

/// One of Sandhill's built-in bioinformatics guides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuiltInSkill {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The answer to [`crate::Call::ListSkills`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SkillList {
    /// By name.
    pub installed: Vec<SkillInfo>,
    /// In the list's order.
    pub suggested: Vec<SuggestedSkill>,
    pub built_in: Vec<BuiltInSkill>,
}

/// One file of a skill sent over the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillFile {
    /// Relative to the skill's folder, `/`-separated.
    pub path: String,
    pub base64: String,
    #[serde(default)]
    pub executable: bool,
}

/// What to fetch for [`crate::Call::PreviewSkills`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillFetch {
    /// A repository as the user typed it: `owner/repo`, a GitHub address
    /// (`…/tree/<ref>/<folder>` picks a branch and folder) or any git URL.
    /// `commit` pins one.
    Git {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    /// An entry of Sandhill's list, at its pinned commit.
    Suggested { id: String },
    /// An installed skill's repository at its newest commit (Check for
    /// Updates…).
    Update { name: String },
    /// A zip file's bytes.
    Zip { label: String, base64: String },
    /// A folder's files (a folder on the Mac, or Copy to… from another
    /// device, which keeps the skill's `source`).
    Files {
        label: String,
        files: Vec<SkillFile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SkillSource>,
    },
}

/// A skill found in what was fetched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillCandidate {
    /// The name it would be installed under.
    pub name: String,
    pub description: String,
    /// Its folder inside what was fetched (`""` for the top).
    pub path: String,
    /// Every file, relative, sorted.
    pub files: Vec<String>,
    pub bytes: u64,
    /// Files agents could run.
    pub scripts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    /// A skill of this name is installed and would be replaced.
    pub replaces: bool,
    /// Its files differ from the installed one's (true when none is).
    pub changed: bool,
}

/// The answer to [`crate::Call::PreviewSkills`]: what would be installed,
/// held on the device under `token` until installed or dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillPreview {
    pub token: String,
    /// Where it comes from, for the card: `github.com/owner/repo @ 1a2b3c4`,
    /// a zip's or folder's name.
    pub source: String,
    pub skills: Vec<SkillCandidate>,
    /// Folders with a `SKILL.md` that cannot be installed, and why.
    #[serde(default)]
    pub refused: Vec<SkillRefusal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillRefusal {
    pub path: String,
    pub reason: String,
}

/// The answer to [`crate::Call::ExportSkill`]: everything Copy to… sends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillExport {
    pub name: String,
    pub source: SkillSource,
    pub files: Vec<SkillFile>,
}
