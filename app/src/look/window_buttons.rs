//! Linux window buttons. There Sandhill draws its own title bar, so it draws
//! minimise, maximise and close too, on the right as Ubuntu puts them. All
//! three always show, whatever the desktop's own button setting says (Huan,
//! 2026-11-18).

use gpui_kit::component::h_flex;
use gpui_kit::*;

use super::{BUTTON_HOVER, GAP_8, ICON, Palette, icon};

/// GNOME's round title-bar buttons: a faint disc that darkens under the
/// pointer.
const SIZE: f32 = 24.;

/// The buttons, or nothing on a Mac and where the system draws a title bar
/// of its own (server-side decorations), which has its own buttons.
pub fn window_buttons(window: &Window, p: &Palette) -> Option<Div> {
    if !cfg!(target_os = "linux")
        || !matches!(window.window_decorations(), Decorations::Client { .. })
    {
        return None;
    }
    let maximized = window.is_maximized();
    Some(
        h_flex()
            .flex_none()
            .items_center()
            .gap(px(GAP_8))
            .pl(px(GAP_8))
            .child(
                button("window-minimize", "titlebar.minimize", p)
                    .on_click(|_, window, _| window.minimize_window()),
            )
            .child(
                button(
                    "window-maximize",
                    if maximized {
                        "titlebar.restore"
                    } else {
                        "titlebar.maximize"
                    },
                    p,
                )
                .on_click(|_, window, _| window.zoom_window()),
            )
            .child(
                // As ⇧Ctrl+W: saves what was edited, then closes.
                button("window-close", "titlebar.close", p).on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::CloseWindow), cx)
                }),
            ),
    )
}

fn button(id: &'static str, name: &str, p: &Palette) -> Stateful<Div> {
    let rest = p.ink(BUTTON_HOVER);
    let hover = p.ink(2. * BUTTON_HOVER);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(SIZE))
        .rounded_full()
        .bg(rest)
        .text_color(p.text)
        .hover(move |d| d.bg(hover))
        .active(move |d| d.bg(hover))
        // Pressing a button must not start moving the window.
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(icon(name).size(px(ICON)))
}
