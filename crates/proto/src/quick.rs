//! Quick Tools: tables a user drops in, what Sandhill knows about them, and
//! the plots its fixed R templates make. See `docs/design/quick-tools-plan.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Run provenance for Explore plots that don't use a fixed template.
pub const QUICK_AI_CODE: &str = "Made by AI code";

/// What a Quick Tools R package is needed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickTool {
    /// Reading tables and Quick Plot.
    Plot,
    /// Quick Analysis: differential expression.
    Differential,
    /// Quick Analysis: enrichment.
    Enrichment,
    /// Gene symbols for Ensembl and Entrez IDs: the organism packages,
    /// installed only when needed (Huan, 2026-11-19).
    Symbols,
}

/// One R package Quick Tools uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickPackage {
    pub name: String,
    /// Installed through BiocManager rather than from CRAN.
    pub bioconductor: bool,
    pub tool: QuickTool,
    /// The installed version, if it is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The library it is installed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// Sandhill installed this version there (its record says so).
    #[serde(default)]
    pub by_sandhill: bool,
}

/// Where an R came from, read from where it is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RSource {
    /// CRAN's installer: `/Library/Frameworks/R.framework`.
    Cran,
    Homebrew,
    Conda,
    /// rig, a system package or a build of its own.
    Other,
}

/// The R that Quick Tools would use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickR {
    /// The R program: `/Library/Frameworks/R.framework/Resources/bin/R`.
    pub path: String,
    /// Like `4.6.2`; empty when R didn't say.
    pub version: String,
    /// At least [`QUICK_R_MIN`].
    pub new_enough: bool,
    /// It installs CRAN's ready-built packages, as CRAN's own R does.
    /// Homebrew's and conda's R build every package from its source.
    #[serde(default)]
    pub binaries: bool,
    /// This computer can build packages from source: Apple's developer
    /// tools on a Mac, a C compiler elsewhere.
    #[serde(default)]
    pub can_build: bool,
    #[serde(default = "other_source")]
    pub source: RSource,
    /// Its package libraries (`.libPaths()`, as R reads them with the
    /// user's own `.Renviron` and `.Rprofile`), in R's order. Sandhill
    /// asks which one before installing and never creates one.
    #[serde(default)]
    pub libraries: Vec<crate::RLibrary>,
}

fn other_source() -> RSource {
    RSource::Other
}

/// Claude Code or Codex, for Getting Ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickAgent {
    /// `claude` or `codex`.
    pub kind: String,
    pub name: String,
    /// The program, when installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The agent's own install command, shown before it runs.
    pub install: String,
    /// It can be installed here: Codex needs Node's `npm`.
    pub installable: bool,
    /// Sandhill installed it.
    #[serde(default)]
    pub by_sandhill: bool,
}

/// One thing Sandhill installed, from its record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickInstalled {
    /// `r-package`, `python-package`, `conda-package`, `kernel`, `agent`,
    /// `python-env`, `conda-env`, `ipykernel`, `miniforge`, `brew`,
    /// `r-cran` or `uv-python`.
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// The library, environment, kernel folder, install method or
    /// Miniforge folder.
    pub place: String,
    /// Still as Sandhill left it, so removing it is Sandhill's to do. A
    /// package updated or reinstalled since is the user's now.
    pub ours: bool,
    /// The day it was installed, `2026-11-20`.
    #[serde(default)]
    pub date: String,
    /// What installed it: an [`InstallTool::id`](crate::InstallTool::id),
    /// `brew`, `npm`, `native` or `installer`; empty in older records.
    #[serde(default)]
    pub tool: String,
    /// Why, in plain words; empty in older records and for what came
    /// along with what was asked for.
    #[serde(default)]
    pub reason: String,
}

/// The conda or mamba Getting Ready uses for Python environments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickCondaTool {
    /// `~/miniforge3/bin/mamba`; mamba is preferred to conda.
    pub path: String,
    #[serde(default)]
    pub version: String,
    /// The installation it belongs to, whose `envs/` new environments go in.
    pub root: String,
}

/// A conda environment with Python but no ipykernel, so no kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickCondaEnv {
    pub name: String,
    pub prefix: String,
    /// Its Python's version, like `3.12.4`; empty when unknown.
    #[serde(default)]
    pub python: String,
    /// Its installation's mamba (else conda), which adds ipykernel to it.
    #[serde(default)]
    pub tool: String,
}

/// Python for notebooks (Huan, 2026-11-19; `docs/design/python-ready-plan.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickPython {
    /// The Python kernels Sandhill finds, by their names ("Python 3.12
    /// (omicverse)").
    #[serde(default)]
    pub kernels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<QuickCondaTool>,
    /// Environments that could get a kernel with ipykernel.
    #[serde(default)]
    pub bare: Vec<QuickCondaEnv>,
    /// The Python that installs go into by default: the environment of
    /// the first Python kernel found (as kernel pickers order them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<QuickPythonFound>,
}

/// A Python on this computer that has a kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickPythonFound {
    /// Its program, `sys.executable`.
    pub program: String,
    /// Its environment's folder, `sys.prefix`.
    pub prefix: String,
    /// `3.12.4`; empty when unknown.
    #[serde(default)]
    pub version: String,
    /// Where it comes from, in plain words: `conda, omicverse`, `uv`,
    /// `Homebrew`, `python.org`.
    #[serde(default)]
    pub description: String,
}

/// The package sets a new Python environment can have besides `python` and
/// `ipykernel`: `(id, label, conda-forge packages)`.
pub const PYTHON_SETS: &[(&str, &str, &[&str])] = &[
    (
        "analysis",
        "Data analysis",
        &["numpy", "pandas", "matplotlib", "seaborn", "scikit-learn"],
    ),
    (
        "singlecell",
        "Single-cell",
        &["scanpy", "anndata", "python-igraph", "leidenalg"],
    ),
];

/// The Python versions offered, the default first.
pub const PYTHON_VERSIONS: &[&str] = &["3.12", "3.11", "3.13"];

/// The name a new environment starts with.
pub const PYTHON_ENV_NAME: &str = "sandhill-python";

/// conda-forge's own installer for Miniforge, run in batch mode, which
/// leaves the shell's startup files alone.
pub const MINIFORGE_COMMAND: &str = "curl -fsSL -o /tmp/Miniforge3.sh \"https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-$(uname)-$(uname -m).sh\" && bash /tmp/Miniforge3.sh -b -p \"$HOME/miniforge3\" && rm -f /tmp/Miniforge3.sh";

/// A name for a new environment: letters, digits, `-` and `_`.
pub fn python_env_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The command that creates a Python environment at `prefix` with `tool`,
/// from conda-forge only: Python, ipykernel and the chosen [`PYTHON_SETS`].
/// Shown before it runs and run as shown.
pub fn python_create_args(tool: &str, prefix: &str, python: &str, sets: &[String]) -> Vec<String> {
    let mut args: Vec<String> = [tool, "create", "-y", "-p", prefix]
        .into_iter()
        .chain(["-c", "conda-forge", "--override-channels"])
        .map(str::to_string)
        .collect();
    args.push(format!("python={python}"));
    args.push("ipykernel".into());
    for (id, _, packages) in PYTHON_SETS {
        if sets.iter().any(|s| s == id) {
            args.extend(packages.iter().map(|p| p.to_string()));
        }
    }
    args
}

/// The command that adds ipykernel to the environment at `prefix`, from
/// conda-forge, changing nothing already there.
pub fn ipykernel_args(tool: &str, prefix: &str) -> Vec<String> {
    [
        tool,
        "install",
        "-y",
        "-p",
        prefix,
        "-c",
        "conda-forge",
        "--override-channels",
        "--freeze-installed",
        "ipykernel",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The oldest R Quick Tools uses.
pub const QUICK_R_MIN: (u32, u32) = (4, 4);

/// What Quick Tools needs on this computer, and what is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickReadiness {
    /// The R an R kernel runs, else the newest R found; `None` without R.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<QuickR>,
    /// The R kernel Quick Tools sessions start: a kernelspec's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// Every package Quick Tools uses, installed or not.
    pub packages: Vec<QuickPackage>,
    #[serde(default)]
    pub agents: Vec<QuickAgent>,
    /// What Sandhill's record says it installed.
    #[serde(default)]
    pub installed: Vec<QuickInstalled>,
    #[serde(default)]
    pub python: QuickPython,
    /// The install tools found on this computer besides each R's own
    /// installer and each Python's pip: mamba or conda, and uv.
    #[serde(default)]
    pub tools: Vec<crate::InstallToolInfo>,
    /// Homebrew's `brew`, when there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brew: Option<String>,
    /// Homebrew's version, like `4.6.3`; empty when unknown.
    #[serde(default)]
    pub brew_version: String,
    #[serde(default)]
    pub prefs: crate::InstallPrefs,
    /// Sandhill's own R and Python, when its folder exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<crate::OwnEnvironment>,
    /// Apple's developer tools (or a C compiler elsewhere) are there, so
    /// packages can be built on this computer.
    #[serde(default)]
    pub developer_tools: bool,
}

/// A package install started: its job, and the file its output also goes
/// to, for the progress and details Quick Tools shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickInstalling {
    pub job: crate::TerminalInfo,
    pub log: String,
}

/// Where a fact on the What we know card came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickFrom {
    /// Sandhill read it from the table.
    Found,
    /// The user said so.
    Told,
    /// The AI suggested it; nothing uses it until the user accepts it.
    Suggested,
}

/// A fact and where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Known<T> {
    pub value: T,
    pub from: QuickFrom,
}

impl<T> Known<T> {
    pub fn found(value: T) -> Self {
        Self {
            value,
            from: QuickFrom::Found,
        }
    }
    pub fn told(value: T) -> Self {
        Self {
            value,
            from: QuickFrom::Told,
        }
    }
    /// The value when something may use it: not while only suggested.
    pub fn usable(&self) -> Option<&T> {
        (self.from != QuickFrom::Suggested).then_some(&self.value)
    }
}

/// What a table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickTableKind {
    /// Differential expression results: a fold change and p-values per gene.
    Results,
    /// Raw counts: whole numbers per gene and sample.
    Counts,
    /// Normalized expression per gene and sample (CPM, TPM, FPKM...).
    Normalized,
    /// One row per sample, with its group, batch and so on.
    SampleSheet,
    /// Enrichment results: one row per term (GO, KEGG, Reactome...).
    Enrichment,
    /// Not recognised.
    Unknown,
}

/// The kind of gene identifiers in a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneIds {
    Ensembl,
    Symbol,
    Entrez,
}

/// What the values of an expression table are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickValues {
    /// Raw counts: whole numbers.
    Counts,
    /// Normalized, not logged (CPM, TPM, FPKM).
    Normalized,
    /// Already on a log scale (log2 CPM, VST, rlog).
    Logged,
}

/// How many of a text column's values (missing ones left out) look like
/// each kind of identifier. Counted over every value, in R.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickPatterns {
    /// `ENSG00000141510` (optionally with a `.12` version).
    #[serde(default)]
    pub ensembl_human: u64,
    /// `ENSMUSG00000059552`.
    #[serde(default)]
    pub ensembl_mouse: u64,
    /// Other Ensembl gene IDs (`ENSRNOG`, `ENSDARG`...).
    #[serde(default)]
    pub ensembl_other: u64,
    /// Whole numbers only (Entrez IDs, when in a text column).
    #[serde(default)]
    pub entrez: u64,
    /// Gene-symbol-like: a letter, then letters, digits, `-`, `.` or `_`.
    #[serde(default)]
    pub symbol: u64,
    /// `GO:0006955`.
    #[serde(default)]
    pub go: u64,
    /// Pathway or term IDs from other databases: `hsa04110`, `R-HSA-109582`,
    /// `KEGG:04110`, `REAC:R-HSA-...`, `WP254`.
    #[serde(default)]
    pub pathway: u64,
    /// Looks like a gene name Excel turned into a date: `1-Mar`, `Sep-02`,
    /// `2-Sep`, or a 5-digit date serial number in an otherwise text column.
    #[serde(default)]
    pub datelike: u64,
    /// Several genes in one value, separated by `/`, `,` or `;` (an
    /// enrichment table's gene list).
    #[serde(default)]
    pub gene_lists: u64,
    /// `12/345`: a ratio written as text (clusterProfiler's GeneRatio).
    #[serde(default)]
    pub ratio: u64,
}

/// The type of a column, as R holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickColumnKind {
    Number,
    Text,
    Logical,
    Date,
}

/// One column of a loaded table, described by the load template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickColumn {
    pub name: String,
    pub kind: QuickColumnKind,
    /// Missing values (`NA`, empty cells).
    pub missing: u64,
    /// Distinct values, missing ones left out.
    pub distinct: u64,
    /// Numbers only: every finite value is a whole number.
    #[serde(default)]
    pub whole: bool,
    /// Numbers only: the smallest and largest finite values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Up to 8 distinct values as text, in table order.
    #[serde(default)]
    pub examples: Vec<String>,
    /// Text columns only.
    #[serde(default)]
    pub patterns: QuickPatterns,
}

/// What Sandhill found a table to be, from its columns alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickRecognition {
    pub kind: QuickTableKind,
    /// The program that made it, when its column names say so: `DESeq2`,
    /// `edgeR`, `limma`, `featureCounts`, `clusterProfiler`, `g:Profiler`,
    /// `DAVID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The column holding gene IDs or names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene_ids: Option<GeneIds>,
    /// `human` or `mouse`, from Ensembl prefixes only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organism: Option<String>,
    /// Results: the log2 fold change, adjusted p-value, p-value and average
    /// expression columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padj: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvalue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<String>,
    /// The average expression is already logged (`logCPM`, `AveExpr`),
    /// rather than a plain mean (`baseMean`).
    #[serde(default)]
    pub mean_log: bool,
    /// Counts, normalized values, or results with a value per sample: the
    /// sample columns, in table order.
    #[serde(default)]
    pub samples: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<QuickValues>,
    /// Groups guessed from sample names (`ctrl_1`, `ctrl_2`, `trt_1`):
    /// `(sample, group)`, empty when the names don't say.
    #[serde(default)]
    pub groups: Vec<(String, String)>,
    /// Sample sheets: the column naming the samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_column: Option<String>,
    /// Enrichment: the term name, gene count and gene ratio columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<String>,
    /// Values in the gene column that look like gene names Excel turned
    /// into dates.
    #[serde(default)]
    pub dates: u64,
    /// When Sandhill can't tell what the table is, the question to ask:
    /// "What are these numbers?".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

/// A table loaded into the session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickTable {
    /// The R variable holding it: `de_results`.
    pub name: String,
    /// The file, relative to the session folder.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    pub rows: u64,
    pub columns: Vec<QuickColumn>,
    pub recognition: QuickRecognition,
}

/// Facts about one table on the card: what was found, then what the user
/// changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickCardTable {
    /// The R variable.
    pub name: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    pub kind: Known<QuickTableKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene_ids: Option<Known<GeneIds>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fc: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padj: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<Known<String>>,
    #[serde(default)]
    pub mean_log: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<Known<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Known<QuickValues>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<Known<String>>,
    /// The column plots label genes by: gene symbols added for the gene
    /// column's IDs (`docs/design/gene-symbols-plan.md`). The gene column
    /// itself stays the IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub differential: Option<QuickDifferentialInfo>,
    /// Gene names that look like Excel dates (a warning on the table).
    #[serde(default)]
    pub dates: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

/// "Higher in `higher` than in `lower`": what a positive fold change means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickComparison {
    pub higher: String,
    pub lower: String,
}

/// The cutoffs a gene must pass to count as changed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QuickCutoffs {
    /// Adjusted p-value below this.
    pub padj: f64,
    /// |log2 fold change| at least this.
    pub lfc: f64,
}

impl Default for QuickCutoffs {
    fn default() -> Self {
        Self {
            padj: 0.05,
            lfc: 1.,
        }
    }
}

/// A named sample annotation. The first level is the reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickFactor {
    pub name: String,
    pub levels: Vec<String>,
    pub assignments: Vec<(String, String)>,
}

impl QuickFactor {
    pub fn from_pairs(name: String, assignments: Vec<(String, String)>) -> Self {
        let mut levels = Vec::new();
        for (_, level) in &assignments {
            if !levels.contains(level) {
                levels.push(level.clone());
            }
        }
        Self {
            name,
            levels,
            assignments,
        }
    }
}

fn read_factors<'de, D>(deserializer: D) -> Result<Option<Known<Vec<QuickFactor>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Factors(Known<Vec<QuickFactor>>),
        Groups(Known<Vec<(String, String)>>),
    }
    Ok(
        Option::<Saved>::deserialize(deserializer)?.map(|saved| match saved {
            Saved::Factors(factors) => factors,
            Saved::Groups(groups) => Known {
                from: groups.from,
                value: vec![QuickFactor::from_pairs("group".into(), groups.value)],
            },
        }),
    )
}

/// The What we know card, kept in the session folder's
/// `.sandhill/quick.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuickCard {
    pub tables: Vec<QuickCardTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organism: Option<Known<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Known<QuickComparison>>,
    #[serde(
        default,
        alias = "groups",
        deserialize_with = "read_factors",
        skip_serializing_if = "Option::is_none"
    )]
    pub factors: Option<Known<Vec<QuickFactor>>>,
    #[serde(default)]
    pub cutoffs: QuickCutoffs,
    /// The tables' own paths, as opened, in order.
    #[serde(default)]
    pub sources: Vec<String>,
    /// The session lives in Sandhill's own storage, not yet kept.
    #[serde(default)]
    pub scratch: bool,
    /// Where the user kept it, which what it writes is copied to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<String>,
}

/// The tools that reach the Explore window rather than the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickAgentTool {
    State,
    Suggest,
    Make,
}

impl QuickAgentTool {
    pub const fn method(self) -> &'static str {
        match self {
            Self::State => "quick_state",
            Self::Suggest => "quick_suggest",
            Self::Make => "quick_make",
        }
    }

    pub fn from_method(method: &str) -> Option<Self> {
        [Self::State, Self::Suggest, Self::Make]
            .into_iter()
            .find(|tool| tool.method() == method)
    }
}

/// Pending facts suggested for the session and, when named, one table.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuickSuggestion {
    pub table: Option<String>,
    pub organism: Option<String>,
    pub comparison: Option<QuickComparison>,
    pub factors: Option<Vec<QuickFactor>>,
    pub kind: Option<QuickTableKind>,
    pub gene: Option<String>,
    pub gene_ids: Option<GeneIds>,
    pub fc: Option<String>,
    pub padj: Option<String>,
    pub mean: Option<String>,
    pub mean_log: Option<bool>,
    pub samples: Option<Vec<String>>,
    pub values: Option<QuickValues>,
    pub term: Option<String>,
    pub count: Option<String>,
    pub ratio: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickMake {
    pub plot: QuickPlot,
    pub table: Option<String>,
    pub gene: Option<String>,
}

/// A plot (or table) a template makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickPlot {
    // From differential expression results.
    Volcano,
    Ma,
    UpDown,
    /// A table of the genes that pass the cutoffs, most significant first.
    TopGenes,
    /// Results with a value per sample: the most significant genes.
    ResultsHeatmap,
    // From counts or normalized values.
    LibrarySizes,
    Pca,
    Correlation,
    VariableHeatmap,
    GeneByGroup,
    // From enrichment results.
    EnrichDot,
    EnrichBar,
}

/// Everything a template reads. Column names refer to `table`; the app
/// fills this from the card's usable facts only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuickSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gene: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padj: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<String>,
    #[serde(default)]
    pub mean_log: bool,
    #[serde(default)]
    pub samples: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<QuickValues>,
    /// `(sample, group)`.
    #[serde(default)]
    pub groups: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    /// The expression table and its optional correction for heatmaps/PCA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plot_batch: Option<Box<QuickPlotBatch>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_note: Option<String>,
    #[serde(default)]
    pub omnibus: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<QuickComparison>,
    #[serde(default)]
    pub cutoffs: QuickCutoffs,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<String>,
    /// [`QuickPlot::GeneByGroup`]: the gene to show, as written in the gene
    /// column or the label column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_gene: Option<String>,
    /// The column genes are labelled by (symbols), else the gene column;
    /// a gene with no label keeps its ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickDeMethod {
    #[default]
    Deseq2,
    Edger,
    Limma,
}

impl QuickDeMethod {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Deseq2 => "DESeq2",
            Self::Edger => "edgeR",
            Self::Limma => "limma-voom",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum QuickDeBatch {
    #[default]
    None,
    Model {
        factor: String,
    },
    Pairs {
        factor: String,
    },
    Sva,
    Ruv,
    CombatSeq {
        factor: String,
    },
    CombatRef {
        factor: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickDeCombined {
    pub name: String,
    pub first: String,
    pub second: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickDeDesign {
    pub formula: String,
    pub factors: Vec<QuickFactor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combined: Option<QuickDeCombined>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuickDeContrast {
    /// Compare levels at the reference levels of other factors unless `at` says otherwise.
    Levels {
        factor: String,
        higher: String,
        lower: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        at: Vec<(String, String)>,
    },
    Coefficient {
        name: String,
    },
    /// The difference of the second factor's effects in two levels of the first.
    Interaction {
        first: String,
        second: String,
        first_higher: String,
        second_higher: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickDeOutput {
    pub name: String,
    pub comparison: QuickComparison,
    pub contrast: QuickDeContrast,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickDeSettings {
    pub method: QuickDeMethod,
    pub design: QuickDeDesign,
    /// A reduced formula selects an omnibus test instead of a single contrast test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduced: Option<String>,
    #[serde(default)]
    pub batch: QuickDeBatch,
    #[serde(default)]
    pub shrink: bool,
    #[serde(default)]
    pub quality_weights: bool,
    pub gene: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub samples: Vec<String>,
    #[serde(default)]
    pub cutoffs: QuickCutoffs,
    pub outputs: Vec<QuickDeOutput>,
    pub normalized: String,
    /// Relative to the session until resolved by the daemon.
    pub directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickPlotBatch {
    /// The biology to preserve (without the batch term).
    pub design: QuickDeDesign,
    pub factor: String,
}

/// Recorded per output so reopening the CSV keeps its plot labels and methods.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuickDifferentialInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Known<QuickComparison>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_table: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub methods: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plot_batch: Option<Box<QuickPlotBatch>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_note: Option<String>,
    /// Whether an omnibus test selected the hits, with a separate fold change for display.
    #[serde(default)]
    pub omnibus: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickDifferentialTable {
    pub table: QuickTable,
    pub gene: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub samples: Vec<String>,
    pub info: QuickDifferentialInfo,
}

/// File suffixes supported by Explore a Table. Gzip is an extra suffix
/// only for delimited tables.
pub const QUICK_TABLE_EXTENSIONS: &[&str] = &[
    "csv.gz", "tsv.gz", "txt.gz", "tab.gz", "csv", "tsv", "txt", "tab", "xlsx", "xls",
];

pub fn quick_table_stem(name: &str) -> Option<&str> {
    quick_stem(name, QUICK_TABLE_EXTENSIONS)
}

pub fn quick_archive_stem(name: &str) -> Option<&str> {
    quick_stem(name, &["tar.gz", "tgz", "tar", "zip"])
}

fn quick_stem<'a>(name: &'a str, extensions: &[&str]) -> Option<&'a str> {
    let name = name.rsplit('/').next()?;
    extensions.iter().find_map(|ext| {
        let dot = name.len().checked_sub(ext.len() + 1)?;
        (dot > 0
            && name.as_bytes()[dot] == b'.'
            && name.as_bytes()[dot + 1..].eq_ignore_ascii_case(ext.as_bytes()))
        .then(|| &name[..dot])
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickArchiveMember {
    pub index: usize,
    pub name: String,
    /// It looks like one gene column and one count column in a short preview.
    pub sample_counts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickArchiveSelection {
    pub path: String,
    pub members: Vec<QuickArchiveMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickArchiveOpened {
    pub folder: String,
    pub files: Vec<String>,
    pub combine: Option<QuickArchiveCounts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickArchiveCounts {
    pub files: Vec<String>,
    pub file: String,
}

/// What a Quick Tools run is asked to do. Sent to R as JSON, never as code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QuickRequest {
    /// The sheets of an Excel file, in order.
    Sheets { file: String },
    /// Reads `file` (one `sheet` of a workbook) into the R variable `name`.
    Load {
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sheet: Option<String>,
        name: String,
    },
    /// Joins per-sample counts files on shared gene IDs and saves a CSV.
    CombineCounts {
        files: Vec<String>,
        file: String,
        name: String,
    },
    /// Draws `plot` from the table in `table`. [`QuickPlot::TopGenes`]
    /// makes a table instead, in the R variable `output`.
    Plot {
        plot: QuickPlot,
        table: String,
        settings: Box<QuickSettings>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// The `(key, value)` pairs of two columns of `table`, rows with
    /// either missing left out: a sample sheet's samples and their groups.
    Pairs {
        table: String,
        key: String,
        value: String,
    },
    /// Every annotation column other than the sample name column.
    Factors { table: String, key: String },
    /// Validate the formula and return the names of its model matrix coefficients.
    DeCoefficients {
        samples: Vec<String>,
        design: Box<QuickDeDesign>,
        batch: QuickDeBatch,
    },
    Differential {
        table: String,
        settings: Box<QuickDeSettings>,
    },
    /// Writes an Excel workbook: a `Read_me` sheet of `(field, text)` rows,
    /// then one sheet per `(sheet name, R variable)`.
    Workbook {
        path: String,
        readme: Vec<(String, String)>,
        sheets: Vec<(String, String)>,
    },
    /// Adds the gene symbols of `column`'s Ensembl or Entrez `ids` for
    /// `organism` (`human` or `mouse`) to `table` as the column `output`,
    /// else the first free of `symbol`, `symbol_2`…, beside `column`.
    /// Version endings (`.17`) are dropped before matching.
    Symbols {
        table: String,
        column: String,
        ids: GeneIds,
        organism: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
}

/// The organism package that has gene symbols for `organism`.
pub fn symbols_package(organism: &str) -> Option<&'static str> {
    match organism {
        "human" => Some("org.Hs.eg.db"),
        "mouse" => Some("org.Mm.eg.db"),
        _ => None,
    }
}

/// What a Quick Tools run returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuickResult {
    /// What the plot shows, in a plain sentence made from its numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentence: Option<String>,
    /// The numbers the template measured, as it returned them.
    #[serde(default)]
    pub facts: Value,
    /// A loaded or generated table, recognised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<QuickTable>,
    /// [`QuickRequest::Sheets`].
    #[serde(default)]
    pub sheets: Vec<String>,
    /// [`QuickRequest::Pairs`].
    #[serde(default)]
    pub pairs: Vec<(String, String)>,
    /// A table the run made (top genes): its R variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub factors: Vec<QuickFactor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coefficients: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differential: Vec<QuickDifferentialTable>,
}
