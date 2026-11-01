//! What milestone 1 shows for each output: text. Plots and HTML get a note
//! saying which milestone draws them.

use serde_json::Value;

use crate::notebook::multiline;

#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    /// stdout, or a result's `text/plain`.
    Text(String),
    /// stderr.
    Warning(String),
    Error(String),
    /// Something this milestone can't draw yet.
    Note(String),
}

/// `output` is an nbformat 4 output object, as the daemon's output rules
/// build it.
pub fn show(output: &Value) -> Shown {
    match output["output_type"].as_str().unwrap_or("") {
        "stream" => {
            let text = strip_ansi(&multiline(&output["text"]));
            if output["name"] == "stderr" {
                Shown::Warning(text)
            } else {
                Shown::Text(text)
            }
        }
        "error" => {
            let traceback: Vec<String> = output["traceback"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(strip_ansi)
                .collect();
            if traceback.is_empty() {
                Shown::Error(format!(
                    "{}: {}",
                    output["ename"].as_str().unwrap_or("Error"),
                    output["evalue"].as_str().unwrap_or("")
                ))
            } else {
                Shown::Error(traceback.join("\n"))
            }
        }
        "execute_result" | "display_data" => {
            let data = &output["data"];
            let Some(mimes) = data.as_object() else {
                return Shown::Note("Empty output".into());
            };
            if let Some(mime) = mimes.keys().find(|m| m.starts_with("image/")) {
                Shown::Note(format!("Plot ({mime}): drawn from milestone 2"))
            } else if mimes.contains_key("text/plain") {
                Shown::Text(strip_ansi(&multiline(&data["text/plain"])))
            } else if mimes.contains_key("text/html") {
                Shown::Note("HTML output: needs the web view (milestone 4)".into())
            } else {
                let kinds: Vec<&str> = mimes.keys().map(String::as_str).collect();
                Shown::Note(format!("Output of type {}", kinds.join(", ")))
            }
        }
        other => Shown::Note(format!("Output of type {other}")),
    }
}

/// Removes terminal colour codes, which R and IPython put in errors.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // Parameters and intermediates, then one final byte in @..~.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shows_streams_results_and_errors_as_text() {
        let out = json!({"output_type": "stream", "name": "stdout", "text": "hi\n"});
        assert_eq!(show(&out), Shown::Text("hi\n".into()));
        let out = json!({"output_type": "stream", "name": "stderr", "text": "careful"});
        assert_eq!(show(&out), Shown::Warning("careful".into()));
        let out = json!({"output_type": "execute_result", "data": {"text/plain": ["[1] ", "2"]}});
        assert_eq!(show(&out), Shown::Text("[1] 2".into()));
        let out = json!({"output_type": "error", "ename": "NameError", "evalue": "x",
            "traceback": ["\u{1b}[0;31mNameError\u{1b}[0m: x"]});
        assert_eq!(show(&out), Shown::Error("NameError: x".into()));
    }

    #[test]
    fn notes_plots_and_html() {
        let out = json!({"output_type": "display_data",
            "data": {"image/png": "...", "text/plain": "<Figure>"}});
        assert!(matches!(show(&out), Shown::Note(n) if n.contains("Plot")));
        let out = json!({"output_type": "display_data", "data": {"text/html": "<b>"}});
        assert!(matches!(show(&out), Shown::Note(n) if n.contains("HTML")));
    }
}
