//! Env files: `NAME=value` lines a project's kernels start with, as
//! python-dotenv and VS Code read them. Comments start with `#`; `export`
//! in front of a name is allowed; values may be quoted. `${NAME}` (or
//! `${NAME:-default}`) in an unquoted or double-quoted value is the
//! variable as set by an earlier line, else in the daemon's environment.
//! Single quotes keep a value as written.

use std::collections::HashMap;
use std::path::Path;

/// The variables of each file in turn, a later file's winning. Files that
/// don't exist are skipped, and so are lines that aren't `NAME=value`.
pub fn load(root: &Path, files: &[String]) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = Vec::new();
    for name in files {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            continue;
        };
        for (key, value) in parse(&text, &vars) {
            vars.retain(|(k, _)| *k != key);
            vars.push((key, value));
        }
    }
    vars
}

/// The variables in one file's text; `earlier` are what `${NAME}` sees
/// before the daemon's environment.
pub fn parse(text: &str, earlier: &[(String, String)]) -> Vec<(String, String)> {
    let mut known: HashMap<String, String> = earlier.iter().cloned().collect();
    let mut vars = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_start();
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        if line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                // A quoted value may go on over several lines.
                let mut body = rest[1..].to_string();
                let end = loop {
                    if let Some(end) = closing(&body, quote) {
                        break Some(end);
                    }
                    match lines.next() {
                        Some(more) => {
                            body.push('\n');
                            body.push_str(more);
                        }
                        None => break None,
                    }
                };
                let Some(end) = end else { continue };
                let body = &body[..end];
                if quote == '\'' {
                    body.to_string()
                } else {
                    expand(&unescape(body), &known)
                }
            }
            _ => {
                // An unquoted value ends at a ` #` comment.
                let value = match rest.find(" #") {
                    Some(at) => &rest[..at],
                    None => rest,
                };
                expand(value.trim_end(), &known)
            }
        };
        known.insert(key.to_string(), value.clone());
        vars.push((key.to_string(), value));
    }
    vars
}

/// Where the closing quote is: the first one not escaped by a backslash
/// (only double quotes have escapes).
fn closing(body: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote == '"' {
            escaped = true;
        } else if c == quote {
            return Some(i);
        }
    }
    None
}

fn unescape(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other @ ('"' | '\\' | '$')) => out.push(other),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Replaces `${NAME}` and `${NAME:-default}`.
fn expand(value: &str, known: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        let inner = &rest[start + 2..start + len];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        let found = known
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .filter(|v| !v.is_empty() || default.is_none());
        out.push_str(&found.or(default.map(String::from)).unwrap_or_default());
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(text: &str) -> Vec<(String, String)> {
        parse(text, &[])
    }

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.into(), v.into())
    }

    #[test]
    fn lines_comments_and_quotes() {
        let text = r#"
# The lab's settings
DATA_DIR=/data/pbmc   # where the counts are
export SEED=42
EMPTY=
SINGLE='keep ${DATA_DIR} \n as is'
DOUBLE="two\nlines and a \"quote\""
not a variable
  SPACED = padded value
HASH=a#b
"#;
        assert_eq!(
            vars(text),
            vec![
                pair("DATA_DIR", "/data/pbmc"),
                pair("SEED", "42"),
                pair("EMPTY", ""),
                pair("SINGLE", r"keep ${DATA_DIR} \n as is"),
                pair("DOUBLE", "two\nlines and a \"quote\""),
                pair("SPACED", "padded value"),
                pair("HASH", "a#b"),
            ]
        );
    }

    #[test]
    fn quoted_values_can_span_lines() {
        let text = "KEY=\"-----BEGIN-----\nabc\n-----END-----\"\nNEXT=1\nOPEN=\"never closed\n";
        assert_eq!(
            vars(text),
            vec![
                pair("KEY", "-----BEGIN-----\nabc\n-----END-----"),
                pair("NEXT", "1")
            ]
        );
    }

    #[test]
    fn variables_expand_from_earlier_lines_then_the_environment() {
        let home = std::env::var("HOME").unwrap_or_default();
        let text = "ROOT=/data\nCOUNTS=${ROOT}/counts\nCACHE=\"${HOME}/.cache\"\nMISSING=${SANDHILL_SURELY_UNSET}x\nDEFAULTED=${SANDHILL_SURELY_UNSET:-fallback}\n";
        assert_eq!(
            vars(text),
            vec![
                pair("ROOT", "/data"),
                pair("COUNTS", "/data/counts"),
                pair("CACHE", &format!("{home}/.cache")),
                pair("MISSING", "x"),
                pair("DEFAULTED", "fallback"),
            ]
        );
    }

    #[test]
    fn later_files_win_and_missing_ones_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "A=1\nB=2\n").unwrap();
        std::fs::write(dir.path().join("local.env"), "B=${A}${A}\nC=3\n").unwrap();
        let files = [".env", "gone.env", "local.env"].map(String::from);
        assert_eq!(
            load(dir.path(), &files),
            vec![pair("A", "1"), pair("B", "11"), pair("C", "3")]
        );
    }
}
