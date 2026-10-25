//! Drawing a plot again at another size, in the kernel that drew it, to save
//! it. Ark keeps a recording of every plot it shows, named by the plot's
//! display id; Python sessions keep their matplotlib figures (`PYTHON_KEEP`,
//! run when a session starts, tags each figure's image with an id in its
//! metadata). IRkernel's plots and plain images can't be drawn again.

use serde_json::Value;

use crate::outputs::is_plot_mime;
use crate::tables::literal;

/// What a redrawn plot can be saved as.
pub const FORMATS: &[&str] = &["png", "svg", "pdf"];

/// The metadata key a Python session tags its figures' images with.
const TAG: &str = "sandhill_plot";

/// What `print` says when the file is written.
const SAVED: &str = "SANDHILL_SAVED";

/// Keeps the last figures a Python session showed, so they can be drawn
/// again: IPython's figure formatters are wrapped to remember each figure and
/// add its id to the image's metadata. Run once, at start, before matplotlib
/// sets the formatters up.
pub const PYTHON_KEEP: &str = r#"def _sandhill_keep_figures():
    try:
        import IPython.core.pylabtools as tools
        from IPython import get_ipython
    except Exception:
        return
    if get_ipython() is None or hasattr(tools, "_sandhill_figures"):
        return
    import itertools
    from collections import OrderedDict

    figures = tools._sandhill_figures = OrderedDict()
    names = {}
    count = itertools.count(1)

    def keep(draw):
        def drawn(fig, *args, **kwargs):
            out = draw(fig, *args, **kwargs)
            if out is None:
                return out
            name = names.get(id(fig))
            if name is None or figures.get(name) is not fig:
                name = names[id(fig)] = f"f{next(count)}"
            figures[name] = fig
            figures.move_to_end(name)
            while len(figures) > 10:
                _, gone = figures.popitem(last=False)
                names.pop(id(gone), None)
            data, meta = out if isinstance(out, tuple) else (out, None)
            return data, {**(meta or {}), "sandhill_plot": name}

        return drawn

    tools.print_figure = keep(tools.print_figure)
    tools.retina_figure = keep(tools.retina_figure)
    try:
        from matplotlib.figure import Figure
    except Exception:
        return
    # Formatters set up before this keep their own copy: wrap those too.
    for formatter in get_ipython().display_formatter.formatters.values():
        if Figure in formatter.type_printers:
            formatter.for_type(Figure, keep(formatter.type_printers[Figure]))


_sandhill_keep_figures()
del _sandhill_keep_figures
"#;

/// Which plot an image is, to the kernel that drew it.
#[derive(Debug, PartialEq)]
pub enum Plot {
    /// Ark's recording of it, by display id.
    Ark(String),
    /// A Python session's figure, by the id it tagged the image with.
    Figure(String),
}

/// Finds which plot the image is among a run's events: the one holding the
/// blob `image`, else the `index`-th image shown. `Err` says why it can't
/// be drawn again.
pub fn find(
    events: &[(String, Value)],
    language: &str,
    image: Option<&str>,
    index: usize,
) -> Result<Plot, String> {
    let shown = events.iter().filter(|(msg_type, content)| {
        ["display_data", "execute_result", "update_display_data"].contains(&msg_type.as_str())
            && plot_mime(content).is_some()
    });
    let content = match image {
        Some(hash) => shown
            .map(|(_, content)| content)
            .find(|content| holds(content, hash)),
        None => shown
            .filter(|(msg_type, _)| msg_type != "update_display_data")
            .map(|(_, content)| content)
            .nth(index),
    }
    .ok_or("the run no longer has this plot")?;
    let mime = plot_mime(content).unwrap_or_default();
    match crate::cells::normalize_language(language).as_str() {
        "python" => content["metadata"][mime][TAG]
            .as_str()
            .map(|name| Plot::Figure(name.to_string()))
            .ok_or_else(|| "only matplotlib figures can be drawn again in Python".to_string()),
        "r" => content["transient"]["display_id"]
            .as_str()
            .map(|id| Plot::Ark(id.to_string()))
            .ok_or_else(|| "only Ark keeps R plots to draw them again".to_string()),
        other => Err(format!("plots can't be drawn again in {other}")),
    }
}

fn plot_mime(content: &Value) -> Option<&str> {
    content["data"]
        .as_object()?
        .keys()
        .map(String::as_str)
        .find(|mime| is_plot_mime(mime))
}

fn holds(content: &Value, hash: &str) -> bool {
    content["data"]
        .as_object()
        .is_some_and(|data| data.values().any(|v| v[sandhill_proto::BLOB_KEY] == hash))
}

/// Where and how to save it: the size in inches, and the resolution of a PNG.
pub struct Save<'a> {
    pub file: &'a str,
    pub format: &'a str,
    pub width: f64,
    pub height: f64,
    pub dpi: u32,
}

/// The code that draws the plot again into `save.file`.
pub fn code(plot: &Plot, save: &Save) -> String {
    match plot {
        Plot::Ark(id) => R.replace("__ID__", &literal(id)),
        Plot::Figure(name) => PYTHON.replace("__ID__", &literal(name)),
    }
    .replace("__FILE__", &literal(save.file))
    .replace("__FORMAT__", &literal(save.format))
    .replace("__W__", &save.width.to_string())
    .replace("__H__", &save.height.to_string())
    .replace("__DPI__", &save.dpi.to_string())
}

/// Whether the code says it wrote the file.
pub fn saved(stdout: &str) -> bool {
    stdout.lines().any(|line| line.trim() == SAVED)
}

/// Replays Ark's recording on a device of the size asked for, then goes back
/// to the device that was in use.
const R: &str = r#"local({
  .render <- get0(".ps.graphics.render_plot_from_recording", mode = "function")
  .recording <- if (!is.null(.render)) get0("get_recording", envir = environment(.render), mode = "function")
  if (is.null(.recording)) stop("this R kernel doesn't keep its plots")
  .plot <- .recording(__ID__)
  if (is.null(.plot)) stop("the session no longer has this plot")
  .file <- __FILE__
  .was <- grDevices::dev.cur()
  switch(__FORMAT__,
    png = grDevices::png(.file, width = round(__W__ * __DPI__), height = round(__H__ * __DPI__), res = __DPI__),
    svg = grDevices::svg(.file, width = __W__, height = __H__),
    pdf = grDevices::pdf(.file, width = __W__, height = __H__))
  .dev <- grDevices::dev.cur()
  tryCatch(suppressWarnings(grDevices::replayPlot(.plot)), finally = {
    grDevices::dev.off(.dev)
    if (.was > 1) grDevices::dev.set(.was)
  })
  cat("SANDHILL_SAVED\n")
})
"#;

/// Saves the figure at the size asked for, then puts its own size back. A
/// "tight" box in the user's settings would change the size, so it is off.
const PYTHON: &str = r#"def _sandhill_redraw():
    import IPython.core.pylabtools as tools
    import matplotlib

    fig = getattr(tools, "_sandhill_figures", {}).get(__ID__)
    if fig is None:
        raise RuntimeError("the session no longer has this figure")
    size = fig.get_size_inches()
    try:
        fig.set_size_inches(__W__, __H__)
        with matplotlib.rc_context({"savefig.bbox": "standard"}):
            fig.savefig(__FILE__, dpi=__DPI__, format=__FORMAT__)
    finally:
        fig.set_size_inches(size)
    print("SANDHILL_SAVED")


try:
    _sandhill_redraw()
finally:
    del _sandhill_redraw
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_which_plot_an_image_is() {
        let ark = vec![
            ("sandhill.run_started".to_string(), json!({})),
            (
                "display_data".to_string(),
                json!({"data": {"image/png": {"sandhill_blob": "a1"}, "text/plain": "<plot>"}, "metadata": {}, "transient": {"display_id": "p1"}}),
            ),
            (
                "display_data".to_string(),
                json!({"data": {"image/png": {"sandhill_blob": "b2"}}, "metadata": {}, "transient": {"display_id": "p2"}}),
            ),
            (
                "update_display_data".to_string(),
                json!({"data": {"image/png": {"sandhill_blob": "c3"}}, "metadata": {}, "transient": {"display_id": "p2"}}),
            ),
        ];
        assert_eq!(find(&ark, "R", Some("c3"), 0), Ok(Plot::Ark("p2".into())));
        assert_eq!(find(&ark, "R", None, 0), Ok(Plot::Ark("p1".into())));
        assert_eq!(find(&ark, "R", None, 1), Ok(Plot::Ark("p2".into())));
        assert!(find(&ark, "R", Some("zz"), 0).is_err());
        let ir = vec![(
            "display_data".to_string(),
            json!({"data": {"image/png": {"sandhill_blob": "a1"}}, "metadata": {}}),
        )];
        assert_eq!(
            find(&ir, "R", Some("a1"), 0),
            Err("only Ark keeps R plots to draw them again".into())
        );
        let py = vec![(
            "display_data".to_string(),
            json!({"data": {"image/png": {"sandhill_blob": "a1"}}, "metadata": {"image/png": {"sandhill_plot": "f3", "width": 400}}}),
        )];
        assert_eq!(
            find(&py, "python3", Some("a1"), 0),
            Ok(Plot::Figure("f3".into()))
        );

        let save = Save {
            file: "/p/umap.png",
            format: "png",
            width: 6.0,
            height: 4.5,
            dpi: 300,
        };
        let r = code(&Plot::Ark("p\"1".into()), &save);
        assert!(r.contains(r#".plot <- .recording("p\"1")"#), "{r}");
        assert!(
            r.contains("round(6 * 300)") && r.contains(r#".file <- "/p/umap.png""#),
            "{r}"
        );
        let python = code(&Plot::Figure("f3".into()), &save);
        assert!(
            python.contains(r#"fig.set_size_inches(6, 4.5)"#),
            "{python}"
        );
        assert!(saved("noise\nSANDHILL_SAVED\n") && !saved("nothing"));
    }
}
