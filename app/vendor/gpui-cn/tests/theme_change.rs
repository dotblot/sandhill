//! A theme change repaints the animated components in the next frame.
//! Their motion follows state, so the old theme's colors never fade out.

use gpui_cn::{ActiveTheme as _, Button, ReduceMotion, Switch, Theme, ThemeMode, ThemeModePicker};
use gpui_kit::base::{Root, Selectable as _};
use gpui_kit::{
    AppContext as _, Background, Bounds, Context, Hsla, IntoElement, ParentElement as _, Render,
    ScaledPixels, Styled as _, TestAppContext, Window, div, px, size, test::TestWindowExt as _,
};

struct Harness;

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(Button::new("row").ghost().selected(true).label("Row"))
            .child(Switch::new("wifi").checked(true))
            .child(ThemeModePicker::new("mode").value(ThemeMode::Light))
    }
}

/// The quads painted inside the element with `id`, in scaled pixels.
fn quads_in(window: &mut Window, id: &'static str) -> Vec<gpui_kit::Quad> {
    let bounds = window.find(id).bounds();
    let scale = window.scale_factor();
    let inside = Bounds::new(bounds.origin.scale(scale), bounds.size.scale(scale));
    window
        .painted_quads()
        .into_iter()
        .filter(|quad| {
            let b: Bounds<ScaledPixels> = quad.bounds;
            inside.contains(&b.origin) && b.size.width <= inside.size.width
        })
        .collect()
}

fn fills(quads: &[gpui_kit::Quad], color: Hsla) -> bool {
    quads
        .iter()
        .any(|quad| quad.background == Background::from(color))
}

fn borders(quads: &[gpui_kit::Quad], color: Hsla) -> bool {
    quads.iter().any(|quad| quad.border_color == color)
}

#[gpui_kit::test]
fn a_theme_change_paints_the_new_colors_in_the_next_frame(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off);
        Theme::change(ThemeMode::Dark, cx);
    });
    let handle = cx.open_window(size(px(600.), px(400.)), |window, cx| {
        let view = cx.new(|_| Harness);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();

    cx.update(|cx| Theme::change(ThemeMode::Light, cx));
    let (selected, track, ink) = cx.update(|cx| {
        let theme = cx.theme();
        (theme.selected, theme.switch_track_on, theme.foreground())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        // No time passes: a color that fades would still be the dark one.
        window.render_frame(cx);
        assert!(
            fills(&quads_in(window, "row"), selected),
            "the selected button's fill is the light theme's"
        );
        assert!(
            fills(&quads_in(window, "wifi"), track),
            "the checked switch's track is the light theme's"
        );
        assert!(
            borders(&quads_in(window, "mode"), ink),
            "the chosen card's ring is the light theme's ink"
        );
    })
    .unwrap();
}
