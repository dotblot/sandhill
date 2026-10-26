//! Web pages R shows, for the app's Viewer. htmlwidgets (DT, plotly,
//! leaflet) printed in R call `getOption("viewer")` with the page they
//! saved. Ark's own viewer waits for Positron, which isn't there, so the
//! kernel hangs; IRkernel has none. Each R session gets a viewer, when it
//! starts, that prints a short escape sequence instead, which works the
//! same in every R kernel. The daemon takes it out of the output and puts
//! in its place a display_data of Sandhill's own type naming the page, with
//! the page itself (made self-contained) as `text/html`. A notebook shows
//! that HTML under its cell; the app's Viewer shows a Console's or a
//! script's page.

use std::path::Path;

use serde_json::{Value, json};

/// The display_data type that names a page: `{path}` for a file on the
/// daemon's machine, or `{url}` for a web address, with its `title`.
pub const MIME: &str = "application/vnd.sandhill.page+json";

const START: &str = "\x1b]sandhill-page;";
const END: char = '\x07';

/// The viewer for R sessions, set as they start (see `startup`): it prints
/// the page's path, or address, between `START` and `END`. Shiny's apps and
/// `browseURL()` go the same way, and the daemon sends an app's address to
/// the Viewer and any other web page to the browser.
pub const R: &str = r#"tryCatch(local({
  page <- function(target) {
    if (!is.character(target) || length(target) != 1) stop("`url` must be a string.")
    target <- sub("^file://", "", target)
    if (!grepl("^https?://", target)) target <- normalizePath(target, mustWork = FALSE)
    show <- getOption("jupyter.base_display_func")
    if (is.function(show) && grepl("^https?://", target)) {
      # IRkernel sends what is printed once the code is done, and an app's never is.
      show(list("application/vnd.sandhill.page+json" = list(url = target), "text/plain" = target), NULL)
      return(invisible())
    }
    flush(stdout())
    cat(paste0("\033]sandhill-page;", utils::URLencode(enc2utf8(target), reserved = TRUE), "\a"))
    flush(stdout())
    invisible()
  }
  options(
    viewer = function(url, height = NULL, ...) page(url),
    shiny.launch.browser = function(url) page(url),
    browser = function(url, ...) page(url)
  )
}), error = function(e) invisible())
"#;

/// What a stream's text names, with the text left once the names are taken
/// out; `None` when it names no page.
pub fn take(text: &str) -> Option<(String, Vec<String>)> {
    if !text.contains(START) {
        return None;
    }
    let mut left = String::new();
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(START) {
        left.push_str(&rest[..at]);
        let after = &rest[at + START.len()..];
        let Some(end) = after.find(END) else {
            // Cut off: leave it as it came.
            left.push_str(&rest[at..]);
            rest = "";
            break;
        };
        targets.push(decode(&after[..end]));
        rest = &after[end + END.len_utf8()..];
    }
    left.push_str(rest);
    Some((left, targets))
}

/// The display_data content for a page R named: an app on this machine
/// goes to the Viewer, and another web page to the browser (`browser`).
pub fn display(target: &str) -> Value {
    if target.starts_with("http://") || target.starts_with("https://") {
        let (page, text) = if is_local(target) {
            (
                json!({ "url": target, "title": target }),
                format!("{target} (in the Viewer)"),
            )
        } else {
            (
                json!({ "url": target, "title": target, "browser": true }),
                format!("Opening {target} in the browser"),
            )
        };
        return json!({
            "data": { MIME: page, "text/plain": text },
            "metadata": {},
            "transient": {},
        });
    }
    let path = Path::new(target);
    let file = if path.is_dir() {
        path.join("index.html")
    } else {
        path.to_path_buf()
    };
    match crate::inline::page(&file) {
        Ok(html) => {
            let title = title(&html);
            json!({
                "data": {
                    MIME: { "path": file.to_string_lossy(), "title": title },
                    "text/html": html,
                    "text/plain": format!("{title} (in the Viewer)"),
                },
                "metadata": { "text/html": { "isolated": true } },
                "transient": {},
            })
        }
        Err(err) => json!({
            "data": {
                "text/plain": format!("Could not show {}: {err}", file.display()),
            },
            "metadata": {},
            "transient": {},
        }),
    }
}

/// Names a web page a kernel showed itself (IRkernel's and Ark's
/// htmlwidgets, a Python plotly figure), so the app can say what it is: the
/// output gets Sandhill's page type with the page's title, when it has one.
/// Pages R's viewer named have it already, except an address IRkernel sent,
/// which gets the rest of what [`display`] gives.
pub fn name(mut content: Value) -> Value {
    let data = &content["data"];
    // An address IRkernel's viewer sent as it is (see `R`).
    if let Some(url) = data[MIME]["url"].as_str()
        && data[MIME].get("title").is_none()
    {
        let mut shown = display(url);
        shown["transient"] = content["transient"].take();
        return shown;
    }
    if data.get(MIME).is_some() {
        return content;
    }
    let html = match &data["text/html"] {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => return content,
    };
    let title = match labelled(&data["text/plain"]) {
        Some(name) => Some(name),
        None if crate::outputs::is_page(data, &content["metadata"]) => named(&html),
        None => None,
    };
    if let Some(title) = title {
        content["data"][MIME] = json!({ "title": title });
    }
    content
}

/// The kind of htmlwidget Ark says it shows: `<datatables HTML widget>`.
fn labelled(plain: &Value) -> Option<String> {
    let plain = match plain {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => return None,
    };
    let name = plain
        .trim()
        .strip_prefix('<')?
        .strip_suffix(" HTML widget>")?;
    (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_string())
}

/// Whether a web address is on this machine (`localhost`, `127.0.0.1`), as
/// an app's is, for the Viewer. `localPort` in `app/src/model/pages.ts`
/// says the same.
pub fn is_local(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let name = match host.rsplit_once(':') {
        Some((name, port)) if !name.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]")
}

/// A page's `<title>`, else what it holds, else "Page".
fn title(html: &str) -> String {
    named(html).unwrap_or_else(|| "Page".into())
}

/// A page's `<title>`, else the kind of htmlwidget it holds (`datatables`,
/// `plotly`), else whether it is a plotly figure.
fn named(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    if let Some(start) = lower.find("<title>") {
        let text = &html[start + 7..];
        if let Some(end) = text.to_ascii_lowercase().find("</title>") {
            let title = text[..end].trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    // `<div class="datatables html-widget ..."`
    if let Some(at) = lower.find(" html-widget") {
        let before = &html[..at];
        if let Some(quote) = before.rfind(['"', '\'']) {
            let name = before[quote + 1..]
                .split_whitespace()
                .last()
                .unwrap_or_default();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    lower.contains("plotly-graph-div").then(|| "plotly".into())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_taken_out_of_the_output() {
        assert_eq!(take("plain\n"), None);
        let text =
            "before\n\x1b]sandhill-page;%2Ftmp%2FRtmp%20x%2Fviewhtml1%2Findex.html\x07after\n";
        assert_eq!(
            take(text),
            Some((
                "before\nafter\n".to_string(),
                vec!["/tmp/Rtmp x/viewhtml1/index.html".to_string()]
            ))
        );
        // Cut off before its end: left alone.
        let cut = "\x1b]sandhill-page;%2Ftmp";
        assert_eq!(take(cut), Some((cut.to_string(), vec![])));
    }

    #[test]
    fn a_widget_page_becomes_an_output_with_its_files_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("viewhtml1");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/w.js"), "var w = 1;").unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<html><head><script src=\"lib/w.js\"></script></head><body><div id=\"htmlwidget-1\" class=\"datatables html-widget html-fill-item\"></div></body></html>",
        )
        .unwrap();
        let shown = display(&dir.to_string_lossy());
        let page = &shown["data"][MIME];
        assert_eq!(page["title"], "datatables");
        assert!(
            page["path"]
                .as_str()
                .unwrap()
                .ends_with("viewhtml1/index.html")
        );
        assert!(
            shown["data"]["text/html"]
                .as_str()
                .unwrap()
                .contains("<script>var w = 1;</script>")
        );
        assert_eq!(shown["metadata"]["text/html"]["isolated"], true);

        let app = display("http://127.0.0.1:7123");
        assert_eq!(app["data"][MIME]["url"], "http://127.0.0.1:7123");
        assert_eq!(app["data"][MIME].get("browser"), None);
        let web = display("https://bioconductor.org/packages/DESeq2");
        assert_eq!(web["data"][MIME]["browser"], true);
        assert!(
            web["data"]["text/plain"]
                .as_str()
                .unwrap()
                .starts_with("Opening")
        );
        assert!(
            display("/nowhere/index.html")["data"]["text/plain"]
                .as_str()
                .unwrap()
                .starts_with("Could not show")
        );
    }

    #[test]
    fn apps_are_on_this_machine() {
        assert!(is_local("http://127.0.0.1:7123"));
        assert!(is_local("http://localhost:8501/?embed=true"));
        assert!(is_local("http://0.0.0.0:8050/"));
        assert!(is_local("http://[::1]:4321/"));
        assert!(!is_local("https://localhost:8443/"));
        assert!(!is_local("http://localhost.example.org/"));
        assert!(!is_local("http://10.0.0.5:8501"));
    }

    #[test]
    fn titles_come_from_the_page() {
        assert_eq!(title("<html><head><title> QC report </title>"), "QC report");
        assert_eq!(title("<div class=\"plotly html-widget\">"), "plotly");
        assert_eq!(
            title(
                "<div id=\"f\" class=\"plotly-graph-div\"></div><script>Plotly.newPlot()</script>"
            ),
            "plotly"
        );
        assert_eq!(title("<p>hi</p>"), "Page");
    }

    #[test]
    fn pages_kernels_show_themselves_get_their_names() {
        let widget = json!({
            "data": { "text/html": ["<div class=\"leaflet html-widget\">", "</div>"] },
            "metadata": { "text/html": { "isolated": true } },
        });
        assert_eq!(name(widget)["data"][MIME]["title"], "leaflet");
        // A table isn't a page; a page with no name keeps the output as it was.
        let table = json!({ "data": { "text/html": "<table></table>" }, "metadata": {} });
        assert_eq!(name(table.clone()), table);
        let nameless = json!({ "data": { "text/html": "<script>1</script>" }, "metadata": {} });
        assert_eq!(name(nameless.clone()), nameless);
        // Ark's, named by its label.
        let ark = json!({
            "data": { "text/html": "<iframe></iframe>", "text/plain": "<datatables HTML widget>" },
            "metadata": {},
        });
        assert_eq!(name(ark)["data"][MIME]["title"], "datatables");
        let spaced =
            json!({ "data": { "text/html": "<p>x</p>", "text/plain": "<a b HTML widget>" } });
        assert_eq!(name(spaced.clone()), spaced);
        // An app's address, as IRkernel sends it.
        let app = json!({ "data": { MIME: { "url": "http://127.0.0.1:7123" } }, "metadata": {} });
        let app = name(app);
        assert_eq!(app["data"][MIME]["title"], "http://127.0.0.1:7123");
        assert_eq!(
            app["data"]["text/plain"],
            "http://127.0.0.1:7123 (in the Viewer)"
        );
    }
}
