//! Openable URLs under the pointer in one row.
//! File references are candidates only, validated asynchronously by the host.
//!
//! Derived from Herdr (Apache-2.0).

use std::ops::Range;

use crate::terminal::frame::TerminalRow;

/// Longest URL offered to the opener, so a pathological row cannot produce
/// an unbounded string.
const MAX_URL: usize = 2048;

const SCHEMES: [&str; 4] = ["https://", "http://", "file://", "mailto:"];

/// Only schemes worth handing to the platform opener. A program can put
/// any string in an OSC 8 target, so an unvalidated target would let output
/// choose an arbitrary URI scheme to launch.
#[must_use]
pub fn is_openable(url: &str) -> bool {
    let url = url.trim();
    url.len() <= MAX_URL
        && !url.contains(char::is_whitespace)
        && !url.contains(char::is_control)
        && SCHEMES
            .iter()
            .any(|scheme| url.len() > scheme.len() && url.starts_with(scheme))
}

/// An untrusted terminal file reference, confirmed by the embedding host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    /// Absolute or shell-relative path.
    pub path: String,
    /// One-based line, when present.
    pub line: Option<u32>,
    /// One-based column, when present.
    pub column: Option<u32>,
}

/// A URL in one row, with the columns it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The URL.
    pub url: String,
    /// The columns the link covers.
    pub columns: Range<u16>,
    /// A candidate file rather than a URL, never opened by the platform opener.
    pub file: Option<FileLink>,
}

/// The link covering `column`, if any. An OSC 8 hyperlink wins; only a cell
/// without one has the row text scanned for a bare URL.
#[must_use]
pub fn at(row: &TerminalRow, column: u16) -> Option<Link> {
    explicit(row, column).or_else(|| detected(row, column))
}

fn explicit(row: &TerminalRow, column: u16) -> Option<Link> {
    let cell = row
        .cells
        .iter()
        .find(|cell| cell.column == column && cell.hyperlink.is_some())?;
    let uri = cell.hyperlink.as_ref()?;
    if !is_openable(uri) {
        return None;
    }
    let mut columns = cell.column..cell.column.saturating_add(u16::from(cell.width.max(1)));
    for other in row.cells.iter() {
        if other.hyperlink.as_deref() != Some(uri.as_ref()) {
            continue;
        }
        let end = other.column.saturating_add(u16::from(other.width.max(1)));
        columns.start = columns.start.min(other.column);
        columns.end = columns.end.max(end);
    }
    Some(Link {
        url: uri.to_string(),
        columns,
        file: None,
    })
}

fn detected(row: &TerminalRow, column: u16) -> Option<Link> {
    let text = row.text.as_ref();
    for scheme in SCHEMES {
        let mut search = 0;
        while let Some(found) = text[search..].find(scheme) {
            let start = search + found;
            if is_url_byte(text.as_bytes().get(start.wrapping_sub(1)).copied()) {
                search = start + scheme.len();
                continue;
            }
            let end = end_of_url(text, start);
            if end - start <= MAX_URL
                && let Some(columns) = columns_for(row, start..end)
            {
                let url = trim_trailing(&text[start..end]);
                if columns.contains(&column) && is_openable(url) {
                    return Some(Link {
                        url: url.to_string(),
                        columns,
                        file: None,
                    });
                }
            }
            search = end.max(start + scheme.len());
        }
    }
    None
}

/// A candidate only. The host must check existence and project membership.
pub fn file_at(row: &TerminalRow, column: u16) -> Option<Link> {
    let text = row.text.as_ref();
    // Python tracebacks quote filenames, including names containing spaces.
    if let Some(start) = text.find("File \"").map(|start| start + 6)
        && let Some(end) = text[start..].find('"').map(|end| start + end)
        && let Some(tail) = text[end + 1..].strip_prefix(", line ")
    {
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(line) = digits.parse::<u32>()
            && line > 0
            && let Some(columns) = columns_for(row, start..end)
            && columns.contains(&column)
        {
            let mut file = parse_file(&text[start..end])?;
            file.line = Some(line);
            return Some(Link { url: file.path.clone(), columns, file: Some(file) });
        }
    }
    let delimiter = |ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}');
    let byte = row.cells.iter().find(|cell| {
        cell.column <= column && column < cell.column.saturating_add(u16::from(cell.width.max(1)))
    })?.text.start;
    if byte >= text.len() || text[byte..].chars().next().is_some_and(delimiter) {
        return None;
    }
    let start = text[..byte].char_indices().rev()
        .find(|(_, ch)| delimiter(*ch)).map_or(0, |(ix, ch)| ix + ch.len_utf8());
    let end = text[byte..].char_indices()
        .find(|(_, ch)| delimiter(*ch)).map_or(text.len(), |(ix, _)| byte + ix);
    let token = trim_trailing(&text[start..end]);
    let file = parse_file(token)?;
    let columns = columns_for(row, start..start + token.len())?;
    columns.contains(&column).then(|| Link {
        url: file.path.clone(), columns, file: Some(file),
    })
}

fn parse_file(token: &str) -> Option<FileLink> {
    let token = trim_trailing(token);
    if token.is_empty() || token.len() > MAX_URL || token.contains(char::is_control)
        || token.contains("://") || token.contains('\\') || token.starts_with("mailto:")
        || token.as_bytes().get(1) == Some(&b':')
            && token.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && token.as_bytes().get(2) == Some(&b'/')
        || matches!(token, "." | ".." | "/" | "~")
    {
        return None;
    }
    let mut path = token;
    let mut numbers = [None, None];
    for number in &mut numbers {
        let Some((head, tail)) = path.rsplit_once(':') else { break };
        if tail.is_empty() || !tail.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let value = tail.parse::<u32>().ok().filter(|value| *value > 0)?;
        *number = Some(value);
        path = head;
    }
    if path.is_empty() { return None; }
    let (line, column) = if numbers[1].is_some() {
        (numbers[1], numbers[0])
    } else {
        (numbers[0], None)
    };
    Some(FileLink { path: path.to_string(), line, column })
}

fn end_of_url(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() && is_url_byte(Some(bytes[end])) {
        end += 1;
    }
    start + trim_trailing(&text[start..end]).len()
}

fn is_url_byte(byte: Option<u8>) -> bool {
    match byte {
        None => false,
        Some(byte) => {
            !byte.is_ascii_whitespace()
                && !byte.is_ascii_control()
                && byte != b'"'
                && byte != b'\''
                && byte != b'<'
                && byte != b'>'
                && byte != b'`'
        }
    }
}

fn trim_trailing(url: &str) -> &str {
    url.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}'])
}

fn columns_for(row: &TerminalRow, text: Range<usize>) -> Option<Range<u16>> {
    let mut columns: Option<Range<u16>> = None;
    for cell in row.cells.iter() {
        if cell.text.start >= text.end || cell.text.end <= text.start {
            continue;
        }
        let end = cell.column.saturating_add(u16::from(cell.width.max(1)));
        columns = Some(match columns {
            None => cell.column..end,
            Some(range) => range.start.min(cell.column)..range.end.max(end),
        });
    }
    columns.filter(|range| range.start < range.end)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::terminal::frame::CellSpan;

    #[test]
    fn parses_file_positions_without_treating_urls_or_windows_as_files() {
        for (text, path, line, column) in [
            ("/p/a.py", "/p/a.py", None, None),
            ("src/a.R:12", "src/a.R", Some(12), None),
            ("./src/a.py:12:4,.", "./src/a.py", Some(12), Some(4)),
            ("name:part.R:2", "name:part.R", Some(2), None),
            ("a:3", "a", Some(3), None),
            ("name:part.R", "name:part.R", None, None),
        ] {
            assert_eq!(parse_file(text), Some(FileLink {
                path: path.into(), line, column,
            }));
        }
        for text in ["https://x/a.py:3", "file:///p/a", "mailto:x@y",
            "C:/work/a.py:3", r"C:\work\a.py:3", "a.py:0", "a.py:2:0",
            "a.py:99999999999999999999", ".", ".."] {
            assert!(parse_file(text).is_none(), "{text}");
        }
    }

    #[test]
    fn finds_traceback_paths_and_limits_underlining_to_file_columns() {
        let traceback = row("  File \"src/my code.py\", line 3, in run");
        let file = file_at(&traceback, 10).unwrap();
        assert_eq!(file.file.unwrap(), FileLink {
            path: "src/my code.py".into(), line: Some(3), column: None,
        });
        let punctuated = row("error (src/a.py:12:4), next");
        let link = file_at(&punctuated, 10).unwrap();
        assert_eq!(link.columns, 7..20);
        assert!(file_at(&punctuated, 20).is_none());
    }

    fn row(text: &str) -> TerminalRow {
        let cells: Vec<CellSpan> = text
            .char_indices()
            .enumerate()
            .map(|(index, (offset, ch))| CellSpan {
                text: offset..offset + ch.len_utf8(),
                column: u16::try_from(index).unwrap(),
                width: 1,
                ..CellSpan::default()
            })
            .collect();
        TerminalRow {
            text: Arc::from(text),
            cells: Arc::from(cells),
            ..TerminalRow::default()
        }
    }

    fn linked(text: &str, uri: &str, columns: Range<u16>) -> TerminalRow {
        let mut row = row(text);
        let cells: Vec<CellSpan> = row
            .cells
            .iter()
            .map(|cell| {
                let mut cell = cell.clone();
                if columns.contains(&cell.column) {
                    cell.hyperlink = Some(Arc::from(uri));
                }
                cell
            })
            .collect();
        row.cells = Arc::from(cells);
        row
    }

    #[test]
    fn a_url_is_found_only_under_the_columns_it_covers() {
        let found = row("see https://example.com/x now");
        assert!(at(&found, 0).is_none());
        let link = at(&found, 6).expect("link");
        assert_eq!(link.url, "https://example.com/x");
        assert_eq!(link.columns, 4..25);
        assert!(at(&found, 26).is_none());
        assert!(at(&row("xhttps://example.com"), 3).is_none());
    }

    #[test]
    fn trailing_punctuation_and_other_schemes_are_handled() {
        let punctuated = row("read https://example.com/page).");
        assert_eq!(
            at(&punctuated, 10).expect("link").url,
            "https://example.com/page"
        );
        for text in [
            "file:///tmp/log.txt",
            "mailto:someone@example.com",
            "http://localhost:8080/health",
        ] {
            assert_eq!(at(&row(text), 2).expect("link").url, text);
        }
        assert!(at(&row("example.com is not a url"), 2).is_none());
    }

    #[test]
    fn an_osc8_target_wins_and_unsupported_schemes_are_refused() {
        let row = linked("click here", "https://example.com/real", 0..5);
        let link = at(&row, 2).expect("link");
        assert_eq!(link.url, "https://example.com/real");
        assert_eq!(link.columns, 0..5);
        assert!(at(&row, 7).is_none());
        let bad = linked("click here", "javascript:alert(1)", 0..5);
        assert!(at(&bad, 2).is_none());
        assert!(!is_openable("file:///etc/passwd extra"));
        assert!(!is_openable("https://"));
        assert!(is_openable("https://example.com"));
    }
}
