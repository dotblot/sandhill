//! UI integration tests for `Switch`: real input through a headless
//! window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{ReduceMotion, Switch, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, base::Disableable as _, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    checked: Rc<Cell<bool>>,
    changes: Rc<Cell<usize>>,
    disabled: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let checked = self.checked.clone();
        let changes = self.changes.clone();
        div().size_full().p_4().child(
            Switch::new("wifi")
                .checked(self.checked.get())
                .disabled(self.disabled)
                .accessibility_label("Wi-Fi")
                .on_change(move |next, _, _| {
                    checked.set(*next);
                    changes.set(changes.get() + 1);
                }),
        )
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    checked: Rc<Cell<bool>>,
    changes: Rc<Cell<usize>>,
}

fn setup(cx: &mut TestAppContext, disabled: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let checked = Rc::new(Cell::new(false));
    let changes = Rc::new(Cell::new(0));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            checked: checked.clone(),
            changes: changes.clone(),
            disabled,
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        checked,
        changes,
    }
}

fn tap(window: &mut Window, key: &'static str, cx: &mut gpui_kit::App) {
    let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
    window.press(key, cx);
    window.dispatch_event(
        gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }),
        cx,
    );
    window.render_frame(cx);
}

#[gpui_kit::test]
fn the_track_is_the_reference_size_and_carries_the_name(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let switch = window.find("wifi");
        assert_eq!(switch.label(), Some("Wi-Fi"));
        assert_eq!(switch.bounds().size, size(px(32.), px(20.)));
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle switch asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn on_touch_the_track_is_a_uiswitch_inside_a_row_high_target(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("wifi").bounds();
        assert_eq!(bounds.size.width, px(51.));
        assert_eq!(bounds.size.height, px(44.), "the HIG hit target");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_reports_the_next_value_each_time(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("wifi", cx);
    })
    .unwrap();
    assert!(setup.checked.get());
    assert_eq!(setup.changes.get(), 1);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("wifi", cx);
    })
    .unwrap();
    assert!(!setup.checked.get());
    assert_eq!(setup.changes.get(), 2);
}

#[gpui_kit::test]
fn the_thumb_slides_rather_than_jumps(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("wifi", cx);
        window.render_frame(cx);
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the slide asks for frames until it lands"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn with_reduce_motion_the_thumb_lands_in_one_frame(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("wifi", cx);
        window.render_frame(cx);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "no slide, so no frame is asked for"
        );
    })
    .unwrap();
    assert!(setup.checked.get());
}

#[gpui_kit::test]
fn tab_reaches_the_switch_and_space_and_enter_flip_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("wifi").focused(), Some(true));
        tap(window, "space", cx);
    })
    .unwrap();
    assert!(setup.checked.get());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        tap(window, "enter", cx);
    })
    .unwrap();
    assert!(!setup.checked.get());
    assert_eq!(setup.changes.get(), 2);
}

#[gpui_kit::test]
fn a_disabled_switch_ignores_input(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("wifi", cx);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_ne!(window.find("wifi").focused(), Some(true));
        tap(window, "space", cx);
    })
    .unwrap();
    assert!(!setup.checked.get());
    assert_eq!(setup.changes.get(), 0);
}
