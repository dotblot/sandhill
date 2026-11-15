mod caret_anchored;
mod code_action_menu;
mod completion_menu;
mod diagnostic_popover;
mod hover_popover;

pub use caret_anchored::set_popup_insets;
pub use completion_menu::show_completion_docs_now;
pub(crate) use caret_anchored::*;
pub(crate) use code_action_menu::*;
pub(crate) use completion_menu::*;
pub(crate) use diagnostic_popover::*;
pub(crate) use hover_popover::*;

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    App, Div, ElementId, EntityId, InteractiveElement as _, Pixels, SharedString, Stateful,
    StyleRefinement, Styled as _, Task, Window, div, px, relative, rems,
};
use lsp_types::CompletionItem;

use crate::{
    ThemeStyled as _,
    text::{TextView, TextViewStyle},
};

/// Sandhill: the prose size of documentation in popups (hover, completion),
/// the one small size every popup uses.
pub(super) const DOC_TEXT: Pixels = px(13.);
/// Sandhill: the code size beside it. The code font is wider than the
/// interface font, so one pixel less looks the same size.
pub(super) const DOC_CODE: Pixels = px(12.);

/// Sandhill: documentation as the AI pane shows replies: prose in the
/// interface font, code blocks in the code font a pixel smaller, with
/// their language's colours. The caller sets the font and prose size.
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
                })
                .code_block(
                    StyleRefinement::default()
                        .text_size(DOC_CODE)
                        .line_height(relative(1.5)),
                ),
        )
        .selectable(true)
}

/// Sandhill: the markdown documentation of a completion item that came
/// without it (R's language server sends it only when asked; kernels never
/// do), or `None` when nobody can say.
pub type CompletionDocs = dyn Fn(&CompletionItem, &mut App) -> Option<Task<Option<String>>>;

thread_local! {
    static COMPLETION_DOCS: RefCell<HashMap<EntityId, Rc<CompletionDocs>>> =
        RefCell::default();
}

/// Sandhill: who documents `editor`'s completion items (`None` forgets).
pub fn set_completion_docs(editor: EntityId, docs: Option<Rc<CompletionDocs>>) {
    COMPLETION_DOCS.with(|all| match docs {
        Some(docs) => {
            all.borrow_mut().insert(editor, docs);
        }
        None => {
            all.borrow_mut().remove(&editor);
        }
    });
}

pub(crate) fn completion_docs(editor: EntityId) -> Option<Rc<CompletionDocs>> {
    COMPLETION_DOCS.with(|all| all.borrow().get(&editor).cloned())
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
