//! Enter inside an open call or list (Huan, 2026-11-16): the new line lines
//! up under the first argument, as RStudio and PEP 8 do, or, when the
//! bracket ends its line, sits one step in from that line.

use gpui_kit::component::input::{EditorState, Enter};
use gpui_kit::*;

/// How far back the brackets are counted.
const LOOK_BACK: usize = 20_000;

/// The indent of a new line after `before` (the text up to the caret), when
/// the caret is inside an open `(` or `[` (or a Python `{`); `None` leaves
/// Enter to the editor.
pub(crate) fn continuation(before: &str, python: bool) -> Option<String> {
    let mut start = before.len().saturating_sub(LOOK_BACK);
    while !before.is_char_boundary(start) {
        start += 1;
    }
    let text = &before[start..];
    let mut open: Vec<usize> = Vec::new();
    let mut quote: Option<(char, bool)> = None;
    let mut chars = text.char_indices().peekable();
    // The last bracket closed: where, and where it opened.
    let mut closed: Option<(usize, usize)> = None;
    while let Some((at, ch)) = chars.next() {
        if let Some((close, triple)) = quote {
            if ch == '\\' {
                chars.next();
            } else if ch == close {
                if !triple {
                    quote = None;
                } else if text[at..].starts_with(&close.to_string().repeat(3)) {
                    chars.next();
                    chars.next();
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '#' => {
                while chars.peek().is_some_and(|(_, c)| *c != '\n') {
                    chars.next();
                }
            }
            '"' | '\'' | '`' => {
                let triple = python && text[at..].starts_with(&ch.to_string().repeat(3));
                if triple {
                    chars.next();
                    chars.next();
                }
                quote = Some((ch, triple));
            }
            '(' | '[' | '{' => open.push(at),
            ')' | ']' | '}' => {
                if let Some(opened) = open.pop() {
                    closed = Some((at, opened));
                }
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return None;
    }
    let indent_at = |at: usize| -> (usize, String) {
        let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
        let indent = text[line_start..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        (line_start, indent)
    };
    let step = " ".repeat(if python { 4 } else { 2 });
    let Some(&at) = open.last() else {
        // A call over several lines closed on this one: the next line goes
        // back to where its statement began, one step in after Python's `:`.
        let this_line = text.rfind('\n').map_or(0, |n| n + 1);
        let (_, opened) =
            closed.filter(|(close, opened)| *close >= this_line && *opened < this_line)?;
        let (_, indent) = indent_at(opened);
        return Some(if python && text[this_line..].trim_end().ends_with(':') {
            format!("{indent}{step}")
        } else {
            indent
        });
    };
    // In R a `{` opens a block, which the editor indents itself.
    if !python && text[at..].starts_with('{') {
        return None;
    }
    let (line_start, indent) = indent_at(at);
    let rest = text[at + 1..].split('\n').next().unwrap_or("");
    let first = rest.trim_start();
    if first.is_empty() || first.starts_with('#') {
        return Some(format!("{indent}{step}"));
    }
    let column = text[line_start..at].chars().count() + 1 + (rest.len() - first.len());
    Some(" ".repeat(column))
}

/// Enter in an R or Python editor whose caret is inside an open call or
/// list: a new line at [`continuation`]'s indent. Whether it did.
pub(crate) fn enter_in_brackets(
    editor: &Entity<EditorState>,
    action: &Enter,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if action.shift || action.secondary {
        return false;
    }
    let indent = {
        let e = editor.read(cx);
        let python = match e.language_name().as_ref() {
            "python" => true,
            "r" => false,
            _ => return false,
        };
        if e.completion_menu_state().open || !e.selected_range().is_empty() {
            return false;
        }
        let value = e.value();
        let cursor = e.cursor().min(value.len());
        match continuation(&value[..cursor], python) {
            Some(indent) => indent,
            None => return false,
        }
    };
    editor.update(cx, |e, cx| e.insert(format!("\n{indent}"), window, cx));
    true
}

#[cfg(test)]
mod tests {
    use super::continuation;

    #[test]
    fn new_lines_in_calls_line_up() {
        // Under the first argument.
        assert_eq!(
            continuation("x <- c(1, 2,", false).as_deref(),
            Some("       ")
        );
        assert_eq!(
            continuation("    df = pd.read_csv(path,", true).as_deref(),
            Some("                     ")
        );
        // One step in when the bracket ends its line.
        assert_eq!(continuation("  f(", false).as_deref(), Some("    "));
        assert_eq!(
            continuation("plot(  # axes\n", true).as_deref(),
            Some("    ")
        );
        // Further lines follow the first argument's line, not the last.
        assert_eq!(continuation("f(a,\n  b,", false).as_deref(), Some("  "));
        // Python dicts too; R blocks are the editor's.
        assert_eq!(
            continuation("d = {\"a\": 1,", true).as_deref(),
            Some("     ")
        );
        assert_eq!(continuation("f <- function(x) {", false), None);
        // Closed calls, strings and comments are not open brackets.
        assert_eq!(continuation("f(a)\n", false), None);
        assert_eq!(continuation("s = \"f(\"", true), None);
        assert_eq!(continuation("# f(", true), None);
        assert_eq!(continuation("x = \"\"\"f(", true), None);
        assert_eq!(continuation("f(\"a)\",", true).as_deref(), Some("  "));
        // After a call over several lines closes, back to its statement.
        assert_eq!(
            continuation("  d = dict(a=1,\n           b=2)", true).as_deref(),
            Some("  ")
        );
        assert_eq!(
            continuation("if f(a,\n     b):", true).as_deref(),
            Some("    ")
        );
        let later = "f(a,\n  b)\nif x:\n    y = 1";
        assert_eq!(continuation(later, true), None);
    }
}
