//! UI integration tests for the circular `Progress`, which a spinner is
//! the unknown-length form of.

use std::time::Duration;

use gpui_cn::{Progress, ReduceMotion, Spinner, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    value: Option<f32>,
    renders: usize,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(
                Progress::new("meter")
                    .circular()
                    .value(self.value)
                    .accessibility_label("Context"),
            )
            .child(Spinner::new("spinner"))
    }
}

fn setup(
    cx: &mut TestAppContext,
    motion: ReduceMotion,
    value: Option<f32>,
) -> (gpui_kit::WindowHandle<Root>, Entity<Harness>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|_| Harness { value, renders: 0 });
        view = Some(harness.clone());
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    (handle, view.unwrap())
}

#[gpui_kit::test]
fn a_ring_with_a_value_is_sixteen_pixels_and_idle(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx, ReduceMotion::On, Some(57.));
    cx.update_window(handle.into(), |_, window, cx| {
        let ring = window.find("meter");
        assert_eq!(ring.bounds().size, size(px(16.), px(16.)));
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle ring asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_new_value_moves_the_arc_over_frames_unless_motion_is_reduced(cx: &mut TestAppContext) {
    let (handle, view) = setup(cx, ReduceMotion::Off, Some(57.));
    view.update(cx, |harness, cx| {
        harness.value = Some(90.);
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the arc is still moving"
        );
    })
    .unwrap();

    let (handle, view) = setup(cx, ReduceMotion::On, Some(57.));
    view.update(cx, |harness, cx| {
        harness.value = Some(90.);
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.simulate_next_frame(cx), 0, "the arc lands at once");
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_indeterminate_ring_turns_on_the_loop_clock_and_holds_still_when_reduced(
    cx: &mut TestAppContext,
) {
    let (_, view) = setup(cx, ReduceMotion::Off, None);
    let before = view.read_with(cx, |harness, _| harness.renders);
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert!(
        view.read_with(cx, |harness, _| harness.renders) > before,
        "the turn repaints the view"
    );

    let (_, view) = setup(cx, ReduceMotion::On, None);
    let before = view.read_with(cx, |harness, _| harness.renders);
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |harness, _| harness.renders),
        before,
        "no turn, no repaint"
    );
}

#[gpui_kit::test]
fn a_spinner_is_an_indeterminate_ring_of_its_own_size(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx, ReduceMotion::On, None);
    cx.update_window(handle.into(), |_, window, _| {
        let spinner = window.find("spinner").bounds();
        let arc = window
            .find(ElementId::NamedChild(
                ElementId::from("spinner").into(),
                "arc".into(),
            ))
            .bounds();
        assert_eq!(
            spinner, arc,
            "the spinner's arc is a circular Progress that fills it"
        );
        assert_eq!(spinner.size, size(px(16.), px(16.)));
        let ring = window.find("meter").bounds();
        assert_eq!(
            ring.size, spinner.size,
            "a ring with no value is the same size"
        );
    })
    .unwrap();
}
