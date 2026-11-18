//! Documentation shown as the AI pane shows replies: prose in the interface
//! font, code in the code font with its language's colours. Python
//! docstrings are plain text (numpydoc, Google style, reStructuredText), so
//! their code (doctests, `::` literal blocks) is put in fenced blocks and
//! the rest made into markdown. R help pages are HTML whose code sits in
//! `<pre>` blocks, which the HTML view shows as plain text, so they are cut
//! out and shown as R code.

/// A plain-text docstring as markdown, its code in ```` ```python ```` blocks.
pub(crate) fn docstring_markdown(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut ix = 0;
    // A paragraph ending in `::` introduces an indented literal block.
    let mut literal_next = false;
    while ix < lines.len() {
        let line = lines[ix];
        let trimmed = line.trim();
        // numpydoc headings: a line underlined with dashes or equals signs.
        if !trimmed.is_empty()
            && let Some(under) = lines.get(ix + 1).map(|l| l.trim())
            && under.len() >= 3
            && (under.chars().all(|c| c == '-') || under.chars().all(|c| c == '='))
        {
            out.push(String::new());
            out.push(format!("**{}**", escape(trimmed)));
            out.push(String::new());
            ix += 2;
            literal_next = false;
            continue;
        }
        // Doctests and their output, up to a blank line.
        if trimmed.starts_with(">>>") {
            let mut code = Vec::new();
            while ix < lines.len() && !lines[ix].trim().is_empty() {
                code.push(lines[ix]);
                ix += 1;
            }
            push_code(&mut out, &code);
            literal_next = false;
            continue;
        }
        if trimmed.is_empty() {
            out.push(String::new());
            ix += 1;
            continue;
        }
        let indent = indent_of(line);
        if literal_next && indent > 0 {
            let mut code = Vec::new();
            while ix < lines.len()
                && (lines[ix].trim().is_empty() || indent_of(lines[ix]) >= indent)
            {
                code.push(lines[ix]);
                ix += 1;
            }
            while code.last().is_some_and(|l| l.trim().is_empty()) {
                code.pop();
            }
            push_code(&mut out, &code);
            literal_next = false;
            continue;
        }
        literal_next = trimmed.ends_with("::");
        let prose = trimmed.strip_suffix("::").map_or(trimmed.to_string(), |t| {
            // "Example::" reads "Example:"; a lone "::" says nothing.
            if t.trim().is_empty() {
                String::new()
            } else {
                format!("{t}:")
            }
        });
        if indent == 0 && is_entry(&prose) {
            // `name : type` in a parameter list starts a line of its own.
            let (name, kind) = prose.split_once(" : ").unwrap_or((&prose, ""));
            out.push(String::new());
            out.push(if kind.is_empty() {
                format!("**{}**", escape(name))
            } else {
                format!("**{}** : {}", escape(name), inline(kind))
            });
            out.push(String::new());
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            out.push(format!("- {}", inline(&prose[2..])));
        } else {
            out.push(inline(&prose));
        }
        ix += 1;
    }
    let mut markdown = out.join("\n");
    while markdown.contains("\n\n\n") {
        markdown = markdown.replace("\n\n\n", "\n\n");
    }
    markdown.trim().to_string()
}

/// A parameter entry: `name : type`, or a bare name followed by an
/// indented description.
fn is_entry(line: &str) -> bool {
    match line.split_once(" : ") {
        Some((name, _)) => !name.is_empty() && !name.contains("  ") && name.len() < 60,
        None => false,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn push_code(out: &mut Vec<String>, lines: &[&str]) {
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent_of(l))
        .min()
        .unwrap_or(0);
    let code: Vec<&str> = lines
        .iter()
        .map(|l| {
            if l.len() >= common {
                &l[common..]
            } else {
                l.trim()
            }
        })
        .collect();
    let fence = fence_for(&code.join("\n"));
    out.push(String::new());
    out.push(format!("{fence}python"));
    out.extend(code.iter().map(|l| l.to_string()));
    out.push(fence);
    out.push(String::new());
}

/// A fence longer than any run of backticks inside the code.
pub(crate) fn fence_for(code: &str) -> String {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// Prose with markdown's own characters escaped, keeping `code` (and
/// reStructuredText's ``code``) as code.
fn inline(text: &str) -> String {
    let text = text.replace("``", "`");
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for ch in text.chars() {
        if ch == '`' {
            in_code = !in_code;
            out.push(ch);
        } else if in_code {
            out.push(ch);
        } else {
            push_escaped(&mut out, ch);
        }
    }
    if in_code {
        // An unclosed backtick is taken literally.
        return escape(&text);
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        push_escaped(&mut out, ch);
    }
    out
}

fn push_escaped(out: &mut String, ch: char) {
    if matches!(
        ch,
        '\\' | '*' | '_' | '#' | '<' | '>' | '[' | ']' | '|' | '`'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

/// A piece of an R help page: HTML, or the code of a `<pre>` block.
#[derive(Debug, PartialEq)]
pub(crate) enum HelpPart {
    Html(String),
    Code(String),
}

/// Cuts an R help page's `<pre>` blocks out of its HTML, as plain code.
pub(crate) fn split_pre(html: &str) -> Vec<HelpPart> {
    let lower = html.to_ascii_lowercase();
    let mut parts = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<pre").map(|i| at + i) {
        let Some(open_end) = lower[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        let Some(close) = lower[open_end..].find("</pre>").map(|i| open_end + i) else {
            break;
        };
        if start > at {
            parts.push(HelpPart::Html(html[at..start].to_string()));
        }
        let code = unescape(&strip_tags(&html[open_end..close]));
        let code = code
            .strip_prefix('\n')
            .unwrap_or(&code)
            .trim_end()
            .to_string();
        if !code.is_empty() {
            parts.push(HelpPart::Code(code));
        }
        at = close + "</pre>".len();
    }
    if at < html.len() && !html[at..].trim().is_empty() {
        parts.push(HelpPart::Html(html[at..].to_string()));
    }
    parts
}

/// R code as a fenced markdown block.
pub(crate) fn r_code_markdown(code: &str) -> String {
    let fence = fence_for(code);
    format!("{fence}r\n{code}\n{fence}")
}

/// Python code as a fenced markdown block.
pub(crate) fn python_code_markdown(code: &str) -> String {
    let fence = fence_for(code);
    format!("{fence}python\n{code}\n{fence}")
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::{HelpPart, docstring_markdown, split_pre};

    #[test]
    fn numpydoc_becomes_headings_entries_prose_and_python_code() {
        let doc = "Compute the *mean*.\n\nParameters\n----------\na : array_like\n    Numbers whose mean is desired; ``None`` is not.\n\nExamples\n--------\n>>> a = np.array([[1, 2], [3, 4]])\n>>> np.mean(a)\n2.5\n";
        let md = docstring_markdown(doc);
        assert!(md.starts_with("Compute the \\*mean\\*."), "{md}");
        assert!(md.contains("**Parameters**"), "{md}");
        assert!(md.contains("**a** : array\\_like"), "{md}");
        assert!(
            md.contains("Numbers whose mean is desired; `None` is not."),
            "{md}"
        );
        assert!(
            md.contains("```python\n>>> a = np.array([[1, 2], [3, 4]])\n>>> np.mean(a)\n2.5\n```"),
            "{md}"
        );
    }

    #[test]
    fn literal_blocks_after_double_colons_are_code() {
        let md =
            docstring_markdown("Use it like this::\n\n    x = f(1)\n    y = f(2)\n\nThen more.");
        assert!(
            md.contains("Use it like this:\n\n```python\nx = f(1)\ny = f(2)\n```"),
            "{md}"
        );
        assert!(md.ends_with("Then more."), "{md}");
    }

    #[test]
    fn r_help_pre_blocks_come_out_as_code() {
        let html = "<h3>Usage</h3>\n<pre><code class='language-R'>mean(x, ...)\n</code></pre>\n<p>Text</p><pre><code>x &lt;- c(<a href='#'>1</a>, 2)</code></pre>";
        assert_eq!(
            split_pre(html),
            [
                HelpPart::Html("<h3>Usage</h3>\n".into()),
                HelpPart::Code("mean(x, ...)".into()),
                HelpPart::Html("\n<p>Text</p>".into()),
                HelpPart::Code("x <- c(1, 2)".into()),
            ]
        );
    }
}
