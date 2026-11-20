//! Changing packages in the user's own R and Python (Huan, 2026-11-20;
//! `docs/design/clean-footprint-plan.md`, section 2): nothing installs,
//! updates or is removed until the user has seen what, with which tool,
//! in which folder, and the exact command, unless they turned on Install
//! Without Asking. Every install is written to Sandhill's record.

use serde::{Deserialize, Serialize};

/// Where packages go or leave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallTarget {
    /// One of the libraries (`.libPaths()`) of the R program `r`.
    R { r: String, library: String },
    /// The Python environment in the folder `prefix`, whose program is
    /// `python`.
    Python { prefix: String, python: String },
}

impl InstallTarget {
    /// `r` or `python`.
    pub fn language(&self) -> &'static str {
        match self {
            InstallTarget::R { .. } => "r",
            InstallTarget::Python { .. } => "python",
        }
    }
}

/// What installs: R's own installer (`install.packages()`,
/// `BiocManager::install()`, `remotes::install_github()`), mamba or conda,
/// pip, or uv.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallTool {
    R,
    Conda,
    Pip,
    Uv,
}

impl InstallTool {
    /// As the user reads it.
    pub fn label(self) -> &'static str {
        match self {
            InstallTool::R => "R's own installer",
            InstallTool::Conda => "mamba/conda",
            InstallTool::Pip => "pip",
            InstallTool::Uv => "uv",
        }
    }

    /// The record's word for it.
    pub fn id(self) -> &'static str {
        match self {
            InstallTool::R => "r",
            InstallTool::Conda => "conda",
            InstallTool::Pip => "pip",
            InstallTool::Uv => "uv",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageAction {
    Install,
    Update,
    Remove,
}

/// A change the user asks for. [`Call::PlanPackages`](crate::Call) shows
/// what it will do; [`Call::RunPackages`](crate::Call) does exactly that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    pub action: PackageAction,
    pub target: InstallTarget,
    /// As the user knows them: `DESeq2`, `scanpy`, `scikit-learn>=1.5`, or
    /// for R a GitHub `owner/repo`.
    pub packages: Vec<String>,
    /// `None`: the suggested tool for the target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<InstallTool>,
    /// Why, in plain words, kept in the record: "Find Changed Genes
    /// needed it", "Installed from the Packages pane".
    #[serde(default)]
    pub reason: String,
}

/// A tool found for a target, or on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallToolInfo {
    pub tool: InstallTool,
    /// Its program; for R's own installer, the R program.
    pub path: String,
    #[serde(default)]
    pub version: String,
}

/// One package of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPackage {
    /// As asked.
    pub name: String,
    /// As the tool is given it: `bioconductor-deseq2` for conda.
    pub spec: String,
    /// Where it comes from: `CRAN`, `Bioconductor`, `GitHub`,
    /// `conda-forge`, `bioconda` or `PyPI`.
    pub source: String,
    /// The version to be installed, or the one installed for a removal,
    /// when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What a [`PackageChange`] will do, shown before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePlan {
    /// The change, with its tool filled in.
    pub change: PackageChange,
    /// The tools that suit the target, the suggested one first.
    pub tools: Vec<InstallToolInfo>,
    /// The full path of the folder packages go into or leave.
    pub folder: String,
    pub packages: Vec<PlannedPackage>,
    /// The exact command, run as shown. Empty when `refused`.
    pub command: String,
    /// Plain-word notes: "This R builds packages from their source code,
    /// which can take a few minutes."
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Why this can't run here, in plain words, with what to do instead:
    /// "This Python is managed by Homebrew and refuses pip. Make an
    /// environment for it first."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// The last target and tool the user chose for a language, which Install
/// Without Asking uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallChoice {
    /// `r` or `python`.
    pub language: String,
    pub target: InstallTarget,
    pub tool: InstallTool,
}

/// Getting Ready's install settings, kept by the daemon for its computer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPrefs {
    /// Install Without Asking: off by default. Applies only to a language
    /// with a `last` choice.
    #[serde(default)]
    pub without_asking: bool,
    #[serde(default)]
    pub last: Vec<InstallChoice>,
}

impl InstallPrefs {
    pub fn last_for(&self, language: &str) -> Option<&InstallChoice> {
        self.last.iter().find(|c| c.language == language)
    }
}

/// One line of the record, as [`Call::RemoveInstalled`](crate::Call)
/// names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstalledKey {
    pub kind: String,
    pub name: String,
    pub place: String,
}

/// R or Python, or a tool to make them, set up in an official way when
/// the computer has none (or the user wants another). Shown first, as a
/// [`RuntimePlan`], then run by [`Call::RunRuntime`](crate::Call).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewRuntime {
    /// CRAN's R installer for macOS, downloaded and opened in Apple's
    /// Installer, which asks for the administrator's password.
    RFromCran,
    /// `brew install r`, when Homebrew is there.
    RFromHomebrew,
    /// A conda environment with R and IRkernel, from conda-forge, made
    /// with the mamba or conda Getting Ready found, at `prefix`.
    RConda { prefix: String },
    /// A conda environment with Python, ipykernel and the package `sets`
    /// (ids from [`PYTHON_SETS`](crate::PYTHON_SETS)), at `prefix`.
    PythonConda {
        prefix: String,
        python: String,
        #[serde(default)]
        sets: Vec<String>,
    },
    /// A uv environment (`uv venv`) with Python, ipykernel and the
    /// package `sets` at `prefix`, its kernel registered for Jupyter.
    PythonUv {
        prefix: String,
        python: String,
        #[serde(default)]
        sets: Vec<String>,
    },
    /// Miniforge (conda-forge's installer) into `~/miniforge3`, when there
    /// is no mamba or conda.
    Miniforge,
}

/// What a [`NewRuntime`] will do, shown before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePlan {
    pub runtime: NewRuntime,
    /// The tool that does it: `~/miniforge3/bin/mamba`, `brew`, `uv`, or
    /// Apple's Installer.
    pub tool: String,
    /// The full path of the folder it goes into.
    pub folder: String,
    /// The exact command, run as shown. Empty when `refused`.
    pub command: String,
    /// About how much it downloads, in MB, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}
