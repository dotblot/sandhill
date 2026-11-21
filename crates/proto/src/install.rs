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
    /// Also copy the packages of these environments ([`EnvCard::id`]s of
    /// the user's own R and Python), which are only read (plan section 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy_from: Vec<String>,
}

/// One package of a copy, and where its copy will come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyRow {
    /// `r` or `python`.
    pub language: String,
    pub name: String,
    /// The version the user has now.
    pub version: String,
    /// Where the user's copy came from: `CRAN`, `Bioconductor`, `GitHub`,
    /// `conda-forge`, `bioconda`, `PyPI` or `this computer`.
    pub from: String,
    /// Where the new copy comes from, as [`PlannedPackage::source`] says;
    /// `None` when it can't be copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The version that will be installed, when it differs or is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<InstallTool>,
    /// Why it can't be copied, in plain words: "Built from a folder on this
    /// computer, so there is nowhere to get it from."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cant: Option<String>,
}

/// What the last copy did, for the report afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyReport {
    pub date: String,
    pub copied: u32,
    /// What failed, each with a plain reason and, when there is one, a fix.
    #[serde(default)]
    pub failed: Vec<EnvProblem>,
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
    /// The copy table, when `setup.copy_from` names environments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copy: Vec<CopyRow>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_report: Option<CopyReport>,
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

/// A package a document or an error needs that its kernel's environment
/// doesn't have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingPackage {
    /// As the code names it: `sklearn`, `Seurat`.
    pub import: String,
    /// What installs it: `scikit-learn`, `Seurat`.
    pub package: String,
}

/// What a document or an error needs, and where it would go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingPackages {
    /// `r` or `python`.
    pub language: String,
    /// The kernel's environment: for R, the suggested library (the last
    /// one chosen for this R when it can be written to, else the first
    /// writable one).
    pub target: InstallTarget,
    /// The kernel, as pickers name it.
    #[serde(default)]
    pub kernel: String,
    pub missing: Vec<MissingPackage>,
}

/// What went wrong in an install, in plain words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallFailureKind {
    /// A package had to be built and the developer tools are missing.
    DeveloperTools,
    /// A site couldn't be reached.
    Network,
    /// Packages that can't be installed together.
    Conflict,
    /// Not enough disk space.
    Space,
    /// No such package (or not for this R or Python).
    NotAvailable,
    /// It needs a program or library from outside R or Python.
    SystemLibrary,
    /// The folder can't be written to.
    Permission,
    Other,
}

/// The button that fixes a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailureFix {
    /// Apple's developer tools ([`Call::InstallDeveloperTools`](crate::Call)).
    InstallDeveloperTools,
    /// Try the same packages with another tool (another source). `change`
    /// is the failed change with that tool, when the daemon knows it
    /// (always for changes it planned, including imported package lists).
    OtherTool {
        tool: InstallTool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change: Option<PackageChange>,
    },
    /// Open Resources and Storage.
    FreeUpSpace,
}

/// A failed install, explained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallFailure {
    pub kind: InstallFailureKind,
    /// One plain sentence: "DESeq2 had to be built on this Mac, which
    /// needs Apple's developer tools."
    pub message: String,
    /// The specifics, one per line: the site, the packages in conflict
    /// and why, the space needed, the missing library.
    #[serde(default)]
    pub detail: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FailureFix>,
}

/// A package a project's file pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPackage {
    pub name: String,
    pub version: String,
    /// `CRAN`, `Bioconductor`, `GitHub`, `conda-forge`, `bioconda`, `PyPI`.
    pub source: String,
}

/// What Save This Project's Packages will write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFilePlan {
    pub project: String,
    /// `r` or `python`.
    pub language: String,
    /// The file's full path: `renv.lock`, `environment.yml` or
    /// `requirements.txt` in the project folder.
    pub path: String,
    /// A file by that name is there and will be replaced.
    #[serde(default)]
    pub replaces: bool,
    /// What the project's code uses, and for R what those need.
    pub packages: Vec<ProjectPackage>,
    /// The file as it will be written.
    pub contents: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// What Set Up This Project's Packages will do with a project's
/// `renv.lock`, `environment.yml` or `requirements.txt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSetupPlan {
    pub project: String,
    /// The file's name in the project folder.
    pub file: String,
    /// One plain sentence.
    pub what: String,
    /// Where the packages go: the project's `renv/library` (Huan,
    /// 2026-11-20: the one exception to "Sandhill never creates an R
    /// library", for projects that carry their own `renv.lock`), a conda
    /// environment, the project's `.venv`, or in Sandhill's own folder.
    pub folder: String,
    pub packages: Vec<PlannedPackage>,
    pub steps: Vec<SetupStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// A project's package files and whether they are set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPackageFile {
    /// `renv.lock`, `environment.yml`, `environment.yaml` or
    /// `requirements.txt`.
    pub file: String,
    /// `r` or `python`.
    pub language: String,
    /// How many packages it pins.
    pub packages: u32,
    /// Its packages are installed where Set Up puts them, at its versions.
    pub set_up: bool,
    /// Those that aren't, when not set up: `Seurat 4.4.0 (have 5.1.0)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differences: Vec<String>,
}

/// What kind of place a [`FootprintItem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FootprintKind {
    /// Inside Sandhill's folder (`~/.sandhill`).
    Folder,
    /// A place the system decides: macOS's folders for the app id, the
    /// Keychain entry, a temporary socket folder.
    System,
    /// An entry Sandhill added to an agent's own settings, or a folder it
    /// made there, recorded so exactly it can be taken out.
    AgentSetting,
    /// An old place from before the one-folder layout, still in use or
    /// left over.
    OldPlace,
}

/// One place Sandhill has put something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FootprintItem {
    /// Its full path (for the Keychain, `Keychain: dev.sandhill.agent-account`).
    pub path: String,
    /// In plain words: "History and outputs", "Sandhill's own R and Python".
    pub what: String,
    pub kind: FootprintKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_mb: Option<u64>,
    /// Anything to know, in plain words: "Still in use: a kernel from
    /// before is running", "Removed once Sandhill has closed".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What Sandhill has put on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Footprint {
    /// Sandhill's folder, `~/.sandhill`.
    pub folder: String,
    /// Everything Sandhill has put here, in MB (its folder and the rest).
    pub size_mb: u64,
    pub items: Vec<FootprintItem>,
    /// What Sandhill installed into the user's own R and Python (the
    /// record, without agents).
    #[serde(default)]
    pub installed: Vec<crate::QuickInstalled>,
    /// The agents Sandhill installed (record lines of kind `agent`).
    #[serde(default)]
    pub agents: Vec<crate::QuickInstalled>,
}

/// The user's answers on Uninstall Sandhill. Every field is required, so
/// nothing is removed by a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallChoice {
    /// Record lines to remove from the user's own R and Python (all ticked
    /// by default, Huan 2026-11-20).
    pub installed: Vec<InstalledKey>,
    /// Agents (`claude`, `codex`) to remove in their official way (none
    /// ticked by default); their sign-ins always stay.
    pub agents: Vec<String>,
    /// Keep Sandhill's history (conversations, file history, outputs) in
    /// `~/.sandhill/state`.
    pub keep_history: bool,
}

/// What Uninstall Sandhill will do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallPlan {
    pub choice: UninstallChoice,
    pub remove: Vec<FootprintItem>,
    /// What stays, each with why: the history when kept, the user's
    /// projects and their `.sandhill` folders, data and results, items
    /// changed since Sandhill left them.
    pub keep: Vec<FootprintItem>,
    /// The commands, in order.
    pub steps: Vec<SetupStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// What Uninstall Sandhill did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UninstallReport {
    /// Removed, by path or name.
    pub removed: Vec<String>,
    /// What failed to go, each with the reason in its `note`. What was
    /// kept on purpose is in [`UninstallPlan::keep`], never here.
    pub left: Vec<FootprintItem>,
    /// Removed by the finisher once Sandhill has closed (Sandhill's
    /// folder, macOS's folders for the app).
    pub finishing: Vec<String>,
    /// Nothing failed and the finisher started: the app may move itself
    /// to the Trash once every ticked device succeeded.
    pub success: bool,
}
