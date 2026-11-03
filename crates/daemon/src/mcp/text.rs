//! Turning kernel outputs into what an agent reads: text, with ANSI colour
//! codes removed, and plots as images.

use sandhill_proto::BLOB_KEY;
use serde_json::Value;

/// Text returned for one tool call; longer output keeps its start and end.
pub const MAX_TEXT: usize = 30_000;
/// Plots returned for one tool call.
pub const MAX_IMAGES: usize = 4;

/// What a run printed, as an agent sees it.
#[derive(Debug, Default)]
pub struct Rendered {
    pub text: String,
    /// `(mime type, base64 data or blob reference)`.
    pub images: Vec<(String, Value)>,
}

impl Rendered {
    /// Adds one nbformat output (`output_type` and its fields).
    pub fn add_output(&mut self, output: &Value) {
        match output["output_type"].as_str().unwrap_or("") {
            "stream" => self.text.push_str(&joined(&output["text"])),
            "error" => self.add_error(output),
            _ => self.add_data(&output["data"]),
        }
    }

    /// Adds one journaled kernel message (`msg_type` and Jupyter content).
    pub fn add_message(&mut self, msg_type: &str, content: &Value) {
        match msg_type {
            "stream" => self.text.push_str(&joined(&content["text"])),
            "error" => self.add_error(content),
            "execute_result" | "display_data" | "update_display_data" => {
                self.add_data(&content["data"])
            }
            _ => {}
        }
    }

    fn add_error(&mut self, error: &Value) {
        let traceback: Vec<String> = error["traceback"]
            .as_array()
            .map(|lines| lines.iter().map(joined).collect())
            .unwrap_or_default();
        self.newline();
        if traceback.is_empty() {
            self.text.push_str(&format!(
                "{}: {}\n",
                error["ename"].as_str().unwrap_or("Error"),
                error["evalue"].as_str().unwrap_or("")
            ));
        } else {
            self.text.push_str(&traceback.join("\n"));
            self.text.push('\n');
        }
    }

    fn add_data(&mut self, data: &Value) {
        let Some(data) = data.as_object() else {
            return;
        };
        for mime in ["image/png", "image/jpeg"] {
            if let Some(value) = data.get(mime) {
                self.images.push((mime.to_string(), value.clone()));
                self.newline();
                self.text
                    .push_str(&format!("[plot {}]\n", self.images.len()));
                return;
            }
        }
        let text = ["text/plain", "text/markdown", "text/latex"]
            .iter()
            .find_map(|mime| data.get(*mime));
        self.newline();
        match text {
            Some(value) if value.get(BLOB_KEY).is_some() => {
                let size = value["size"].as_u64().unwrap_or(0);
                self.text
                    .push_str(&format!("[a long text output, {}]\n", bytes(size)));
            }
            Some(value) => {
                self.text.push_str(&joined(value));
                self.newline();
            }
            None => {
                let kinds: Vec<&str> = data.keys().map(String::as_str).collect();
                self.text
                    .push_str(&format!("[{} output]\n", kinds.join(", ")));
            }
        }
    }

    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    /// The text without colour codes, shortened to [`MAX_TEXT`].
    pub fn finished_text(&self) -> String {
        shorten(&strip_ansi(&self.text), MAX_TEXT)
    }
}

/// nbformat text is a string or a list of lines.
fn joined(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// Removes ANSI escape sequences (colours in R and Python tracebacks).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // Parameters and intermediates, then one final byte.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                // An OSC string, ended by BEL or ESC \.
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// An HTML page (R's help) as plain text: block elements on lines of
/// their own, headings marked with `#`, tags dropped, entities read.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut in_pre = false;
    let mut skip_until: Option<&str> = None;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            if skip_until.is_none() {
                push_text(&mut out, rest, in_pre);
            }
            break;
        };
        if skip_until.is_none() {
            push_text(&mut out, &rest[..lt], in_pre);
        }
        let after = &rest[lt..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let tag = after[1..gt].trim().to_ascii_lowercase();
        rest = &after[gt + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_string();
        if let Some(end) = skip_until {
            if tag == end {
                skip_until = None;
            }
            continue;
        }
        let closing = tag.starts_with('/');
        match name.as_str() {
            "script" | "style" | "head" if !closing => {
                skip_until = Some(match name.as_str() {
                    "script" => "/script",
                    "style" => "/style",
                    _ => "/head",
                });
            }
            "pre" => {
                in_pre = !closing;
                out.push('\n');
            }
            "h1" | "h2" | "h3" | "h4" if !closing => {
                out.push_str("\n\n## ");
            }
            "br" => out.push('\n'),
            "li" if !closing => out.push_str("\n- "),
            "p" | "div" | "tr" | "table" | "dl" | "dt" | "dd" | "ul" | "ol" | "h1" | "h2"
            | "h3" | "h4" => out.push('\n'),
            "td" | "th" if !closing => out.push(' '),
            _ => {}
        }
    }
    // At most one blank line in a row, and no spaces at line ends.
    let mut text = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        text.push_str(line);
        text.push('\n');
    }
    text.trim().to_string()
}

/// Text between tags: entities read; outside `<pre>`, runs of spaces and
/// line breaks made one space.
fn push_text(out: &mut String, text: &str, in_pre: bool) {
    let text = entities(text);
    if in_pre {
        out.push_str(&text);
        return;
    }
    let mut last_space = out.ends_with([' ', '\n']);
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
    }
}

fn entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let semi = rest[..rest.len().min(10)].find(';');
        let decoded = semi.and_then(|semi| {
            let name = &rest[1..semi];
            let c = match name {
                "lt" => '<',
                "gt" => '>',
                "amp" => '&',
                "quot" => '"',
                "apos" | "#39" => '\'',
                "nbsp" => ' ',
                _ => {
                    let n = name.strip_prefix('#')?;
                    let n = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(n)?
                }
            };
            Some((c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Keeps the start and the end of a long text.
pub fn shorten(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let skipped = text[head..tail].lines().count();
    format!(
        "{}\n[… {skipped} lines left out …]\n{}",
        &text[..head],
        &text[tail..]
    )
}

/// "1.2 MB" (powers of 1024, as R's object.size reports).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// "3.1 s", "2 min 5 s".
pub fn duration_ms(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {} s", ms / 60_000, ms % 60_000 / 1000)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_an_r_help_page_as_text() {
        let html = "<h2>Fitting Linear Models</h2>\n<h3>Description</h3>\n<p><code>lm</code> is used to fit\n  linear models &amp; more.</p>\n<h3>Usage</h3>\n<pre>lm(formula, data)\n  # two lines</pre><ul><li>one</li><li>two</li></ul><script>x()</script>";
        let text = super::html_to_text(html);
        assert_eq!(
            text,
            "## Fitting Linear Models\n\n## Description\n\nlm is used to fit linear models & more.\n\n## Usage\n\nlm(formula, data)\n  # two lines\n\n- one\n- two"
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn outputs_become_text_and_plots() {
        let mut out = Rendered::default();
        out.add_output(&json!({"output_type": "stream", "name": "stdout", "text": ["a\n", "b"]}));
        out.add_output(&json!({"output_type": "execute_result", "data": {"text/plain": "[1] 2"}}));
        out.add_output(&json!({"output_type": "display_data", "data": {"image/png": "iVBOR", "text/plain": "<Figure>"}}));
        out.add_output(
            &json!({"output_type": "error", "ename": "ValueError", "evalue": "bad",
            "traceback": ["\u{1b}[0;31mValueError\u{1b}[0m: bad"]}),
        );
        assert_eq!(
            out.finished_text(),
            "a\nb\n[1] 2\n[plot 1]\nValueError: bad\n"
        );
        assert_eq!(out.images, vec![("image/png".to_string(), json!("iVBOR"))]);
    }

    #[test]
    fn long_text_keeps_both_ends() {
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let short = shorten(&text, 200);
        assert!(short.starts_with("line 0\n"));
        assert!(short.ends_with("line 999\n"));
        assert!(short.contains("lines left out"));
    }

    #[test]
    fn sizes_read_like_r() {
        assert_eq!(bytes(28), "28 B");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(312_000_000), "297.5 MB");
    }
}
