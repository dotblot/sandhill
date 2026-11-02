//! Variables' Import menu, from the Tauri app's `model/imports.ts`:
//! RStudio's list of ways to read a dataset into R, plus R data files and
//! 10x Genomics output, and the same for Python. Each writes the code it
//! will run, which the Import dialog shows first.

pub struct ImportKind {
    pub label: &'static str,
    /// The function, shown at the right of the menu.
    pub function: &'static str,
    /// File extensions it reads, lower case.
    pub exts: &'static [&'static str],
    /// It reads a folder (10x output) as well as files.
    pub folder: bool,
    /// The menu draws a line before it.
    pub group: bool,
    code: fn(&str, &str) -> String,
    /// False when the code names its own variables (R's `load()`).
    named: fn(&str) -> bool,
}

impl ImportKind {
    /// The code for a file and the variable to read it into.
    pub fn code(&self, path: &str, name: &str) -> String {
        (self.code)(path, name)
    }

    pub fn named(&self, path: &str) -> bool {
        (self.named)(path)
    }
}

fn q(path: &str) -> String {
    serde_json::to_string(path).unwrap_or_default()
}

fn ext(path: &str) -> String {
    path.rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

fn tabbed(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let plain = lower.strip_suffix(".gz").unwrap_or(&lower);
    ["tsv", "tab", "txt"].contains(&ext(plain).as_str())
}

fn always(_: &str) -> bool {
    true
}

const TEXT: &[&str] = &["csv", "tsv", "txt", "tab", "gz"];

const fn kind(
    label: &'static str,
    function: &'static str,
    exts: &'static [&'static str],
    group: bool,
    code: fn(&str, &str) -> String,
) -> ImportKind {
    ImportKind {
        label,
        function,
        exts,
        folder: false,
        group,
        code,
        named: always,
    }
}

static R_IMPORTS: [ImportKind; 8] = [
    kind("From text (base)…", "read.csv()", TEXT, false, |p, n| {
        format!(
            "{n} <- {}({})",
            if tabbed(p) { "read.delim" } else { "read.csv" },
            q(p)
        )
    }),
    kind(
        "From text (readr)…",
        "readr::read_csv()",
        TEXT,
        false,
        |p, n| {
            format!(
                "{n} <- readr::{}({})",
                if tabbed(p) { "read_tsv" } else { "read_csv" },
                q(p)
            )
        },
    ),
    kind(
        "From Excel…",
        "readxl::read_excel()",
        &["xlsx", "xls"],
        true,
        |p, n| format!("{n} <- readxl::read_excel({})", q(p)),
    ),
    kind(
        "From SPSS…",
        "haven::read_sav()",
        &["sav", "zsav", "por"],
        true,
        |p, n| format!("{n} <- haven::read_sav({})", q(p)),
    ),
    kind(
        "From SAS…",
        "haven::read_sas()",
        &["sas7bdat"],
        false,
        |p, n| format!("{n} <- haven::read_sas({})", q(p)),
    ),
    kind(
        "From Stata…",
        "haven::read_dta()",
        &["dta"],
        false,
        |p, n| format!("{n} <- haven::read_dta({})", q(p)),
    ),
    ImportKind {
        label: "From R data (.rds, .RData)…",
        function: "readRDS()",
        exts: &["rds", "rdata", "rda"],
        folder: false,
        group: true,
        code: |p, n| {
            if ext(p) == "rds" {
                format!("{n} <- readRDS({})", q(p))
            } else {
                format!("load({})", q(p))
            }
        },
        named: |p| ext(p) == "rds",
    },
    ImportKind {
        label: "From 10x Genomics…",
        function: "Seurat::Read10X()",
        exts: &["h5"],
        folder: true,
        group: false,
        code: |p, n| {
            if ext(p) == "h5" {
                format!("{n} <- Seurat::Read10X_h5({})", q(p))
            } else {
                format!("{n} <- Seurat::Read10X(data.dir = {})", q(p))
            }
        },
        named: always,
    },
];

static PYTHON_IMPORTS: [ImportKind; 5] = [
    kind(
        "From text (pandas)…",
        "pd.read_csv()",
        TEXT,
        false,
        |p, n| {
            format!(
                "import pandas as pd\n{n} = pd.read_csv({}{})",
                q(p),
                if tabbed(p) { ", sep=\"\\t\"" } else { "" }
            )
        },
    ),
    kind(
        "From Excel…",
        "pd.read_excel()",
        &["xlsx", "xls"],
        false,
        |p, n| format!("import pandas as pd\n{n} = pd.read_excel({})", q(p)),
    ),
    kind(
        "From Parquet…",
        "pd.read_parquet()",
        &["parquet", "pq"],
        false,
        |p, n| format!("import pandas as pd\n{n} = pd.read_parquet({})", q(p)),
    ),
    kind(
        "From AnnData (.h5ad)…",
        "ad.read_h5ad()",
        &["h5ad"],
        true,
        |p, n| format!("import anndata as ad\n{n} = ad.read_h5ad({})", q(p)),
    ),
    ImportKind {
        label: "From 10x Genomics…",
        function: "sc.read_10x_mtx()",
        exts: &["h5"],
        folder: true,
        group: false,
        code: |p, n| {
            format!(
                "import scanpy as sc\n{n} = sc.{}({})",
                if ext(p) == "h5" {
                    "read_10x_h5"
                } else {
                    "read_10x_mtx"
                },
                q(p)
            )
        },
        named: always,
    },
];

pub fn imports_for(language: &str) -> &'static [ImportKind] {
    match language.to_ascii_lowercase().as_str() {
        "r" => &R_IMPORTS,
        "python" => &PYTHON_IMPORTS,
        _ => &[],
    }
}

/// Whether a file is one the import reads (compressed text counts as text).
pub fn importable(kind: &ImportKind, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if let Some(inner) = lower.strip_suffix(".gz") {
        return kind.exts.contains(&"gz") && kind.exts.contains(&ext(inner).as_str());
    }
    kind.exts.contains(&ext(&lower).as_str())
}

/// A variable name from a file's name: "pbmc3k_counts.csv" is
/// pbmc3k_counts, "10x out" is x10x_out.
pub fn variable_name(path: &str) -> String {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let lower = base.to_ascii_lowercase();
    let unzipped = [".gz", ".bz2", ".xz", ".zst"]
        .iter()
        .find(|s| lower.ends_with(*s))
        .map_or(base, |s| &base[..base.len() - s.len()]);
    let stem = match unzipped.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ if unzipped.is_empty() => base,
        _ => unzipped,
    };
    let mut name = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name = name.trim_matches('_');
    if name.is_empty() {
        return "data".into();
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("x{name}")
    } else {
        name.to_string()
    }
}

/// Whether `name` can be assigned to as is.
pub fn valid_variable(name: &str, language: &str) -> bool {
    let mut chars = name.chars();
    let rest_ok = |c: char| c.is_ascii_alphanumeric() || c == '_';
    if language.eq_ignore_ascii_case("r") {
        if name == "." {
            return true;
        }
        let r_rest = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_';
        return match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => chars.all(r_rest),
            Some('.') => {
                matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '.')
                    && chars.all(r_rest)
            }
            _ => false,
        };
    }
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => chars.all(rest_ok),
        _ => false,
    }
}

/// The path as the code should say it: relative to the session's folder
/// when inside it.
pub fn code_path(path: &str, cwd: Option<&str>) -> String {
    let Some(cwd) = cwd else {
        return path.to_string();
    };
    let dir = cwd.trim_end_matches('/');
    match path.strip_prefix(&format!("{dir}/")) {
        Some(rest) => rest.to_string(),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{code_path, importable, imports_for, valid_variable, variable_name};

    #[test]
    fn writes_the_code_it_runs() {
        let r = imports_for("R");
        assert_eq!(
            r[0].code("data/counts.tsv", "counts"),
            "counts <- read.delim(\"data/counts.tsv\")"
        );
        assert_eq!(r[6].code("x.RData", "x"), "load(\"x.RData\")");
        assert!(!r[6].named("x.RData"));
        assert_eq!(
            r[7].code("filtered", "pbmc"),
            "pbmc <- Seurat::Read10X(data.dir = \"filtered\")"
        );
        let py = imports_for("python");
        assert_eq!(
            py[0].code("a.tsv.gz", "a"),
            "import pandas as pd\na = pd.read_csv(\"a.tsv.gz\", sep=\"\\t\")"
        );
        assert!(imports_for("julia").is_empty());
    }

    #[test]
    fn picks_files_and_names() {
        let r = imports_for("r");
        assert!(importable(&r[0], "a.csv.gz"));
        assert!(!importable(&r[0], "a.h5ad.gz"));
        assert!(importable(&r[2], "Book1.XLSX"));
        assert_eq!(variable_name("/d/pbmc3k_counts.csv"), "pbmc3k_counts");
        assert_eq!(variable_name("/d/10x out/"), "x10x_out");
        assert_eq!(variable_name("/d/a.tsv.gz"), "a");
        assert!(valid_variable("seu.obj", "r"));
        assert!(!valid_variable("seu.obj", "python"));
        assert!(!valid_variable("1a", "python"));
        assert_eq!(code_path("/p/data/a.csv", Some("/p")), "data/a.csv");
        assert_eq!(code_path("/q/a.csv", Some("/p/")), "/q/a.csv");
    }
}
