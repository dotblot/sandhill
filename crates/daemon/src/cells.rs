//! Agents' edits to documents that no window has open: a code cell added
//! after another, or a cell's code replaced. Quarto and R Markdown chunks,
//! notebook cells, and code appended to an R or Python script. The file is
//! replaced atomically, as the app's own saves are.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::qmd::{self, Chunk};
use crate::{files, notebook};

/// Languages a cell can run in.
const RUNNABLE: [&str; 2] = ["r", "python"];

#[derive(Debug, Clone, PartialEq)]
pub struct Edited {
    /// The cell's id after the edit; empty for a script.
    pub cell_id: String,
    pub language: String,
    /// The kernel the document asks for: a notebook's kernelspec, or the
    /// `jupyter:` kernel of a Quarto document's front matter.
    pub kernel: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Quarto,
    Notebook,
    Script(&'static str),
}

pub fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    Some(match ext.as_str() {
        "qmd" | "rmd" => Kind::Quarto,
        "ipynb" => Kind::Notebook,
        "r" => Kind::Script("r"),
        "py" => Kind::Script("python"),
        _ => return None,
    })
}

/// `r` or `python` for the names people and kernels use.
pub fn normalize_language(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "py" | "python3" | "python" => "python".into(),
        other => other.into(),
    }
}

/// Adds `code` as a new cell after cell `after`, or after the last cell.
pub fn add_cell(
    path: &str,
    code: &str,
    language: Option<&str>,
    after: Option<&str>,
) -> Result<Edited> {
    let resolved = files::resolve(path)?;
    let text = files::read_text(path)?.text;
    let language = language.map(normalize_language);
    let (text, edited) = match kind_of(&resolved)? {
        Kind::Quarto => add_chunk(&text, code, language, after, path)?,
        Kind::Notebook => add_notebook_cell(&text, code, language, after, path)?,
        Kind::Script(own) => {
            if let Some(language) = language.filter(|l| l != own) {
                bail!("{path} is a {own} script; it can't hold {language} code");
            }
            let edited = Edited {
                cell_id: String::new(),
                language: own.into(),
                kernel: None,
            };
            (append(&text, code), edited)
        }
    };
    files::write_text(path, &text, None)?;
    Ok(edited)
}

/// Replaces the code of cell `cell_id`.
pub fn edit_cell(path: &str, cell_id: &str, code: &str) -> Result<Edited> {
    let resolved = files::resolve(path)?;
    let text = files::read_text(path)?.text;
    let (text, edited) = match kind_of(&resolved)? {
        Kind::Quarto => edit_chunk(&text, cell_id, code, path)?,
        Kind::Notebook => edit_notebook_cell(&text, cell_id, code, path)?,
        Kind::Script(_) => bail!("a script has no cells; edit {path} as a file instead"),
    };
    files::write_text(path, &text, None)?;
    Ok(edited)
}

/// A cell's language and code as the file has it.
pub fn cell_code(path: &str, cell_id: &str) -> Result<Option<(String, String)>> {
    let resolved = files::resolve(path)?;
    let text = files::read_text(path)?.text;
    Ok(match kind_of(&resolved)? {
        Kind::Quarto => qmd::parse_chunks(&text)
            .into_iter()
            .find(|c| c.cell_id == cell_id)
            .map(|c| (c.language, c.code)),
        Kind::Notebook => {
            let nb = parse_notebook(&text, path)?;
            let language = notebook_language(&nb).unwrap_or_default();
            cells(&nb)
                .iter()
                .find(|c| c["id"] == cell_id)
                .map(|c| (language, join_source(&c["source"])))
        }
        Kind::Script(_) => None,
    })
}

fn kind_of(path: &Path) -> Result<Kind> {
    kind(path).with_context(|| {
        format!(
            "{} is not a notebook, a Quarto or R Markdown document, or an R or Python script",
            path.display()
        )
    })
}

fn add_chunk(
    text: &str,
    code: &str,
    language: Option<String>,
    after: Option<&str>,
    path: &str,
) -> Result<(String, Edited)> {
    let chunks = qmd::parse_chunks(text);
    let anchor = match after {
        Some(id) => Some(find_chunk(&chunks, id, path)?),
        None => chunks.last(),
    };
    let language = match language {
        Some(language) => language,
        None => anchor
            .map(|c| c.language.clone())
            .filter(|l| RUNNABLE.contains(&l.as_str()))
            .or_else(|| {
                chunks
                    .iter()
                    .map(|c| c.language.clone())
                    .find(|l| RUNNABLE.contains(&l.as_str()))
            })
            .or_else(|| qmd::front_matter_kernel(text).map(|_| "python".into()))
            .or_else(|| path.to_lowercase().ends_with(".rmd").then(|| "r".into()))
            .context("say which `language` the cell is in (r or python)")?,
    };
    if !RUNNABLE.contains(&language.as_str()) {
        bail!("cells can be r or python, not {language}");
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let body = code.trim_end_matches(['\n', '\r']).replace('\n', nl);
    let body = if nl == "\r\n" {
        body.replace("\r\r\n", "\r\n")
    } else {
        body
    };
    let chunk = format!("```{{{language}}}{nl}{body}{nl}```");
    // After the anchor's closing fence, or at the end, a blank line apart.
    let (at, insert) = match anchor.filter(|c| c.closed) {
        Some(anchor) => (anchor.to, format!("{nl}{nl}{chunk}")),
        None if text.trim().is_empty() => (0, format!("{chunk}{nl}")),
        None if text.ends_with('\n') => (text.len(), format!("{nl}{chunk}{nl}")),
        None => (text.len(), format!("{nl}{nl}{chunk}{nl}")),
    };
    let start = at + insert.find("```").unwrap_or(0);
    let mut new_text = String::with_capacity(text.len() + insert.len());
    new_text.push_str(&text[..at]);
    new_text.push_str(&insert);
    new_text.push_str(&text[at..]);
    let cell_id = qmd::parse_chunks(&new_text)
        .into_iter()
        .find(|c| c.from == start)
        .map(|c| c.cell_id)
        .context("the new chunk could not be found again")?;
    let kernel = (language == "python")
        .then(|| qmd::front_matter_kernel(text))
        .flatten();
    Ok((
        new_text,
        Edited {
            cell_id,
            language,
            kernel,
        },
    ))
}

fn edit_chunk(text: &str, cell_id: &str, code: &str, path: &str) -> Result<(String, Edited)> {
    let chunks = qmd::parse_chunks(text);
    let chunk = find_chunk(&chunks, cell_id, path)?;
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut body = code.trim_end_matches(['\n', '\r']).to_string();
    if nl == "\r\n" {
        body = body.replace("\r\n", "\n").replace('\n', nl);
    }
    // An empty chunk's code starts where its closing fence does.
    if chunk.code.is_empty() && chunk.closed && !body.is_empty() {
        body.push_str(nl);
    }
    let mut new_text = String::with_capacity(text.len() + body.len());
    new_text.push_str(&text[..chunk.code_from]);
    new_text.push_str(&body);
    new_text.push_str(&text[chunk.code_to..]);
    let edited = qmd::parse_chunks(&new_text)
        .into_iter()
        .find(|c| c.from == chunk.from)
        .context("the chunk could not be found again")?;
    let kernel = (edited.language == "python")
        .then(|| qmd::front_matter_kernel(text))
        .flatten();
    Ok((
        new_text,
        Edited {
            cell_id: edited.cell_id,
            language: edited.language,
            kernel,
        },
    ))
}

fn find_chunk<'a>(chunks: &'a [Chunk], id: &str, path: &str) -> Result<&'a Chunk> {
    chunks.iter().find(|c| c.cell_id == id).with_context(|| {
        let ids: Vec<&str> = chunks.iter().map(|c| c.cell_id.as_str()).collect();
        if ids.is_empty() {
            format!("{path} has no code chunks")
        } else {
            format!("{path} has no cell {id}; its cells are {}", ids.join(", "))
        }
    })
}

fn add_notebook_cell(
    text: &str,
    code: &str,
    language: Option<String>,
    after: Option<&str>,
    path: &str,
) -> Result<(String, Edited)> {
    let mut nb = parse_notebook(text, path)?;
    give_cells_ids(&mut nb);
    let own = notebook_language(&nb);
    let language = match (language, own) {
        (Some(asked), Some(own)) if asked != own => {
            bail!("{path} is a {own} notebook; it can't hold {asked} code")
        }
        (Some(language), _) | (None, Some(language)) => language,
        (None, None) => "python".into(),
    };
    let index = match after {
        Some(id) => find_cell(&nb, id, path)? + 1,
        None => cells(&nb).len(),
    };
    let id = new_cell_id(&nb);
    let cell = json!({
        "cell_type": "code",
        "execution_count": null,
        "id": id,
        "metadata": {},
        "outputs": [],
        "source": notebook::split_lines(code.trim_end_matches('\n')),
    });
    if let Some(list) = nb["cells"].as_array_mut() {
        list.insert(index, cell);
    }
    let kernel = kernel_name(&nb);
    Ok((
        write_notebook(&nb)?,
        Edited {
            cell_id: id,
            language,
            kernel,
        },
    ))
}

fn edit_notebook_cell(
    text: &str,
    cell_id: &str,
    code: &str,
    path: &str,
) -> Result<(String, Edited)> {
    let mut nb = parse_notebook(text, path)?;
    let index = find_cell(&nb, cell_id, path)?;
    let language = notebook_language(&nb).unwrap_or_else(|| "python".into());
    let kernel = kernel_name(&nb);
    let cell = &mut nb["cells"][index];
    if cell["cell_type"] != "code" {
        bail!("cell {cell_id} of {path} is not a code cell");
    }
    cell["source"] = json!(notebook::split_lines(code.trim_end_matches('\n')));
    // Its outputs were of the old code.
    cell["outputs"] = json!([]);
    cell["execution_count"] = Value::Null;
    Ok((
        write_notebook(&nb)?,
        Edited {
            cell_id: cell_id.into(),
            language,
            kernel,
        },
    ))
}

fn parse_notebook(text: &str, path: &str) -> Result<Value> {
    let nb: Value =
        serde_json::from_str(text).with_context(|| format!("{path} is not a Jupyter notebook"))?;
    if !nb["cells"].is_array() {
        bail!("{path} is not a Jupyter notebook");
    }
    if nb["nbformat"] != 4 {
        bail!("{path} is not an nbformat 4 notebook");
    }
    Ok(nb)
}

fn cells(nb: &Value) -> &[Value] {
    nb["cells"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn find_cell(nb: &Value, id: &str, path: &str) -> Result<usize> {
    cells(nb)
        .iter()
        .position(|c| c["id"] == id)
        .with_context(|| format!("{path} has no cell {id}"))
}

/// Notebooks older than nbformat 4.5 have no cell ids; they get them, as
/// Jupyter gives them when it saves such a notebook.
fn give_cells_ids(nb: &mut Value) {
    let missing = cells(nb).iter().any(|c| !c["id"].is_string());
    if !missing {
        return;
    }
    let count = cells(nb).len();
    for i in 0..count {
        if !nb["cells"][i]["id"].is_string() {
            let id = new_cell_id(nb);
            nb["cells"][i]["id"] = json!(id);
        }
    }
    if nb["nbformat_minor"].as_u64().unwrap_or(0) < 5 {
        nb["nbformat_minor"] = json!(5);
    }
}

fn new_cell_id(nb: &Value) -> String {
    loop {
        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        if !cells(nb).iter().any(|c| c["id"] == id.as_str()) {
            return id;
        }
    }
}

fn notebook_language(nb: &Value) -> Option<String> {
    let metadata = &nb["metadata"];
    metadata["kernelspec"]["language"]
        .as_str()
        .or_else(|| metadata["language_info"]["name"].as_str())
        .map(normalize_language)
}

fn kernel_name(nb: &Value) -> Option<String> {
    nb["metadata"]["kernelspec"]["name"]
        .as_str()
        .map(str::to_string)
}

fn join_source(source: &Value) -> String {
    match source {
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        Value::String(text) => text.clone(),
        _ => String::new(),
    }
}

/// The notebook as nbformat writes it: keys sorted, one-space indent.
fn write_notebook(nb: &Value) -> Result<String> {
    use serde::Serialize;
    let mut out = Vec::new();
    let mut json = serde_json::Serializer::with_formatter(
        &mut out,
        serde_json::ser::PrettyFormatter::with_indent(b" "),
    );
    nb.serialize(&mut json)?;
    out.push(b'\n');
    Ok(String::from_utf8(out)?)
}

/// A script's text with `code` added at its end, a blank line apart.
fn append(text: &str, code: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let code = code.trim_end_matches(['\n', '\r']);
    let separator = if text.trim().is_empty() {
        ""
    } else if text.ends_with('\n') {
        nl
    } else {
        "\n\n"
    };
    format!("{}{separator}{code}{nl}", text.trim_end_matches(' '))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_chunk_goes_after_the_one_named_or_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let doc = file(
            dir.path(),
            "a.qmd",
            "# QC\n\n```{r setup}\nlibrary(x)\n```\n\nText.\n\n```{r}\nplot(1)\n```\n",
        );
        let edited = add_cell(&doc, "dim(x)\n", None, Some("setup")).unwrap();
        assert_eq!(edited.cell_id, "chunk-2");
        assert_eq!(edited.language, "r");
        let text = std::fs::read_to_string(&doc).unwrap();
        assert_eq!(
            text,
            "# QC\n\n```{r setup}\nlibrary(x)\n```\n\n```{r}\ndim(x)\n```\n\nText.\n\n```{r}\nplot(1)\n```\n"
        );
        // The chunk that was second is third now.
        let ids: Vec<String> = qmd::parse_chunks(&text)
            .into_iter()
            .map(|c| c.cell_id)
            .collect();
        assert_eq!(ids, ["setup", "chunk-2", "chunk-3"]);

        let edited = add_cell(&doc, "summary(x)", None, None).unwrap();
        assert_eq!(edited.cell_id, "chunk-4");
        let text = std::fs::read_to_string(&doc).unwrap();
        assert!(
            text.ends_with("plot(1)\n```\n\n```{r}\nsummary(x)\n```\n"),
            "{text}"
        );
    }

    #[test]
    fn a_chunk_gets_a_language_from_its_neighbours_or_the_front_matter() {
        let dir = tempfile::tempdir().unwrap();
        let doc = file(
            dir.path(),
            "b.qmd",
            "---\njupyter: conda-env-omicverse-py\n---\n\nNo code yet.",
        );
        let edited = add_cell(&doc, "import scanpy as sc", None, None).unwrap();
        assert_eq!(edited.language, "python");
        assert_eq!(edited.kernel.as_deref(), Some("conda-env-omicverse-py"));
        assert_eq!(edited.cell_id, "chunk-1");
        let text = std::fs::read_to_string(&doc).unwrap();
        assert!(
            text.ends_with("No code yet.\n\n```{python}\nimport scanpy as sc\n```\n"),
            "{text}"
        );

        let bare = file(dir.path(), "c.qmd", "Just prose.\n");
        let err = add_cell(&bare, "x", None, None).unwrap_err();
        assert!(format!("{err}").contains("language"), "{err}");
        let err = add_cell(&doc, "x", None, Some("nope")).unwrap_err();
        assert!(format!("{err}").contains("its cells are chunk-1"), "{err}");
    }

    #[test]
    fn a_chunk_s_code_is_replaced_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let doc = file(
            dir.path(),
            "d.Rmd",
            "```{r load}\r\nx <- read()\r\n```\r\n\r\n```{r}\r\n```\r\n",
        );
        let edited = edit_cell(&doc, "load", "x <- read(\"a.csv\")\n").unwrap();
        assert_eq!(edited.cell_id, "load");
        let edited = edit_cell(&doc, "chunk-2", "head(x)").unwrap();
        assert_eq!(edited.cell_id, "chunk-2");
        let text = std::fs::read_to_string(&doc).unwrap();
        assert_eq!(
            text,
            "```{r load}\r\nx <- read(\"a.csv\")\r\n```\r\n\r\n```{r}\r\nhead(x)\r\n```\r\n"
        );
        assert_eq!(
            cell_code(&doc, "chunk-2").unwrap(),
            Some(("r".into(), "head(x)".into()))
        );
    }

    #[test]
    fn notebook_cells_are_added_and_edited() {
        let dir = tempfile::tempdir().unwrap();
        let nb = json!({
            "cells": [
                {"cell_type": "markdown", "id": "m1", "metadata": {}, "source": ["# Title"]},
                {"cell_type": "code", "id": "c1", "metadata": {}, "execution_count": 3,
                 "outputs": [{"output_type": "stream", "name": "stdout", "text": ["1\n"]}],
                 "source": ["print(1)"]},
            ],
            "metadata": {"kernelspec": {"name": "conda-env-omicverse-py", "language": "python", "display_name": "omicverse"}},
            "nbformat": 4,
            "nbformat_minor": 5,
        });
        let doc = file(dir.path(), "e.ipynb", &serde_json::to_string(&nb).unwrap());
        let edited = add_cell(&doc, "x = 1\ny = 2\n", None, Some("m1")).unwrap();
        assert_eq!(edited.language, "python");
        assert_eq!(edited.kernel.as_deref(), Some("conda-env-omicverse-py"));
        let text = std::fs::read_to_string(&doc).unwrap();
        assert!(text.starts_with("{\n \"cells\": [\n  {\n"), "{text}");
        let written: Value = serde_json::from_str(&text).unwrap();
        let ids: Vec<&str> = written["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["m1", edited.cell_id.as_str(), "c1"]);
        assert_eq!(written["cells"][1]["source"], json!(["x = 1\n", "y = 2"]));

        edit_cell(&doc, "c1", "print(2)").unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&doc).unwrap()).unwrap();
        assert_eq!(written["cells"][2]["source"], json!(["print(2)"]));
        assert_eq!(written["cells"][2]["outputs"], json!([]));
        assert_eq!(written["cells"][2]["execution_count"], Value::Null);

        let err = add_cell(&doc, "x <- 1", Some("r"), None).unwrap_err();
        assert!(format!("{err}").contains("python notebook"), "{err}");
    }

    #[test]
    fn old_notebooks_get_cell_ids() {
        let dir = tempfile::tempdir().unwrap();
        let nb = json!({
            "cells": [{"cell_type": "code", "metadata": {}, "execution_count": null, "outputs": [], "source": "1"}],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 2,
        });
        let doc = file(dir.path(), "f.ipynb", &nb.to_string());
        let edited = add_cell(&doc, "2", None, None).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&doc).unwrap()).unwrap();
        assert_eq!(written["nbformat_minor"], 5);
        assert!(written["cells"][0]["id"].is_string());
        assert_eq!(written["cells"][1]["id"], edited.cell_id.as_str());
    }

    #[test]
    fn scripts_get_the_code_at_their_end() {
        let dir = tempfile::tempdir().unwrap();
        let script = file(dir.path(), "g.R", "x <- 1\n");
        let edited = add_cell(&script, "y <- x + 1", None, None).unwrap();
        assert_eq!(edited.cell_id, "");
        assert_eq!(edited.language, "r");
        assert_eq!(
            std::fs::read_to_string(&script).unwrap(),
            "x <- 1\n\ny <- x + 1\n"
        );
        assert!(add_cell(&script, "import os", Some("python"), None).is_err());
        assert!(edit_cell(&script, "x", "1").is_err());
    }
}
