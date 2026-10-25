//! The code a new session runs, out of sight, before anything else:
//! Sandhill's `View` and `%view` (see `view`) and, in Python, keeping the
//! figures it shows so they can be drawn again at another size (see
//! `redraw`).

use crate::{redraw, view};

pub fn code(language: &str) -> Option<String> {
    match crate::cells::normalize_language(language).as_str() {
        "r" => Some(view::R.to_string()),
        "python" => Some(format!("{}\n{}", view::PYTHON, redraw::PYTHON_KEEP)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn r_and_python_sessions_get_code_at_start() {
        assert!(super::code("R").unwrap().contains("tools:sandhill"));
        let python = super::code("python3").unwrap();
        assert!(python.contains("%view") && python.contains("_sandhill_figures"));
        assert!(super::code("julia").is_none());
    }
}
