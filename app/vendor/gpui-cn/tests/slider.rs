//! UI integration tests for `Slider`: real input through a headless
//! window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{Slider, SliderEvent, SliderState};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, base::Disableable as _, div, point, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<SliderState>,
    disabled: bool,
    stops: Option<usize>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let slider = Slider::new(&self.state)
            .id("effort")
            .accessibility_label("Effort")
            .disabled(self.disabled);
        let slider = match self.stops {
            Some(count) => slider.stops(count),
            None => slider,
        };
        div()
            .size_full()
            .p_4()
            .child(div().w(px(200.)).child(slider))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<SliderState>,
    events: Rc<RefCell<Vec<String>>>,
}

fn setup(cx: &mut TestAppContext, disabled: bool, stops: Option<usize>) -> Setup {
    setup_range(cx, disabled, stops, (0., 5., 1.), 2.)
}

fn setup_range(
    cx: &mut TestAppContext,
    disabled: bool,
    stops: Option<usize>,
    (min, max, step): (f32, f32, f32),
    start: f32,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let slot = Rc::new(RefCell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let state = cx.new(|_| {
            SliderState::new()
                .min(min)
                .max(max)
                .step(step)
                .default_value(start)
        });
        let log = events.clone();
        cx.subscribe(&state, move |_, _, event: &SliderEvent, _| {
            log.borrow_mut().push(match event {
                SliderEvent::Change(value) => format!("change {value}"),
                SliderEvent::Release(value) => format!("release {value}"),
            });
        })
        .detach();
        *slot.borrow_mut() = Some(state.clone());
        let harness = cx.new(|_| Harness {
            state,
            disabled,
            stops,
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let state = slot.borrow_mut().take().unwrap();
    Setup {
        handle,
        state,
        events,
    }
}

fn value(setup: &Setup, cx: &mut TestAppContext) -> f32 {
    setup.state.read_with(cx, |state, _| state.value().end())
}

#[gpui_kit::test]
fn the_control_is_a_thumb_high_and_as_wide_as_its_parent(cx: &mut TestAppContext) {
    let setup = setup(cx, false, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let bounds = window.find("effort").bounds();
        assert_eq!(bounds.size.width, px(200.));
        assert_eq!(bounds.size.height, px(16.), "the thumb diameter");
        assert_eq!(window.find("effort").label(), Some("Effort"));
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle slider asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_press_on_the_track_moves_the_thumb_to_the_nearest_step(cx: &mut TestAppContext) {
    let setup = setup(cx, false, Some(6));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click_at("effort", point(px(190.), px(8.)), cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 5.);
    assert_eq!(
        setup.events.borrow().last().map(String::as_str),
        Some("release 5")
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click_at("effort", point(px(2.), px(8.)), cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 0.);
}

#[gpui_kit::test]
fn dragging_the_thumb_follows_the_pointer(cx: &mut TestAppContext) {
    let setup = setup(cx, false, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let bounds = window.find("effort").bounds();
        let from = point(bounds.left() + bounds.size.width * 0.4, bounds.center().y);
        let to = point(bounds.left() + bounds.size.width * 0.8, bounds.center().y);
        window.drag(from, to, cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 4.);
}

#[gpui_kit::test]
fn tab_reaches_the_slider_and_the_arrows_home_and_end_move_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("effort").focused(), Some(true));
        window.press("right", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 3.);
    assert_eq!(
        setup.events.borrow().as_slice(),
        ["change 3", "release 3"],
        "a key press reports like a click"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("left", cx);
        window.press("left", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 1.);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("end", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 5.);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("right", cx);
        window.press("home", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 0.);
}

#[gpui_kit::test]
fn a_disabled_slider_ignores_pointer_and_keys(cx: &mut TestAppContext) {
    let setup = setup(cx, true, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click_at("effort", point(px(190.), px(8.)), cx);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_ne!(window.find("effort").focused(), Some(true));
        window.press("right", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 2.);
    assert!(setup.events.borrow().is_empty());
}

fn track() -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedChild(gpui_kit::ElementId::from("effort").into(), "track".into())
}

fn thumb() -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedChild(gpui_kit::ElementId::from("effort").into(), "thumb".into())
}

fn tick(index: usize) -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedChild(
        gpui_kit::ElementId::from("effort").into(),
        format!("tick-{index}").into(),
    )
}

#[gpui_kit::test]
fn the_rail_is_four_pixels_inset_by_half_the_thumb(cx: &mut TestAppContext) {
    let setup = setup(cx, false, Some(6));
    cx.update_window(setup.handle.into(), |_, window, _| {
        let control = window.find("effort").bounds();
        let rail = window.find(track()).bounds();
        assert_eq!(rail.size.height, px(4.));
        assert_eq!(rail.left() - control.left(), px(8.), "half the 16px thumb");
        assert_eq!(control.right() - rail.right(), px(8.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_thumb_at_either_end_stays_inside_the_control(cx: &mut TestAppContext) {
    let setup = setup(cx, false, Some(6));
    for (key, at_start) in [("home", true), ("end", false)] {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.press("tab", cx);
            window.press(key, cx);
            window.render_frame(cx);
            let control = window.find("effort").bounds();
            let rail = window.find(track()).bounds();
            let thumb = window.find(thumb()).bounds();
            assert!(thumb.left() >= control.left() && thumb.right() <= control.right());
            let edge = if at_start { rail.left() } else { rail.right() };
            assert!(
                (thumb.center().x - edge).abs() <= px(0.5),
                "centered on the rail's end"
            );
            assert!((thumb.center().y - rail.center().y).abs() <= px(0.5));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn each_tick_is_centered_on_the_thumb_when_the_value_is_at_its_stop(cx: &mut TestAppContext) {
    let setup = setup(cx, false, Some(6));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("home", cx);
        window.render_frame(cx);
    })
    .unwrap();
    for index in 0..6 {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let thumb = window.find(thumb()).bounds().center();
            let tick = window.find(tick(index)).bounds().center();
            assert!(
                (thumb.x - tick.x).abs() <= px(0.5) && (thumb.y - tick.y).abs() <= px(0.5),
                "stop {index}: thumb {thumb:?} tick {tick:?}"
            );
            window.press("right", cx);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn a_touch_drag_on_the_track_moves_the_thumb(cx: &mut TestAppContext) {
    use gpui_kit::{PlatformInput, TouchDragEvent, TouchPhase};
    let setup = setup(cx, false, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let bounds = window.find("effort").bounds();
        let y = bounds.center().y;
        let start = point(bounds.left() + bounds.size.width * 0.2, y);
        for (phase, at) in [
            (TouchPhase::Started, 0.2),
            (TouchPhase::Moved, 0.6),
            (TouchPhase::Moved, 1.0),
            (TouchPhase::Ended, 1.0),
        ] {
            window.dispatch_event(
                PlatformInput::TouchDrag(TouchDragEvent {
                    phase,
                    start_position: start,
                    position: point(bounds.left() + bounds.size.width * at, y),
                }),
                cx,
            );
            window.render_frame(cx);
        }
    })
    .unwrap();
    assert_eq!(value(&setup, cx), 5., "the finger dragged to the end");
    assert_eq!(
        setup.events.borrow().last().map(String::as_str),
        Some("release 5")
    );
}

fn click_at_fraction(setup: &Setup, fraction: f32, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let bounds = window.find("effort").bounds();
        let at = point(
            bounds.left() + (bounds.size.width - px(1.)) * fraction,
            bounds.center().y,
        );
        window.click_at("effort", at - bounds.origin, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_press_between_two_steps_snaps_to_the_nearer_one(cx: &mut TestAppContext) {
    let setup = setup(cx, false, None);
    for (fraction, expected) in [(0.45, 2.), (0.55, 3.), (0.05, 0.), (0.95, 5.), (0.3, 1.)] {
        click_at_fraction(&setup, fraction, cx);
        assert_eq!(value(&setup, cx), expected, "at {fraction}");
    }
}

#[gpui_kit::test]
fn an_empty_range_and_odd_stops_and_steps_do_not_break_the_slider(cx: &mut TestAppContext) {
    let same = setup_range(cx, false, Some(3), (3., 3., 1.), 3.);
    click_at_fraction(&same, 0.5, cx);
    assert_eq!(value(&same, cx), 3., "min equals max");
    for count in [0, 1] {
        let few = setup_range(cx, false, Some(count), (0., 5., 1.), 2.);
        click_at_fraction(&few, 1.0, cx);
        assert_eq!(
            value(&few, cx),
            5.,
            "{count} stops draw no ticks and still work"
        );
    }
    let odd = setup_range(cx, false, None, (0., 5., 2.), 0.);
    cx.update_window(odd.handle.into(), |_, window, cx| {
        window.activate_window();
        window.press("tab", cx);
        window.press("end", cx);
    })
    .unwrap();
    assert_eq!(
        value(&odd, cx),
        5.,
        "End goes to the max even when the step does not divide the range"
    );
    cx.update_window(odd.handle.into(), |_, window, cx| {
        window.press("right", cx);
    })
    .unwrap();
    assert!(
        value(&odd, cx) <= 5.,
        "a step past the end stays at the max"
    );
}
