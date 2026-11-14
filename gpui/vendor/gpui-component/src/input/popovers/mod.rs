mod caret_anchored;
mod code_action_menu;
mod completion_menu;
mod diagnostic_popover;
mod hover_popover;

pub use caret_anchored::set_popup_insets;
pub(crate) use caret_anchored::*;
pub(crate) use code_action_menu::*;
pub(crate) use completion_menu::*;
pub(crate) use diagnostic_popover::*;
pub(crate) use hover_popover::*;

use gpui::{
    App, Div, ElementId, InteractiveElement as _, SharedString, Stateful, Styled as _, Window, div,
    rems,
};

use crate::{
    ThemeStyled as _,
    text::{TextView, TextViewStyle},
};

/// Sandhill: documentation as the AI pane shows replies: prose in the
/// interface font, code blocks in the code font at their usual size, with
/// their language's colours.
pub(super) fn render_markdown(
    id: impl Into<ElementId>,
    markdown: impl Into<SharedString>,
    _: &mut Window,
    _: &mut App,
) -> TextView {
    TextView::markdown(id, markdown)
        .style(
            TextViewStyle::default()
                .paragraph_gap(rems(0.4))
                .heading_font_size(|level, rem_size| match level {
                    1..=3 => rem_size * 1,
                    4 => rem_size * 0.9,
                    _ => rem_size * 0.8,
                }),
        )
        .selectable(true)
}

/// Sandhill: names `language` on code fences that name none, so the code in
/// documentation is coloured as the editor's language.
pub(crate) fn tag_fences(markdown: &str, language: &str) -> String {
    if language.is_empty() {
        return markdown.to_string();
    }
    let mut open: Option<String> = None;
    let mut out = String::with_capacity(markdown.len() + 16);
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence: String = trimmed.chars().take_while(|c| *c == '`' || *c == '~').collect();
        let is_fence = fence.len() >= 3 && fence.chars().all(|c| c == fence.chars().next().unwrap());
        match (&open, is_fence) {
            (None, true) => {
                let rest = trimmed[fence.len()..].trim();
                open = Some(fence.clone());
                if rest.is_empty() {
                    let indent = &line[..line.len() - trimmed.len()];
                    let ending = if line.ends_with('\n') { "\n" } else { "" };
                    out.push_str(&format!("{indent}{fence}{language}{ending}"));
                    continue;
                }
            }
            (Some(opened), true)
                if fence.starts_with(opened.as_str())
                    && trimmed[fence.len()..].trim().is_empty() =>
            {
                open = None;
            }
            _ => {}
        }
        out.push_str(line);
    }
    out
}

pub(super) fn editor_popover(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .occlude()
        .popover_style(cx)
        .shadow_md()
        .text_xs()
        .p_1()
}
