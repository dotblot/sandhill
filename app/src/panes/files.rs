//! Names, templates and matching for files, from the Tauri app's
//! `model/files.ts` and `model/projectFiles.ts`: what the files pane's New
//! items make, why a typed name can't be used, Duplicate's names, and the
//! loose match the ⌘K palette ranks files and commands by.

use std::collections::HashSet;

use sandhill_proto::KernelspecInfo;

/// What the files pane's New items make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewKind {
    File,
    Folder,
    Quarto,
    RScript,
    PythonScript,
    Notebook,
}

impl NewKind {
    pub const ALL: [NewKind; 6] = [
        NewKind::File,
        NewKind::Folder,
        NewKind::Quarto,
        NewKind::RScript,
        NewKind::PythonScript,
        NewKind::Notebook,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NewKind::File => "New file",
            NewKind::Folder => "New folder",
            NewKind::Quarto => "New Quarto document",
            NewKind::RScript => "New R script",
            NewKind::PythonScript => "New Python script",
            NewKind::Notebook => "New notebook",
        }
    }

    /// The name the field starts with.
    pub fn start_name(self) -> &'static str {
        match self {
            NewKind::File | NewKind::Folder => "",
            NewKind::Quarto => "untitled.qmd",
            NewKind::RScript => "untitled.R",
            NewKind::PythonScript => "untitled.py",
            NewKind::Notebook => "untitled.ipynb",
        }
    }

    pub fn keys(self) -> &'static str {
        match self {
            NewKind::File => "⌘N",
            NewKind::Folder => "⌥⌘N",
            _ => "",
        }
    }
}

pub fn base_name(path: &str) -> &str {
    let path = path.trim_end_matches('/');
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

pub fn parent_of(path: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((dir, _)) => dir.into(),
    }
}

pub fn join_path(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

/// Whether `path` is `dir` or inside it.
pub fn within(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{}/", dir.trim_end_matches('/')))
}

/// Why a typed name can't be used, if it can't. New items may be paths
/// (`raw/samples.csv` makes the folder too); a rename is one name.
pub fn name_problem(name: &str, path: bool) -> Option<&'static str> {
    if name.chars().any(|c| "\\:*?\"<>|".contains(c)) {
        return Some("A name can't contain \\ : * ? \" < > |");
    }
    if !path && name.contains('/') {
        return Some("A name can't contain /");
    }
    if name.starts_with('/')
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Some("That isn't a name inside this folder");
    }
    None
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    }
}

/// `name`, or "untitled 2.qmd" if that's taken, then "untitled 3.qmd".
pub fn free_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    (2..)
        .map(|n| format!("{stem} {n}{ext}"))
        .find(|c| !taken.contains(c))
        .unwrap_or_default()
}

/// Duplicate's name: "fig3 copy.qmd" or "data copy", then "fig3 copy 2.qmd".
pub fn copy_name(name: &str, dir: bool, taken: &HashSet<String>) -> String {
    let (stem, ext) = if dir { (name, "") } else { split_ext(name) };
    free_name(&format!("{stem} copy{ext}"), taken)
}

/// New Quarto code uses its initial kernel's language; without a known kernel,
/// leave only the title block so adding code after kernel selection is unambiguous.
/// R Markdown keeps its R chunk. Jupyter notebooks come from [`new_notebook`].
pub fn template(name: &str, language: Option<&str>) -> String {
    let (stem, ext) = split_ext(name);
    let title = stem.replace('"', "\\\"");
    match ext.to_ascii_lowercase().as_str() {
        ".qmd" => {
            let mut text = format!("---\ntitle: \"{title}\"\nformat: html\n---\n");
            if let Some(language) = language {
                let language = sandhill_daemon::cells::normalize_language(language);
                if matches!(language.as_str(), "r" | "python") {
                    text.push_str(&format!("\n```{{{language}}}\n\n```\n"));
                }
            }
            text
        }
        ".rmd" => {
            format!("---\ntitle: \"{title}\"\noutput: html_document\n---\n\n```{{r}}\n\n```\n")
        }
        ".r" => format!("# {name}\n\n"),
        ".md" => format!("# {stem}\n"),
        _ => String::new(),
    }
}

/// An empty notebook with one code cell, for a kernel if there is one.
pub fn new_notebook(spec: Option<&KernelspecInfo>) -> String {
    let mut metadata = serde_json::Map::new();
    if let Some(spec) = spec {
        metadata.insert(
            "kernelspec".into(),
            serde_json::json!({
                "name": spec.name,
                "display_name": spec.display_name,
                "language": spec.language,
            }),
        );
        metadata.insert(
            "language_info".into(),
            serde_json::json!({ "name": spec.language.to_ascii_lowercase() }),
        );
    }
    let notebook = serde_json::json!({
        "cells": [{
            "cell_type": "code",
            "execution_count": null,
            "id": "cell-1",
            "metadata": {},
            "outputs": [],
            "source": [],
        }],
        "metadata": metadata,
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&notebook).unwrap_or_default()
    )
}

/// Folders with nothing to open that can hold many files; the palette
/// doesn't look in them.
pub const SKIP: [&str; 8] = [
    "node_modules",
    "renv",
    "__pycache__",
    "site-packages",
    "venv",
    "env",
    "_freeze",
    "_site",
];
pub const MAX_DIRS: usize = 400;
pub const MAX_FILES: usize = 3000;

/// How well `query` matches `text`: its letters must appear in order. Runs
/// of consecutive letters, and letters at the start of words, score higher.
/// None when it doesn't match.
pub fn match_score(text: &str, query: &str) -> Option<f32> {
    let q: Vec<char> = query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0.);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0.;
    let mut from = 0;
    let mut previous: isize = -2;
    for ch in q {
        let at = (from..t.len()).find(|&i| t[i] == ch)?;
        score += if at as isize == previous + 1 { 3. } else { 1. };
        if at == 0 || " /_.-\t".contains(t[at - 1]) {
            score += 2.;
        }
        previous = at as isize;
        from = at + 1;
    }
    // Shorter names win among equal matches.
    Some(score - t.len() as f32 / 100.)
}

#[cfg(test)]
mod tests {
    use super::{
        NewKind, base_name, copy_name, free_name, match_score, name_problem, new_notebook,
        parent_of, template, within,
    };
    use crate::editor::notebook::{CellKind, parse, recover_kernel};
    use sandhill_proto::KernelspecInfo;
    use std::collections::HashSet;

    #[test]
    fn python_creation_reuses_naming_and_document_dispatch() {
        let kind = NewKind::PythonScript;
        assert!(NewKind::ALL.contains(&kind));
        assert_eq!(kind.start_name(), "untitled.py");
        let taken = HashSet::from(["untitled.py".into()]);
        let name = free_name(kind.start_name(), &taken);
        assert_eq!(name, "untitled 2.py");
        assert_eq!(name_problem(&name, true), None);
        assert_eq!(
            crate::editor::notebook::parse(&name, "")
                .unwrap()
                .language
                .as_deref(),
            Some("python")
        );
    }

    #[test]
    fn checks_names() {
        assert_eq!(name_problem("a.csv", false), None);
        assert_eq!(name_problem("raw/a.csv", true), None);
        assert!(name_problem("raw/a.csv", false).is_some());
        assert!(name_problem("../a", true).is_some());
        assert!(name_problem("a:b", true).is_some());
        assert!(name_problem("", true).is_some());
    }

    #[test]
    fn names_copies() {
        let taken: HashSet<String> = ["fig3 copy.qmd".to_string()].into();
        assert_eq!(copy_name("fig3.qmd", false, &taken), "fig3 copy 2.qmd");
        assert_eq!(copy_name("data", true, &taken), "data copy");
        assert_eq!(copy_name(".env", false, &taken), ".env copy");
        assert_eq!(base_name("/p/a/b.R"), "b.R");
        assert_eq!(parent_of("/p/a"), "/p");
        assert_eq!(parent_of("/p"), "/");
        assert!(within("/p/a", "/p"));
        assert!(!within("/pa", "/p"));
    }

    #[test]
    fn quarto_template_matches_initial_kernel_and_added_cells() {
        let spec = KernelspecInfo {
            name: "python3".into(),
            display_name: "Python".into(),
            language: "python".into(),
            env: None,
            version: None,
            engine: None,
        };
        let empty = parse("report.qmd", "").unwrap();
        let kernel = recover_kernel(&empty, std::slice::from_ref(&spec), None, None, None);
        assert_eq!(kernel.as_deref(), Some("python3"));
        let mut notebook =
            parse("report.qmd", &template("report.qmd", Some(&spec.language))).unwrap();
        let first = notebook
            .cells
            .iter()
            .position(|cell| cell.kind == CellKind::Code)
            .unwrap();
        assert_eq!(notebook.cell_language(first), Some("python"));
        let mut sources: Vec<_> = notebook
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        sources[first] = "import pandas as pd\n\ndf = pd.DataFrame({'value': [1]})".into();
        let (second, _) = notebook
            .insert_quarto_cell(
                notebook.cells.len(),
                CellKind::Code,
                Some("python"),
                &sources,
            )
            .unwrap();
        assert_eq!(notebook.cell_language(second), Some("python"));
    }

    #[test]
    fn quarto_templates_preserve_known_languages_and_defer_unknown_ones() {
        for (language, expected) in [
            ("python", "python"),
            ("python3", "python"),
            ("PYTHON", "python"),
            ("R", "r"),
        ] {
            let notebook = parse("report.qmd", &template("report.qmd", Some(language))).unwrap();
            assert_eq!(notebook.cell_language(1), Some(expected));
        }
        for language in [None, Some("unknown")] {
            let mut notebook = parse("report.qmd", &template("report.qmd", language)).unwrap();
            assert!(
                notebook
                    .cells
                    .iter()
                    .all(|cell| cell.kind != CellKind::Code)
            );
            // No kernel was known when opened; choosing Python before + Code
            // must not leave behind a provisional R chunk.
            let sources = notebook
                .cells
                .iter()
                .map(|cell| cell.source.clone())
                .collect::<Vec<_>>();
            let (at, _) = notebook
                .insert_quarto_cell(
                    notebook.cells.len(),
                    CellKind::Code,
                    Some("python"),
                    &sources,
                )
                .unwrap();
            assert_eq!(notebook.cell_language(at), Some("python"));
        }
        let rmd = parse("report.Rmd", &template("report.Rmd", Some("python"))).unwrap();
        assert_eq!(rmd.cell_language(1), Some("r"));
    }

    #[test]
    fn writes_templates() {
        assert!(template("fig 3.qmd", None).starts_with("---\ntitle: \"fig 3\"\nformat: html"));
        assert_eq!(template("a.R", None), "# a.R\n\n");
        assert_eq!(template("a.csv", None), "");
        let nb: serde_json::Value = serde_json::from_str(&new_notebook(None)).unwrap();
        assert_eq!(nb["nbformat"], 4);
        assert_eq!(nb["cells"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn matches_loosely() {
        assert!(match_score("analysis.qmd", "anq").is_some());
        assert!(match_score("analysis.qmd", "qa").is_none());
        let start = match_score("run all", "ra").unwrap();
        let middle = match_score("hydra", "ra").unwrap();
        assert!(start > middle);
        assert_eq!(match_score("x", "  "), Some(0.));
    }
}
