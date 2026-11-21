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

/// Where Sandhill's R and Python come from (Huan, 2026-11-20;
/// `docs/design/clean-footprint-plan.md`, section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvMode {
    /// The R and Python already on this computer; installs ask first.
    Existing,
    /// Sandhill's own R and Python in one folder, kept apart: every
    /// kernel, the Terminal, jobs and the agents' commands use only them.
    Own,
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
    /// `None` until the user chooses; Sandhill then behaves as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<EnvMode>,
    /// The folder of Sandhill's own R and Python (see [`OwnSetup`]);
    /// empty until one is set up.
    #[serde(default)]
    pub own_folder: String,
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

/// The folder Sandhill's own R and Python go in, under the home folder,
/// unless the user picks another.
pub const OWN_FOLDER: &str = ".sandhill/environment";

/// A task kit (Huan, 2026-11-20; `docs/design/clean-footprint-plan.md`,
/// section 9): what a beginner ticks instead of packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kit {
    pub id: &'static str,
    pub label: &'static str,
    /// One plain line under the tick.
    pub about: &'static str,
    /// R packages: `(name, from Bioconductor)`.
    pub r: &'static [(&'static str, bool)],
    /// Python packages: `(conda name, PyPI name)`.
    pub python: &'static [(&'static str, &'static str)],
    /// Programs only conda has (Quarto): installed in Sandhill's own
    /// environment; elsewhere the review says where to get them.
    pub conda_only: &'static [&'static str],
    /// About how much it adds to the download, in MB, from dry runs on
    /// an Apple-silicon Mac (2026-11-20).
    pub download_mb: u64,
}

/// Always installed, whichever kits are ticked.
pub const CORE_KIT: Kit = Kit {
    id: "core",
    label: "Core",
    about: "What Sandhill itself needs: the kernels and Explore a Table's plots.",
    r: &[
        ("IRkernel", false),
        ("jsonlite", false),
        ("ggplot2", false),
        ("ggrepel", false),
        ("pheatmap", false),
        ("readxl", false),
        ("writexl", false),
        ("svglite", false),
        ("BiocManager", false),
        ("remotes", false),
    ],
    python: &[
        ("ipykernel", "ipykernel"),
        ("numpy", "numpy"),
        ("pandas", "pandas"),
        ("matplotlib", "matplotlib"),
    ],
    conda_only: &[],
    download_mb: 384,
};

/// The five kits, in the order shown (Huan, 2026-11-20).
pub const KITS: &[Kit] = &[
    Kit {
        id: "bulk",
        label: "Bulk RNA-seq",
        about: "Find changed genes, handle batches, read Salmon or kallisto output, gene symbols for human and mouse.",
        r: &[
            ("DESeq2", true),
            ("edgeR", true),
            ("limma", true),
            ("apeglm", true),
            ("sva", true),
            ("RUVSeq", true),
            ("tximport", true),
            ("AnnotationDbi", true),
            ("org.Hs.eg.db", true),
            ("org.Mm.eg.db", true),
        ],
        python: &[],
        conda_only: &[],
        download_mb: 155,
    },
    Kit {
        id: "singlecell-r",
        label: "Single-cell in R",
        about: "Seurat, joining batches, SCTransform, the Bioconductor single-cell tools, finding doublets.",
        r: &[
            ("Seurat", false),
            ("SeuratObject", false),
            ("harmony", false),
            ("glmGamPoi", true),
            ("SingleCellExperiment", true),
            ("scater", true),
            ("scran", true),
            ("scDblFinder", true),
        ],
        python: &[],
        conda_only: &[],
        download_mb: 286,
    },
    Kit {
        id: "singlecell-python",
        label: "Single-cell in Python",
        about: "Scanpy, clustering, joining batches, plots.",
        r: &[],
        python: &[
            ("scanpy", "scanpy"),
            ("anndata", "anndata"),
            ("python-igraph", "igraph"),
            ("leidenalg", "leidenalg"),
            ("harmonypy", "harmonypy"),
            ("seaborn", "seaborn"),
            ("scikit-learn", "scikit-learn"),
        ],
        conda_only: &[],
        download_mb: 101,
    },
    Kit {
        id: "plots",
        label: "Plots and reports",
        about: "Figures for papers, panels side by side, R Markdown and Quarto reports.",
        r: &[
            ("ComplexHeatmap", true),
            ("patchwork", false),
            ("cowplot", false),
            ("ggpubr", false),
            ("rmarkdown", false),
            ("knitr", false),
        ],
        python: &[("seaborn", "seaborn")],
        conda_only: &["quarto"],
        download_mb: 262,
    },
    Kit {
        id: "pathways",
        label: "Pathways and gene sets",
        about: "GO, KEGG and Reactome enrichment, GSEA, MSigDB gene sets, pathway scores.",
        r: &[
            ("clusterProfiler", true),
            ("enrichplot", true),
            ("msigdbr", false),
            ("fgsea", true),
            ("GSVA", true),
            ("ReactomePA", true),
        ],
        python: &[("gseapy", "gseapy"), ("decoupler-py", "decoupler")],
        conda_only: &[],
        download_mb: 180,
    },
];

/// What "Everything in one folder" sets up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnSetup {
    /// The full path of the folder; it must be empty or Sandhill's own,
    /// and have no spaces (conda can't use them).
    pub folder: String,
    /// Set up R (`sandhill-r`).
    pub r: bool,
    /// Set up Python (`sandhill-python`).
    pub python: bool,
    /// [`Kit::id`]s besides the core. Run again with more kits to add
    /// them; what is there already stays.
    #[serde(default)]
    pub kits: Vec<String>,
}

/// One step of a set-up, shown before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    /// In plain words: "Download micromamba, conda's one-file installer".
    pub what: String,
    /// The exact command.
    pub command: String,
}

/// What an [`OwnSetup`] will do, shown on the Review step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnSetupPlan {
    pub setup: OwnSetup,
    pub steps: Vec<SetupStep>,
    /// What goes into `sandhill-r` and `sandhill-python`, and from where.
    #[serde(default)]
    pub r_packages: Vec<PlannedPackage>,
    #[serde(default)]
    pub python_packages: Vec<PlannedPackage>,
    /// About how much it downloads, in MB (nothing already there counts).
    pub download_mb: u64,
    /// Free space where the folder goes, in MB.
    pub free_mb: u64,
    /// About how long, in minutes, on a typical connection.
    pub minutes: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// A package has to be built on this computer and Apple's developer
    /// tools are missing: the Review offers Install Developer Tools, and
    /// without them the set-up goes on without that package.
    #[serde(default)]
    pub needs_developer_tools: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// Sandhill's own R or Python, as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnRuntime {
    /// `4.5.3`, `3.12.16`.
    pub version: String,
    /// The environment's folder.
    pub prefix: String,
    /// Its kernel's name, as kernel pickers show it.
    #[serde(default)]
    pub kernel: String,
}

/// Sandhill's own R and Python folder, for Getting Ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnEnvironment {
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<OwnRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<OwnRuntime>,
    /// The kits whose packages are all there.
    #[serde(default)]
    pub kits: Vec<String>,
    /// Its size on disk, in MB.
    #[serde(default)]
    pub size_mb: u64,
}

/// The last change Sandhill made to an environment, which Undo Last
/// Change puts back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoInfo {
    /// In plain words: "Installed DESeq2 and 12 packages it needed".
    pub what: String,
    /// `2026-11-20`.
    pub date: String,
}

/// One package Check and Repair found wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvProblem {
    pub package: String,
    /// In plain words: "Built for R 4.5; this is R 4.6.", "Needs
    /// Rcpp, which isn't installed.", "Its compiled part can't load."
    pub reason: String,
    /// The change that repairs it, planned like any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<PackageChange>,
}

/// What Check and Repair found last time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvCheck {
    /// `2026-11-20`.
    pub date: String,
    /// How many packages were loaded.
    pub checked: u32,
    /// Why the kernel didn't start, in plain words; `None` when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    #[serde(default)]
    pub problems: Vec<EnvProblem>,
}

/// One R or Python on this computer, as the R and Python page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvCard {
    /// Stable while it exists: `r:<R program>` or `python:<prefix>`.
    pub id: String,
    /// `r` or `python`.
    pub language: String,
    /// As kernel pickers name it: "R 4.6.2", "Python 3.12 (omicverse)".
    pub name: String,
    /// Where it comes from: `own` (Sandhill's folder), `conda`, `venv`,
    /// `uv`, `homebrew`, `cran`, `python.org`, `system`, `other`.
    pub kind: String,
    pub program: String,
    /// Python: the environment's folder. R: `R.home()`.
    pub folder: String,
    /// R: its libraries, in R's order.
    #[serde(default)]
    pub libraries: Vec<crate::RLibrary>,
    /// Its kernel's name, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// One plain line: "Ready for Explore a Table and bulk RNA-seq",
    /// "2 packages missing for single-cell", "Needs repair: 3 packages
    /// were built for R 4.5".
    pub status: String,
    /// The kits whose packages are all there.
    #[serde(default)]
    pub kits: Vec<String>,
    /// R packages built for another R version.
    #[serde(default)]
    pub needs_repair: Vec<String>,
    /// Its size on disk, in MB (its libraries' for R).
    #[serde(default)]
    pub size_mb: u64,
    /// How many packages it has.
    #[serde(default)]
    pub packages: u32,
    /// Sandhill made it (its record says so).
    #[serde(default)]
    pub by_sandhill: bool,
    /// The tools that change its packages, the suggested one first.
    #[serde(default)]
    pub tools: Vec<InstallToolInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<EnvCheck>,
}

/// A package Add Packages found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHit {
    /// As it is installed with `tool`.
    pub name: String,
    /// Its one-line description.
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// `CRAN`, `Bioconductor`, `conda-forge`, `bioconda`, `PyPI`,
    /// `GitHub` or `This file`.
    pub source: String,
    pub tool: InstallTool,
    /// Already installed in this environment, at this version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
}

/// What the R and Python page's More menu, Check and Repair and Undo Last
/// Change do. Shown as an [`EnvActionPlan`] first, then run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvAction {
    /// Load every package and start the kernel; the result becomes the
    /// card's `check`.
    Check { env: String },
    /// Put back the packages as they were before Sandhill's last change.
    Undo { env: String },
    /// A copy of a conda environment or a venv, at `prefix`.
    Duplicate { env: String, prefix: String },
    /// Write its package list to `path`: `environment.yml` for conda,
    /// `requirements.txt` for a venv, a `.csv` of R packages for R.
    Export { env: String, path: String },
    /// From a file [`EnvAction::Export`] writes (or the usual ones): an
    /// `environment.yml` makes a new conda environment at `prefix`; a
    /// `requirements.txt` or an R package list installs into `env`.
    Import {
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        env: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
    /// Delete a conda environment or a venv (never an R installation or
    /// a Python the system or Homebrew manages).
    Delete { env: String },
}

/// What an [`EnvAction`] will do, shown before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvActionPlan {
    pub action: EnvAction,
    /// One plain sentence.
    pub what: String,
    /// The folder it changes or makes.
    pub folder: String,
    /// The exact command, run as shown. Empty when `refused`.
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}
