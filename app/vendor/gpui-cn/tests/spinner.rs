//! UI integration tests for `Spinner`.

use std::time::Duration;

use gpui_cn::{ReduceMotion, Spinner, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, WindowHandle, div, prelude::FluentBuilder as _, px, size,
    test::TestWindowExt as _,
};

/// A row of spinners that counts its renders, so a test can see when the
/// loop clock repaints it.
struct Harness {
    spinners: usize,
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
            .when(self.spinners > 0, |row| {
                row.child(Spinner::new("default")).child(
                    Spinner::new("large")
                        .size_8()
                        .accessibility_label("Syncing"),
                )
            })
            .children((2..self.spinners).map(|n| Spinner::new(("more", n))))
    }
}

fn setup(
    cx: &mut TestAppContext,
    motion: ReduceMotion,
    spinners: usize,
) -> (WindowHandle<Root>, Entity<Harness>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let mut harness = None;
    let handle = cx.open_window(size(px(600.), px(300.)), |window, cx| {
        let view = cx.new(|_| Harness {
            spinners,
            renders: 0,
        });
        harness = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    (handle, harness.unwrap())
}

/// Moves the fake clock past one 30 fps tick and runs what came due.
fn tick(cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
}

fn renders(harness: &Entity<Harness>, cx: &mut TestAppContext) -> usize {
    harness.read_with(cx, |harness, _| harness.renders)
}

#[gpui_kit::test]
fn a_spinner_is_a_status_and_a_rem_square(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx, ReduceMotion::Off, 2);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let spinner = window.find("default");
        assert_eq!(spinner.bounds().size.width, px(16.));
        assert_eq!(spinner.bounds().size.height, px(16.));
        assert_eq!(spinner.role(), Some(gpui_kit::Role::Status));
        assert_eq!(spinner.label(), Some("Loading"));
        let large = window.find("large");
        assert_eq!(large.bounds().size.width, px(32.));
        assert_eq!(large.label(), Some("Syncing"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_shown_spinner_repaints_its_view_each_tick(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off, 2);
    let before = renders(&harness, cx);
    tick(cx);
    assert!(renders(&harness, cx) > before, "the turn repaints the view");
}

#[gpui_kit::test]
fn with_reduce_motion_a_spinner_holds_still(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::On, 2);
    let before = renders(&harness, cx);
    tick(cx);
    tick(cx);
    assert_eq!(renders(&harness, cx), before, "no turn, no repaint");
}

#[gpui_kit::test]
fn a_removed_spinner_stops_asking_after_one_tick(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off, 2);
    harness.update(cx, |harness, cx| {
        harness.spinners = 0;
        cx.notify();
    });
    cx.run_until_parked();
    tick(cx);
    let settled = renders(&harness, cx);
    tick(cx);
    tick(cx);
    tick(cx);
    assert_eq!(
        renders(&harness, cx),
        settled,
        "nothing loops, nothing paints"
    );
}

#[gpui_kit::test]
fn ten_spinners_repaint_their_view_once_per_tick(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off, 10);
    let before = renders(&harness, cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(
        renders(&harness, cx),
        before + 30,
        "one shared 30 fps tick, one repaint of the view"
    );
}
