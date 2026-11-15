//! A notebook as the app shows it: its cells, in order, read from a
//! Jupyter `.ipynb` or a Quarto `.qmd`/`.Rmd` document, and written back
//! with the cells' new text. Scripts, standalone Markdown and delimited text
//! use one editor for the whole file and the same save/conflict lifecycle.

use std::ops::Range;

use anyhow::{Context, Result};
use sandhill_daemon::qmd;
use sandhill_proto::KernelspecInfo;
use serde_json::{Map, Value};

mod quarto_cells;

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
        Some("csv" | "tsv" | "tab") => Some(Format::Delimited),
        Some(
            "sh" | "bash" | "zsh" | "json" | "jsonc" | "yaml" | "yml" | "toml" | "txt"
            | "gitignore" | "gitattributes" | "smk" | "cwl" | "ini" | "cfg" | "config" | "lock"
            | "fa" | "fasta" | "fna" | "faa" | "ffn" | "frn" | "fas" | "afa" | "aln" | "fq"
            | "fastq" | "vcf" | "gvcf" | "maf" | "bed" | "bedgraph" | "bedpe" | "narrowpeak"
            | "broadpeak" | "gappedpeak" | "interval_list" | "wig" | "gff" | "gff3" | "gtf" | "sam"
            | "paf" | "psl" | "blast" | "fai" | "dict" | "counts" | "tpm" | "mtx" | "bim" | "fam"
            | "pvar" | "psam" | "nwk" | "newick" | "tree" | "treefile" | "nex" | "nexus" | "nf"
            | "wdl" | "bib",
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
                | "bai"
                | "crai"
                | "tbi"
                | "csi"
                | "bw"
                | "bigwig"
                | "bb"
                | "bigbed"
                | "tdf"
                | "h5mu"
                | "cool"
                | "mcool"
                | "qs"
                | "pgen"
                // Microsoft Office: only the default app opens them.
                | "doc"
                | "docx"
                | "docm"
                | "dot"
                | "dotx"
                | "xls"
                | "xlsx"
                | "xlsm"
                | "xlsb"
                | "xltx"
                | "ppt"
                | "pptx"
                | "pptm"
                | "pps"
                | "ppsx"
                | "potx"
        )
    )
}

/// Extensions that binary formats share with a text one: PLINK's genotypes
/// are `.bed` files, as BED intervals are. Their first bytes decide.
pub(crate) fn shared_with_binary(path: &str) -> bool {
    extension(path).as_deref() == Some("bed")
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
        Some("fa" | "fasta" | "fna" | "faa" | "ffn" | "frn" | "fas" | "afa") => "fasta",
        Some("fq" | "fastq") => "fastq",
        Some(
            "vcf" | "gvcf" | "maf" | "bed" | "bedgraph" | "bedpe" | "narrowpeak" | "broadpeak"
            | "gappedpeak" | "interval_list" | "wig" | "gff" | "gff3" | "gtf" | "sam" | "paf"
            | "psl" | "blast" | "fai" | "dict" | "counts" | "tpm" | "mtx" | "bim" | "fam" | "pvar"
            | "psam",
        ) => "bio-table",
        Some("nwk" | "newick" | "tree" | "treefile" | "nex" | "nexus") => "tree",
        Some("nf") => "nextflow",
        Some("wdl") => "wdl",
        Some("bib") => "bibtex",
        _ => "text",
    }
}

pub fn parse(path: &str, text: &str) -> Result<Notebook> {
    anyhow::ensure!(!is_binary(path), "binary files open with the default app");
    let mut format = format_of(path).unwrap_or(Format::Text);
    if format == Format::Text
        && extension(path).as_deref() == Some("txt")
        && looks_tab_separated(text)
    {
        format = Format::Delimited;
    }
    parse_as(path, text, format)
}

/// A table saved as `.txt` (Huan, 2026-11-14): its first lines, at least
/// two and at most 20 non-empty ones, all have the same number of tabs,
/// at least one. A last line cut off by a 5 MB preview is not counted.
pub(crate) fn looks_tab_separated(text: &str) -> bool {
    let complete = text.rfind('\n').map_or("", |end| &text[..end]);
    let mut tabs = complete
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(20)
        .map(|line| line.matches('\t').count());
    let Some(first) = tabs.next().filter(|&count| count > 0) else {
        return false;
    };
    let mut lines = 1;
    for count in tabs {
        if count != first {
            return false;
        }
        lines += 1;
    }
    lines >= 2
}

/// What a standalone text file can be coloured as, chosen in its editor:
/// its key (the document's language) and its name.
pub(crate) const COLOURINGS: [(&str, &str); 16] = [
    ("text", "Plain text"),
    ("csv", "Comma-separated"),
    ("tsv", "Tab-separated"),
    ("json", "JSON"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("bash", "Shell"),
    ("python", "Python"),
    ("r", "R"),
    ("fasta", "FASTA"),
    ("fastq", "FASTQ"),
    ("bio-table", "Genomics table"),
    ("tree", "Tree (Newick, Nexus)"),
    ("nextflow", "Nextflow"),
    ("wdl", "WDL"),
    ("bibtex", "BibTeX"),
];

impl Notebook {
    /// Whether its colouring can be chosen: plain text and delimited files,
    /// not notebooks, Quarto, scripts or Markdown, whose type decides.
    pub(crate) fn can_recolour(&self) -> bool {
        matches!(self.format, Format::Text | Format::Delimited)
    }

    /// Coloured as `choice` (a `COLOURINGS` key); colour only, nothing runs.
    pub(crate) fn recoloured(mut self, choice: &str) -> Notebook {
        if !self.can_recolour() || !COLOURINGS.iter().any(|(key, _)| *key == choice) {
            return self;
        }
        self.format = if matches!(choice, "csv" | "tsv") {
            Format::Delimited
        } else {
            Format::Text
        };
        self.language = Some(choice.into());
        self
    }
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
        // Its separator: comma for `.csv`, a tab for `.tsv`, `.tab` and a
        // `.txt` found tab-separated.
        Format::Delimited => Some(
            if extension(path).as_deref() == Some("csv") {
                "csv"
            } else {
                "tsv"
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
mod tests;
