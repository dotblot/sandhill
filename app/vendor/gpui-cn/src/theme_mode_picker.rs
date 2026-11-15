use std::rc::Rc;

use gpui_kit::{
    App, Axis, BorderStyle, Bounds, ContentMask, Corners, ElementId, Hsla, InteractiveElement as _,
    IntoElement, ParentElement, Pixels, RenderOnce, Size, StyleRefinement, Styled, Window,
    base::{self, Disableable, Interpolate, StyledExt as _, TestSupportExt as _, transition},
    canvas, div, fill, point,
    prelude::FluentBuilder as _,
    px, quad, size,
};

use crate::{
    ActiveTheme as _, Theme, ThemeConfig, ThemeMode,
    theme::{lightness, mix},
};

type ChangeHandler = Rc<dyn Fn(&ThemeMode, &mut Window, &mut App)>;

const MODES: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

/// A choice of [`ThemeMode`] on `gpui_base::RadioGroup`: three cards, each
/// a small picture of the appearance it stands for, with the chosen one
/// ringed and its label in the foreground color.
///
/// Base owns focus, Tab order, Enter and Space activation, the disabled
/// contract, and the radio roles. This type owns the look: the pictures,
/// the ring, and the labels. The pictures come from the theme's light and
/// dark configs, so a custom theme shows itself. The System card is the
/// light picture on its left half and the dark picture on its right.
///
/// ```
/// use gpui_cn::{Theme, ThemeMode, ThemeModePicker};
///
/// let _ = ThemeModePicker::new("theme-mode")
///     .value(ThemeMode::System)
///     .on_change(|mode, _, cx| Theme::change(*mode, cx));
/// ```
///
/// The id keys focus and motion, so it must be stable across frames and
/// unique under the nearest stateful ancestor.
#[derive(IntoElement)]
pub struct ThemeModePicker {
    id: ElementId,
    style: StyleRefinement,
    value: ThemeMode,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl ThemeModePicker {
    /// A picker with a stable id, showing [`ThemeMode::System`] chosen.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            value: ThemeMode::System,
            disabled: false,
            on_change: None,
        }
    }

    /// The chosen mode.
    pub fn value(mut self, value: ThemeMode) -> Self {
        self.value = value;
        self
    }

    /// Called with the mode of a card that is activated while another is
    /// chosen. Activating the chosen card does nothing. The mode comes by
    /// reference so `cx.listener` fits.
    pub fn on_change(
        mut self,
        handler: impl Fn(&ThemeMode, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// The id of one card: the picker's id with the mode's name as the
    /// child, so tests and callers can find it.
    pub fn option_id(id: &ElementId, mode: ThemeMode) -> ElementId {
        ElementId::NamedChild(id.clone().into(), option_name(mode).into())
    }
}

impl Disableable for ThemeModePicker {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for ThemeModePicker {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn option_label(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "System",
        ThemeMode::Light => "Light",
        ThemeMode::Dark => "Dark",
    }
}

fn option_name(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

/// The colors of one picture, each a step from the config's surface toward
/// its ink in OKLab.
///
/// The amounts reproduce the reference app's appearance settings, sampled
/// at 2x. A light config draws one picture whether the window fills the
/// card or sits on a desktop; a dark config draws a lifted gray window with
/// a white card when it fills the card, and a dark window with a gray card
/// on the desktop, as the reference does.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Palette {
    desktop: Hsla,
    window: Hsla,
    rim: Hsla,
    heading: Hsla,
    subheading: Hsla,
    card: Hsla,
    row_bar: Hsla,
    row_line: Hsla,
    separator: Hsla,
}

impl Palette {
    fn derive(config: &ThemeConfig, far: bool) -> Self {
        let step = |amount: f32| mix(config.surface, config.ink, amount);
        let dark = lightness(config.ink) > lightness(config.surface);
        let lighter = if dark { config.ink } else { config.surface };
        let rim = |edge: Hsla| mix(edge, lighter, 0.09);
        match (dark, far) {
            // Light: window #f3f3f3 with a #f4f4f4 rim, heading #cdcdcd,
            // subheading and bars #dfdfdf, separators #f6f6f6, and a
            // #9f9f9f desktop. The thin bars are #f6f6f6 in the window
            // filling the card and #f3f3f3 on the desktop.
            (false, far) => {
                let desktop = step(0.385);
                let window = step(0.046);
                Self {
                    desktop,
                    window,
                    rim: rim(if far { desktop } else { window }),
                    heading: step(0.195),
                    subheading: step(0.125),
                    card: config.surface,
                    row_bar: step(0.125),
                    row_line: if far { window } else { step(0.034) },
                    separator: step(0.034),
                }
            }
            // Dark, filling the card: window #5d5d5d with a #6a6a6a rim,
            // heading #9f9f9f, subheading #8f8f8f, a white card with
            // #dfdfdf bars and #f6f6f6 lines.
            (true, false) => {
                let window = step(0.339);
                Self {
                    desktop: window,
                    window,
                    rim: rim(window),
                    heading: step(0.622),
                    subheading: step(0.556),
                    card: config.ink,
                    row_bar: step(0.877),
                    row_line: step(0.965),
                    separator: step(0.965),
                }
            }
            // Dark, on the desktop: desktop #5d5d5d, window #393939,
            // heading, bars, and separators #767676, subheading #8f8f8f,
            // card #4f4f4f.
            (true, true) => {
                let desktop = step(0.339);
                let bar = step(0.449);
                Self {
                    desktop,
                    window: step(0.169),
                    rim: rim(desktop),
                    heading: bar,
                    subheading: step(0.556),
                    card: step(0.275),
                    row_bar: bar,
                    row_line: bar,
                    separator: bar,
                }
            }
        }
    }
}

/// The size the reference app draws one card at, in points: 248 by 175.
/// Every picture is measured in this space, and it scales with the width
/// the card gets; the card keeps this ratio.
const CARD: Size<f32> = Size {
    width: 248.,
    height: 175.,
};

/// The card's corner radius in reference points.
const CARD_RADIUS: f32 = 10.;

/// The rim at rest and the ring of the chosen card, in reference points.
const RIM_WIDTH: f32 = 1.;
const RING_WIDTH: f32 = 2.;

/// One rounded rectangle of a picture, in reference points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    x: f32,
    y: f32,
    w: f32,
    h: Height,
    radius: f32,
    color: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Height {
    Fixed(f32),
    /// To the bottom of the card, with square bottom corners: the
    /// reference clips its panels there.
    ToBottom,
}

fn pill(x: f32, y: f32, w: f32, h: f32, color: Hsla) -> Shape {
    Shape {
        x,
        y,
        w,
        h: Height::Fixed(h),
        radius: h / 2.,
        color,
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32, color: Hsla) -> Shape {
    Shape {
        x,
        y,
        w,
        h: Height::Fixed(h),
        radius: 0.,
        color,
    }
}

fn panel(x: f32, y: f32, w: f32, radius: f32, color: Hsla) -> Shape {
    Shape {
        x,
        y,
        w,
        h: Height::ToBottom,
        radius,
        color,
    }
}

/// The window filling the card: a heading and a subheading centered over
/// a card of three rows. Measured from the reference at 2x.
fn near(p: &Palette) -> Vec<Shape> {
    let mut shapes = vec![
        Shape {
            x: 0.,
            y: 0.,
            w: CARD.width,
            h: Height::Fixed(CARD.height),
            radius: CARD_RADIUS,
            color: p.window,
        },
        pill(68., 38.5, 112., 8.5, p.heading),
        pill(39., 51.5, 170., 5.5, p.subheading),
        panel(23., 64.5, 202., 10., p.card),
    ];
    for row in 0..3 {
        let y = 34.5 * row as f32;
        shapes.push(pill(33.5, 82. + y, 64., 8., p.row_bar));
        shapes.push(pill(33., 97.5 + y, 94., 2.5, p.row_line));
        shapes.push(rect(23., 110.5 + y, 202., 1.5, p.separator));
    }
    shapes
}

/// The window on a desktop: the near picture pulled back so the window's
/// edge shows. Measured from the reference at 2x. The System card's dark
/// half has its thick bars `mirrored` to the right side of the rows, so
/// both halves show them beside the split; the rest is centered.
fn far(p: &Palette, mirrored: bool) -> Vec<Shape> {
    let mut shapes = vec![
        Shape {
            x: 0.,
            y: 0.,
            w: CARD.width,
            h: Height::Fixed(CARD.height),
            radius: CARD_RADIUS,
            color: p.desktop,
        },
        panel(13., 50.5, 222., 12., p.window),
        pill(103., 86.5, 37.5, 8., p.heading),
        pill(78.5, 99., 91., 4., p.subheading),
        panel(40., 112., 168., 8., p.card),
    ];
    for row in 0..3 {
        let y = 37. * row as f32;
        let bar = if mirrored { 150. } else { 48.5 };
        shapes.push(pill(bar, 123.5 + y, 49.5, 8., p.row_bar));
        shapes.push(pill(48.5, 139. + y, 151., 2.5, p.row_line));
        shapes.push(rect(40., 151.5 + y, 168., 1.5, p.separator));
    }
    shapes
}

enum Picture {
    Whole(Palette),
    Split { light: Palette, dark: Palette },
}

fn paint_shapes(
    shapes: &[Shape],
    bounds: Bounds<Pixels>,
    mask: Bounds<Pixels>,
    scale: f32,
    window: &mut Window,
) {
    window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
        for shape in shapes {
            let origin = bounds.origin + point(px(shape.x * scale), px(shape.y * scale));
            let (height, corners) = match shape.h {
                Height::Fixed(h) => (px(h * scale), Corners::all(px(shape.radius * scale))),
                Height::ToBottom => (
                    bounds.bottom() - origin.y,
                    Corners {
                        top_left: px(shape.radius * scale),
                        top_right: px(shape.radius * scale),
                        bottom_right: px(0.),
                        bottom_left: px(0.),
                    },
                ),
            };
            if height <= px(0.) {
                continue;
            }
            let quad = Bounds::new(origin, size(px(shape.w * scale), height));
            window.paint_quad(fill(quad, shape.color).corner_radii(corners));
        }
    });
}

fn paint_edge(bounds: Bounds<Pixels>, width: f32, color: Hsla, scale: f32, window: &mut Window) {
    window.paint_quad(quad(
        bounds,
        px(CARD_RADIUS * scale),
        gpui_kit::transparent_black(),
        px(width * scale),
        color,
        BorderStyle::Solid,
    ));
}

fn paint_card(picture: &Picture, ring: Hsla, bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = f32::from(bounds.size.width) / CARD.width;
    match picture {
        Picture::Whole(palette) => {
            paint_shapes(&near(palette), bounds, bounds, scale, window);
            paint_edge(bounds, RIM_WIDTH, palette.rim, scale, window);
        }
        Picture::Split { light, dark } => {
            let half = bounds.size.width / 2.;
            let left = Bounds::new(bounds.origin, size(half, bounds.size.height));
            let right = Bounds::new(
                bounds.origin + point(half, px(0.)),
                size(bounds.size.width - half, bounds.size.height),
            );
            paint_shapes(&far(light, false), bounds, left, scale, window);
            window.with_content_mask(Some(ContentMask { bounds: left }), |window| {
                paint_edge(bounds, RIM_WIDTH, light.rim, scale, window);
            });
            paint_shapes(&far(dark, true), bounds, right, scale, window);
            window.with_content_mask(Some(ContentMask { bounds: right }), |window| {
                paint_edge(bounds, RIM_WIDTH, dark.rim, scale, window);
            });
        }
    }
    if ring.a > 0. {
        paint_edge(bounds, RING_WIDTH, ring, scale, window);
    }
}

impl RenderOnce for ThemeModePicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let settings = Theme::global(cx);
        let fast = settings.motion.fast_transition();
        let pointer_cursors = settings.pointer_cursors;
        let light = &settings.light;
        let dark = &settings.dark;
        let pictures = [
            Picture::Split {
                light: Palette::derive(light, true),
                dark: Palette::derive(dark, true),
            },
            Picture::Whole(Palette::derive(light, false)),
            Picture::Whole(Palette::derive(dark, false)),
        ];
        let theme = cx.theme();
        let look = Look {
            // The reference rings the chosen card in the ink and paints
            // the other labels a step short of it.
            ring: theme.foreground(),
            label: theme.foreground(),
            muted_label: theme.muted_foreground(),
            focus_ring: theme.focus_ring(),
            focus_ring_spread: theme.metrics.focus_ring,
            focus_radius: theme.radius_lg(),
            disabled_opacity: theme.disabled_opacity,
            text_size: theme.text_control.size,
            line_height: theme.text_control.line_height,
        };
        let disabled = self.disabled;
        let value = self.value;
        let on_change = self.on_change;
        let id = self.id;

        // The group is a base element without observation, so the picker's
        // id goes on an observed wrapper around it.
        div()
            .id(id.clone())
            .test_support()
            .w_full()
            .refine_style(&self.style)
            .child(
                base::RadioGroup::new(ElementId::NamedChild(id.clone().into(), "group".into()))
                    .axis(Axis::Horizontal)
                    .flex()
                    .items_start()
                    .gap_3()
                    .w_full()
                    .children(MODES.into_iter().zip(pictures).enumerate().map(
                        |(ix, (mode, picture))| {
                            let option_id = ThemeModePicker::option_id(&id, mode);
                            let chosen = mode == value;
                            // The choice animates, not the colors, so a theme
                            // change repaints the ring and label at once.
                            let weight = transition(
                                ElementId::NamedChild(option_id.clone().into(), "chosen".into()),
                                if chosen { 1. } else { 0. },
                                fast.clone(),
                                window,
                                cx,
                            );
                            let ring = look.ring.alpha(look.ring.a * weight);
                            let label = look.muted_label.interpolate(&look.label, weight);
                            let focus_handle = window
                                .use_keyed_state(option_id.clone(), cx, |_, cx| cx.focus_handle())
                                .read(cx)
                                .clone();
                            let focus_visible =
                                focus_handle.is_focused(window) && window.last_input_was_keyboard();
                            let on_change = on_change.clone();
                            base::Radio::new(option_id)
                                .track_focus(&focus_handle)
                                .checked(chosen)
                                .disabled(disabled)
                                .accessibility_label(option_label(mode))
                                .set_position(ix + 1, MODES.len())
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2p5()
                                .map(|this| {
                                    if !disabled && pointer_cursors {
                                        this.cursor_pointer()
                                    } else {
                                        this.cursor_default()
                                    }
                                })
                                .when(disabled, |this| this.opacity(look.disabled_opacity))
                                .child(
                                    div()
                                        .relative()
                                        .w_full()
                                        .aspect_ratio(CARD.width / CARD.height)
                                        .rounded(look.focus_radius)
                                        .when(focus_visible, |this| {
                                            this.shadow(vec![gpui_kit::BoxShadow {
                                                color: look.focus_ring,
                                                offset: point(px(0.), px(0.)),
                                                blur_radius: px(0.),
                                                spread_radius: look.focus_ring_spread,
                                                inset: false,
                                            }])
                                        })
                                        .child(
                                            canvas(
                                                |_, _, _| {},
                                                move |bounds, _, window, _| {
                                                    paint_card(&picture, ring, bounds, window)
                                                },
                                            )
                                            .absolute()
                                            .inset_0(),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(look.text_size)
                                        .line_height(look.line_height)
                                        .text_color(label)
                                        .child(option_label(mode)),
                                )
                                .when_some(on_change, |this, on_change| {
                                    this.on_change(move |_, _, window, cx| {
                                        on_change(&mode, window, cx)
                                    })
                                })
                        },
                    )),
            )
    }
}

struct Look {
    ring: Hsla,
    label: Hsla,
    muted_label: Hsla,
    focus_ring: Hsla,
    focus_ring_spread: Pixels,
    focus_radius: Pixels,
    disabled_opacity: f32,
    text_size: Pixels,
    line_height: Pixels,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::to_hex;

    #[test]
    fn light_picture_reproduces_the_reference() {
        let palette = Palette::derive(&ThemeConfig::light(), false);
        assert_eq!(to_hex(palette.window), "#f3f3f3");
        assert_eq!(to_hex(palette.rim), "#f4f4f4");
        assert_eq!(to_hex(palette.heading), "#cdcdce");
        assert_eq!(to_hex(palette.subheading), "#dfdfdf");
        assert_eq!(to_hex(palette.card), "#ffffff");
        assert_eq!(to_hex(palette.row_bar), "#dfdfdf");
        assert_eq!(to_hex(palette.row_line), "#f6f6f6");
        assert_eq!(to_hex(palette.desktop), "#9e9fa0");
        let far = Palette::derive(&ThemeConfig::light(), true);
        assert_eq!(
            far.window, palette.window,
            "the light window is the same on the desktop"
        );
        assert_eq!(
            far.rim,
            mix(far.desktop, ThemeConfig::light().surface, 0.09)
        );
        assert_eq!(far.row_line, far.window, "thin bars on the desktop");
    }

    #[test]
    fn dark_pictures_reproduce_the_reference() {
        let near = Palette::derive(&ThemeConfig::dark(), false);
        assert_eq!(to_hex(near.window), "#5d5d5d");
        assert_eq!(to_hex(near.rim), "#6a6a6a");
        assert_eq!(to_hex(near.heading), "#9f9f9f");
        assert_eq!(to_hex(near.subheading), "#8f8f8f");
        assert_eq!(to_hex(near.card), "#ffffff");
        assert_eq!(to_hex(near.row_bar), "#dfdfdf");
        assert_eq!(to_hex(near.row_line), "#f6f6f6");
        let far = Palette::derive(&ThemeConfig::dark(), true);
        assert_eq!(to_hex(far.desktop), "#5d5d5d");
        assert_eq!(to_hex(far.window), "#393939");
        assert_eq!(to_hex(far.heading), "#767676");
        assert_eq!(to_hex(far.subheading), "#8f8f8f");
        assert_eq!(to_hex(far.card), "#4f4f4f");
        assert_eq!(to_hex(far.row_bar), "#767676");
        assert_eq!(far.separator, far.row_bar);
        assert_eq!(far.row_line, far.row_bar);
    }

    #[test]
    fn pictures_fit_the_card_and_run_off_its_bottom() {
        let light = Palette::derive(&ThemeConfig::light(), false);
        let plain = far(&light, false);
        let mirrored = far(&light, true);
        for shape in near(&light).iter().chain(&plain).chain(&mirrored) {
            assert!(
                shape.x >= 0. && shape.x + shape.w <= CARD.width,
                "{shape:?}"
            );
            assert!(shape.y >= 0., "{shape:?}");
        }
        assert!(plain.last().unwrap().y > CARD.height, "the clip drops it");
        assert_eq!(mirrored[1].x, plain[1].x);
        assert_eq!(mirrored[2].x, plain[2].x);
        assert_eq!(mirrored[4].x, plain[4].x);
        assert_eq!(mirrored[6], plain[6]);
        assert!((mirrored[5].x - (CARD.width - plain[5].x - plain[5].w)).abs() < 1.);
        assert!(mirrored[5].x > CARD.width / 2.);
    }

    #[test]
    fn a_custom_config_is_told_apart_by_its_ink() {
        let mut config = ThemeConfig::light();
        config.surface = crate::theme::hex("#101820");
        config.ink = crate::theme::hex("#f0f0f0");
        let palette = Palette::derive(&config, false);
        assert_eq!(
            palette.card, config.ink,
            "a dark config gets the dark picture"
        );
        assert!(lightness(palette.window) > lightness(config.surface));
    }

    #[test]
    fn option_ids_are_named_children_of_the_picker() {
        let id = ElementId::from("theme-mode");
        assert_eq!(
            ThemeModePicker::option_id(&id, ThemeMode::Dark),
            ElementId::NamedChild(id.into(), "dark".into())
        );
    }
}
