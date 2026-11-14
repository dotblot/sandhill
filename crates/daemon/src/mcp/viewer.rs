//! Read-only summaries of the same journaled pages the Viewer displays.
//! Only declarative widget JSON is read; JavaScript is never run or returned.

use sandhill_proto::Plot;
use scraper::{ElementRef, Html, Selector};

use super::*;

const MAX_REPLY: usize = 64 * 1024;
const MAX_TEXT: usize = 16 * 1024;
const MAX_CELL: usize = 1024;
const MAX_TABLES: usize = 20;
const MAX_SERIES: usize = 100;
const PLOTLY: &str = "application/vnd.plotly.v1+json";
const LIMITATION: &str = "This is the latest saved Viewer page, not a live browser snapshot. In-page interaction state (including plotly zoom, DT filtering/sorting and leaflet navigation) is unavailable. JavaScript is not executed; dynamically generated content without declarative widget data is unavailable.";

impl Tools {
    pub(super) async fn get_viewer(&self, session: Option<&str>) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let sessions = client.list_sessions().await?;
        let sessions: Vec<_> = sessions
            .into_iter()
            .filter(|s| match session {
                Some(id) => s.id.starts_with(id),
                None => self.in_project(s),
            })
            .collect();
        if let Some(id) = session {
            match sessions.len() {
                0 => bail!("no session {id}; list_sessions shows the running ones"),
                1 => {}
                _ => bail!("{id} could be several sessions; give more of its id"),
            }
        }
        let mut latest: Option<Plot> = None;
        for session in sessions {
            if let Some(page) = client
                .session_pages(&session.id, Some(1))
                .await?
                .plots
                .pop()
                && latest.as_ref().is_none_or(|old| page.seq > old.seq)
            {
                latest = Some(page);
            }
        }
        let Some(page) = latest else {
            let mut summary = empty_summary();
            summary["limitation"] =
                json!(format!("No saved Viewer page is available. {LIMITATION}"));
            return bounded_reply(summary);
        };
        let html = stored_text(&client, &page.data["text/html"]).await?;
        let mut summary = summarize(&html, &page.data);
        if let Some(hash) = page.data[PLOTLY][BLOB_KEY].as_str() {
            let figure: Value = serde_json::from_str(&client.get_blob(hash).await?.data)
                .context("the saved plotly figure is not valid JSON")?;
            add_plotly(&mut summary, &figure);
        }
        bounded_reply(summary)
    }
}

async fn stored_text(client: &Client, value: &Value) -> Result<String> {
    Ok(match value {
        Value::String(text) => text.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => match value[BLOB_KEY].as_str() {
            Some(hash) => client.get_blob(hash).await?.data,
            None => String::new(),
        },
    })
}

fn empty_summary() -> Value {
    json!({
        "title": null,
        "widget_types": [],
        "visible_text": "",
        "tables": [],
        "plot_series": [],
        "limitation": LIMITATION,
        "truncated": false,
    })
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static Viewer selector")
}

fn summarize(html: &str, data: &Value) -> Value {
    let document = Html::parse_document(html);
    let mut summary = empty_summary();
    let title = document
        .select(&selector("title"))
        .next()
        .map(|e| clipped(&e.text().collect::<String>(), MAX_CELL))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            data[crate::viewer::MIME]["title"]
                .as_str()
                .map(|s| clipped(s, MAX_CELL))
        });
    summary["title"] = json!(title);
    let root = document
        .select(&selector("body"))
        .next()
        .unwrap_or_else(|| document.root_element());
    summary["visible_text"] = json!(element_text(root, MAX_TEXT));
    for table in document
        .select(&selector("table"))
        .filter(|e| !hidden(*e))
        .take(MAX_TABLES)
    {
        summary["tables"]
            .as_array_mut()
            .unwrap()
            .push(html_table(table));
    }
    for widget in document.select(&selector(".html-widget, .plotly-graph-div")) {
        if widget.value().classes().any(|c| c == "plotly-graph-div") {
            widget_type(&mut summary, "plotly");
        }
        if widget.value().classes().any(|c| c == "html-widget") {
            let mut found = false;
            for class in widget.value().classes().filter(|c| {
                !matches!(
                    *c,
                    "html-widget"
                        | "html-widget-static-bound"
                        | "html-fill-item"
                        | "html-fill-container"
                ) && !c.starts_with("html-widget-")
            }) {
                widget_type(
                    &mut summary,
                    if class == "datatables" { "DT" } else { class },
                );
                found = true;
            }
            if !found {
                widget_type(&mut summary, "htmlwidgets");
            }
        }
    }
    for script in document.select(&selector("script[type='application/json'][data-for]")) {
        let Some(id) = script.value().attr("data-for") else {
            continue;
        };
        let Some(widget) = document
            .select(&selector(".html-widget"))
            .find(|w| w.value().id() == Some(id))
        else {
            continue;
        };
        let text: String = script.text().collect();
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if widget.value().classes().any(|c| c == "plotly") {
            add_plotly(&mut summary, &payload["x"]);
        }
        if widget.value().classes().any(|c| c == "datatables") {
            add_dt(&mut summary, &payload["x"]);
        }
    }
    // Python's standalone plotly HTML uses JSON arguments to newPlot,
    // rather than an htmlwidgets application/json payload. Read only those
    // literal arguments; do not interpret or return the surrounding code.
    if summary["plot_series"].as_array().unwrap().is_empty() && data.get(PLOTLY).is_none() {
        for script in document.select(&selector("script:not([src])")) {
            for text in script.text() {
                for (_, arguments) in text
                    .match_indices("Plotly.newPlot(")
                    .map(|(at, call)| (at, &text[at + call.len()..]))
                {
                    if let Some(figure) = plotly_arguments(arguments) {
                        add_plotly(&mut summary, &figure);
                    }
                }
            }
        }
    }
    if data.get(PLOTLY).is_some_and(|v| v.get(BLOB_KEY).is_none()) {
        add_plotly(&mut summary, &data[PLOTLY]);
    }
    summary
}
fn json_argument(text: &str) -> Option<(Value, &str)> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    let value = stream.next()?.ok()?;
    Some((value, &text[stream.byte_offset()..]))
}

fn plotly_arguments(arguments: &str) -> Option<Value> {
    let (target, rest) = json_argument(arguments.trim_start())?;
    if !target.is_string() {
        return None;
    }
    let (data, rest) = json_argument(rest.trim_start().strip_prefix(',')?.trim_start())?;
    if !data.is_array() {
        return None;
    }
    let (layout, _) = json_argument(rest.trim_start().strip_prefix(',')?.trim_start())?;
    if !layout.is_object() {
        return None;
    }
    Some(json!({"data": data, "layout": layout}))
}

fn clipped(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn hidden(element: ElementRef<'_>) -> bool {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .chain(std::iter::once(element))
        .any(|e| {
            let tag = e.value().name();
            if matches!(tag, "script" | "style" | "head" | "template" | "noscript")
                || e.value().attr("hidden").is_some()
                || e.value()
                    .attr("aria-hidden")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
                || (tag == "input" && e.value().attr("type") == Some("hidden"))
            {
                return true;
            }
            e.value().attr("style").is_some_and(|style| {
                style.split(';').any(|declaration| {
                    let Some((property, value)) = declaration.split_once(':') else {
                        return false;
                    };
                    let value = value.split('!').next().unwrap_or_default().trim();
                    (property.trim().eq_ignore_ascii_case("display")
                        && value.eq_ignore_ascii_case("none"))
                        || (property.trim().eq_ignore_ascii_case("visibility")
                            && matches!(value, "hidden" | "collapse"))
                })
            })
        })
}

fn element_text(element: ElementRef<'_>, limit: usize) -> String {
    let mut out = String::new();
    for node in element.descendants() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        // `<template>` content lives in its own fragment beneath the template,
        // so its text has no element parent; it is never rendered.
        let Some(parent) = node.parent() else {
            continue;
        };
        if parent.value().is_fragment() || ElementRef::wrap(parent).is_some_and(hidden) {
            continue;
        }
        for word in text.split_whitespace() {
            if !out.is_empty() {
                if out.len() == limit {
                    return out;
                }
                out.push(' ');
            }
            let available = limit.saturating_sub(out.len());
            out.push_str(&clipped(word, available));
            if out.len() >= limit || word.len() > available {
                return out;
            }
        }
    }
    out
}

fn html_table(table: ElementRef<'_>) -> Value {
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for row in table.select(&selector("tr")).filter(|r| !hidden(*r)) {
        // Do not count rows belonging to nested tables twice.
        if row
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "table")
            != Some(table)
        {
            continue;
        }
        let cells: Vec<_> = row
            .select(&selector("th, td"))
            .filter(|c| !hidden(*c))
            .take(100)
            .collect();
        if cells.is_empty() {
            continue;
        }
        let values: Vec<_> = cells.iter().map(|c| element_text(*c, MAX_CELL)).collect();
        if rows.is_empty() && cells.iter().all(|c| c.value().name() == "th") {
            columns = values;
        } else if rows.len() < 20 {
            rows.push(values);
        } else {
            break;
        }
    }
    json!({"kind": "html", "columns": columns, "rows": rows})
}

fn widget_type(summary: &mut Value, name: &str) {
    let types = summary["widget_types"].as_array_mut().unwrap();
    let name = json!(clipped(name, 100));
    if types.len() < 100 && !types.contains(&name) {
        types.push(name);
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => element_text(Html::parse_fragment(text).root_element(), MAX_CELL),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        _ => String::from("[nested value]"),
    }
}

fn add_dt(summary: &mut Value, x: &Value) {
    widget_type(summary, "DT");
    let Some(data) = x["data"].as_array() else {
        return;
    };
    if summary["tables"].as_array().unwrap().len() >= MAX_TABLES {
        return;
    }
    let columns: Vec<_> = data.iter().take(100).collect();
    let count = columns
        .iter()
        .filter_map(|c| c.as_array().map(Vec::len))
        .max()
        .unwrap_or(0)
        .min(20);
    let rows: Vec<Vec<String>> = (0..count)
        .map(|r| columns.iter().map(|c| cell(&c[r])).collect())
        .collect();
    let names = x["container"]
        .as_str()
        .map(|container| {
            let html = Html::parse_fragment(container);
            html.select(&selector("thead th"))
                .take(100)
                .map(|e| element_text(e, MAX_CELL))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    summary["tables"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "DT", "columns": names, "rows": rows}));
}

fn axis_title(layout: &Value, axis: &str) -> String {
    let title = &layout[axis]["title"];
    cell(title.get("text").unwrap_or(title))
}

fn point_count(value: &Value) -> usize {
    if let Some(array) = value.as_array() {
        if array.first().is_some_and(Value::is_array) {
            return array.iter().filter_map(Value::as_array).map(Vec::len).sum();
        }
        return array.len();
    }
    let Some(encoded) = value["bdata"].as_str() else {
        return 0;
    };
    let size = match value["dtype"].as_str().unwrap_or("") {
        "i1" | "u1" => 1,
        "i2" | "u2" => 2,
        "i4" | "u4" | "f4" => 4,
        "f8" | "i8" | "u8" => 8,
        _ => return 0,
    };
    encoded
        .len()
        .saturating_mul(3)
        .saturating_div(4)
        .saturating_sub(encoded.bytes().rev().take_while(|b| *b == b'=').count())
        / size
}

fn add_plotly(summary: &mut Value, figure: &Value) {
    widget_type(summary, "plotly");
    let Some(traces) = figure["data"].as_array() else {
        return;
    };
    for trace in traces {
        if summary["plot_series"].as_array().unwrap().len() >= MAX_SERIES {
            summary["truncated"] = json!(true);
            break;
        }
        let count = [
            "x", "y", "z", "values", "labels", "lat", "lon", "r", "theta",
        ]
        .iter()
        .map(|key| point_count(&trace[*key]))
        .max()
        .unwrap_or(0);
        let mut axes = serde_json::Map::new();
        for dimension in ["x", "y", "z"] {
            let reference = format!("{dimension}axis");
            let axis = trace[&reference]
                .as_str()
                .map(|name| {
                    format!(
                        "{dimension}axis{}",
                        name.strip_prefix(dimension).unwrap_or_default()
                    )
                })
                .unwrap_or(reference);
            let layout = &figure["layout"];
            let scene = trace["scene"].as_str().unwrap_or("scene");
            let title = if matches!(
                trace["type"].as_str(),
                Some(
                    "scatter3d"
                        | "surface"
                        | "mesh3d"
                        | "cone"
                        | "streamtube"
                        | "volume"
                        | "isosurface"
                )
            ) {
                axis_title(&layout[scene], &axis)
            } else {
                axis_title(layout, &axis)
            };
            if !title.is_empty() {
                axes.insert(dimension.into(), json!(title));
            }
        }
        summary["plot_series"].as_array_mut().unwrap().push(json!({
            "type": trace["type"].as_str().map(|kind| clipped(kind, MAX_CELL)).unwrap_or_else(|| "scatter".into()),
            "name": cell(&trace["name"]),
            "point_count": count,
            "axis_titles": axes,
        }));
    }
}

fn bounded_reply(mut summary: Value) -> Result<Vec<Value>> {
    loop {
        let serialized = serde_json::to_string(&summary)?;
        let items = vec![json!({"type": "text", "text": serialized})];
        if serde_json::to_vec(&items)?.len() <= MAX_REPLY {
            return Ok(items);
        }
        summary["truncated"] = json!(true);
        let tables = summary["tables"].as_array_mut().unwrap();
        if tables.len() > 1 {
            tables.pop();
        } else if let Some(table) = tables.last_mut() {
            let rows = table["rows"].as_array_mut().unwrap();
            if rows.is_empty() {
                tables.pop();
            } else {
                rows.truncate(rows.len() / 2);
            }
        } else {
            let series = summary["plot_series"].as_array_mut().unwrap();
            if !series.is_empty() {
                series.truncate(series.len() / 2);
            } else {
                let text = summary["visible_text"].as_str().unwrap_or_default();
                if !text.is_empty() {
                    summary["visible_text"] = json!(clipped(text, text.len() / 2));
                } else if summary["widget_types"]
                    .as_array_mut()
                    .unwrap()
                    .pop()
                    .is_none()
                {
                    let title = summary["title"].as_str().unwrap_or_default();
                    summary["title"] = json!(clipped(title, title.len() / 2));
                }
            }
        }
    }
}
