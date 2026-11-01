//! Splits an R or Python script into the statements ⌘↵ runs, as RStudio and
//! VS Code do: a call spread over several lines, a pipe chain, a function or
//! a Python block is one statement, wherever in it the cursor is. The same
//! rules as the Tauri app's `app/src/model/statements.ts`.

/// Lines of one statement, counted from 0, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statement {
    pub from: usize,
    pub to: usize,
}

struct LineInfo {
    /// Has code outside comments, or is inside a string.
    code: bool,
    /// The line's code without its comment, trimmed.
    text: String,
    indent: usize,
    /// Still inside brackets or a string at the end of the line.
    open: bool,
}

/// R operators that, ending a line, continue the expression on the next.
fn r_continues(text: &str) -> bool {
    if text.ends_with("|>") {
        return true;
    }
    // %>%, %in% and other %op%.
    if text.ends_with('%')
        && let Some(start) = text[..text.len() - 1].rfind('%')
        && !text[start + 1..text.len() - 1].contains(char::is_whitespace)
    {
        return true;
    }
    text.ends_with(|c: char| "-+*/^,|&~=<>!:".contains(c))
}

/// Python lines that keep a block going at its own indent.
fn python_block_keeps(text: &str) -> bool {
    ["else", "elif", "except", "finally"].iter().any(|word| {
        text.strip_prefix(word)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    })
}

pub fn statements(text: &str, language: &str) -> Vec<Statement> {
    let python = language.to_lowercase().starts_with("python");
    let source: Vec<&str> = text.split('\n').collect();
    let lines = scan(&source, python);
    let continues = |n: usize| -> bool {
        let line = &lines[n];
        line.open
            || if python {
                line.text.ends_with('\\') || line.text.starts_with('@')
            } else {
                r_continues(&line.text)
            }
    };
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].code {
            i += 1;
            continue;
        }
        let from = i;
        while i + 1 < lines.len() && continues(i) {
            i += 1;
        }
        // A Python block runs on while lines are indented further.
        if python && lines[i].text.ends_with(':') {
            let base = lines[from].indent;
            let mut last = i;
            let mut n = i + 1;
            while n < lines.len() {
                let line = &lines[n];
                if !line.code {
                    n += 1;
                    continue;
                }
                if line.indent > base || (line.indent == base && python_block_keeps(&line.text)) {
                    last = n;
                    while last + 1 < lines.len() && continues(last) {
                        last += 1;
                    }
                    n = last + 1;
                } else {
                    break;
                }
            }
            i = last;
        }
        result.push(Statement { from, to: i });
        i += 1;
    }
    result
}

#[cfg(test)]
/// The statement on `line` (from 0), or none on a blank or comment line.
pub fn statement_at(text: &str, language: &str, line: usize) -> Option<Statement> {
    statements(text, language)
        .into_iter()
        .find(|s| s.from <= line && line <= s.to)
}

/// Tracks brackets, strings and comments line by line.
fn scan(source: &[&str], python: bool) -> Vec<LineInfo> {
    let mut depth = 0usize;
    // The quote that opened the string we are in, if any.
    let mut quote = String::new();
    source
        .iter()
        .map(|line| {
            let starts_in_string = !quote.is_empty();
            let chars: Vec<char> = line.chars().collect();
            let starts_with = |at: usize, s: &str| {
                let s: Vec<char> = s.chars().collect();
                chars.len() >= at + s.len() && chars[at..at + s.len()] == s[..]
            };
            let mut end = chars.len();
            let mut i = 0;
            while i < chars.len() {
                let ch = chars[i];
                if !quote.is_empty() {
                    if ch == '\\' {
                        i += 1;
                    } else if starts_with(i, &quote) {
                        i += quote.chars().count() - 1;
                        quote.clear();
                    }
                    i += 1;
                    continue;
                }
                if ch == '#' {
                    end = i;
                    break;
                }
                if ch == '"' || ch == '\'' || (!python && ch == '`') {
                    let triple: String = std::iter::repeat_n(ch, 3).collect();
                    if python && starts_with(i, &triple) {
                        quote = triple;
                        i += 2;
                    } else {
                        quote = ch.to_string();
                    }
                } else if "([{".contains(ch) {
                    depth += 1;
                } else if ")]}".contains(ch) {
                    depth = depth.saturating_sub(1);
                }
                i += 1;
            }
            let text: String = chars[..end].iter().collect::<String>().trim().to_string();
            LineInfo {
                code: starts_in_string || !text.is_empty(),
                indent: line.len() - line.trim_start().len(),
                open: depth > 0 || !quote.is_empty(),
                text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Statement, statement_at, statements};

    /// As 1-based line pairs, like the Tauri app's tests.
    fn lines(text: &str, language: &str) -> Vec<[usize; 2]> {
        statements(text, language)
            .iter()
            .map(|s| [s.from + 1, s.to + 1])
            .collect()
    }

    #[test]
    fn keeps_r_calls_pipes_and_functions_together() {
        let script = [
            "library(Seurat)",
            "",
            "# Load the counts",
            "pbmc <- Read10X(",
            "  data.dir = 'data/'",
            ")",
            "pbmc |>",
            "  NormalizeData() %>%",
            "  FindVariableFeatures()",
            "p <- ggplot(df) +",
            "  geom_point()",
            "f <- function(x) {",
            "  x + 1 # }",
            "}",
            "msg <- \"a # not a comment (",
            "still the string\"",
            "x <- 1; y <- 2",
        ]
        .join("\n");
        assert_eq!(
            lines(&script, "r"),
            [
                [1, 1],
                [4, 6],
                [7, 9],
                [10, 11],
                [12, 14],
                [15, 16],
                [17, 17]
            ]
        );
    }

    #[test]
    fn finds_the_statement_around_the_cursor() {
        let script = "x <- c(1,\n  2,\n  3)\n\n# done\nprint(x)";
        assert_eq!(
            statement_at(script, "r", 1),
            Some(Statement { from: 0, to: 2 })
        );
        assert_eq!(statement_at(script, "r", 3), None);
        assert_eq!(statement_at(script, "r", 4), None);
        assert_eq!(
            statement_at(script, "r", 5),
            Some(Statement { from: 5, to: 5 })
        );
    }

    #[test]
    fn keeps_python_blocks_decorators_and_brackets_together() {
        let script = [
            "import scanpy as sc",
            "adata = sc.read_h5ad(",
            "    \"pbmc.h5ad\")",
            "for gene in genes:",
            "    print(gene)",
            "",
            "    print('still in the loop')",
            "else:",
            "    pass",
            "@cache",
            "def f(x):",
            "    \"\"\"Doc",
            "    string: with a colon",
            "    \"\"\"",
            "    return x",
            "",
            "total = 1 + \\",
            "    2",
            "sc.pl.umap(adata)  # plot: colour",
        ]
        .join("\n");
        assert_eq!(
            lines(&script, "python"),
            [[1, 1], [2, 3], [4, 9], [10, 15], [17, 18], [19, 19]]
        );
    }
}
