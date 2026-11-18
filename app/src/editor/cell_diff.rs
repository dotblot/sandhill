//! The removed and added rows shared by notebook cell comparisons.

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::editor::text_diff::{DiffLine, diff};
use crate::look::{self, Palette};

pub(crate) struct Row {
    mark: &'static str,
    text: SharedString,
}

pub(crate) fn rows(before: &str, after: &str) -> impl Iterator<Item = Row> {
    diff(before, after).into_iter().map(|line| {
        let (mark, text) = match line {
            DiffLine::Same(text) => (" ", text),
            DiffLine::Removed(text) => ("-", text),
            DiffLine::Added(text) => ("+", text),
            DiffLine::Gap(1) => ("", "⋯ 1 line".to_string()),
            DiffLine::Gap(n) => ("", format!("⋯ {n} lines")),
        };
        Row {
            mark,
            text: text.into(),
        }
    })
}

impl Row {
    pub(crate) fn render(&self, p: &Palette) -> Div {
        let (bg, color) = match self.mark {
            "-" => (Some(p.err.opacity(0.12)), p.text),
            "+" => (Some(p.ok.opacity(0.14)), p.text),
            " " => (None, p.muted),
            _ => (None, p.faint),
        };
        h_flex()
            .w_full()
            .px(px(look::GAP_8))
            .when_some(bg, |d, bg| d.bg(bg))
            .text_color(color)
            .child(div().w(px(look::ICON)).flex_none().child(self.mark))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_nowrap()
                    .child(self.text.clone()),
            )
    }
}
