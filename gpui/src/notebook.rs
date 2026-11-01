//! A notebook as the trial app shows it: its cells, in order, read from a
//! Jupyter `.ipynb` or a Quarto `.qmd`/`.Rmd` document, and written back
//! with the cells' new text.

use std::ops::Range;

use anyhow::{Context, Result};
use sandhill_daemon::qmd;
use sandhill_proto::KernelspecInfo;
use serde_json::{Map, Value};

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
    /// The outputs saved in an `.ipynb` (with big values as blob references).
    pub outputs: Vec<Value>,
    pub execution_count: Option<u64>,
    /// Where the cell's text is in a Quarto document.
    span: Range<usize>,
    /// The `.ipynb` cell had no id; one is written when saving.
    new_id: bool,
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
    /// The `.ipynb` JSON or the Quarto text, as read.
    original: Original,
}

#[derive(Debug, Clone, PartialEq)]
enum Original {
    Json(Value),
    Text(String),
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

    /// The document with each cell's text replaced by `sources` (one per
    /// cell, in order): nbformat's layout for an `.ipynb`, the same text
    /// around the chunks for Quarto.
    pub fn render(&self, sources: &[String]) -> Result<String> {
        anyhow::ensure!(sources.len() == self.cells.len(), "one text per cell");
        match &self.original {
            Original::Json(json) => {
                let mut json = json.clone();
                let cells = json["cells"].as_array_mut().context("no cells")?;
                let mut any_new_id = false;
                for ((cell, json_cell), source) in self.cells.iter().zip(cells).zip(sources) {
                    json_cell["source"] = lines(source);
                    if cell.new_id {
                        json_cell["id"] = Value::String(cell.id.clone());
                        any_new_id = true;
                    }
                }
                // Cell ids need nbformat 4.5.
                if any_new_id && json["nbformat_minor"].as_u64().unwrap_or(0) < 5 {
                    json["nbformat_minor"] = Value::from(5);
                }
                sort_keys(&mut json);
                let mut out = Vec::new();
                let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
                let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
                serde::Serialize::serialize(&json, &mut ser)?;
                out.push(b'\n');
                Ok(String::from_utf8(out)?)
            }
            Original::Text(text) => {
                let mut out = String::with_capacity(text.len());
                let mut at = 0;
                for (cell, source) in self.cells.iter().zip(sources) {
                    out.push_str(&text[at..cell.span.start]);
                    out.push_str(source);
                    // An empty chunk has no line of its own to put code on.
                    if cell.span.is_empty() && cell.kind == CellKind::Code && !source.is_empty() {
                        out.push('\n');
                    }
                    at = cell.span.end;
                }
                out.push_str(&text[at..]);
                Ok(out)
            }
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
        let (id, new_id) = match cell["id"].as_str() {
            Some(id) if !id.is_empty() && !seen.contains(id) => (id.to_string(), false),
            _ => (new_cell_id(i, &seen), true),
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
            outputs: cell["outputs"].as_array().cloned().unwrap_or_default(),
            execution_count: cell["execution_count"].as_u64(),
            span: 0..0,
            new_id,
        });
    }
    Ok(Notebook {
        path: path.to_string(),
        format: Format::Ipynb,
        cells,
        language,
        kernel,
        original: Original::Json(json),
    })
}

/// An id for a cell that has none: `cell-<n>`, unless that is taken.
fn new_cell_id(index: usize, seen: &std::collections::HashSet<String>) -> String {
    let mut n = index + 1;
    loop {
        let id = format!("cell-{n}");
        if !seen.contains(&id) {
            return id;
        }
        n += 1000;
    }
}

fn parse_quarto(path: &str, text: &str) -> Notebook {
    let chunks = qmd::parse_chunks(text);
    let mut cells = Vec::new();
    let prose = |from: usize, to: usize, cells: &mut Vec<Cell>| {
        let piece = &text[from..to];
        let trimmed = piece.trim();
        if !trimmed.is_empty() {
            let start = from + (piece.len() - piece.trim_start().len());
            cells.push(Cell {
                id: format!("text-{start}"),
                kind: CellKind::Text,
                source: trimmed.to_string(),
                outputs: Vec::new(),
                execution_count: None,
                span: start..start + trimmed.len(),
                new_id: false,
            });
        }
    };
    let mut at = 0;
    for chunk in &chunks {
        prose(at, chunk.from, &mut cells);
        let code = chunk.code.trim_end_matches(['\n', '\r']);
        cells.push(Cell {
            id: chunk.cell_id.clone(),
            kind: CellKind::Code,
            source: code.to_string(),
            outputs: Vec::new(),
            execution_count: None,
            span: chunk.code_from..chunk.code_from + code.len(),
            new_id: false,
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
        original: Original::Text(text.to_string()),
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

/// Text as nbformat writes it: a list of lines, each but the last ending
/// in a newline.
fn lines(text: &str) -> Value {
    Value::Array(
        text.split_inclusive('\n')
            .map(|l| Value::String(l.to_string()))
            .collect(),
    )
}

fn sort_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut sorted = Map::new();
            for (key, mut value) in entries {
                sort_keys(&mut value);
                sorted.insert(key, value);
            }
            *map = sorted;
        }
        Value::Array(items) => items.iter_mut().for_each(sort_keys),
        _ => {}
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

    fn sources(nb: &Notebook) -> Vec<String> {
        nb.cells.iter().map(|c| c.source.clone()).collect()
    }

    const IPYNB: &str = r##"{
      "metadata": {"kernelspec": {"name": "python3", "language": "python"}},
      "nbformat": 4, "nbformat_minor": 4,
      "cells": [
        {"cell_type": "markdown", "id": "a", "metadata": {}, "source": ["# Title\n", "text"]},
        {"cell_type": "code", "metadata": {}, "execution_count": 3, "source": "print(1)",
         "outputs": [{"output_type": "stream", "name": "stdout", "text": "1\n"}]},
        {"cell_type": "code", "id": "a", "metadata": {}, "source": "x = 2", "outputs": []}
      ]
    }"##;

    #[test]
    fn reads_ipynb_cells_and_their_saved_outputs() {
        let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
        assert_eq!(nb.kernel.as_deref(), Some("python3"));
        assert_eq!(nb.language.as_deref(), Some("python"));
        let ids: Vec<_> = nb.cells.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["a", "cell-2", "cell-3"]);
        assert_eq!(nb.cells[0].source, "# Title\ntext");
        assert_eq!(nb.cells[0].kind, CellKind::Text);
        assert_eq!(nb.cells[1].kind, CellKind::Code);
        assert_eq!(nb.cells[1].outputs.len(), 1);
        assert_eq!(nb.cells[1].execution_count, Some(3));
        assert_eq!(nb.folder(), "/p");
        assert_eq!(nb.name(), "analysis.ipynb");
    }

    #[test]
    fn writes_ipynb_in_nbformats_layout_with_new_ids() {
        let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
        let mut texts = sources(&nb);
        texts[2] = "x = 3\ny = 4".into();
        let out = nb.render(&texts).unwrap();
        assert!(out.ends_with("}\n"));
        assert!(
            out.contains("\n \"cells\": [\n"),
            "one-space indent:\n{out}"
        );
        let json: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(json["nbformat_minor"], 5);
        assert_eq!(json["cells"][1]["id"], "cell-2");
        assert_eq!(
            json["cells"][2]["source"],
            serde_json::json!(["x = 3\n", "y = 4"])
        );
        assert_eq!(json["cells"][1]["outputs"][0]["text"], "1\n");
        // Keys sorted, as nbformat writes them.
        let keys: Vec<_> = json["cells"][1]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        // Reading it back gives the same cells.
        let again = parse("/p/analysis.ipynb", &out).unwrap();
        assert_eq!(sources(&again), texts);
    }

    const QMD: &str = "---\ntitle: QC\n---\n\nSome text.\n\n```{r}\nx <- 1\n```\n\n```{r qc-plot}\nplot(x)\n```\n\n```{r}\n```\nEnd.\n";

    #[test]
    fn reads_quarto_chunks_with_the_daemons_ids() {
        let nb = parse("/p/qc.qmd", QMD).unwrap();
        let shown: Vec<_> = nb
            .cells
            .iter()
            .map(|c| (c.kind, c.id.as_str(), c.source.as_str()))
            .collect();
        assert_eq!(shown[1], (CellKind::Code, "chunk-1", "x <- 1"));
        assert_eq!(shown[2], (CellKind::Code, "qc-plot", "plot(x)"));
        assert_eq!(shown[3], (CellKind::Code, "chunk-3", ""));
        assert_eq!(shown[4].2, "End.");
        assert!(shown[0].2.contains("Some text."));
        assert_eq!(nb.language.as_deref(), Some("r"));
    }

    #[test]
    fn writes_quarto_with_only_the_cells_changed() {
        let nb = parse("/p/qc.qmd", QMD).unwrap();
        assert_eq!(nb.render(&sources(&nb)).unwrap(), QMD);
        let mut texts = sources(&nb);
        texts[1] = "x <- 2\ny <- 3".into();
        texts[3] = "summary(x)".into();
        texts[4] = "The end.".into();
        let out = nb.render(&texts).unwrap();
        assert_eq!(
            out,
            QMD.replace("x <- 1", "x <- 2\ny <- 3")
                .replace("```{r}\n```", "```{r}\nsummary(x)\n```")
                .replace("End.", "The end.")
        );
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
