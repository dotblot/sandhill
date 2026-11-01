//! A notebook as the trial app shows it: its cells, in order, read from a
//! Jupyter `.ipynb` or a Quarto `.qmd`/`.Rmd` document. Milestone 1 only
//! reads; editing and saving come later.

use anyhow::{Context, Result};
use sandhill_daemon::qmd;
use sandhill_proto::KernelspecInfo;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ipynb,
    Quarto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Code,
    /// Markdown, raw cells and the prose between Quarto chunks.
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The id outputs are journaled under: the `.ipynb` cell id, or the
    /// Quarto chunk's label or `chunk-<n>`, as the daemon and the web UI
    /// give it.
    pub id: String,
    pub kind: CellKind,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notebook {
    pub path: String,
    pub format: Format,
    pub cells: Vec<Cell>,
    /// The language the document's code is in, such as `python` or `r`.
    pub language: Option<String>,
    /// The kernel the document names, if any.
    pub kernel: Option<String>,
}

impl Notebook {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The folder the document is in, where its code runs.
    pub fn folder(&self) -> &str {
        match self.path.rfind('/') {
            Some(0) => "/",
            Some(i) => &self.path[..i],
            None => ".",
        }
    }
}

pub fn format_of(path: &str) -> Option<Format> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "ipynb" => Some(Format::Ipynb),
        "qmd" | "rmd" => Some(Format::Quarto),
        _ => None,
    }
}

pub fn parse(path: &str, text: &str) -> Result<Notebook> {
    match format_of(path).context("not a notebook or Quarto document")? {
        Format::Ipynb => parse_ipynb(path, text),
        Format::Quarto => Ok(parse_quarto(path, text)),
    }
}

fn parse_ipynb(path: &str, text: &str) -> Result<Notebook> {
    let json: Value = serde_json::from_str(text).context("the notebook is not valid JSON")?;
    let meta = &json["metadata"];
    let kernel = meta["kernelspec"]["name"].as_str().map(str::to_string);
    let language = meta["kernelspec"]["language"]
        .as_str()
        .or(meta["language_info"]["name"].as_str())
        .map(str::to_ascii_lowercase);
    let mut cells = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, cell) in json["cells"].as_array().into_iter().flatten().enumerate() {
        // Old notebooks have no ids. The web UI makes some up and saves
        // them; this read-only trial numbers them instead.
        let id = match cell["id"].as_str() {
            Some(id) if !id.is_empty() && !seen.contains(id) => id.to_string(),
            _ => format!("cell-{}", i + 1),
        };
        seen.insert(id.clone());
        let kind = if cell["cell_type"] == "code" {
            CellKind::Code
        } else {
            CellKind::Text
        };
        cells.push(Cell {
            id,
            kind,
            source: multiline(&cell["source"]),
        });
    }
    Ok(Notebook {
        path: path.to_string(),
        format: Format::Ipynb,
        cells,
        language,
        kernel,
    })
}

fn parse_quarto(path: &str, text: &str) -> Notebook {
    let chunks = qmd::parse_chunks(text);
    let mut cells = Vec::new();
    let prose = |from: usize, to: usize, cells: &mut Vec<Cell>| {
        let piece = text[from..to].trim();
        if !piece.is_empty() {
            cells.push(Cell {
                id: format!("text-{from}"),
                kind: CellKind::Text,
                source: piece.to_string(),
            });
        }
    };
    let mut at = 0;
    for chunk in &chunks {
        prose(at, chunk.from, &mut cells);
        cells.push(Cell {
            id: chunk.cell_id.clone(),
            kind: CellKind::Code,
            source: chunk.code.trim_end_matches(['\n', '\r']).to_string(),
        });
        at = chunk.to;
    }
    prose(at.min(text.len()), text.len(), &mut cells);
    Notebook {
        path: path.to_string(),
        format: Format::Quarto,
        cells,
        language: chunks.first().map(|c| c.language.clone()),
        kernel: qmd::front_matter_kernel(text),
    }
}

/// The kernel to run a notebook in: the one it names if installed, else the
/// first for its language, else the first there is.
pub fn pick_kernel(notebook: &Notebook, specs: &[KernelspecInfo]) -> Option<String> {
    if let Some(name) = &notebook.kernel
        && specs.iter().any(|s| &s.name == name)
    {
        return Some(name.clone());
    }
    if let Some(language) = &notebook.language
        && let Some(spec) = specs
            .iter()
            .find(|s| s.language.eq_ignore_ascii_case(language))
    {
        return Some(spec.name.clone());
    }
    specs.first().map(|s| s.name.clone())
}

/// nbformat keeps text as a string or a list of lines.
pub fn multiline(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, language: &str) -> KernelspecInfo {
        KernelspecInfo {
            name: name.into(),
            display_name: name.into(),
            language: language.into(),
            env: None,
        }
    }

    #[test]
    fn reads_ipynb_cells_and_numbers_cells_without_ids() {
        let text = r##"{
          "metadata": {"kernelspec": {"name": "python3", "language": "python"}},
          "cells": [
            {"cell_type": "markdown", "id": "a", "source": ["# Title\n", "text"]},
            {"cell_type": "code", "source": "print(1)"},
            {"cell_type": "code", "id": "a", "source": "x = 2"}
          ]
        }"##;
        let nb = parse("/p/analysis.ipynb", text).unwrap();
        assert_eq!(nb.kernel.as_deref(), Some("python3"));
        assert_eq!(nb.language.as_deref(), Some("python"));
        let ids: Vec<_> = nb.cells.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "cell-2", "cell-3"]);
        assert_eq!(nb.cells[0].source, "# Title\ntext");
        assert_eq!(nb.cells[0].kind, CellKind::Text);
        assert_eq!(nb.cells[1].kind, CellKind::Code);
        assert_eq!(nb.folder(), "/p");
        assert_eq!(nb.name(), "analysis.ipynb");
    }

    #[test]
    fn reads_quarto_chunks_with_the_daemons_ids() {
        let text = "---\ntitle: QC\n---\n\nSome text.\n\n```{r}\nx <- 1\n```\n\n```{r qc-plot}\nplot(x)\n```\nEnd.\n";
        let nb = parse("/p/qc.qmd", text).unwrap();
        let shown: Vec<_> = nb
            .cells
            .iter()
            .map(|c| (c.kind, c.id.as_str(), c.source.as_str()))
            .collect();
        assert_eq!(shown[1], (CellKind::Code, "chunk-1", "x <- 1"));
        assert_eq!(shown[2], (CellKind::Code, "qc-plot", "plot(x)"));
        assert_eq!(shown[3].2, "End.");
        assert!(shown[0].2.contains("Some text."));
        assert_eq!(nb.language.as_deref(), Some("r"));
    }

    #[test]
    fn picks_the_named_kernel_then_the_language() {
        let specs = [spec("python3", "python"), spec("ark", "R")];
        let mut nb = parse("/p/a.qmd", "```{r}\n1\n```\n").unwrap();
        assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("ark"));
        nb.kernel = Some("python3".into());
        assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("python3"));
        nb.kernel = Some("missing".into());
        nb.language = None;
        assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("python3"));
    }

    #[test]
    fn knows_which_files_are_notebooks() {
        assert_eq!(format_of("a/b.ipynb"), Some(Format::Ipynb));
        assert_eq!(format_of("b.Rmd"), Some(Format::Quarto));
        assert_eq!(format_of("b.R"), None);
        assert_eq!(format_of("Makefile"), None);
    }
}
