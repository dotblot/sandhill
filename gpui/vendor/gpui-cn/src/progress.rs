use std::time::Duration;

use std::f32::consts::TAU;

use gpui_kit::{
    App, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, PathBuilder,
    Pixels, Point, RenderOnce, SharedString, StyleRefinement, Styled, TestSupportExt as _, Window,
    base::{self, ProgressIndicator, ProgressTrack, StyledExt as _, transition},
    canvas, div, ease_in_out, point,
    prelude::FluentBuilder as _,
    px, relative,
};

use crate::{ActiveTheme as _, Theme, looping};

/// A shadcn-style progress bar, or a circular progress indicator, on
/// `gpui_base::Progress`.
///
/// A ring and a bar are one concept in two shapes: work that is done by a
/// percentage, or work of unknown length. [`circular`](Self::circular)
/// draws the ring: a track with an arc that grows clockwise from twelve
/// o'clock, in the inherited text color at the theme's ring size and stroke
/// (the context meter of a composer is one). With no value the ring is the
/// spinner's turning arc, and [`Spinner`](crate::Spinner) is that.
///
/// Base owns the progress role and the value assistive technology reads.
/// This type owns the look, which is shadcn's `Progress`: as wide as its
/// parent, `h-2` (8px at the default font size) on the rem scale, fully
/// rounded, a track of `primary` at 20% (the theme's `progress_track`),
/// and an indicator of `primary`.
///
/// ```
/// use gpui_cn::Progress;
/// use gpui_kit::Styled as _;
///
/// let _ = Progress::new("upload").value(40.);
/// let _ = Progress::new("sync").accessibility_label("Syncing");
/// let _ = Progress::new("thin").value(None).h_1();
/// ```
///
/// A value is a percentage, clamped to `0..=100`. With no value the bar is
/// indeterminate, as in Radix: a segment slides across the track and
/// repeats, and under reduced motion it holds still in the middle. The
/// sweep repaints at 60 fps on the app's shared loop clock, not at the
/// display rate, and every indeterminate bar on screen repaints on the
/// same tick. A new
/// value moves the indicator with the theme's slide motion, shadcn's
/// `transition-all`.
///
/// The id keys the motion, so it must be stable across frames.
#[derive(IntoElement)]
pub struct Progress {
    id: ElementId,
    style: StyleRefinement,
    value: Option<f32>,
    accessibility_label: Option<SharedString>,
    circular: bool,
}

impl Progress {
    /// An indeterminate bar with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            value: None,
            accessibility_label: None,
            circular: false,
        }
    }

    /// Draws a ring in place of the bar. Its size is the theme's ring
    /// size, and `Styled` sizes it. With a value the arc runs clockwise
    /// from the top; with none, an arc turns, as a spinner's.
    pub fn circular(mut self) -> Self {
        self.circular = true;
        self
    }

    /// Whether the indicator is a ring.
    pub fn is_circular(&self) -> bool {
        self.circular
    }

    /// The percentage done, clamped to `0..=100`, or `None` for work whose
    /// length is not known.
    pub fn value(mut self, value: impl Into<Option<f32>>) -> Self {
        self.value = value.into().map(|value| value.clamp(0., 100.));
        self
    }

    /// The percentage done, or `None` when the bar is indeterminate. The
    /// builder takes the name `value`, so the reader says what the value is.
    pub fn percentage(&self) -> Option<f32> {
        self.value
    }

    /// The name a screen reader announces.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
}

impl Styled for Progress {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// How long one sweep of the indeterminate segment takes: one second, as
/// gpui-kit's progress bar. shadcn has no indeterminate bar.
const SWEEP: Duration = Duration::from_secs(1);

/// How often the sweep repaints: 60 fps. The segment crosses a wide track
/// in half a second at its fastest, and at 30 fps its steps show.
const SWEEP_FPS: u32 = 60;

/// How far in from each end the still indeterminate segment sits under
/// reduced motion, as a fraction of the track: the middle 35%, as
/// gpui-kit's.
const STILL_INSET: f32 = 0.325;

/// Where the indeterminate segment spans at `delta` through a sweep, as
/// fractions of the track: its trailing edge eases to the far end over the
/// whole sweep, and its leading edge follows over the second half.
fn sweep(delta: f32) -> (f32, f32) {
    let start = ease_in_out(((delta - 0.5) / 0.5).clamp(0., 1.));
    let end = 1. - ease_in_out(1. - delta);
    (start, end.max(start))
}

/// How long one turn of the indeterminate ring takes: one second, as
/// Tailwind's `animate-spin`.
const TURN: Duration = Duration::from_secs(1);

/// How often the turn repaints: 30 fps, which a small ring shows as smooth.
const TURN_FPS: u32 = 30;

/// How much of a turn the indeterminate arc covers: three quarters, as
/// Lucide's `loader-circle`.
const SPINNER_ARC: f32 = 0.75;

/// The arc of the indeterminate ring at `phase` through a turn, as
/// (start, length) in turns clockwise from the top. The spinner and every
/// indeterminate ring draw this arc.
pub(crate) fn indeterminate_arc(phase: f32) -> (f32, f32) {
    (phase.rem_euclid(1.), SPINNER_ARC)
}

/// The point on a circle at `fraction` of a turn clockwise from the top.
fn on_circle(center: Point<Pixels>, radius: Pixels, fraction: f32) -> Point<Pixels> {
    let angle = fraction * TAU;
    point(
        center.x + radius * angle.sin(),
        center.y - radius * angle.cos(),
    )
}

/// Paints the arc that starts `start` of a turn clockwise from the top and
/// runs `length` of a turn. A full turn is two half arcs, because an arc
/// cannot end where it began.
fn paint_arc(
    center: Point<Pixels>,
    radius: Pixels,
    (start, length): (f32, f32),
    width: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    if length <= 0. {
        return;
    }
    let mut path = PathBuilder::stroke(width);
    let radii = point(radius, radius);
    path.move_to(on_circle(center, radius, start));
    if length >= 1. {
        path.arc_to(
            radii,
            px(0.),
            false,
            true,
            on_circle(center, radius, start + 0.5),
        );
        path.arc_to(radii, px(0.), false, true, on_circle(center, radius, start));
    } else {
        path.arc_to(
            radii,
            px(0.),
            length > 0.5,
            true,
            on_circle(center, radius, start + length),
        );
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

impl Progress {
    /// The ring: a track and a growing arc for a value, or the turning arc
    /// for none.
    fn render_ring(self, window: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
        let slide = Theme::global(cx).motion.slide_transition();
        let color = self
            .style
            .text
            .color
            .unwrap_or_else(|| window.text_style().color);
        let (size, stroke, track_strength) = {
            let theme = cx.theme();
            (
                theme.metrics.ring_size,
                theme.metrics.ring_stroke,
                theme.progress_track.a,
            )
        };
        let id = self.id.clone();
        let arc = match self.value {
            Some(value) => {
                let fraction = transition(
                    ElementId::NamedChild(id.clone().into(), "value".into()),
                    value / 100.,
                    slide,
                    window,
                    cx,
                )
                .clamp(0., 1.);
                Some((0., fraction))
            }
            None if Theme::holds_still(cx) => Some(indeterminate_arc(0.)),
            None => Some(indeterminate_arc(looping::phase(
                TURN, TURN_FPS, window, cx,
            ))),
        };
        let has_track = self.value.is_some();
        let progress_id = ElementId::NamedChild(id.clone().into(), "progress".into());
        div()
            .id(id)
            .test_support()
            .flex_shrink_0()
            .size(size)
            .refine_style(&self.style)
            .child(
                base::Progress::new(progress_id)
                    .value(self.value.unwrap_or(0.))
                    .indeterminate(self.value.is_none())
                    .when_some(self.accessibility_label, |this, label| {
                        this.accessibility_label(label)
                    })
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                let radius =
                                    (bounds.size.width.min(bounds.size.height) - stroke) / 2.;
                                let center = bounds.center();
                                if has_track {
                                    let track = color.opacity(track_strength);
                                    paint_arc(center, radius, (0., 1.), stroke, track, window);
                                }
                                if let Some(arc) = arc {
                                    paint_arc(center, radius, arc, stroke, color, window);
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
            .into_any_element()
    }
}

impl RenderOnce for Progress {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.circular {
            return self.render_ring(window, cx);
        }
        let still = Theme::holds_still(cx);
        let slide = Theme::global(cx).motion.slide_transition();
        let (track, fill, radius) = {
            let theme = cx.theme();
            (theme.progress_track, theme.primary(), theme.radius_full())
        };
        let id = self.id;
        let child_id = |name: &'static str| ElementId::NamedChild(id.clone().into(), name.into());
        let fraction = self.value.map(|value| {
            transition(child_id("value"), value / 100., slide, window, cx).clamp(0., 1.)
        });

        // GPUI clips to rectangles, so the indicator carries the track's
        // radius itself rather than relying on the track to round it off.
        let indicator = ProgressIndicator::new()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .child(
                div()
                    .id(child_id("indicator"))
                    .test_support()
                    .size_full()
                    .rounded(radius)
                    .bg(fill),
            );
        let indicator = match fraction {
            Some(fraction) => indicator.w(relative(fraction)).into_any_element(),
            None if still => indicator
                .left(relative(STILL_INSET))
                .w(relative(1. - STILL_INSET * 2.))
                .into_any_element(),
            None => {
                let (start, end) = sweep(looping::phase(SWEEP, SWEEP_FPS, window, cx));
                indicator
                    .left(relative(start))
                    .w(relative(end - start))
                    .into_any_element()
            }
        };

        div()
            .id(id.clone())
            .test_support()
            .flex()
            .w_full()
            .h_2()
            .refine_style(&self.style)
            .child(
                base::Progress::new(child_id("progress"))
                    .value(self.value.unwrap_or(0.))
                    .indeterminate(self.value.is_none())
                    .when_some(self.accessibility_label, |this, label| {
                        this.accessibility_label(label)
                    })
                    .relative()
                    .size_full()
                    .child(
                        ProgressTrack::new()
                            .absolute()
                            .size_full()
                            .rounded(radius)
                            .bg(track),
                    )
                    .child(indicator),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_clamp_and_none_is_indeterminate() {
        assert_eq!(Progress::new("p").percentage(), None);
        assert_eq!(Progress::new("p").value(40.).percentage(), Some(40.));
        assert_eq!(Progress::new("p").value(150.).percentage(), Some(100.));
        assert_eq!(Progress::new("p").value(-5.).percentage(), Some(0.));
        assert_eq!(Progress::new("p").value(40.).value(None).percentage(), None);
    }

    #[test]
    fn the_ring_arc_starts_at_the_top_and_runs_clockwise() {
        let center = point(px(10.), px(10.));
        let radius = px(8.);
        let top = on_circle(center, radius, 0.);
        assert_eq!((top.x, top.y), (px(10.), px(2.)));
        let right = on_circle(center, radius, 0.25);
        assert!((right.x - px(18.)).abs() < px(0.001) && (right.y - px(10.)).abs() < px(0.001));
        let bottom = on_circle(center, radius, 0.5);
        assert!((bottom.x - px(10.)).abs() < px(0.001) && (bottom.y - px(18.)).abs() < px(0.001));
    }

    #[test]
    fn the_indeterminate_arc_turns_once_and_keeps_its_length() {
        assert_eq!(indeterminate_arc(0.), (0., SPINNER_ARC));
        assert_eq!(indeterminate_arc(0.25), (0.25, SPINNER_ARC));
        assert_eq!(indeterminate_arc(1.25).0, 0.25);
    }

    #[test]
    fn circular_is_a_shape_not_a_value() {
        assert!(!Progress::new("p").is_circular());
        let ring = Progress::new("p").circular().value(40.);
        assert!(ring.is_circular());
        assert_eq!(ring.percentage(), Some(40.));
    }

    #[test]
    fn a_sweep_grows_from_the_start_and_leaves_by_the_end() {
        assert_eq!(sweep(0.), (0., 0.));
        assert_eq!(sweep(0.5).0, 0.);
        assert_eq!(sweep(1.), (1., 1.));
        let (start, end) = sweep(0.75);
        assert!(start > 0. && end > start && end <= 1.);
    }
}
