//! Shared Skills and Connectors card and menu styling.
use super::*;

pub(super) fn card_shell(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    p: &Palette,
) -> Stateful<Div> {
    v_flex()
        .id(id)
        .track_focus(focus)
        .occlude()
        .w(px(380.))
        .max_w(relative(0.95))
        .max_h(relative(0.9))
        .p(px(20.))
        .gap(px(14.))
        .rounded(px(look::RADIUS))
        .bg(p.panel)
        .border_1()
        .border_color(p.glass_edge)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

pub(super) fn grey(text: &str, p: &Palette) -> Div {
    div()
        .min_w_0()
        .text_size(px(look::SMALL))
        .text_color(p.muted)
        .child(text.to_string())
}

pub(super) fn item(
    id: impl Into<ElementId>,
    label: &str,
    enabled: bool,
    p: &Palette,
) -> Stateful<Div> {
    let hover = p.ink(6.);
    h_flex()
        .id(id)
        .h(px(30.))
        .px(px(8.))
        .rounded(px(look::RADIUS - 4.))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |d| d.bg(hover))
                .active(|d| d.opacity(0.8))
        })
        .when(!enabled, |d| d.opacity(0.4).cursor_default())
        .child(label.to_string())
}
