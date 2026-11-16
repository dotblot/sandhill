//! Compact static meters shared by host resources and provider quotas.
use gpui_kit::*;

/// `value` is a fraction, 0..=1. Missing data has a dashed, slashed track,
/// visually different from a known zero. The meter never implies activity.
pub(crate) fn ring(value: Option<f32>, color: Hsla, track: Hsla) -> AnyElement {
    ring_track(value, color, track, 14., true)
}

fn ring_track(value: Option<f32>, color: Hsla, track: Hsla, size: f32, slash: bool) -> AnyElement {
    let value = value.filter(|v| v.is_finite()).map(|v| v.clamp(0., 1.));
    let radius = size / 2. - 1.5;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            let at = |angle: f32| {
                point(
                    center.x + px(radius * angle.cos()),
                    center.y + px(radius * angle.sin()),
                )
            };
            let start = -std::f32::consts::FRAC_PI_2;
            let mut background = PathBuilder::stroke(px(1.5));
            if value.is_none() {
                background = background.dash_array(&[px(1.5), px(1.5)]);
            }
            background.move_to(at(start));
            for step in 1..=40 {
                background.line_to(at(start + std::f32::consts::TAU * step as f32 / 40.));
            }
            if let Ok(path) = background.build() {
                window.paint_path(path, track);
            }
            if let Some(value) = value {
                if value > 0. {
                    let mut arc = PathBuilder::stroke(px(1.5));
                    arc.move_to(at(start));
                    let steps = (40. * value).ceil() as usize;
                    for step in 1..=steps {
                        arc.line_to(at(
                            start + std::f32::consts::TAU * value * step as f32 / steps as f32
                        ));
                    }
                    if let Ok(path) = arc.build() {
                        window.paint_path(path, color);
                    }
                }
            } else if slash {
                let mut slash = PathBuilder::stroke(px(1.));
                slash.move_to(point(center.x - px(2.), center.y + px(2.)));
                slash.line_to(point(center.x + px(2.), center.y - px(2.)));
                if let Ok(path) = slash.build() {
                    window.paint_path(path, track);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
    .into_any_element()
}
