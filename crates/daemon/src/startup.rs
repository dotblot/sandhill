//! The code a new session runs, out of sight, before anything else:
//! Sandhill's `View` and `%view` (see `view`), in R a viewer for web pages
//! (see `viewer`), keeping the plots IRkernel and Python show so they can be
//! drawn again at another size (see `redraw`), in Python `breakpoint()`
//! stopping in the debugger, and in R started in a project's folder, moving
//! to the session's own.

use std::path::Path;

use crate::{redraw, view, viewer};

pub fn code(language: &str, chdir: Option<&Path>) -> Option<String> {
    match crate::cells::normalize_language(language).as_str() {
        "r" => Some(match chdir {
            // A JSON string is also an R string literal.
            Some(dir) => format!(
                "{}\n{}\n{}\nsetwd({})",
                view::R,
                viewer::R,
                redraw::R_KEEP,
                serde_json::to_string(&dir.to_string_lossy()).unwrap_or_default()
            ),
            None => format!("{}\n{}\n{}", view::R, viewer::R, redraw::R_KEEP),
        }),
        "python" => Some(format!(
            "{}\n{}\n{PYTHON_BREAKPOINT}",
            view::PYTHON,
            redraw::PYTHON_KEEP
        )),
        _ => None,
    }
}

/// ipykernel 6+ hands `breakpoint()` to debugpy, which does nothing unless a
/// visual debugger is attached, and Sandhill has none: Python's own hook
/// stops in pdb, answered in the Console (and still follows
/// `PYTHONBREAKPOINT`).
const PYTHON_BREAKPOINT: &str =
    "__import__('sys').breakpointhook = __import__('sys').__breakpointhook__";

#[cfg(test)]
mod tests {
    #[test]
    fn r_and_python_sessions_get_code_at_start() {
        let r = super::code("R", None).unwrap();
        assert!(r.contains("tools:sandhill") && r.contains("shiny.launch.browser"));
        assert!(r.contains(".sandhill_plots"));
        let python = super::code("python3", None).unwrap();
        assert!(python.contains("%view") && python.contains("_sandhill_figures"));
        assert!(super::code("julia", None).is_none());
    }

    #[test]
    fn r_started_in_the_project_moves_to_the_session_folder() {
        let dir = std::path::Path::new("/data/my \"lab\"/analysis");
        let code = super::code("R", Some(dir)).unwrap();
        assert!(
            code.ends_with(r#"setwd("/data/my \"lab\"/analysis")"#),
            "{code}"
        );
    }
}
