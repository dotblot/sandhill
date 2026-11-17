//! Code-initiated Data Explorer requests, kept separate from Console output.

use super::*;

impl Workbench {
    pub(super) fn view_from_code(&mut self, event: &Event, cx: &mut Context<Self>) {
        let title = event.content["title"].as_str().unwrap_or_default();
        let language = self.session_language(&event.session).unwrap_or("python");
        let Some(path) = variable_path(title, language) else {
            self.problem = Some("To view an expression, assign it to a variable first.".into());
            cx.notify();
            return;
        };
        let language = editor_language(Some(language)).to_string();
        if self.daemon().is_none() {
            self.problem = Some(format!(
                "Could not open {title}: not connected to the daemon."
            ));
            cx.notify();
            return;
        }
        let session = event.session.clone();
        let handle = self.window_handle;
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |w, cx| {
                    w.open_session_explorer(session, language, path, window, cx);
                })
                .ok();
            })
            .ok();
        });
    }
}

/// Only variable/part references are accepted; never evaluate an arbitrary
/// expression to open a table. The daemon walks the same paths as Variables.
fn variable_path(title: &str, language: &str) -> Option<Vec<String>> {
    let r = language.eq_ignore_ascii_case("r");
    let (root, mut rest) = name(title.trim(), r)?;
    let mut path = vec![root];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Some(path);
        }
        let field = if r {
            rest.strip_prefix('$').or_else(|| rest.strip_prefix('@'))
        } else {
            rest.strip_prefix('.')
        };
        if let Some(field) = field {
            let (part, tail) = name(field.trim_start(), r)?;
            path.push(part);
            rest = tail;
            continue;
        }
        rest = rest.strip_prefix(if r { "[[" } else { "[" })?.trim_start();
        let (part, tail) = if rest.starts_with(['\'', '"']) {
            quoted(rest)?
        } else {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if end == 0 || !r {
                return None;
            }
            let index = &rest[..end];
            let part = format!("[[{index}]]");
            (part, &rest[end..])
        };
        rest = tail.trim_start().strip_prefix(if r { "]]" } else { "]" })?;
        path.push(part);
    }
}

fn name(text: &str, r: bool) -> Option<(String, &str)> {
    if r && text.starts_with('`') {
        return quoted(text);
    }
    let first = text.chars().next()?;
    if !(first.is_alphabetic() || first == '_' || r && first == '.') {
        return None;
    }
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || r && c == '.'))
        .unwrap_or(text.len());
    Some((text[..end].to_string(), &text[end..]))
}

fn quoted(text: &str) -> Option<(String, &str)> {
    let quote = text.chars().next()?;
    let mut chars = text.char_indices().skip(1);
    let mut part = String::new();
    while let Some((ix, c)) = chars.next() {
        if c == quote {
            return Some((part, &text[ix + c.len_utf8()..]));
        }
        if c == '\\' {
            let (_, escaped) = chars.next()?;
            // Names with literal quotes/backslashes are unambiguous; do not
            // reinterpret language-specific escape sequences differently.
            if escaped != quote && escaped != '\\' {
                return None;
            }
            part.push(escaped);
        } else {
            part.push(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::variable_path;

    #[test]
    fn view_titles_resolve_to_variable_paths() {
        for (language, title, expected) in [
            ("python", " counts ", vec!["counts"]),
            ("python3", "adata.obs", vec!["adata", "obs"]),
            (
                "python",
                "adata.layers['raw counts']",
                vec!["adata", "layers", "raw counts"],
            ),
            ("R", "seu@meta.data", vec!["seu", "meta.data"]),
            ("R", "fit$tab", vec!["fit", "tab"]),
            (
                "R",
                "`my tables`[[\"gene counts\"]]",
                vec!["my tables", "gene counts"],
            ),
            ("R", "tables[[2]]", vec!["tables", "[[2]]"]),
        ] {
            assert_eq!(
                variable_path(title, language),
                Some(expected.into_iter().map(str::to_string).collect()),
                "{language}: {title}"
            );
        }
        for (language, title) in [
            ("python", ""),
            ("python", "head(df)"),
            ("python", "df + other"),
            ("python", "df."),
            ("python", "df[key]"),
            ("python", "tables[2]"),
            ("python", "df['a']()"),
            ("R", "head(df)"),
            ("R", "df[1,]"),
            ("R", "fit$"),
        ] {
            assert_eq!(variable_path(title, language), None, "{language}: {title}");
        }
    }
}
