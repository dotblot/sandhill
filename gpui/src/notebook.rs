//! A notebook as the app shows it: its cells, in order, read from a
//! Jupyter `.ipynb` or a Quarto `.qmd`/`.Rmd` document, and written back
//! with the cells' new text. Scripts, standalone Markdown and delimited text
//! use one editor for the whole file and the same save/conflict lifecycle.

use std::ops::Range;

use anyhow::{Context, Result};
use sandhill_daemon::qmd;
use sandhill_proto::KernelspecInfo;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ipynb,
    Quarto,
    /// An `.R` or `.py` file.
    Script,
    /// A standalone Markdown source, rendered without Quarto or a kernel.
    Markdown,
    /// A text file that is edited, never run: shell scripts, JSON, YAML,
    /// TOML, ignore rules and `.txt`. `language` picks its highlighting.
    Text,
    /// An editable CSV/TSV source, not an imported data frame.
    Delimited,
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
    /// The `.ipynb` cell as read or made: its metadata and attachments
    /// are written back as they were.
    json: Value,
    /// Exact Quarto bytes and placement retained by delete/undo.
    removed: Option<RemovedText>,
}

impl Cell {
    pub(crate) fn attachments(&self) -> &Value {
        &self.json["attachments"]
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RemovedText {
    text: String,
    offset: usize,
    next_gap: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Notebook {
    pub path: String,
    pub format: Format,
    /// A bounded prefix, never writable back to the original file.
    pub truncated: bool,
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
    /// Whole-file editor, retaining the notebook's cells and output identity.
    Source(Box<Notebook>),
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

    pub fn is_text_document(&self) -> bool {
        matches!(
            self.format,
            Format::Markdown | Format::Text | Format::Delimited
        ) || self.quarto_source()
    }

    pub fn single_editor(&self) -> bool {
        self.format == Format::Script || self.is_text_document()
    }

    pub(crate) fn quarto_source(&self) -> bool {
        matches!(self.original, Original::Source(_))
    }

    pub(crate) fn source_notebook(&self) -> Option<&Self> {
        match &self.original {
            Original::Source(notebook) => Some(notebook),
            _ => None,
        }
    }

    pub fn can_run(&self) -> bool {
        !self.is_text_document()
    }

    /// The actual language of a cell, including mixed-language Quarto chunks.
    pub(crate) fn cell_language(&self, ix: usize) -> Option<&str> {
        let cell = self.cells.get(ix)?;
        if cell.kind == CellKind::Text {
            return Some("markdown");
        }
        if self.format == Format::Quarto {
            return cell.json["language"].as_str();
        }
        self.language.as_deref()
    }

    pub(crate) fn chunk_label(&self, ix: usize) -> Option<&str> {
        let cell = self.cells.get(ix)?;
        if self.format != Format::Quarto || cell.kind != CellKind::Code {
            return None;
        }
        cell.json["label"].as_str()
    }

    /// The cell and line within it for a 0-based line of the file as read.
    /// Scripts and text documents are one cell; Quarto maps through each
    /// cell's span (fences belong to the next cell); `.ipynb` has no file
    /// lines that correspond to cells, so it opens at the top.
    pub(crate) fn source_location(&self, line: usize) -> Option<(usize, usize)> {
        if self.single_editor() {
            return (!self.cells.is_empty()).then_some((0, line));
        }
        let Original::Text(text) = &self.original else {
            return Some((0, 0));
        };
        let offset = text
            .split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>()
            .min(text.len());
        let cell = self
            .cells
            .iter()
            .position(|cell| offset < cell.span.end || cell.span.start >= offset)
            .unwrap_or(self.cells.len().checked_sub(1)?);
        let span = &self.cells[cell].span;
        let within = offset.clamp(span.start, span.end);
        Some((cell, text[span.start..within].matches('\n').count()))
    }

    /// The document with each cell's text replaced by `sources` (one per
    /// cell, in order): nbformat's layout for an `.ipynb`, the same text
    /// around the chunks for Quarto.
    pub fn render(&self, sources: &[String]) -> Result<String> {
        anyhow::ensure!(!self.truncated, "a truncated preview cannot be saved");
        anyhow::ensure!(sources.len() == self.cells.len(), "one text per cell");
        match &self.original {
            Original::Source(_) => Ok(sources[0].clone()),
            Original::Json(json) => {
                let mut json = json.clone();
                let mut any_new_id = false;
                let cells: Vec<Value> = self
                    .cells
                    .iter()
                    .zip(sources)
                    .map(|(cell, source)| {
                        any_new_id |= cell.new_id;
                        cell_json(cell, source)
                    })
                    .collect();
                json["cells"] = Value::Array(cells);
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
                    if self.format == Format::Quarto
                        && cell.span.is_empty()
                        && cell.kind == CellKind::Code
                        && !source.is_empty()
                        && !text[cell.span.end..].starts_with(['\n', '\r'])
                    {
                        out.push('\n');
                    }
                    at = cell.span.end;
                }
                out.push_str(&text[at..]);
                Ok(out)
            }
        }
    }

    /// Adding and deleting are supported in source notebooks; moves and kind
    /// changes remain Jupyter-only.
    pub fn can_add_cells(&self) -> bool {
        matches!(self.format, Format::Ipynb | Format::Quarto) && !self.quarto_source()
    }

    /// Only Jupyter cells can be moved or changed in kind.
    pub fn can_restructure(&self) -> bool {
        self.format == Format::Ipynb
    }

    pub fn can_delete_cells(&self) -> bool {
        self.can_add_cells()
    }

    /// Commit current cell values into the lossless source and rebase spans.
    fn rebase_quarto(&mut self, sources: &[String]) -> Result<()> {
        let text = self.render(sources)?;
        let Original::Text(original) = &self.original else {
            anyhow::bail!("a Quarto notebook view is required");
        };
        let mut delta = 0isize;
        for (cell, source) in self.cells.iter_mut().zip(sources) {
            let start = cell
                .span
                .start
                .checked_add_signed(delta)
                .context("cell offset")?;
            let extra = cell.kind == CellKind::Code
                && cell.span.is_empty()
                && !source.is_empty()
                && !original[cell.span.end..].starts_with(['\n', '\r']);
            delta += source.len() as isize - cell.span.len() as isize + isize::from(extra);
            cell.span = start..start + source.len();
            cell.source.clone_from(source);
        }
        self.original = Original::Text(text);
        Ok(())
    }

    fn refresh_quarto_ids(&mut self) -> Result<()> {
        let Original::Text(text) = &self.original else {
            anyhow::bail!("a Quarto notebook view is required");
        };
        let chunks = qmd::parse_chunks(text);
        self.language = chunks.first().map(|chunk| chunk.language.clone());
        let mut chunks = chunks.into_iter();
        for cell in &mut self.cells {
            cell.id = if cell.kind == CellKind::Code {
                chunks
                    .next()
                    .context("the chunk could not be found")?
                    .cell_id
            } else {
                format!("text-{}", cell.span.start)
            };
        }
        anyhow::ensure!(
            chunks.next().is_none(),
            "the document's chunk fences changed"
        );
        Ok(())
    }

    pub(crate) fn remove_quarto_cell(&mut self, ix: usize, sources: &[String]) -> Result<Cell> {
        self.rebase_quarto(sources)?;
        let cell = self.cells.get(ix).context("the cell could not be found")?;
        let Original::Text(text) = &self.original else {
            unreachable!()
        };
        let mut range = if cell.kind == CellKind::Code {
            let chunk = qmd::parse_chunks(text)
                .into_iter()
                .find(|chunk| chunk.code_from == cell.span.start)
                .context("the chunk's fences could not be found")?;
            chunk.from..chunk.to
        } else {
            cell.span.clone()
        };
        // Trimmed prose still owns indentation/trailing spaces on its lines.
        let line_start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
        if text[line_start..range.start].trim().is_empty() {
            range.start = line_start;
        }
        let tail = &text[range.end..];
        if let Some(n) = tail.find('\n').filter(|n| tail[..*n].trim().is_empty()) {
            range.end += n + 1;
        } else if tail.trim().is_empty() {
            range.end = text.len();
        }
        // Include one adjacent blank-line separator, preferring the next.
        let tail = &text[range.end..];
        if let Some(n) = tail.find('\n').filter(|n| tail[..*n].trim().is_empty()) {
            range.end += n + 1;
        } else if range.start > 0 {
            let prefix = &text[..range.start];
            let before = prefix
                .strip_suffix("\r\n")
                .or_else(|| prefix.strip_suffix('\n'));
            if let Some(before) = before {
                let from = before.rfind('\n').map_or(0, |n| n + 1);
                if before[from..].trim().is_empty() {
                    range.start = from;
                }
            }
        }
        // Empty prose cells inserted between chunks still own their position.
        if let Some(previous) = ix.checked_sub(1).and_then(|ix| self.cells.get(ix)) {
            range.start = range.start.max(previous.span.end);
        }
        if let Some(next) = self.cells.get(ix + 1) {
            range.end = range.end.min(next.span.start);
        }
        let removed = RemovedText {
            text: text[range.clone()].to_string(),
            offset: text.len() - range.end,
            next_gap: self
                .cells
                .get(ix + 1)
                .map(|next| next.span.start - range.end),
        };
        let mut cell = self.cells.remove(ix);
        cell.span = cell.span.start - range.start..cell.span.end - range.start;
        cell.removed = Some(removed);
        let len = range.len();
        for cell in &mut self.cells[ix..] {
            cell.span.start -= len;
            cell.span.end -= len;
        }
        let Original::Text(text) = &mut self.original else {
            unreachable!()
        };
        text.replace_range(range, "");
        self.refresh_quarto_ids()?;
        Ok(cell)
    }

    pub(crate) fn restore_quarto_cell(
        &mut self,
        at: usize,
        mut cell: Cell,
        sources: &[String],
    ) -> Result<()> {
        self.rebase_quarto(sources)?;
        let removed = cell
            .removed
            .take()
            .context("the deleted source could not be found")?;
        let at = at.min(self.cells.len());
        let Original::Text(text) = &mut self.original else {
            unreachable!()
        };
        let offset = match (self.cells.get(at), removed.next_gap) {
            (Some(next), Some(gap)) => {
                next.span.start.checked_sub(gap).context("restore offset")?
            }
            _ => text
                .len()
                .checked_sub(removed.offset)
                .context("restore offset")?,
        };
        text.insert_str(offset, &removed.text);
        for cell in &mut self.cells[at..] {
            cell.span.start += removed.text.len();
            cell.span.end += removed.text.len();
        }
        cell.span.start += offset;
        cell.span.end += offset;
        self.cells.insert(at, cell);
        self.refresh_quarto_ids()
    }

    pub(crate) fn as_quarto_source(&self, sources: &[String]) -> Result<Self> {
        let mut notebook = self.clone();
        notebook.rebase_quarto(sources)?;
        let text = notebook.render(sources)?;
        let mut source = parse_text(&self.path, &text, Format::Quarto);
        source.cells[0].kind = CellKind::Text;
        source.language.clone_from(&self.language);
        source.kernel.clone_from(&self.kernel);
        source.original = Original::Source(Box::new(notebook));
        Ok(source)
    }

    /// Return to cells, matching unchanged code/labels before positional edits.
    pub(crate) fn as_quarto_notebook(&self, text: &str) -> Result<(Self, Vec<(usize, usize)>)> {
        let Original::Source(previous) = &self.original else {
            anyhow::bail!("a Quarto source view is required");
        };
        let values: Vec<_> = previous
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        if previous.render(&values)? == text {
            return Ok((
                (**previous).clone(),
                (0..previous.cells.len()).map(|ix| (ix, ix)).collect(),
            ));
        }
        let mut notebook = parse_quarto(&self.path, text);
        let mut matched = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (new, cell) in notebook.cells.iter_mut().enumerate() {
            let old = previous
                .cells
                .iter()
                .enumerate()
                .find(|(ix, old)| {
                    !used.contains(ix)
                        && old.kind == cell.kind
                        && ((old.json["label"].is_string()
                            && old.json["label"] == cell.json["label"])
                            || old.source == cell.source)
                })
                .map(|(ix, _)| ix);
            if let Some(old) = old {
                used.insert(old);
                cell.outputs.clone_from(&previous.cells[old].outputs);
                cell.execution_count = previous.cells[old].execution_count;
                matched.push((old, new));
            }
        }
        if previous.cells.len() == notebook.cells.len() {
            for ix in 0..notebook.cells.len() {
                if !used.contains(&ix)
                    && !matched.iter().any(|(_, new)| *new == ix)
                    && previous.cells[ix].kind == notebook.cells[ix].kind
                {
                    notebook.cells[ix]
                        .outputs
                        .clone_from(&previous.cells[ix].outputs);
                    notebook.cells[ix].execution_count = previous.cells[ix].execution_count;
                    matched.push((ix, ix));
                }
            }
        }
        Ok((notebook, matched))
    }

    pub(crate) fn cell_source_line(&self, ix: usize) -> usize {
        let Original::Text(text) = &self.original else {
            return 0;
        };
        self.cells
            .get(ix)
            .map_or(0, |cell| text[..cell.span.start].matches('\n').count())
    }

    /// Adds an empty cell at `at`, and returns its id.
    pub fn insert_cell(&mut self, at: usize, kind: CellKind) -> String {
        let id = self.fresh_id();
        let mut json = serde_json::json!({"metadata": {}});
        set_cell_type(&mut json, kind);
        let at = at.min(self.cells.len());
        self.cells.insert(
            at,
            Cell {
                id: id.clone(),
                kind,
                source: String::new(),
                outputs: Vec::new(),
                execution_count: None,
                span: 0..0,
                new_id: true,
                json,
                removed: None,
            },
        );
        id
    }

    /// Inserts into Quarto's original text, keeping every surrounding byte.
    /// Existing editor values are rebased first, without replacing their editors.
    /// Adding above YAML splits that prose cell after its header; returns the
    /// actual inserted index and whether the header gained a trailing prose cell.
    pub(crate) fn insert_quarto_cell(
        &mut self,
        at: usize,
        kind: CellKind,
        language: Option<&str>,
        sources: &[String],
    ) -> Result<(usize, bool)> {
        anyhow::ensure!(
            self.format == Format::Quarto,
            "a Quarto document is required"
        );
        let text = self.render(sources)?;
        let language = sandhill_daemon::cells::normalize_language(
            language.or(self.language.as_deref()).unwrap_or("python"),
        );
        anyhow::ensure!(
            kind == CellKind::Text || matches!(language.as_str(), "r" | "python"),
            "cells can be r or python, not {language}"
        );
        let chunks = qmd::parse_chunks(&text);
        anyhow::ensure!(
            chunks.len()
                == self
                    .cells
                    .iter()
                    .filter(|cell| cell.kind == CellKind::Code)
                    .count(),
            "the document's chunk fences changed; save and reopen before adding a cell"
        );
        if chunks.last().is_some_and(|chunk| !chunk.closed) {
            let last = self
                .cells
                .iter()
                .rposition(|cell| cell.kind == CellKind::Code)
                .context("the unclosed chunk could not be found")?;
            anyhow::ensure!(
                at <= last,
                "close the last chunk's fence before adding below it"
            );
        }
        let mut delta = 0isize;
        let Original::Text(original) = &self.original else {
            unreachable!("Quarto has original text");
        };
        for (cell, source) in self.cells.iter_mut().zip(sources) {
            let start = cell
                .span
                .start
                .checked_add_signed(delta)
                .context("cell offset")?;
            let extra_newline = cell.kind == CellKind::Code
                && cell.span.is_empty()
                && !source.is_empty()
                && !original[cell.span.end..].starts_with(['\n', '\r']);
            delta += source.len() as isize - cell.span.len() as isize + isize::from(extra_newline);
            cell.span = start..start + source.len();
            cell.source.clone_from(source);
        }
        let mut at = at.min(self.cells.len());
        let mut split = false;
        let mut after_header = None;
        if at == 0
            && let Some(end) = front_matter_end(&text)
            && let Some(first) = self.cells.first_mut()
            && first.kind == CellKind::Text
            && first.span.start == 0
            && first.span.end >= text[..end].trim_end_matches(['\n', '\r']).len()
        {
            let tail_end = first.span.end;
            let header_end = text[..end].trim_end_matches(['\n', '\r']).len();
            first.span.end = header_end;
            first.source = text[..header_end].to_string();
            let tail = &text[end.min(tail_end)..tail_end];
            if !tail.trim().is_empty() {
                let start = end + tail.len() - tail.trim_start().len();
                let mut cell = first.clone();
                cell.id = self.fresh_id();
                cell.span = start..tail_end;
                cell.source = text[start..tail_end].to_string();
                self.cells.insert(1, cell);
                split = true;
            }
            at = 1;
            after_header = Some(end);
        }
        let offset = if let Some(end) = after_header {
            end
        } else {
            match self.cells.get(at) {
                Some(cell) if cell.kind == CellKind::Code => chunks
                    .iter()
                    .find(|chunk| chunk.code_from == cell.span.start)
                    .map(|chunk| chunk.from)
                    .context("the next chunk could not be found")?,
                Some(cell) => cell.span.start,
                None => text.len(),
            }
        };
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let (body, source_offset) = if kind == CellKind::Code {
            // Use the daemon's fence, language and newline rules on an empty
            // document; placement here also supports prose and "above".
            let (chunk, _) = sandhill_daemon::cells::add_cell_to_text(
                std::path::Path::new(&self.path),
                "",
                "",
                Some(&language),
                None,
                &self.path,
            )?;
            let chunk = chunk.replace('\n', nl);
            let source_offset = chunk.find(nl).context("chunk opening fence")? + nl.len();
            (
                chunk.trim_end_matches(['\n', '\r']).to_string(),
                source_offset,
            )
        } else {
            (String::new(), 0)
        };
        // One blank line before the block, none doubled; a block added at the
        // end leaves the file ending in a single newline.
        let mut trailing = 0;
        let mut head = &text[..offset];
        while trailing < 2
            && let Some(rest) = head.strip_suffix(nl)
        {
            trailing += 1;
            head = rest;
        }
        let prefix = if offset == 0 {
            String::new()
        } else {
            nl.repeat(2 - trailing)
        };
        let suffix = if offset == text.len() {
            nl.to_string()
        } else {
            let mut leading = 0;
            let mut tail = &text[offset..];
            while leading < 2
                && let Some(rest) = tail.strip_prefix(nl)
            {
                leading += 1;
                tail = rest;
            }
            nl.repeat(2 - leading)
        };
        let insert = format!("{prefix}{body}{suffix}");
        let start = offset + prefix.len() + source_offset;
        for cell in &mut self.cells[at..] {
            cell.span.start += insert.len();
            cell.span.end += insert.len();
        }
        let mut text = text;
        text.insert_str(offset, &insert);
        let id = self.fresh_id();
        self.cells.insert(
            at,
            Cell {
                id,
                kind,
                source: String::new(),
                outputs: Vec::new(),
                execution_count: None,
                span: start..start,
                new_id: false,
                removed: None,
                json: if kind == CellKind::Code {
                    serde_json::json!({"language": language, "label": null})
                } else {
                    Value::Null
                },
            },
        );
        // Positional ids must agree with the daemon after an earlier insertion.
        let chunks = qmd::parse_chunks(&text);
        self.language = chunks.first().map(|chunk| chunk.language.clone());
        let mut chunks = chunks.into_iter();
        for cell in &mut self.cells {
            if cell.kind == CellKind::Code {
                cell.id = chunks
                    .next()
                    .context("the inserted chunk could not be found")?
                    .cell_id;
            }
        }
        self.original = Original::Text(text);
        Ok((at, split))
    }

    /// Puts back a cell `remove_cell` took out.
    pub fn restore_cell(&mut self, at: usize, cell: Cell) {
        let at = at.min(self.cells.len());
        self.cells.insert(at, cell);
    }

    pub fn remove_cell(&mut self, ix: usize) -> Option<Cell> {
        (ix < self.cells.len()).then(|| self.cells.remove(ix))
    }

    /// Moves the cell at `from` so it ends up at `to`.
    pub fn move_cell(&mut self, from: usize, to: usize) {
        if from < self.cells.len() && to < self.cells.len() && from != to {
            let cell = self.cells.remove(from);
            self.cells.insert(to, cell);
        }
    }

    /// Makes a cell code or Markdown; a cell made Markdown loses its
    /// outputs.
    pub fn set_kind(&mut self, ix: usize, kind: CellKind) {
        if let Some(cell) = self.cells.get_mut(ix)
            && cell.kind != kind
        {
            cell.kind = kind;
            cell.outputs.clear();
            cell.execution_count = None;
            set_cell_type(&mut cell.json, kind);
        }
    }

    /// Forgets the outputs the file holds for a cell.
    pub fn clear_outputs(&mut self, ix: usize) {
        if let Some(cell) = self.cells.get_mut(ix) {
            cell.outputs.clear();
        }
    }

    /// An id no cell has, in the style Jupyter makes them.
    fn fresh_id(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let mut n = nanos ^ ((self.cells.len() as u64) << 48);
        loop {
            n = n.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
            let id = format!("{:08x}", n >> 32);
            if !self.cells.iter().any(|c| c.id == id) {
                return id;
            }
        }
    }
}

/// The extension of a path's file name, lowercased: `gitignore` for
/// `.gitignore`.
fn extension(path: &str) -> Option<String> {
    Some(
        path.rsplit('/')
            .next()?
            .rsplit_once('.')?
            .1
            .to_ascii_lowercase(),
    )
}

pub fn format_of(path: &str) -> Option<Format> {
    if is_binary(path) {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name.starts_with("readme") || matches!(name.as_str(), "agents.md" | "claude.md") {
        return Some(Format::Markdown);
    }
    if matches!(
        name.as_str(),
        "snakefile" | "license" | "copying" | "cargo.lock" | "uv.lock" | "renv.lock"
    ) {
        return Some(Format::Text);
    }
    match extension(path).as_deref() {
        Some("ipynb") => Some(Format::Ipynb),
        Some("qmd" | "rmd") => Some(Format::Quarto),
        Some("r" | "py") => Some(Format::Script),
        Some("md" | "markdown" | "mdown" | "mkd") => Some(Format::Markdown),
        Some("csv" | "tsv") => Some(Format::Delimited),
        Some(
            "sh" | "bash" | "zsh" | "json" | "jsonc" | "yaml" | "yml" | "toml" | "txt"
            | "gitignore" | "gitattributes" | "smk" | "cwl" | "ini" | "cfg" | "config" | "lock"
            | "fa" | "fasta" | "fna" | "faa" | "fq" | "fastq" | "vcf" | "bed" | "bedgraph"
            | "narrowpeak" | "gff" | "gff3" | "gtf" | "sam" | "mtx" | "nf" | "wdl" | "bib",
        ) => Some(Format::Text),
        _ => None,
    }
}

pub(crate) fn is_binary(path: &str) -> bool {
    matches!(
        extension(path).as_deref(),
        Some(
            "bam"
                | "cram"
                | "bcf"
                | "h5"
                | "h5ad"
                | "h5seurat"
                | "hdf5"
                | "loom"
                | "parquet"
                | "rds"
                | "rda"
                | "rdata"
                | "zip"
                | "tar"
                | "tgz"
                | "gz"
                | "bgz"
                | "bz2"
                | "xz"
                | "zst"
        )
    )
}

/// Unknown files are candidates too: their daemon-read prefix decides.
pub(crate) fn can_open(path: &str) -> bool {
    !is_binary(path)
}

/// Decoration-only languages keep their identity for per-type wrapping.
fn text_language(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    match name.as_str() {
        "snakefile" => return "python",
        "cargo.lock" | "uv.lock" => return "toml",
        "renv.lock" => return "json",
        _ => {}
    }
    match extension(path).as_deref() {
        Some("json" | "jsonc") => "json",
        Some("yaml" | "yml" | "cwl") => "yaml",
        Some("toml" | "ini" | "cfg" | "config") => "toml",
        Some("smk") => "python",
        Some("sh" | "bash" | "zsh" | "gitignore" | "gitattributes") => "bash",
        Some("fa" | "fasta" | "fna" | "faa") => "fasta",
        Some("fq" | "fastq") => "fastq",
        Some(
            "vcf" | "bed" | "bedgraph" | "narrowpeak" | "gff" | "gff3" | "gtf" | "sam" | "mtx",
        ) => "bio-table",
        Some("nf") => "nextflow",
        Some("wdl") => "wdl",
        Some("bib") => "bibtex",
        _ => "text",
    }
}

pub fn parse(path: &str, text: &str) -> Result<Notebook> {
    anyhow::ensure!(!is_binary(path), "binary files open with the default app");
    let format = format_of(path).unwrap_or(Format::Text);
    parse_as(path, text, format)
}

pub(crate) fn parse_as(path: &str, text: &str, format: Format) -> Result<Notebook> {
    match format {
        Format::Ipynb => parse_ipynb(path, text),
        Format::Quarto => Ok(parse_quarto(path, text)),
        Format::Script => Ok(parse_script(path, text)),
        Format::Markdown | Format::Text | Format::Delimited => Ok(parse_text(path, text, format)),
    }
}

/// Recognises interpreters, not arbitrary commands containing a shell name.
pub fn shell_shebang(text: &str) -> bool {
    let Some(line) = text.lines().next().and_then(|line| line.strip_prefix("#!")) else {
        return false;
    };
    let mut words = line.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    let interpreter = if name == "env" {
        let mut candidate = words.next();
        if candidate == Some("-S") {
            candidate = words.next();
        }
        candidate
    } else {
        Some(name)
    };
    matches!(interpreter, Some("sh" | "bash" | "zsh" | "dash" | "ksh"))
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
            json: cell.clone(),
            removed: None,
        });
    }
    Ok(Notebook {
        path: path.to_string(),
        truncated: false,
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

/// The byte after a complete YAML header's closing line, including its newline.
fn front_matter_end(text: &str) -> Option<usize> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\n', '\r']) != "---" {
        return None;
    }
    let mut end = first.len();
    for line in lines {
        end += line.len();
        if matches!(line.trim_end_matches(['\n', '\r']), "---" | "...") {
            return Some(end);
        }
    }
    None
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
                json: Value::Null,
                removed: None,
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
            json: serde_json::json!({"language": chunk.language, "label": chunk.label}),
            removed: None,
        });
        at = chunk.to;
    }
    prose(at.min(text.len()), text.len(), &mut cells);
    Notebook {
        path: path.to_string(),
        truncated: false,
        format: Format::Quarto,
        cells,
        language: chunks.first().map(|c| c.language.clone()),
        kernel: qmd::front_matter_kernel(text),
        original: Original::Text(text.to_string()),
    }
}

/// The cell id a script's one cell has. (Its runs are journaled under
/// `console-<n>`, as the Tauri app does.)
pub const SCRIPT_CELL: &str = "script";

fn parse_script(path: &str, text: &str) -> Notebook {
    let language = match path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()) {
        Some(ext) if ext == "py" => "python",
        _ => "r",
    };
    Notebook {
        path: path.to_string(),
        truncated: false,
        format: Format::Script,
        cells: vec![Cell {
            id: SCRIPT_CELL.into(),
            kind: CellKind::Code,
            source: text.to_string(),
            outputs: Vec::new(),
            execution_count: None,
            span: 0..text.len(),
            new_id: false,
            json: Value::Null,
            removed: None,
        }],
        language: Some(language.into()),
        kernel: None,
        original: Original::Text(text.to_string()),
    }
}

fn parse_text(path: &str, text: &str, format: Format) -> Notebook {
    let mut nb = parse_script(path, text);
    nb.format = format;
    nb.language = match format {
        Format::Markdown => Some("markdown".into()),
        Format::Text => Some(
            if format_of(path).is_none() && shell_shebang(text) {
                "bash"
            } else {
                text_language(path)
            }
            .into(),
        ),
        _ => None,
    };
    nb
}

/// The kernel to run a notebook in: the one it names if installed, else the
/// first for its language, else the first there is.
pub fn pick_kernel(notebook: &Notebook, specs: &[KernelspecInfo]) -> Option<String> {
    if !notebook.can_run() {
        return None;
    }
    if let Some(name) = &notebook.kernel
        && specs.iter().any(|s| &s.name == name)
        && (notebook.format != Format::Script
            || specs.iter().any(|s| {
                &s.name == name
                    && notebook
                        .language
                        .as_deref()
                        .is_some_and(|l| s.language.eq_ignore_ascii_case(l))
            }))
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
    // A script runs only in a kernel for its language.
    if notebook.format == Format::Script {
        return None;
    }
    specs.first().map(|s| s.name.clone())
}

/// Reopening never replaces a remembered choice just because it is missing.
/// An already-running session takes precedence; otherwise project defaults
/// apply only to documents which have no personal kernel history.
pub fn recover_kernel(
    notebook: &Notebook,
    specs: &[KernelspecInfo],
    live: Option<&str>,
    remembered: Option<&str>,
    project: Option<&str>,
) -> Option<String> {
    if !notebook.can_run() {
        return None;
    }
    live.or(remembered)
        .or(project)
        .map(str::to_string)
        .or_else(|| pick_kernel(notebook, specs))
}

/// A cell as nbformat writes it, with its text as edited. Run counts are
/// not kept: they say only the order cells last ran in, and change the
/// file each time it runs.
fn cell_json(cell: &Cell, source: &str) -> Value {
    let mut json = match &cell.json {
        Value::Object(_) => cell.json.clone(),
        _ => serde_json::json!({"metadata": {}}),
    };
    if json["cell_type"].is_null() {
        set_cell_type(&mut json, cell.kind);
    }
    json["source"] = lines(source);
    if cell.new_id {
        json["id"] = Value::String(cell.id.clone());
    }
    if json["cell_type"] == "code" {
        let mut outputs = cell.outputs.clone();
        for output in &mut outputs {
            if output["output_type"] == "execute_result" {
                output["execution_count"] = Value::Null;
            }
        }
        json["outputs"] = Value::Array(outputs);
        json["execution_count"] = Value::Null;
    }
    json
}

/// Gives a cell's JSON the fields its kind has in nbformat.
fn set_cell_type(json: &mut Value, kind: CellKind) {
    let Some(map) = json.as_object_mut() else {
        return;
    };
    match kind {
        CellKind::Code => {
            map.insert("cell_type".into(), "code".into());
            map.insert("outputs".into(), Value::Array(Vec::new()));
            map.insert("execution_count".into(), Value::Null);
        }
        CellKind::Text => {
            map.insert("cell_type".into(), "markdown".into());
            map.remove("outputs");
            map.remove("execution_count");
        }
    }
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
    use super::{
        CellKind, Format, Notebook, format_of, is_binary, parse, parse_as, pick_kernel,
        recover_kernel, shell_shebang,
    };
    use sandhill_proto::KernelspecInfo;
    use serde_json::Value;

    #[test]
    fn mixed_quarto_languages_and_labels_use_the_chunk_parser() {
        let nb = parse(
            "mixed.qmd",
            "# Report\n\n```{r setup}\nx <- 1\n```\n\n```{python}\n#| label: model\nx = 1\n```\n",
        )
        .unwrap();
        assert_eq!(
            nb.cells
                .iter()
                .enumerate()
                .map(|(ix, _)| nb.cell_language(ix))
                .collect::<Vec<_>>(),
            vec![Some("markdown"), Some("r"), Some("python")]
        );
        assert_eq!(nb.chunk_label(1), Some("setup"));
        assert_eq!(nb.chunk_label(2), Some("model"));
    }

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

    #[test]
    fn quarto_delete_restore_preserves_exact_bytes() {
        for extension in ["qmd", "Rmd"] {
            for nl in ["\n", "\r\n"] {
                let header = "---\ntitle: 'QC  '\njupyter: ir\n---\n\n";
                let first = "```{r first, echo=FALSE}\n#| warning: false\nx <- 1  \n```\n\n";
                let prose = "Between **chunks**.  \n\n";
                let last = "~~~{python}\n#| echo: false\nprint(2)\n~~~\n";
                let original = format!("{header}{first}{prose}{last}").replace('\n', nl);
                let path = format!("/p/report.{extension}");
                for (ix, removed) in [(1, first), (2, prose), (3, last)] {
                    let mut nb = parse(&path, &original).unwrap();
                    assert!(nb.can_delete_cells());
                    assert!(!nb.can_restructure());
                    let cell = nb.remove_quarto_cell(ix, &sources(&nb)).unwrap();
                    let expected = if ix == 3 {
                        original.replace(&format!("{nl}{}", removed.replace('\n', nl)), "")
                    } else {
                        original.replacen(&removed.replace('\n', nl), "", 1)
                    };
                    assert_eq!(nb.render(&sources(&nb)).unwrap(), expected);
                    assert_eq!(
                        nb.cells
                            .iter()
                            .filter(|cell| cell.kind == CellKind::Code)
                            .map(|cell| cell.id.clone())
                            .collect::<Vec<_>>(),
                        sandhill_daemon::qmd::parse_chunks(&expected)
                            .into_iter()
                            .map(|chunk| chunk.cell_id)
                            .collect::<Vec<_>>(),
                    );
                    nb.restore_quarto_cell(ix, cell, &sources(&nb)).unwrap();
                    assert_eq!(nb.render(&sources(&nb)).unwrap(), original);
                }
            }
        }
    }

    #[test]
    fn quarto_delete_restore_keeps_unsaved_edits_and_multiple_deletions() {
        let original = "---\ntitle: QC\n---\n\n```{r}\nx <- 1\n```\n\n```{r}\nx <- 2\n```\n";
        let mut nb = parse("a.Rmd", original).unwrap();
        let mut text = sources(&nb);
        text[2] = "x <- 42".into();
        let first = nb.remove_quarto_cell(1, &text).unwrap();
        assert_eq!(nb.cells[1].id, "chunk-1");
        let second = nb.remove_quarto_cell(1, &sources(&nb)).unwrap();
        nb.restore_quarto_cell(1, second, &sources(&nb)).unwrap();
        let mut text = sources(&nb);
        text[1] = "x <- 100".into();
        nb.restore_quarto_cell(1, first, &text).unwrap();
        assert_eq!(
            nb.render(&sources(&nb)).unwrap(),
            original.replace("x <- 2", "x <- 100")
        );
        assert_eq!(nb.cells[2].id, "chunk-2");
    }

    #[test]
    fn quarto_source_notebook_round_trip_is_lossless() {
        for extension in ["qmd", "Rmd"] {
            for nl in ["\n", "\r\n"] {
                for text in [
                    "",
                    "  Just prose.  \n\n",
                    "---\ntitle: QC\n---\n\n```{r, echo=FALSE}\n#| label: setup\nx <- 1  \n```\n\n~~~{python}\nprint('λ')\n~~~",
                ] {
                    let original = text.replace('\n', nl);
                    let path = format!("a.{extension}");
                    let nb = parse(&path, &original).unwrap();
                    let source = nb.as_quarto_source(&sources(&nb)).unwrap();
                    assert!(source.single_editor());
                    assert!(!source.can_run());
                    assert_eq!(source.render(&sources(&source)).unwrap(), original);
                    let (restored, _) = source.as_quarto_notebook(&original).unwrap();
                    assert_eq!(restored, nb);
                    assert_eq!(restored.render(&sources(&restored)).unwrap(), original);
                    let edited = format!("{original}{nl}New prose.  {nl}");
                    let (restored, _) = source.as_quarto_notebook(&edited).unwrap();
                    assert_eq!(restored.render(&sources(&restored)).unwrap(), edited);
                }
            }
        }
    }

    #[test]
    fn quarto_view_round_trip_keeps_outputs_and_edited_cell_identity() {
        let mut nb = parse("a.qmd", "```{r}\nx <- 1\n```\n\n```{r}\nx <- 2\n```\n").unwrap();
        nb.cells[1]
            .outputs
            .push(serde_json::json!({"output_type": "stream", "text": "kept"}));
        let mut values = sources(&nb);
        values[1] = "x <- 42".into();
        let source = nb.as_quarto_source(&values).unwrap();
        let original = source.render(&sources(&source)).unwrap();
        let edited = format!("```{{r}}\nx <- 0\n```\n\n{original}");
        let (restored, matched) = source.as_quarto_notebook(&edited).unwrap();
        assert!(matched.contains(&(1, 2)));
        assert_eq!(restored.cells[2].outputs, nb.cells[1].outputs);
        assert_eq!(restored.render(&sources(&restored)).unwrap(), edited);
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

    #[test]
    fn does_not_save_run_counts() {
        let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
        let mut json = nb.cells[1].json.clone();
        json["outputs"][0] = serde_json::json!({
            "output_type": "execute_result", "execution_count": 3,
            "data": {"text/plain": "2"}, "metadata": {}
        });
        let mut nb = nb;
        nb.cells[1].outputs = json["outputs"].as_array().unwrap().clone();
        let out: Value = serde_json::from_str(&nb.render(&sources(&nb)).unwrap()).unwrap();
        assert_eq!(out["cells"][1]["execution_count"], Value::Null);
        assert_eq!(
            out["cells"][1]["outputs"][0]["execution_count"],
            Value::Null
        );
        assert_eq!(out["cells"][1]["outputs"][0]["data"]["text/plain"], "2");
    }

    #[test]
    fn adds_moves_changes_and_removes_ipynb_cells() {
        let mut nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
        assert!(nb.can_restructure());
        let added = nb.insert_cell(1, CellKind::Code);
        assert_eq!(nb.cells[1].id, added);
        assert_eq!(added.len(), 8);
        nb.move_cell(1, 3);
        assert_eq!(nb.cells[3].id, added);
        nb.set_kind(3, CellKind::Text);
        nb.set_kind(1, CellKind::Text);
        assert!(nb.cells[1].outputs.is_empty());
        nb.remove_cell(0);
        let mut texts = sources(&nb);
        texts[2] = "## Notes".into();
        let out: Value = serde_json::from_str(&nb.render(&texts).unwrap()).unwrap();
        let cells = out["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 3);
        assert_eq!(cells[0]["cell_type"], "markdown");
        assert_eq!(cells[0]["id"], "cell-2");
        assert!(cells[0].get("outputs").is_none());
        assert!(cells[0].get("execution_count").is_none());
        assert_eq!(cells[1]["cell_type"], "code");
        assert_eq!(cells[2]["id"], added.as_str());
        assert_eq!(cells[2]["cell_type"], "markdown");
        assert_eq!(cells[2]["source"], serde_json::json!(["## Notes"]));
        // Read back, the cells are the same.
        let again = parse("/p/analysis.ipynb", &serde_json::to_string(&out).unwrap()).unwrap();
        assert_eq!(sources(&again), texts);
        // A Quarto document's cells stay where they are.
        assert!(!parse("/p/qc.qmd", QMD).unwrap().can_restructure());
    }

    #[test]
    fn clears_a_cells_saved_outputs() {
        let mut nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
        nb.clear_outputs(1);
        let out: Value = serde_json::from_str(&nb.render(&sources(&nb)).unwrap()).unwrap();
        assert_eq!(out["cells"][1]["outputs"], serde_json::json!([]));
    }

    #[test]
    fn quarto_insertions_preserve_source_bytes_and_yaml() {
        for extension in ["qmd", "Rmd"] {
            for nl in ["\n", "\r\n"] {
                let header = "---\ntitle: \"QC  \"\njupyter: python3\n---\n".replace('\n', nl);
                let original = format!(
                    "{header}{nl}Intro  é.{nl}{nl}```{{r setup, echo=FALSE}}{nl}#| warning: false{nl}x <- 1  {nl}```{nl}{nl}Tail  {nl}"
                );
                let offsets = [
                    header.len(),
                    original.find("```").unwrap(),
                    original.find("Tail").unwrap(),
                    original.len(),
                ];
                for (requested, offset) in offsets.into_iter().enumerate() {
                    for kind in [CellKind::Code, CellKind::Text] {
                        let mut nb = parse(&format!("/p/qc.{extension}"), &original).unwrap();
                        assert!(nb.can_add_cells());
                        assert!(!nb.can_restructure());
                        let before = sources(&nb);
                        let (at, split) = nb
                            .insert_quarto_cell(requested, kind, Some("python"), &before)
                            .unwrap();
                        assert_eq!(split, requested == 0);
                        let mut texts = sources(&nb);
                        texts[at] = if kind == CellKind::Code {
                            "print(2)".into()
                        } else {
                            "**New text.**".into()
                        };
                        let block = if kind == CellKind::Code {
                            format!("```{{python}}{nl}print(2){nl}```")
                        } else {
                            "**New text.**".to_string()
                        };
                        // Exactly one blank line on each side; the file still
                        // ends in one newline when the block goes last.
                        let head = original[..offset].trim_end_matches(nl);
                        let tail = original[offset..].trim_start_matches(nl);
                        let expected = if tail.is_empty() {
                            format!("{head}{nl}{nl}{block}{nl}")
                        } else {
                            format!("{head}{nl}{nl}{block}{nl}{nl}{tail}")
                        };
                        assert_eq!(nb.render(&texts).unwrap(), expected);
                        assert_eq!(
                            nb.cells
                                .iter()
                                .filter(|cell| cell.kind == CellKind::Code)
                                .map(|cell| cell.id.as_str())
                                .collect::<Vec<_>>(),
                            sandhill_daemon::qmd::parse_chunks(&expected)
                                .iter()
                                .map(|chunk| chunk.cell_id.as_str())
                                .collect::<Vec<_>>(),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn quarto_insertions_choose_language_and_rebase_unsaved_edits() {
        for extension in ["qmd", "Rmd"] {
            for (original, selected, expected) in [
                ("", None, "python"),
                ("Just prose.", None, "python"),
                ("```{r}\nx <- 1\n```\n", None, "r"),
                ("```{python}\nx = 1\n```\n", Some("R"), "r"),
                ("```{r}\nx <- 1\n```\n", Some("python3"), "python"),
            ] {
                let mut nb = parse(&format!("/p/qc.{extension}"), original).unwrap();
                let (at, _) = nb
                    .insert_quarto_cell(nb.cells.len(), CellKind::Code, selected, &sources(&nb))
                    .unwrap();
                assert_eq!(nb.cell_language(at), Some(expected));
                let mut texts = sources(&nb);
                texts[at] = "value = 42".into();
                let before_second = nb.render(&texts).unwrap();
                let (at, _) = nb
                    .insert_quarto_cell(nb.cells.len(), CellKind::Text, None, &texts)
                    .unwrap();
                let mut texts = sources(&nb);
                texts[at] = "Notes".into();
                assert_eq!(
                    nb.render(&texts).unwrap(),
                    if before_second.is_empty() {
                        "Notes\n".to_string()
                    } else {
                        format!("{}\n\nNotes\n", before_second.trim_end_matches('\n'))
                    },
                );
            }
            let original = "---\ntitle: QC\n---\n";
            let mut nb = parse(&format!("/p/qc.{extension}"), original).unwrap();
            let (at, split) = nb
                .insert_quarto_cell(0, CellKind::Code, None, &sources(&nb))
                .unwrap();
            assert_eq!((at, split), (1, false));
            assert!(nb.render(&sources(&nb)).unwrap().starts_with(original));
        }
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
    fn recovery_prefers_live_then_document_history_then_project() {
        let specs = [spec("python3", "python"), spec("ark", "R")];
        let nb = parse("/p/qc.py", "").unwrap();
        assert_eq!(
            recover_kernel(
                &nb,
                &specs,
                Some("live"),
                Some("remembered"),
                Some("project")
            )
            .as_deref(),
            Some("live")
        );
        assert_eq!(
            recover_kernel(
                &nb,
                &specs,
                None,
                Some("missing-remembered"),
                Some("python3")
            )
            .as_deref(),
            Some("missing-remembered")
        );
        assert_eq!(
            recover_kernel(&nb, &specs, None, None, Some("missing-project")).as_deref(),
            Some("missing-project")
        );
        assert_eq!(
            recover_kernel(&nb, &specs, None, None, None).as_deref(),
            Some("python3")
        );
        let markdown = parse("/p/readme.md", "").unwrap();
        assert_eq!(
            recover_kernel(
                &markdown,
                &specs,
                Some("live"),
                Some("remembered"),
                Some("project")
            ),
            None
        );
        let mut script = parse("/p/qc.R", "").unwrap();
        script.kernel = Some("python3".into());
        assert_eq!(pick_kernel(&script, &specs).as_deref(), Some("ark"));
    }

    #[test]
    fn knows_which_files_are_notebooks() {
        assert_eq!(format_of("a/b.ipynb"), Some(Format::Ipynb));
        assert_eq!(format_of("b.Rmd"), Some(Format::Quarto));
        assert_eq!(format_of("b.R"), Some(Format::Script));
        assert_eq!(format_of("Makefile"), None);
    }

    #[test]
    fn reads_and_writes_scripts_whole() {
        let nb = parse("/p/qc.py", "import os\nprint(1)\n").unwrap();
        assert_eq!(nb.format, Format::Script);
        assert_eq!(nb.language.as_deref(), Some("python"));
        assert_eq!(sources(&nb), ["import os\nprint(1)\n"]);
        assert_eq!(nb.render(&["x <- 1".into()]).unwrap(), "x <- 1");
        let r = parse("/p/qc.R", "").unwrap();
        assert_eq!(r.language.as_deref(), Some("r"));
        let python = KernelspecInfo {
            name: "python3".into(),
            display_name: "Python 3".into(),
            language: "python".into(),
            env: None,
        };
        assert_eq!(
            pick_kernel(&nb, std::slice::from_ref(&python)).as_deref(),
            Some("python3")
        );
        assert_eq!(pick_kernel(&r, &[python]), None);
    }

    #[test]
    fn standalone_markdown_keeps_raw_source_and_has_no_kernel() {
        let specs = [spec("python3", "python"), spec("ark", "R")];
        for ext in ["md", "MARKDOWN", "Mdown", "mkd"] {
            let nb = parse(
                &format!("/p/readme.{ext}"),
                "# Hi\n<script>alert(1)</script>\n",
            )
            .unwrap();
            assert_eq!(nb.format, Format::Markdown);
            assert!(nb.single_editor());
            assert!(!nb.can_run());
            assert_eq!(nb.render(&["# Unsaved\n".into()]).unwrap(), "# Unsaved\n");
            assert_eq!(pick_kernel(&nb, &specs), None);
        }
        assert_eq!(parse("/p/README", "# Hi").unwrap().format, Format::Markdown);
        assert_eq!(format_of("/p/doc.mdx"), None);
        assert_eq!(format_of("/p/doc.qmd"), Some(Format::Quarto));
        assert_eq!(format_of("/p/doc.Rmd"), Some(Format::Quarto));
    }

    #[test]
    fn shell_recognition_is_specific_and_never_picks_a_kernel() {
        let specs = [spec("python3", "python"), spec("ark", "R")];
        for first in [
            "#!/bin/sh",
            "#! /bin/bash -e",
            "#!/usr/bin/env zsh",
            "#!/usr/bin/env -S bash -eu",
            "#!/bin/dash",
            "#!/bin/ksh",
        ] {
            let text = format!("{first}\necho safe\n");
            let nb = parse("/p/launch", &text).unwrap();
            assert_eq!(nb.format, Format::Text);
            assert_eq!(nb.language.as_deref(), Some("bash"));
            assert!(!nb.can_run());
            assert_eq!(pick_kernel(&nb, &specs), None);
            assert_eq!(nb.render(std::slice::from_ref(&text)).unwrap(), text);
            let saved = parse_as("/p/launch", "echo edited", nb.format).unwrap();
            assert_eq!(
                saved.render(&["echo edited".into()]).unwrap(),
                "echo edited"
            );
        }
        for text in [
            "echo bash",
            "#!/usr/bin/env python",
            "#!/bin/bashful",
            "#!/bin/echo bash",
        ] {
            assert!(!shell_shebang(text));
            assert_eq!(
                parse("/p/launch", text).unwrap().language.as_deref(),
                Some("text")
            );
        }
        for ext in ["SH", "bash", "zsh"] {
            assert_eq!(
                parse(&format!("/p/run.{ext}"), "").unwrap().format,
                Format::Text
            );
        }
    }

    #[test]
    fn text_files_open_with_their_highlighting_and_never_run() {
        let specs = [spec("python3", "python"), spec("ark", "R")];
        for (path, language) in [
            ("/p/data.json", "json"),
            ("/p/env.YML", "yaml"),
            ("/p/config.yaml", "yaml"),
            ("/p/pyproject.toml", "toml"),
            ("/p/.gitignore", "bash"),
            // A shebang does not make a .txt file a script.
            ("/p/notebook.txt", "text"),
        ] {
            let nb = parse(path, "#!/bin/bash\n").unwrap();
            assert_eq!(nb.format, Format::Text, "{path}");
            assert_eq!(nb.language.as_deref(), Some(language), "{path}");
            assert!(!nb.can_run());
            assert_eq!(pick_kernel(&nb, &specs), None);
        }
        assert_eq!(format_of("/p/.gitignore"), Some(Format::Text));
        assert_eq!(format_of("/p/Makefile"), None);
    }

    #[test]
    fn single_editor_empty_files_save_without_adding_a_newline() {
        for path in ["/p/a.md", "/p/a.sh", "/p/a.csv", "/p/a.tsv", "/p/a.py"] {
            let nb = parse(path, "").unwrap();
            assert!(nb.single_editor());
            assert_eq!(
                nb.render(&["é,\"x\"\r\nnext".into()]).unwrap(),
                "é,\"x\"\r\nnext"
            );
            assert_eq!(nb.render(&["".into()]).unwrap(), "");
        }
    }

    #[test]
    fn icon_text_types_have_lossless_non_runnable_groups() {
        for (language, paths) in [
            ("python", vec!["Snakefile", "pipeline.SMK"]),
            ("yaml", vec!["workflow.cwl"]),
            (
                "toml",
                vec![
                    "Cargo.lock",
                    "UV.LOCK",
                    "a.ini",
                    "a.cfg",
                    "a.config",
                    "nextflow.config",
                ],
            ),
            ("json", vec!["renv.lock"]),
            ("bash", vec![".gitattributes"]),
            ("fasta", vec!["a.fa", "a.fasta", "a.fna", "a.faa"]),
            ("fastq", vec!["a.fq", "a.fastq"]),
            (
                "bio-table",
                vec![
                    "a.vcf",
                    "a.bed",
                    "a.bedgraph",
                    "a.narrowpeak",
                    "a.gff",
                    "a.gff3",
                    "a.gtf",
                    "a.sam",
                    "a.mtx",
                ],
            ),
            ("nextflow", vec!["a.nf"]),
            ("wdl", vec!["a.wdl"]),
            ("bibtex", vec!["a.bib"]),
            (
                "text",
                vec!["LICENSE", "COPYING", "other.lock", "unknown.ext", "unknown"],
            ),
        ] {
            for path in paths {
                let nb = parse(path, "A\tα\r\n").unwrap();
                assert_eq!(nb.format, Format::Text, "{path}");
                assert_eq!(nb.language.as_deref(), Some(language), "{path}");
                assert!(!nb.can_run(), "{path}");
                assert_eq!(nb.render(&["new\té\r\n".into()]).unwrap(), "new\té\r\n");
            }
        }
        for path in ["AGENTS.MD", "CLAUDE.MD", "Readme", "README.notes"] {
            assert_eq!(format_of(path), Some(Format::Markdown), "{path}");
        }
        for ext in [
            "bam", "cram", "bcf", "h5", "h5ad", "h5seurat", "hdf5", "loom", "parquet", "rds",
            "rda", "rdata", "zip", "tar", "tgz", "gz", "bgz", "bz2", "xz", "zst",
        ] {
            let path = format!("reads.FASTQ.{}", ext.to_uppercase());
            assert!(is_binary(&path), "{path}");
            assert_eq!(format_of(&path), None, "{path}");
            assert!(parse(&path, "not really text").is_err(), "{path}");
        }
    }

    #[test]
    fn truncated_preview_can_never_render_for_saving() {
        let mut nb = parse("large.vcf", "#CHROM\nchr1\t2\n").unwrap();
        nb.truncated = true;
        assert!(nb.render(&["replacement".into()]).is_err());
    }
}
