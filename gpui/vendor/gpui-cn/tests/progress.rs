//! UI integration tests for `Progress`.

use std::time::Duration;

use gpui_cn::{Progress, ReduceMotion, Spinner, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, div, px, size, test::TestWindowExt as _,
};

/// A 200px column of bars, one per value, that counts its renders so a
/// test can see when the loop clock repaints it.
struct Harness {
    values: Vec<(&'static str, Option<f32>)>,
    renders: usize,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().size_full().p_4().child(
            div().w(px(200.)).flex().flex_col().gap_4().children(
                self.values
                    .iter()
                    .map(|(id, value)| Progress::new(*id).value(*value)),
            ),
        )
    }
}

fn setup(
    cx: &mut TestAppContext,
    motion: ReduceMotion,
    values: Vec<(&'static str, Option<f32>)>,
) -> (gpui_kit::WindowHandle<Root>, Entity<Harness>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let mut harness = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let view = cx.new(|_| Harness { values, renders: 0 });
        harness = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    (handle, harness.unwrap())
}

/// Moves the fake clock past one 60 fps tick and runs what came due.
fn tick(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(20));
    cx.run_until_parked();
}

fn renders(harness: &Entity<Harness>, cx: &mut TestAppContext) -> usize {
    harness.read_with(cx, |harness, _| harness.renders)
}

fn indicator(id: &'static str) -> gpui_kit::ElementId {
    gpui_kit::ElementId::NamedChild(gpui_kit::ElementId::from(id).into(), "indicator".into())
}

#[gpui_kit::test]
fn the_indicator_covers_the_value_of_the_track(cx: &mut TestAppContext) {
    let (handle, _) = setup(
        cx,
        ReduceMotion::On,
        vec![
            ("forty", Some(40.)),
            ("over", Some(150.)),
            ("zero", Some(0.)),
        ],
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let track = window.find("forty").bounds();
        assert_eq!(track.size.width, px(200.));
        assert_eq!(track.size.height, px(8.), "h-2 at the default font size");
        let forty = window.find(indicator("forty")).bounds();
        assert_eq!(forty.size.width, px(80.));
        assert_eq!(forty.size.height, px(8.));
        assert_eq!(forty.origin.x, track.origin.x);
        assert_eq!(window.find(indicator("over")).bounds().size.width, px(200.));
        assert_eq!(window.find(indicator("zero")).bounds().size.width, px(0.));
    })
    .unwrap();
}

/// One bar whose value and height the test changes.
struct Moving {
    value: Option<f32>,
}

impl Render for Moving {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child(
            div()
                .w(px(200.))
                .child(Progress::new("bar").value(self.value).h(px(12.))),
        )
    }
}

#[gpui_kit::test]
fn a_new_value_slides_the_indicator_and_the_height_follows_the_root(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let moving = cx.new(|_| Moving { value: Some(0.) });
        view = Some(moving.clone());
        Root::new(moving, window, cx)
    });
    let view = view.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("bar").bounds().size.height, px(12.));
        let indicator = window.find(indicator("bar")).bounds();
        assert_eq!(indicator.size.width, px(0.));
        assert_eq!(indicator.size.height, px(12.));
    })
    .unwrap();
    view.update(cx, |view, cx| {
        view.value = Some(50.);
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the indicator is on its way"
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(indicator("bar")).bounds().size.width, px(100.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_indeterminate_bar_sweeps_at_60_fps(cx: &mut TestAppContext) {
    let (handle, harness) = setup(cx, ReduceMotion::Off, vec![("busy", None)]);
    let at = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, _| {
            window.find(indicator("busy")).bounds()
        })
        .unwrap()
    };
    let first = at(cx);
    let before = renders(&harness, cx);
    tick(cx);
    assert!(
        renders(&harness, cx) > before,
        "the sweep repaints the view"
    );
    cx.executor().advance_clock(Duration::from_millis(300));
    cx.run_until_parked();
    assert!(at(cx).size.width > first.size.width, "the segment grew");
    let before = renders(&harness, cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(renders(&harness, cx), before + 60);
}

#[gpui_kit::test]
fn a_bar_that_turns_determinate_stops_asking_after_one_tick(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off, vec![("busy", None)]);
    harness.update(cx, |harness, cx| {
        harness.values = vec![("busy", Some(40.))];
        cx.notify();
    });
    // Let the value's slide settle first; it ends.
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    let settled = renders(&harness, cx);
    tick(cx);
    tick(cx);
    assert_eq!(
        renders(&harness, cx),
        settled,
        "nothing sweeps, nothing paints"
    );
}

#[gpui_kit::test]
fn with_reduce_motion_an_indeterminate_bar_holds_still_in_the_middle(cx: &mut TestAppContext) {
    let (handle, harness) = setup(cx, ReduceMotion::On, vec![("busy", None)]);
    let before = renders(&harness, cx);
    tick(cx);
    tick(cx);
    assert_eq!(renders(&harness, cx), before, "no sweep, no repaint");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let track = window.find("busy").bounds();
        let segment = window.find(indicator("busy")).bounds();
        assert_eq!(segment.size.width, px(70.));
        assert_eq!(segment.origin.x - track.origin.x, px(65.));
    })
    .unwrap();
}

/// A 30 fps spinner laid out before a 60 fps bar, counting renders.
struct Mixed {
    renders: usize,
}

impl Render for Mixed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div()
            .size_full()
            .p_4()
            .child(Spinner::new("spinner"))
            .child(div().w(px(200.)).child(Progress::new("busy")))
    }
}

#[gpui_kit::test]
fn a_faster_loop_moves_the_shared_tick_earlier(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let mut view = None;
    cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let mixed = cx.new(|_| Mixed { renders: 0 });
        view = Some(mixed.clone());
        Root::new(mixed, window, cx)
    });
    cx.run_until_parked();
    let view = view.unwrap();
    let before = view.read_with(cx, |view, _| view.renders);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |view, _| view.renders),
        before + 60,
        "the bar's 60 fps wins over the spinner's 30"
    );
}
