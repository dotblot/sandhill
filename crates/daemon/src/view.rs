//! `View(df)` in R and `%view df` in Python open the table in the app's Data
//! Explorer. Ark's own `View()` opens a `positron.dataExplorer` comm: the
//! daemon takes its title and closes the comm, as no Positron is there to
//! answer it. IRkernel and IPython get a small `View` (and a `%view` magic)
//! when the session starts, which send a display_data of Sandhill's own type
//! instead. Either becomes a `sandhill.view` event naming what was viewed,
//! and the app opens it.

use serde_json::Value;

/// The display_data type the started-up `View` sends.
pub const MIME: &str = "application/vnd.sandhill.view+json";

/// The comm Ark's `View()` opens.
const ARK_COMM: &str = "positron.dataExplorer";

/// A `View` for IRkernel (Ark keeps its own), in an environment of its own
/// on the search path, so the user's variables stay as they are.
const R: &str = r#"tryCatch(local({
  if (is.null(getOption("jupyter.base_display_func")) || !requireNamespace("IRdisplay", quietly = TRUE)) return(invisible())
  tools <- attach(NULL, name = "tools:sandhill", warn.conflicts = FALSE)
  assign("View", function(x, title) {
    if (missing(title)) title <- paste(deparse(substitute(x)), collapse = " ")
    IRdisplay::publish_mimebundle(list("application/vnd.sandhill.view+json" = list(title = title)))
    invisible(x)
  }, envir = tools)
  invisible()
}), error = function(e) invisible())
"#;

/// `%view df`, as in Positron.
const PYTHON: &str = r#"def _sandhill_view():
    try:
        from IPython import get_ipython
        from IPython.display import publish_display_data
    except Exception:
        return
    shell = get_ipython()
    if shell is None:
        return

    def view(line):
        """Opens a data frame in Sandhill's Data Explorer: %view df"""
        publish_display_data({"application/vnd.sandhill.view+json": {"title": line.strip()}})

    shell.register_magic_function(view, "line", "view")


_sandhill_view()
del _sandhill_view
"#;

/// The code a new session runs, quietly, before anything else.
pub fn prelude(language: &str) -> Option<&'static str> {
    match crate::cells::normalize_language(language).as_str() {
        "r" => Some(R),
        "python" => Some(PYTHON),
        _ => None,
    }
}

/// What an IOPub message asks the app to view, if it is a `View()`: the
/// expression, and the comm to close for Ark's.
pub fn request(msg_type: &str, content: &Value) -> Option<(String, Option<String>)> {
    match msg_type {
        "comm_open" if content["target_name"] == ARK_COMM => {
            // An inline explorer is one Ark offers for a notebook's output.
            if content["data"]["inline_only"] == true {
                return None;
            }
            let title = content["data"]["title"].as_str().unwrap_or_default();
            let comm = content["comm_id"].as_str().map(str::to_string);
            Some((title.to_string(), comm))
        }
        "display_data" => {
            let ask = content["data"].get(MIME)?;
            // IRkernel can send the title as a one-item array.
            let title = ask["title"]
                .as_str()
                .or_else(|| ask["title"][0].as_str())
                .unwrap_or_default();
            Some((title.to_string(), None))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_what_view_asks_for() {
        let ark = json!({"comm_id": "c1", "target_name": "positron.dataExplorer", "data": {"title": "seu@meta.data", "inline_only": false}});
        assert_eq!(
            request("comm_open", &ark),
            Some(("seu@meta.data".to_string(), Some("c1".to_string())))
        );
        let inline = json!({"comm_id": "c2", "target_name": "positron.dataExplorer", "data": {"title": "df", "inline_only": true}});
        assert_eq!(request("comm_open", &inline), None);
        let other = json!({"comm_id": "c3", "target_name": "positron.variables", "data": {}});
        assert_eq!(request("comm_open", &other), None);
        let ir = json!({"data": {MIME: {"title": ["df"]}}, "metadata": {}});
        assert_eq!(request("display_data", &ir), Some(("df".to_string(), None)));
        let py = json!({"data": {MIME: {"title": "adata.obs"}}, "metadata": {}});
        assert_eq!(
            request("display_data", &py),
            Some(("adata.obs".to_string(), None))
        );
        let plot = json!({"data": {"image/png": "..."}, "metadata": {}});
        assert_eq!(request("display_data", &plot), None);
        assert!(
            prelude("R").is_some() && prelude("python3").is_some() && prelude("julia").is_none()
        );
    }
}
