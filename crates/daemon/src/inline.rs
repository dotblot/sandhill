//! Makes a page saved with its files beside it (an htmlwidget's
//! `index.html` and `lib/` folder) into one HTML text: scripts and
//! stylesheets go inside it, and pictures and fonts become `data:` URLs.
//! That text can then be a notebook output, which outlives the R session
//! whose temporary folder held the files.

use std::path::Path;

use base64::Engine;

/// Files larger than this stay outside the page.
const MAX_FILE: u64 = 20 * 1024 * 1024;

/// `index.html` (or another page) with what it loads from its own folder
/// put inside it. What can't be read is left as it was.
pub fn page(file: &Path) -> std::io::Result<String> {
    let html = std::fs::read_to_string(file)?;
    let folder = file.parent().unwrap_or(Path::new("/"));
    Ok(inline_html(&html, folder))
}

fn inline_html(html: &str, folder: &Path) -> String {
    let mut out = String::with_capacity(html.len() * 2);
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..=end];
        let name = tag_name(tag);
        match name.as_str() {
            "script" => {
                if let Some(text) = attr(tag, "src").and_then(|src| read_text(folder, &src)) {
                    // The script's own (empty) body and its closing tag go too.
                    let after = &rest[end + 1..];
                    let close = find_ci(after, "</script")
                        .and_then(|c| after[c..].find('>').map(|e| c + e + 1));
                    if let Some(close) = close {
                        out.push_str(&rebuild(tag, &["src"]));
                        out.push_str(&text.replace("</script", "<\\/script"));
                        out.push_str("</script>");
                        rest = &after[close..];
                        continue;
                    }
                }
                out.push_str(tag);
            }
            "link" if attr(tag, "rel").is_some_and(|r| r.eq_ignore_ascii_case("stylesheet")) => {
                match attr(tag, "href").and_then(|href| {
                    let css = read_text(folder, &href)?;
                    let base = folder.join(strip_query(&href));
                    Some(inline_css(&css, base.parent().unwrap_or(folder)))
                }) {
                    Some(css) => {
                        out.push_str("<style>");
                        out.push_str(&css.replace("</style", "<\\/style"));
                        out.push_str("</style>");
                    }
                    None => out.push_str(tag),
                }
            }
            "img" => match attr(tag, "src").and_then(|src| data_url(folder, &src)) {
                Some(url) => out.push_str(&set_attr(tag, "src", &url)),
                None => out.push_str(tag),
            },
            _ => out.push_str(tag),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A stylesheet with the files its `url(...)`s name as `data:` URLs.
fn inline_css(css: &str, folder: &Path) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = find_ci(rest, "url(") {
        out.push_str(&rest[..at + 4]);
        rest = &rest[at + 4..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let target = rest[..end].trim().trim_matches(['"', '\'']);
        match data_url(folder, target) {
            Some(url) => {
                out.push('"');
                out.push_str(&url);
                out.push('"');
            }
            None => out.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A local file named relative to the page, not a web address.
fn local(folder: &Path, reference: &str) -> Option<std::path::PathBuf> {
    let reference = strip_query(reference);
    if reference.is_empty()
        || reference.starts_with('/')
        || reference.starts_with('#')
        || reference.contains("://")
        || reference.starts_with("data:")
        || reference.starts_with("//")
    {
        return None;
    }
    let path = folder.join(decode(reference));
    let size = std::fs::metadata(&path).ok()?.len();
    (size <= MAX_FILE).then_some(path)
}

fn read_text(folder: &Path, reference: &str) -> Option<String> {
    std::fs::read_to_string(local(folder, reference)?).ok()
}

fn data_url(folder: &Path, reference: &str) -> Option<String> {
    let path = local(folder, reference)?;
    let bytes = std::fs::read(&path).ok()?;
    let mime = crate::pages::content_type(&path);
    let mime = mime.split(';').next().unwrap_or(mime);
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn strip_query(reference: &str) -> &str {
    reference.split(['?', '#']).next().unwrap_or_default()
}

/// `%20` and the like, as in a file's name in a URL.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn tag_name(tag: &str) -> String {
    tag[1..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// The tag's attributes, in order: names lower-cased, values unquoted.
fn attrs(tag: &str) -> Vec<(String, Option<String>, std::ops::Range<usize>)> {
    let bytes = tag.as_bytes();
    let mut i = 1 + tag_name(tag).len();
    let mut found = Vec::new();
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !b" \t\r\n=>/".contains(&bytes[i]) {
            i += 1;
        }
        if i == start {
            break;
        }
        let name = tag[start..i].to_ascii_lowercase();
        let mut value = None;
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                let open = i + 1;
                let close = tag[open..]
                    .find(quote as char)
                    .map_or(tag.len(), |c| open + c);
                value = Some(tag[open..close.min(tag.len())].to_string());
                i = (close + 1).min(tag.len());
            } else {
                let open = i;
                while i < bytes.len() && !b" \t\r\n>".contains(&bytes[i]) {
                    i += 1;
                }
                value = Some(tag[open..i].to_string());
            }
        }
        found.push((name, value, start..i));
    }
    found
}

fn attr(tag: &str, name: &str) -> Option<String> {
    attrs(tag)
        .into_iter()
        .find(|(n, _, _)| n == name)
        .and_then(|(_, v, _)| v)
}

/// The tag without the attributes named.
fn rebuild(tag: &str, without: &[&str]) -> String {
    let mut out = tag.to_string();
    for (name, _, span) in attrs(tag).into_iter().rev() {
        if without.contains(&name.as_str()) {
            // With the space before it.
            let start = out[..span.start].trim_end().len();
            out.replace_range(start..span.end, "");
        }
    }
    out
}

fn set_attr(tag: &str, name: &str, value: &str) -> String {
    let mut out = tag.to_string();
    if let Some((_, _, span)) = attrs(tag).into_iter().find(|(n, _, _)| n == name) {
        out.replace_range(span, &format!("{name}=\"{value}\""));
    }
    out
}

fn find_ci(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, file: &str, text: &[u8]) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_widget_page_takes_in_its_scripts_styles_and_pictures() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "lib/htmlwidgets-1.6.4/htmlwidgets.js",
            b"var w = '</script>';",
        );
        write(
            root,
            "lib/dt-core/css/dt.css",
            b".sort { background: url(\"../images/sort.png\") } .x { background: url(none.png) }",
        );
        write(root, "lib/dt-core/images/sort.png", b"PNG");
        write(root, "logo.svg", b"<svg/>");
        let html = r#"<!DOCTYPE html>
<html><head>
<script src="lib/htmlwidgets-1.6.4/htmlwidgets.js"></script>
<link href="lib/dt-core/css/dt.css" rel="stylesheet" />
<script src="https://cdn.example.org/plotly.js"></script>
<script src="lib/missing.js"></script>
</head><body><img alt="logo" src='logo.svg'><div class="datatables html-widget" id="htmlwidget-1"></div>
<script type="application/json" data-for="htmlwidget-1">{"x":1}</script></body></html>"#;
        write(root, "index.html", html.as_bytes());

        let page = page(&root.join("index.html")).unwrap();
        assert!(
            page.contains("<script>var w = '<\\/script>';</script>"),
            "{page}"
        );
        assert!(
            page.contains("<style>.sort { background: url(\"data:image/png;base64,UE5H\") } .x { background: url(none.png) }</style>"),
            "{page}"
        );
        assert!(page.contains("<script src=\"https://cdn.example.org/plotly.js\"></script>"));
        assert!(page.contains("<script src=\"lib/missing.js\"></script>"));
        assert!(
            page.contains("<img alt=\"logo\" src=\"data:image/svg+xml;base64,PHN2Zy8+\">"),
            "{page}"
        );
        assert!(page.contains(
            r#"<script type="application/json" data-for="htmlwidget-1">{"x":1}</script>"#
        ));
    }
}
