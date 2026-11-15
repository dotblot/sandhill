use gpui_kit::{
    App, Axis, ElementId, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled, Window,
    base::{
        self, Disableable, StyledExt as _, TestSupportExt as _,
        slider::{SliderEvent, SliderState, SliderValue},
    },
    div, point,
    prelude::FluentBuilder as _,
    px, relative,
};

use crate::{ActiveTheme as _, Theme};

/// A shadcn-style slider on `gpui_base::Slider`: a rounded track, a filled
/// range from the start to the thumb, and a round thumb.
///
/// Base owns the pointer behavior (a press on the track moves the thumb,
/// a drag follows the pointer) and the slider role with its value. The
/// range, the step, and the value live in a
/// [`SliderState`](gpui_kit::base::slider::SliderState), which emits a
/// `Change` event as the value moves and a `Release` event when the
/// pointer lets go. This type owns the look and the keyboard: with focus,
/// the arrow keys move the thumb one step, Page Up and Page Down move it a
/// tenth of the range, and Home and End go to the ends.
///
/// With [`stops`](Self::stops) the track shows a tick at each of `n` evenly
/// spaced values. The state's step should be the range divided by `n - 1`,
/// so the thumb lands on the ticks.
///
/// ```no_run
/// use gpui_cn::{Slider, SliderState};
/// use gpui_kit::{AppContext as _, Context};
///
/// fn effort(cx: &mut Context<()>) -> Slider {
///     let state = cx.new(|_| SliderState::new().min(0.).max(5.).step(1.).default_value(2.));
///     Slider::new(&state).stops(6)
/// }
/// ```
///
/// The look follows the switch and the radio: a thin fully rounded rail in
/// a translucent ink, the filled range in the primary color, and a
/// round white thumb with a hairline and a soft shadow, centered on the rail.
/// The rail is inset by half the thumb at each end, so the thumb at either
/// end stays inside the control's box, and the ticks of `stops` are
/// centered on the stop positions by the same math.
///
/// `Styled` refinements apply to the control's box, which is as wide as its
/// parent by default.
#[derive(IntoElement)]
#[non_exhaustive]
pub struct Slider {
    state: Entity<SliderState>,
    id: ElementId,
    style: StyleRefinement,
    disabled: bool,
    stops: Option<usize>,
    accessibility_label: Option<SharedString>,
    tab_index: isize,
    tab_stop: bool,
}

impl Slider {
    /// A slider on `state`. The id is derived from the state.
    pub fn new(state: &Entity<SliderState>) -> Self {
        Self {
            state: state.clone(),
            id: ("cn-slider", state.entity_id()).into(),
            style: StyleRefinement::default(),
            disabled: false,
            stops: None,
            accessibility_label: None,
            tab_index: 0,
            tab_stop: true,
        }
    }

    /// The control's id, for tests and focus.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Shows a tick at each of `count` evenly spaced values, the ends
    /// included. Fewer than two ticks show nothing.
    pub fn stops(mut self, count: usize) -> Self {
        self.stops = Some(count);
        self
    }

    /// The number of ticks the track shows, if it shows any.
    pub fn stop_count(&self) -> Option<usize> {
        self.stops
    }

    /// The name a screen reader announces.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// The focus traversal index. The default is `0`.
    pub fn tab_index(mut self, tab_index: isize) -> Self {
        self.tab_index = tab_index;
        self
    }

    /// Whether Tab reaches the slider. The default is `true`.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
}

impl Disableable for Slider {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for Slider {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The value a key moves a slider to, or `None` when the key does nothing
/// or the value does not change.
fn nudged(key: &str, value: f32, min: f32, max: f32, step: f32) -> Option<f32> {
    if ![value, min, max, step].iter().all(|n| n.is_finite()) || min > max {
        return None;
    }
    let page = ((max - min) / 10.).max(step);
    let next = match key {
        "right" | "up" => value + step,
        "left" | "down" => value - step,
        "pageup" => value + page,
        "pagedown" => value - page,
        "home" => min,
        "end" => max,
        _ => return None,
    }
    .clamp(min, max);
    ((next - value).abs() > f32::EPSILON).then_some(next)
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let disabled = self.disabled;
        let percentage = self.state.read(cx).percentage().end.clamp(0., 1.);
        let (rail, range, thumb, border, tick, tick_on, ring, lift) = {
            let theme = cx.theme();
            (
                theme.progress_track,
                theme.primary(),
                theme.switch_thumb,
                theme.field_border,
                theme.muted_foreground(),
                theme.solid_foreground,
                theme.focus_ring(),
                theme.base.shadow.sm.clone(),
            )
        };
        let (track_height, thumb_size, tick_size, ring_spread, hit_height, radius, strength) = {
            let theme = cx.theme();
            let metrics = &theme.metrics;
            (
                metrics.slider_track,
                metrics.slider_thumb,
                metrics.slider_tick,
                metrics.focus_ring,
                if theme.touch {
                    metrics.control_md
                } else {
                    metrics.slider_thumb
                },
                theme.radius_full(),
                theme.disabled_opacity,
            )
        };
        let focus_handle = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let focus_visible = focus_handle.is_focused(window) && window.last_input_was_keyboard();
        let state = self.state.clone();
        let track_id = ElementId::NamedChild(self.id.clone().into(), "track".into());
        let thumb_id = ElementId::NamedChild(self.id.clone().into(), "thumb".into());
        let stops = self.stops.filter(|count| *count >= 2);
        let tick_id = self.id.clone();
        let ticks = stops.into_iter().flat_map(move |count| {
            let tick_id = tick_id.clone();
            (0..count).map(move |index| {
                let at = index as f32 / (count - 1) as f32;
                div()
                    .id(ElementId::NamedChild(
                        tick_id.clone().into(),
                        format!("tick-{index}").into(),
                    ))
                    .test_support()
                    .absolute()
                    .top(relative(0.5))
                    .left(relative(at))
                    .mt(-tick_size / 2.)
                    .ml(-tick_size / 2.)
                    .size(tick_size)
                    .rounded(radius)
                    .bg(if at <= percentage + f32::EPSILON {
                        tick_on
                    } else {
                        tick
                    })
            })
        });

        div()
            .id(self.id)
            .test_support()
            .flex()
            .items_center()
            .w_full()
            .h(hit_height)
            .when(disabled, |this| this.opacity(strength))
            .map(|this| {
                if !disabled && pointer_cursors {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .when_some(self.accessibility_label, |this, label| {
                this.aria_label(label)
            })
            .refine_style(&self.style)
            .when(!disabled, |this| {
                let focus = focus_handle.clone();
                let handle = focus_handle
                    .tab_index(self.tab_index)
                    .tab_stop(self.tab_stop);
                this.track_focus(&handle)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.focus(&focus, cx);
                    })
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let (value, min, max, step) = {
                            let state = state.read(cx);
                            (
                                state.value().end(),
                                state.min_value(),
                                state.max_value(),
                                state.step_value(),
                            )
                        };
                        if let Some(next) =
                            nudged(event.keystroke.key.as_str(), value, min, max, step)
                        {
                            cx.stop_propagation();
                            state.update(cx, |state, cx| {
                                state.set_value(next, window, cx);
                                let value = SliderValue::Single(next);
                                cx.emit(SliderEvent::Change(value));
                                cx.emit(SliderEvent::Release(value));
                            });
                        }
                    })
            })
            .child(
                base::Slider::new(&self.state)
                    .axis(Axis::Horizontal)
                    .disabled(disabled)
                    .flex()
                    .items_center()
                    .w_full()
                    .h(hit_height)
                    .child(
                        base::SliderTrack::new(&self.state)
                            .axis(Axis::Horizontal)
                            .disabled(disabled)
                            .flex()
                            .items_center()
                            .w_full()
                            .h(hit_height)
                            .child(
                                div().w_full().mx(thumb_size / 2.).child(
                                    base::SliderIndicator::new(&self.state)
                                        .relative()
                                        .w_full()
                                        .h(track_height)
                                        .child(
                                            div()
                                                .id(track_id)
                                                .test_support()
                                                .absolute()
                                                .size_full()
                                                .rounded(radius)
                                                .bg(rail)
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top_0()
                                                        .bottom_0()
                                                        .left_0()
                                                        .w(relative(percentage))
                                                        .rounded(radius)
                                                        .bg(range),
                                                ),
                                        )
                                        .children(ticks)
                                        .child(
                                            base::SliderThumb::new(&self.state)
                                                .axis(Axis::Horizontal)
                                                .disabled(disabled)
                                                .absolute()
                                                .top(relative(0.5))
                                                .left(relative(percentage))
                                                .mt(-thumb_size / 2.)
                                                .ml(-thumb_size / 2.)
                                                .size(thumb_size)
                                                .child(
                                                    div()
                                                        .id(thumb_id)
                                                        .test_support()
                                                        .size_full()
                                                        .rounded(radius)
                                                        .bg(thumb)
                                                        .border_1()
                                                        .border_color(border)
                                                        .shadow(if focus_visible {
                                                            let mut shadows = lift.clone();
                                                            shadows.push(gpui_kit::BoxShadow {
                                                                color: ring,
                                                                offset: point(px(0.), px(0.)),
                                                                blur_radius: px(0.),
                                                                spread_radius: ring_spread,
                                                                inset: false,
                                                            });
                                                            shadows
                                                        } else {
                                                            lift
                                                        }),
                                                ),
                                        ),
                                ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_never_panics_on_a_reversed_or_broken_range() {
        assert_eq!(nudged("right", 1., 5., 0., 1.), None, "min above max");
        assert_eq!(nudged("right", 1., 0., f32::NAN, 1.), None);
        assert_eq!(nudged("end", 1., 0., 5., f32::INFINITY), None);
        assert_eq!(nudged("left", f32::NAN, 0., 5., 1.), None);
        assert_eq!(nudged("right", 2., 0., 5., 1.), Some(3.));
        assert_eq!(nudged("end", 2., 0., 5., 1.), Some(5.));
        assert_eq!(nudged("right", 5., 0., 5., 1.), None);
    }

    #[test]
    fn keys_move_the_value_one_step_and_clamp_at_the_ends() {
        assert_eq!(nudged("right", 2., 0., 5., 1.), Some(3.));
        assert_eq!(nudged("up", 2., 0., 5., 1.), Some(3.));
        assert_eq!(nudged("left", 2., 0., 5., 1.), Some(1.));
        assert_eq!(nudged("down", 2., 0., 5., 1.), Some(1.));
        assert_eq!(nudged("home", 2., 0., 5., 1.), Some(0.));
        assert_eq!(nudged("end", 2., 0., 5., 1.), Some(5.));
        assert_eq!(nudged("right", 5., 0., 5., 1.), None, "already at the end");
        assert_eq!(nudged("left", 0., 0., 5., 1.), None);
        assert_eq!(nudged("pageup", 10., 0., 100., 1.), Some(20.));
        assert_eq!(nudged("pagedown", 3., 0., 100., 1.), Some(0.));
        assert_eq!(nudged("a", 2., 0., 5., 1.), None);
    }

    #[test]
    fn a_page_is_never_smaller_than_a_step() {
        assert_eq!(nudged("pageup", 0., 0., 5., 1.), Some(1.));
    }
}
