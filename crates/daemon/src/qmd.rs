//! The executable chunks of a Quarto (`.qmd`) or R Markdown (`.Rmd`)
//! document: fenced blocks whose info string is in braces, like ```` ```{r} ````.
//! The app finds them too (`app/src/model/qmd.ts`), and both must give a
//! chunk the same id, since outputs are saved under it;
//! `tests/fixtures/chunks.json` keeps the two in step.

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Lower-case engine name: `r`, `python`, ...
    pub language: String,
    pub label: Option<String>,
    /// The label, else `chunk-<position>` (1-based, among executable chunks).
    pub cell_id: String,
    /// Byte offsets of the opening fence line's start and the closing
    /// fence line's end.
    pub from: usize,
    pub to: usize,
    /// 1-based line numbers of the fences; `close_line` is the last line
    /// when the chunk is never closed.
    pub open_line: usize,
    pub close_line: usize,
    pub closed: bool,
    /// The code between the fences, and where it is.
    pub code: String,
    pub code_from: usize,
    pub code_to: usize,
}

struct Line<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

pub fn parse_chunks(text: &str) -> Vec<Chunk> {
    let lines = split_with_offsets(text);
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((fence, info)) = open_fence(lines[i].text) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut end = start + 1;
        while end < lines.len() && !is_closing_fence(lines[end].text, fence) {
            end += 1;
        }
        let closed = end < lines.len();
        if let Some((language, header_rest)) = engine(info) {
            let code_from = lines[start].end + line_break_length(text, lines[start].end);
            let code_to = if end > start + 1 {
                lines[end - 1].end
            } else {
                code_from
            };
            let code = text[code_from..code_to.max(code_from)].to_string();
            let label = label_from(header_rest, &code);
            let last_line = if closed { end } else { lines.len() - 1 };
            chunks.push(Chunk {
                language: language.to_lowercase(),
                cell_id: label
                    .clone()
                    .unwrap_or_else(|| format!("chunk-{}", chunks.len() + 1)),
                label,
                from: lines[start].start,
                to: lines[last_line].end,
                open_line: start + 1,
                close_line: last_line + 1,
                closed,
                code,
                code_from,
                code_to: code_to.max(code_from),
            });
        }
        i = if closed { end + 1 } else { lines.len() };
    }
    chunks
}

/// The kernel the YAML front matter names with `jupyter: python3` or
/// `jupyter: {kernelspec: {name: ...}}`, as Quarto reads it.
pub fn front_matter_kernel(text: &str) -> Option<String> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    if lines.next()? != "---" {
        return None;
    }
    let header: Vec<&str> = lines.take_while(|l| !l.starts_with("---")).collect();
    for (i, line) in header.iter().enumerate() {
        let Some(value) = line
            .strip_prefix("jupyter")
            .map(str::trim_start)
            .and_then(|s| s.strip_prefix(':'))
        else {
            continue;
        };
        let value = value.split('#').next().unwrap_or("").trim();
        if !value.is_empty() {
            return Some(unquote(value));
        }
        let nested = header[i + 1..]
            .iter()
            .take_while(|l| l.starts_with([' ', '\t']));
        let mut in_kernelspec = false;
        for line in nested {
            let line = line.trim();
            if line.starts_with("kernelspec") {
                in_kernelspec = true;
            } else if in_kernelspec
                && let Some(name) = line.strip_prefix("name").map(str::trim_start)
                && let Some(name) = name.strip_prefix(':')
            {
                return Some(unquote(name.trim()));
            }
        }
    }
    None
}

/// The label from `#| label:` lines or the R Markdown header
/// (`{r name, echo=FALSE}`, `{r, label="name"}`).
fn label_from(header_rest: &str, code: &str) -> Option<String> {
    for line in code.split('\n') {
        let Some(option) = line.trim_start().strip_prefix("#|") else {
            break;
        };
        let value = option
            .trim_start()
            .strip_prefix("label")
            .map(str::trim_start)
            .and_then(|s| s.strip_prefix(':'))
            .map(str::trim)
            .filter(|v| !v.is_empty());
        if let Some(value) = value {
            return Some(unquote(value));
        }
    }
    let rest = header_rest.strip_prefix(',').unwrap_or(header_rest).trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(named) = label_argument(rest) {
        return Some(unquote(named));
    }
    let first = rest.split(',').next().unwrap_or("").trim();
    (!first.is_empty() && !first.contains('=')).then(|| unquote(first))
}

/// An opening fence: up to three spaces, then three or more backticks or
/// tildes. Returns the fence and the info string after it.
fn open_fence(line: &str) -> Option<(&str, &str)> {
    let rest = strip_indent(line)?;
    let mark = *rest.as_bytes().first()?;
    if mark != b'`' && mark != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b == mark).count();
    (len >= 3).then(|| (&rest[..len], rest[len..].trim()))
}

fn is_closing_fence(line: &str, fence: &str) -> bool {
    open_fence(line).is_some_and(|(found, info)| {
        info.is_empty() && found.as_bytes()[0] == fence.as_bytes()[0] && found.len() >= fence.len()
    })
}

/// Up to three leading spaces or tabs, as `^\s{0,3}`.
fn strip_indent(line: &str) -> Option<&str> {
    let indent = line.bytes().take_while(|b| b.is_ascii_whitespace()).count();
    (indent <= 3).then(|| &line[indent..])
}

/// `{r name, echo=FALSE}`: the engine and the rest of the header.
fn engine(info: &str) -> Option<(&str, &str)> {
    let inner = info.strip_prefix('{')?.trim_end().strip_suffix('}')?;
    let inner = inner.trim_start();
    let len = inner
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    (len > 0).then(|| (&inner[..len], inner[len..].trim()))
}

/// The value of `label=...` among a header's options, quotes kept.
fn label_argument(options: &str) -> Option<&str> {
    let starts = std::iter::once(0).chain(options.match_indices(',').map(|(i, _)| i + 1));
    for start in starts {
        let value = options[start..]
            .trim_start()
            .strip_prefix("label")
            .map(str::trim_start)
            .and_then(|s| s.strip_prefix('='))
            .map(str::trim_start);
        let Some(value) = value else { continue };
        for quote in ['"', '\''] {
            if let Some(inside) = value.strip_prefix(quote)
                && let Some(end) = inside.find(quote)
            {
                return Some(&value[..end + 2]);
            }
        }
        let len = value
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(value.len());
        if len > 0 {
            return Some(&value[..len]);
        }
    }
    None
}

fn unquote(s: &str) -> String {
    for quote in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(quote) && s.ends_with(quote) {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn split_with_offsets(text: &str) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    loop {
        let newline = text[start..].find('\n').map(|n| start + n);
        let raw_end = newline.unwrap_or(text.len());
        let end = if raw_end > start && text.as_bytes()[raw_end - 1] == b'\r' {
            raw_end - 1
        } else {
            raw_end
        };
        lines.push(Line {
            text: &text[start..end],
            start,
            end,
        });
        match newline {
            Some(n) => start = n + 1,
            None => break,
        }
    }
    lines
}

fn line_break_length(text: &str, at: usize) -> usize {
    if text[at..].starts_with("\r\n") {
        2
    } else if at < text.len() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// The cases the app's parser is tested against too.
    #[test]
    fn chunks_match_the_shared_fixture() {
        let text = include_str!("../../../tests/fixtures/chunks.json");
        let cases: Vec<Value> = serde_json::from_str(text).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let chunks = parse_chunks(case["text"].as_str().unwrap());
            let expected = case["chunks"].as_array().unwrap();
            assert_eq!(chunks.len(), expected.len(), "{name}");
            for (chunk, want) in chunks.iter().zip(expected) {
                assert_eq!(chunk.cell_id, want["cell_id"], "{name}");
                assert_eq!(chunk.language, want["language"], "{name}");
                assert_eq!(chunk.code, want["code"], "{name}");
                assert_eq!(chunk.open_line as u64, want["open_line"], "{name}");
                assert_eq!(chunk.close_line as u64, want["close_line"], "{name}");
            }
        }
    }

    #[test]
    fn front_matter_names_the_kernel() {
        assert_eq!(
            front_matter_kernel("---\ntitle: x\njupyter: conda-env-omicverse-py\n---\n").as_deref(),
            Some("conda-env-omicverse-py")
        );
        let nested =
            "---\njupyter:\n  kernelspec:\n    display_name: Py\n    name: \"py311\"\n---\n";
        assert_eq!(front_matter_kernel(nested).as_deref(), Some("py311"));
        assert_eq!(front_matter_kernel("# no front matter\njupyter: x\n"), None);
    }

    #[test]
    fn offsets_cover_the_fences_and_the_code() {
        let text = "Intro\n\n```{r setup}\nlibrary(x)\n```\n";
        let chunk = &parse_chunks(text)[0];
        assert_eq!(&text[chunk.from..chunk.to], "```{r setup}\nlibrary(x)\n```");
        assert_eq!(&text[chunk.code_from..chunk.code_to], "library(x)");
    }
}
