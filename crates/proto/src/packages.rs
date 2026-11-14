//! Packages and help: installed packages, updates, help topics, pages and sources.

use super::*;

/// Help topics, as [`Call::HelpTopics`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelpTopics {
    pub session: String,
    pub topics: Vec<HelpTopic>,
    /// How many more matched than are listed.
    #[serde(default)]
    pub more: u64,
    /// R's vignettes: a package's, or those whose name or title matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vignettes: Vec<HelpVignette>,
}

/// An R package's vignette, as it was built when the package was installed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HelpVignette {
    /// Its name, as `vignette()` takes it: `dplyr`, `colwise`.
    pub name: String,
    pub package: String,
    pub title: String,
    /// The built file, HTML or PDF, on the session's machine.
    pub file: String,
    /// The vignette's R code, if the package has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HelpTopic {
    /// The function or topic: `read.csv`, `pandas.read_csv`.
    pub topic: String,
    /// R's package, or Python's top-level module.
    pub package: String,
    /// Its one-line title, if it has one.
    #[serde(default)]
    pub title: String,
}

/// A help page, as [`Call::HelpPage`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelpPage {
    pub session: String,
    pub topic: String,
    pub package: String,
    pub title: String,
    /// The package's version (Python's, for built-ins).
    pub version: String,
    /// R: the page as HTML, without its `<head>`. Links to other topics go
    /// to `../../{package}/help/{topic}`, examples to `../Example/{topic}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Python: `module`, `class`, `function`, `method`, `property` or
    /// `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Python: how it is called, like `(adata, target_sum=None)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Python: its docstring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Python: a module's or class's public names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<HelpMember>,
    /// R: other packages with a page on the same topic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<HelpTopic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelpMember {
    pub name: String,
    /// As [`HelpPage::kind`].
    pub kind: String,
    #[serde(default)]
    pub title: String,
}

/// A function's source, as [`Call::HelpSource`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HelpSource {
    pub session: String,
    pub topic: String,
    pub package: String,
    pub text: String,
    /// Python: the file it is in, on the session's machine, and the line
    /// it starts on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Packages with a newer version, as [`Call::PackageUpdates`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageUpdates {
    pub session: String,
    pub updates: Vec<PackageUpdate>,
    /// The repositories asked.
    pub repos: Vec<String>,
    /// Those that couldn't be reached. When none could, the call fails.
    #[serde(default)]
    pub unreached: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PackageUpdate {
    pub name: String,
    pub installed: String,
    pub available: String,
    /// The library the installed one is in.
    pub library: String,
    /// It comes from Bioconductor.
    #[serde(default)]
    pub bioconductor: bool,
}

/// Installed packages, as [`Call::ListPackages`] asked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Packages {
    pub session: String,
    /// By name.
    pub packages: Vec<Package>,
    /// Where they are installed: R's library folders (`.libPaths()`), or
    /// the Python environment.
    pub libraries: Vec<String>,
    /// R: whether a CRAN mirror is set in `options("repos")`.
    #[serde(default)]
    pub cran: bool,
    /// R: whether BiocManager is installed.
    #[serde(default)]
    pub bioconductor: bool,
    /// Python: whether the environment is a conda one.
    #[serde(default)]
    pub conda: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Package {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub title: String,
    /// The library folder or environment it is installed in.
    #[serde(default)]
    pub library: String,
    /// R: attached with `library()`. Python: imported by the user's code.
    #[serde(default)]
    pub attached: bool,
    /// R: its namespace is loaded. Python: one of its modules is imported.
    #[serde(default)]
    pub loaded: bool,
    /// R: part of R itself, so it can't be removed or updated on its own.
    #[serde(default)]
    pub base: bool,
    /// Python: the modules it provides, which can differ from its name
    /// (`scikit-learn` provides `sklearn`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Python: what installed it, like `pip` or `conda`, when it says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub installer: String,
}
