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
    Notebook,
}

impl NewKind {
    pub const ALL: [NewKind; 5] = [
        NewKind::File,
        NewKind::Folder,
        NewKind::Quarto,
        NewKind::RScript,
        NewKind::Notebook,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NewKind::File => "New file",
            NewKind::Folder => "New folder",
            NewKind::Quarto => "New Quarto document",
            NewKind::RScript => "New R script",
            NewKind::Notebook => "New notebook",
        }
    }

    /// The name the field starts with.
    pub fn start_name(self) -> &'static str {
        match self {
            NewKind::File | NewKind::Folder => "",
            NewKind::Quarto => "untitled.qmd",
            NewKind::RScript => "untitled.R",
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

/// What a new file starts with: a Quarto title block and an empty R chunk,
/// a comment naming an R script, a heading for Markdown, and nothing for
/// anything else. Notebooks come from [`new_notebook`].
pub fn template(name: &str) -> String {
    let (stem, ext) = split_ext(name);
    let title = stem.replace('"', "\\\"");
    match ext.to_ascii_lowercase().as_str() {
        ".qmd" => format!("---\ntitle: \"{title}\"\nformat: html\n---\n\n```{{r}}\n\n```\n"),
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
    use super::*;

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
    fn writes_templates() {
        assert!(template("fig 3.qmd").starts_with("---\ntitle: \"fig 3\"\nformat: html"));
        assert_eq!(template("a.R"), "# a.R\n\n");
        assert_eq!(template("a.csv"), "");
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
