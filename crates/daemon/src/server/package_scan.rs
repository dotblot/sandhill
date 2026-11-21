use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Language {
    R,
    Python,
}

impl Language {
    pub(super) fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("r") {
            Some(Self::R)
        } else if name
            .get(..6)
            .is_some_and(|s| s.eq_ignore_ascii_case("python"))
        {
            Some(Self::Python)
        } else {
            None
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::R => "r",
            Self::Python => "python",
        }
    }

    pub(super) fn preferred_kernels(self) -> &'static [&'static str] {
        match self {
            Self::R => &["ark", "ir"],
            Self::Python => &["python3"],
        }
    }
}

pub(super) struct Document {
    pub(super) language: Language,
    pub(super) kernel: Option<String>,
    pub(super) imports: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NotebookMetadata {
    kernelspec: NotebookKernel,
    language_info: NotebookLanguage,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NotebookKernel {
    name: String,
    language: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NotebookLanguage {
    name: String,
}

#[derive(Deserialize)]
struct Notebook {
    #[serde(default)]
    metadata: NotebookMetadata,
    cells: Vec<NotebookCell>,
}

#[derive(Deserialize)]
#[serde(tag = "cell_type", rename_all = "snake_case")]
enum NotebookCell {
    Code {
        source: Source,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Source {
    Text(String),
    Lines(Vec<String>),
}

pub(super) fn document(path: &Path, text: &str, chosen: Option<Language>) -> Result<Document> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let mut imports = Vec::new();
    let (language, kernel) = match ext.to_ascii_lowercase().as_str() {
        "r" => {
            scan(Language::R, text, &mut imports);
            (Language::R, None)
        }
        "py" => {
            scan(Language::Python, text, &mut imports);
            (Language::Python, None)
        }
        "ipynb" => {
            let notebook: Notebook = serde_json::from_str(text)?;
            let meta = notebook.metadata;
            let kernel = (!meta.kernelspec.name.is_empty()).then_some(meta.kernelspec.name);
            let language = chosen
                .or_else(|| Language::parse(&meta.language_info.name))
                .or_else(|| Language::parse(&meta.kernelspec.language))
                .or_else(|| kernel.as_deref().and_then(language_from_kernel))
                .unwrap_or(Language::Python);
            for cell in notebook.cells {
                match cell {
                    NotebookCell::Code {
                        source: Source::Text(code),
                    } => {
                        scan(language, &code, &mut imports);
                    }
                    NotebookCell::Code {
                        source: Source::Lines(lines),
                    } => {
                        scan(language, &lines.concat(), &mut imports);
                    }
                    NotebookCell::Other => {}
                }
            }
            (language, kernel)
        }
        "rmd" | "qmd" => {
            let kernel = crate::qmd::front_matter_kernel(text);
            let chunks = crate::qmd::parse_chunks(text);
            let language = if ext.eq_ignore_ascii_case("rmd") {
                Language::R
            } else {
                chosen
                    .or_else(|| kernel.as_deref().and_then(language_from_kernel))
                    .or_else(|| chunks.iter().find_map(|c| Language::parse(&c.language)))
                    .unwrap_or(Language::R)
            };
            for chunk in chunks
                .iter()
                .filter(|c| Language::parse(&c.language) == Some(language))
            {
                scan(language, &chunk.code, &mut imports);
            }
            (language, kernel)
        }
        _ => bail!("This kind of file doesn't use an R or Python kernel."),
    };
    let mut seen = HashSet::new();
    imports.retain(|name| seen.insert(name.clone()));
    Ok(Document {
        language,
        kernel,
        imports,
    })
}

fn language_from_kernel(name: &str) -> Option<Language> {
    if matches!(name, "ir" | "irkernel" | "ark") {
        Some(Language::R)
    } else {
        Language::parse(name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    Text(&'a str),
    Mark(u8),
    Namespace,
    Newline,
    Other,
}

pub(super) fn scan(language: Language, code: &str, imports: &mut Vec<String>) {
    let tokens = tokens(language, code);
    match language {
        Language::R => scan_r(&tokens, imports),
        Language::Python => scan_python(&tokens, imports),
    }
}

fn tokens(language: Language, code: &str) -> Vec<Token<'_>> {
    use Token::*;
    let mut tokens = Vec::new();
    let bytes = code.as_bytes();
    let python = language == Language::Python;
    let mut i = 0;
    let mut line_start = true;
    while let Some(&b) = bytes.get(i) {
        let rest = &bytes[i..];
        match b {
            b'\n' | b'\r' => {
                if python {
                    tokens.push(Newline);
                }
                line_start = true;
                i += if rest.starts_with(b"\r\n") { 2 } else { 1 };
                continue;
            }
            b' ' | b'\t' | 0x0c => {
                i += 1;
                continue;
            }
            b'\\' if python && (rest.starts_with(b"\\\n") || rest.starts_with(b"\\\r\n")) => {
                i += if rest[1] == b'\r' { 3 } else { 2 };
                continue;
            }
            _ => {}
        }
        if b == b'#' || (python && line_start && matches!(b, b'%' | b'!')) {
            i += rest
                .iter()
                .position(|&c| matches!(c, b'\n' | b'\r'))
                .unwrap_or(rest.len());
            continue;
        }
        line_start = false;
        if !python
            && matches!(b, b'r' | b'R')
            && let Some(end) = r_raw_string(rest)
        {
            // The contents of a raw string are never scanned as code.
            tokens.push(Other);
            i += end;
        } else if matches!(b, b'\'' | b'"') || (!python && b == b'`') {
            let delimiter = if python && rest.starts_with(&[b; 3]) {
                3
            } else {
                1
            };
            let mut end = i + delimiter;
            while end < bytes.len() && !bytes[end..].starts_with(&rest[..delimiter]) {
                if bytes[end] == b'\\' {
                    end += 1;
                }
                end += 1;
            }
            if end + delimiter <= bytes.len() {
                tokens.push(Text(&code[i + delimiter..end]));
            }
            i = end + delimiter;
        } else if !python && rest.starts_with(b"::") {
            tokens.push(Namespace);
            i += if rest.starts_with(b":::") { 3 } else { 2 };
        } else if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 || (!python && b == b'.') {
            let len = rest
                .iter()
                .take_while(|&&c| {
                    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 || (!python && c == b'.')
                })
                .count();
            tokens.push(Name(&code[i..i + len]));
            i += len;
        } else {
            tokens.push(Mark(b));
            i += 1;
        }
    }
    tokens
}

fn r_raw_string(bytes: &[u8]) -> Option<usize> {
    if !matches!(bytes.get(1), Some(b'"' | b'\'')) {
        return None;
    }
    let dashes = bytes[2..].iter().take_while(|&&b| b == b'-').count();
    let open = 2 + dashes;
    let close = match bytes.get(open)? {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let end_len = dashes + 2;
    Some(
        bytes[open + 1..]
            .windows(end_len)
            .position(|end| {
                end[0] == close && end[1..=dashes] == bytes[2..open] && end[dashes + 1] == bytes[1]
            })
            .map_or(bytes.len(), |pos| open + 1 + pos + end_len),
    )
}

fn scan_r(tokens: &[Token<'_>], imports: &mut Vec<String>) {
    use Token::*;
    for (i, &token) in tokens.iter().enumerate() {
        let tail = &tokens[i + 1..];
        if matches!(tail, [Namespace, Name(_) | Text(_), ..])
            && let Name(name) | Text(name) = token
            && r_package(name)
        {
            imports.push(name.into());
        }
        let Name(function) = token else { continue };
        let pacman =
            function == "p_load" && i >= 2 && tokens[i - 2..i] == [Name("pacman"), Namespace];
        if !pacman && !matches!(function, "library" | "require" | "requireNamespace") {
            continue;
        }
        if i > 0 && matches!(tokens[i - 1], Mark(b'$' | b'@')) {
            continue;
        }
        let [Mark(b'('), args @ ..] = tail else {
            continue;
        };
        let args = arguments(args);
        let character_only = args.iter().any(|arg| {
            matches!(arg, [Name("character.only"), Mark(b'='), value @ ..]
                if !matches!(value, [Name("FALSE" | "F")]))
        });
        if pacman {
            for arg in args {
                push_r_arg(arg, character_only, imports);
            }
        } else {
            let package = args
                .iter()
                .find_map(|arg| match arg {
                    [Name("package"), Mark(b'='), value @ ..] => Some(value),
                    _ => None,
                })
                .or_else(|| {
                    args.into_iter()
                        .find(|arg| !matches!(arg, [Name(_), Mark(b'='), ..]))
                });
            if let Some(arg) = package {
                push_r_arg(
                    arg,
                    character_only || function == "requireNamespace",
                    imports,
                );
            }
        }
    }
}

fn arguments<'a, 'code>(tokens: &'a [Token<'code>]) -> Vec<&'a [Token<'code>]> {
    use Token::Mark;
    let mut args = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Mark(b'(' | b'[' | b'{') => depth += 1,
            Mark(b')') if depth == 0 => {
                args.push(&tokens[start..i]);
                break;
            }
            Mark(b')' | b']' | b'}') => depth = depth.saturating_sub(1),
            Mark(b',') if depth == 0 => {
                args.push(&tokens[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    args
}

fn push_r_arg(arg: &[Token<'_>], character_only: bool, imports: &mut Vec<String>) {
    match arg {
        [Token::Text(name)] if r_package(name) => imports.push((*name).into()),
        [Token::Name(name)] if !character_only && r_package(name) => imports.push((*name).into()),
        _ => {}
    }
}

fn scan_python(tokens: &[Token<'_>], imports: &mut Vec<String>) {
    use Token::*;
    let mut statement = true;
    let mut i = 0;
    while let Some(&token) = tokens.get(i) {
        let start = statement;
        statement = matches!(token, Newline | Mark(b';' | b':'));
        i += 1;
        if !start {
            continue;
        }
        if token == Name("from") {
            let (name, end) = dotted_name(tokens, i);
            if tokens.get(end) == Some(&Name("import")) {
                if let Some(name) = name {
                    imports.push(name.into());
                }
                i = end + 1;
            }
        } else if token == Name("import") {
            loop {
                let (name, end) = dotted_name(tokens, i);
                let Some(name) = name else { break };
                imports.push(name.into());
                i = end;
                if matches!(tokens.get(i..), Some([Name("as"), Name(_), ..])) {
                    i += 2;
                }
                if tokens.get(i) != Some(&Mark(b',')) {
                    break;
                }
                i += 1;
            }
        }
    }
}

fn dotted_name<'a>(tokens: &[Token<'a>], mut i: usize) -> (Option<&'a str>, usize) {
    use Token::*;
    let start = i;
    while tokens.get(i) == Some(&Mark(b'.')) {
        i += 1;
    }
    let mut name = None;
    if let Some(Name(word)) = tokens.get(i).filter(|token| **token != Name("import")) {
        if i == start && python_module(word) {
            name = Some(*word);
        }
        i += 1;
        while matches!(tokens.get(i..), Some([Mark(b'.'), Name(_), ..])) {
            i += 2;
        }
    }
    (name, i)
}

pub(super) fn r_package(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
}

pub(super) fn python_module(s: &str) -> bool {
    s.as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_r_packages_without_evaluating_expressions() {
        let doc = document(
            Path::new("analysis.R"),
            r#"
            # library(comment)
            print("string::f")
            print(r"--( library(wrong) " library(still_wrong) )--")
            library(Seurat); library("harmony"); require(Matrix)
            requireNamespace('jsonlite', quietly=TRUE)
            ggplot2::ggplot(); utils:::something()
            pacman::p_load(dplyr, 'tidyr', install = FALSE)
            suppressPackageStartupMessages(library(SeuratObject))
            library(value, character.only=TRUE); requireNamespace(variable)
            library(character.only=TRUE, package="quarto")
            library(package = decide()); foo$library(not_a_package)
            library(Seurat)
        "#,
            None,
        )
        .unwrap();
        assert_eq!(
            doc.imports,
            [
                "Seurat",
                "harmony",
                "Matrix",
                "jsonlite",
                "ggplot2",
                "utils",
                "pacman",
                "dplyr",
                "tidyr",
                "SeuratObject",
                "quarto"
            ]
        );
    }

    #[test]
    fn finds_python_imports_and_skips_non_python_lines() {
        let doc = document(
            Path::new("analysis.py"),
            r#"
# import comment
"import string"
text = r""" no
import in_triple_quotes
"""
import a, b.c as d, e
from sklearn.cluster import KMeans
from . import local
from ..inner import something
%pip install umap-learn; import not_code
!echo 'import not_code_either'
if ready: import numpy; import pandas
import a
from PIL \
    import Image
        "#,
            None,
        )
        .unwrap();
        assert_eq!(
            doc.imports,
            ["a", "b", "e", "sklearn", "numpy", "pandas", "PIL"]
        );
    }

    #[test]
    fn scans_notebook_code_cells_in_its_language() {
        let notebook = r#"{"metadata":{"kernelspec":{"name":"ir","language":"R"}},"cells":[{"cell_type":"markdown","source":"library(markdown)"},{"cell_type":"code","source":["library(Seurat)\n","harmony::RunHarmony(x)"]},{"cell_type":"code","source":"library(Seurat)\nlibrary('Matrix')"}]}"#;
        let doc = document(Path::new("analysis.ipynb"), notebook, None).unwrap();
        assert_eq!(doc.language, Language::R);
        assert_eq!(doc.kernel.as_deref(), Some("ir"));
        assert_eq!(doc.imports, ["Seurat", "harmony", "Matrix"]);
    }

    #[test]
    fn scans_only_executable_chunks_for_the_chosen_language() {
        let qmd = "---\njupyter: python3\n---\n\nimport prose\n\n```python\nimport example\n```\n\n```{r}\nlibrary(Seurat)\n```\n\n~~~{python}\nimport scanpy\n~~~\n";
        let doc = document(Path::new("analysis.qmd"), qmd, None).unwrap();
        assert_eq!(doc.language, Language::Python);
        assert_eq!(doc.imports, ["scanpy"]);
        assert_eq!(
            document(Path::new("analysis.qmd"), qmd, Some(Language::R))
                .unwrap()
                .imports,
            ["Seurat"]
        );
        assert_eq!(
            document(Path::new("analysis.Rmd"), qmd, None)
                .unwrap()
                .imports,
            ["Seurat"]
        );
    }
}
