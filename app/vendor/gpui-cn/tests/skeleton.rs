//! UI integration tests for `Skeleton`.

use std::time::Duration;

use gpui_cn::{ActiveTheme as _, ReduceMotion, Skeleton, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, WindowHandle, div, prelude::FluentBuilder as _, px, size,
    test::TestWindowExt as _,
};

/// A block in a 200px column that counts its renders, so a test can see
/// when the loop clock repaints it.
struct Harness {
    shown: bool,
    renders: usize,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().size_full().p_4().child(
            div()
                .w(px(200.))
                .when(self.shown, |column| column.child(Skeleton::new("line"))),
        )
    }
}

fn setup(cx: &mut TestAppContext, motion: ReduceMotion) -> (WindowHandle<Root>, Entity<Harness>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let mut harness = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let view = cx.new(|_| Harness {
            shown: true,
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
fn a_block_is_a_line_high_and_as_wide_as_its_parent(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx, ReduceMotion::Off);
    let line = cx.update(|cx| cx.theme().text_control.line_height);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let block = window.find("line").bounds();
        assert_eq!(block.size.width, px(200.));
        assert_eq!(block.size.height, line);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_shown_block_pulses_at_30_fps(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off);
    let before = renders(&harness, cx);
    tick(cx);
    assert!(
        renders(&harness, cx) > before,
        "the pulse repaints the view"
    );
    let before = renders(&harness, cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(renders(&harness, cx), before + 30);
}

#[gpui_kit::test]
fn with_reduce_motion_the_block_holds_still(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::On);
    let before = renders(&harness, cx);
    tick(cx);
    tick(cx);
    assert_eq!(renders(&harness, cx), before, "no pulse, no repaint");
}

#[gpui_kit::test]
fn a_removed_block_stops_asking_after_one_tick(cx: &mut TestAppContext) {
    let (_, harness) = setup(cx, ReduceMotion::Off);
    harness.update(cx, |harness, cx| {
        harness.shown = false;
        cx.notify();
    });
    cx.run_until_parked();
    tick(cx);
    let settled = renders(&harness, cx);
    tick(cx);
    tick(cx);
    assert_eq!(
        renders(&harness, cx),
        settled,
        "nothing pulses, nothing paints"
    );
}
