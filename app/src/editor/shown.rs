//! What each output shows as: text, a plot, a web page, or a note for
//! what the trial can't draw.

use serde_json::Value;

use sandhill_daemon::outputs::{PLOTLY_MIME, is_page};
use sandhill_daemon::viewer::MIME as VIEWER_MIME;
use sandhill_proto::BLOB_KEY;

use crate::editor::notebook::multiline;

#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    /// stdout, or a result's `text/plain`.
    Text(String),
    /// stderr.
    Warning(String),
    Error(String),
    /// An image: its MIME type and its value, a string (base64, or SVG
    /// text) or a blob reference (`{"sandhill_blob": hash}`).
    Plot {
        mime: String,
        value: Value,
    },
    /// A web page (an htmlwidget, a plotly figure, a report), drawn in a
    /// web view.
    Page(Page),
    /// Something the trial can't draw.
    Note(String),
    /// A data frame, drawn as a grid.
    Table(crate::editor::html_table::Table),
}

/// A web page an output holds, as `is_page` in the daemon's output rules
/// decides.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Page {
    /// The page's HTML.
    Html(String),
    /// HTML kept in the daemon's blob store, by hash.
    Blob(String),
    /// A plotly figure kept as a blob.
    PlotlyBlob(String),
    /// An HTML file on the daemon's machine (the page R's viewer named).
    File(String),
    /// A web address on the daemon's machine (a Shiny app).
    Url(String),
}

impl Page {
    /// Tells pages apart, for keeping one web view per page.
    pub fn key(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

/// The page in an output's data, if it is one.
fn page(data: &Value, metadata: &Value) -> Option<Page> {
    if !is_page(data, metadata) {
        return None;
    }
    let named = &data[VIEWER_MIME];
    if let Some(path) = named["path"].as_str() {
        return Some(Page::File(path.to_string()));
    }
    if let Some(url) = named["url"].as_str() {
        return Some(Page::Url(url.to_string()));
    }
    match &data["text/html"] {
        Value::Null => {}
        html if html[BLOB_KEY].is_string() => {
            return Some(Page::Blob(html[BLOB_KEY].as_str()?.to_string()));
        }
        html => return Some(Page::Html(multiline(html))),
    }
    match &data[PLOTLY_MIME] {
        figure if figure[BLOB_KEY].is_string() => {
            Some(Page::PlotlyBlob(figure[BLOB_KEY].as_str()?.to_string()))
        }
        Value::Null => None,
        figure => Some(Page::Html(plotly_page(figure))),
    }
}

/// A page's title for the Viewer: the one R's viewer named, the HTML's
/// `<title>`, else what kind of page it is.
pub fn page_title(output: &Value) -> String {
    let data = &output["data"];
    if let Some(title) = data[VIEWER_MIME]["title"]
        .as_str()
        .filter(|t| !t.is_empty())
    {
        return title.to_string();
    }
    if data.get(PLOTLY_MIME).is_some() {
        return "plotly".into();
    }
    let html = multiline(&data["text/html"]);
    let lower = html.to_ascii_lowercase();
    // Scripts can hold `<title>`s of their own (plotly.js's logo).
    let scripts = lower.find("<script").unwrap_or(lower.len());
    if let Some(start) = lower.find("<title").filter(|&i| i < scripts) {
        let after = &html[start..];
        if let (Some(open), Some(close)) = (after.find('>'), after.find("</")) {
            let title = after[open + 1..close.max(open + 1)].trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }
    if lower.contains("html-widget") {
        return "htmlwidget".into();
    }
    if lower.contains("plotly-graph-div") {
        return "plotly".into();
    }
    "Page".into()
}

/// plotly.js, which draws a figure Python's plotly sent as data; loaded from
/// plotly's own address, as the Tauri app does (`app/src/model/pages.ts`).
const PLOTLY_JS: &str = "https://cdn.plot.ly/plotly-2.35.2.min.js";

/// A page that draws a plotly figure (`{data, layout, config}`).
pub fn plotly_page(figure: &Value) -> String {
    let height = figure["layout"]["height"].as_f64().unwrap_or(450.);
    // `</script` can't end the script it is in.
    let json = |v: &Value| {
        let v = if v.is_null() { &Value::Null } else { v };
        v.to_string().replace("</", "<\\/")
    };
    let data = if figure["data"].is_null() {
        "[]".to_string()
    } else {
        json(&figure["data"])
    };
    let layout = if figure["layout"].is_null() {
        "{}".to_string()
    } else {
        json(&figure["layout"])
    };
    let config = if figure["config"].is_null() {
        "{}".to_string()
    } else {
        json(&figure["config"])
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>html,body{{margin:0}}#figure{{width:100%;height:{height}px}}</style>\
         <script src=\"{PLOTLY_JS}\"></script></head><body><div id=\"figure\"></div><script>\
         if (window.Plotly) Plotly.newPlot(\"figure\", {data}, {layout}, Object.assign({{responsive: true}}, {config}));\
         else document.body.textContent = \"Could not load plotly.js, which draws this figure, from cdn.plot.ly.\";\
         </script></body></html>"
    )
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
            if let Some(page) = page(data, &output["metadata"]) {
                return Shown::Page(page);
            }
            let plot = ["image/png", "image/jpeg", "image/gif", "image/svg+xml"]
                .into_iter()
                .find(|m| mimes.contains_key(*m));
            if let Some(mime) = plot {
                Shown::Plot {
                    mime: mime.to_string(),
                    value: data[mime].clone(),
                }
            } else if let Some(table) = data
                .get("text/html")
                .filter(|h| !h.is_null())
                .and_then(|h| crate::editor::html_table::parse(&multiline(h)))
            {
                Shown::Table(table)
            } else if mimes.contains_key("text/plain") {
                Shown::Text(strip_ansi(&multiline(&data["text/plain"])))
            } else if mimes.contains_key("text/html") {
                Shown::Note("HTML without text: not drawn in the trial".into())
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
    fn shows_plots_and_notes_html() {
        let out = json!({"output_type": "display_data",
            "data": {"image/png": {"sandhill_blob": "ab"}, "text/plain": "<Figure>"}});
        assert_eq!(
            show(&out),
            Shown::Plot {
                mime: "image/png".into(),
                value: json!({"sandhill_blob": "ab"})
            }
        );
        let out = json!({"output_type": "display_data", "data": {"text/html": "<b>"}});
        assert!(matches!(show(&out), Shown::Note(n) if n.contains("HTML")));
    }

    #[test]
    fn names_pages() {
        let titled =
            json!({"data": {"text/html": "<html><head><title> QC report </title></head>"}});
        assert_eq!(page_title(&titled), "QC report");
        let plotly = json!({"data": {"text/html":
            "<script>s='<title>plotly-logomark</title>'</script><div class=\"plotly-graph-div\">"}});
        assert_eq!(page_title(&plotly), "plotly");
        let named = json!({"data": {"application/vnd.sandhill.page+json": {"title": "leaflet"}}});
        assert_eq!(page_title(&named), "leaflet");
    }

    #[test]
    fn shows_web_pages() {
        let widget = json!({"output_type": "display_data",
            "data": {"text/html": ["<div class='html-widget'></div>", "<script>x</script>"],
                "text/plain": "widget"}, "metadata": {}});
        assert_eq!(
            show(&widget),
            Shown::Page(Page::Html(
                "<div class='html-widget'></div><script>x</script>".into()
            ))
        );
        let blob = json!({"output_type": "display_data",
            "data": {"text/html": {"sandhill_blob": "cd"}}, "metadata": {}});
        assert_eq!(show(&blob), Shown::Page(Page::Blob("cd".into())));
        let named = json!({"output_type": "display_data", "data": {
            "application/vnd.sandhill.page+json": {"path": "/tmp/v/index.html", "title": "leaflet"},
            "text/html": {"sandhill_blob": "ef"}}, "metadata": {}});
        assert_eq!(
            show(&named),
            Shown::Page(Page::File("/tmp/v/index.html".into()))
        );
        let plotly = json!({"output_type": "display_data", "data": {
            "application/vnd.plotly.v1+json": {"data": [{"y": [1, 2]}], "layout": {"height": 300}},
            "text/plain": "Figure"}, "metadata": {}});
        let Shown::Page(Page::Html(html)) = show(&plotly) else {
            panic!("a plotly figure is a page");
        };
        assert!(html.contains(r#"Plotly.newPlot("figure", [{"y":[1,2]}], {"height":300}"#));
        assert!(html.contains("height:300px"));
        // A table without scripts stays text.
        let table = json!({"output_type": "execute_result",
            "data": {"text/html": "<table></table>", "text/plain": "   a\n0  1"}, "metadata": {}});
        assert!(matches!(show(&table), Shown::Text(_)));
    }
}
