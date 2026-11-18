//! The right-click menu over text that can be selected but not edited: the
//! AI conversation and the Console (Huan, 2026-11-16). It copies what is
//! selected, else the whole message or run under the pointer; the AI pane
//! also sends it to its message box.

use gpui_kit::base::TextSelection;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::look::{self, Palette};

/// Where the menu opened and what it acts on.
pub(crate) struct TextMenu {
    pub at: Point<Pixels>,
    /// The window's selected text when the menu opened; empty for none.
    pub selection: String,
    /// The whole message or run under the pointer.
    pub whole: String,
}

impl TextMenu {
    /// The menu for a right click at `at` over `whole`.
    pub(crate) fn open(
        at: Point<Pixels>,
        whole: String,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self {
            at,
            selection: TextSelection::selected_text(window, cx),
            whole,
        }
    }

    /// The selection, else the whole message.
    pub(crate) fn text(&self) -> String {
        if self.selection.trim().is_empty() {
            self.whole.clone()
        } else {
            self.selection.clone()
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        !self.selection.trim().is_empty()
    }
}

/// One row of the menu. `run` closes the menu itself.
pub(crate) fn row<V: 'static>(
    id: &'static str,
    label: &'static str,
    keys: &'static str,
    p: &Palette,
    cx: &mut Context<V>,
    run: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> AnyElement {
    let hover = p.ink(look::ROW_HOVER);
    h_flex()
        .id(id)
        .h(px(look::ROW))
        .px(px(look::GAP_8))
        .gap(px(look::GAP_8))
        .rounded(px(look::CONTROL_RADIUS))
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(div().flex_1().whitespace_nowrap().child(label))
        .when(!keys.is_empty(), |d| {
            d.child(div().flex_none().text_color(p.faint).child(keys))
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            run(this, window, cx);
            cx.notify();
        }))
        .into_any_element()
}

/// The menu at `at`; a click outside it calls `close`.
pub(crate) fn render<V: 'static>(
    id: &'static str,
    at: Point<Pixels>,
    rows: Vec<AnyElement>,
    p: &Palette,
    cx: &mut Context<V>,
    close: impl Fn(&mut V) + 'static,
) -> AnyElement {
    let body = crate::agents::composer_view::popup(p)
        .id(id)
        .min_w(px(200.))
        .children(rows)
        .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
            close(this);
            cx.notify();
        }));
    deferred(anchored().position(at).snap_to_window().child(body))
        .with_priority(3)
        .into_any_element()
}

/// Copies `text` to the clipboard.
pub(crate) fn copy(text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}
