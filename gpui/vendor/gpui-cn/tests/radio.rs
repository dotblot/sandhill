//! UI integration tests for `Radio` and `RadioGroup`: real input through
//! a headless window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{Radio, RadioGroup};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Role, Styled as _,
    TestAppContext, Window, base::Disableable as _, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    chosen: Rc<RefCell<&'static str>>,
    disabled: &'static str,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let chosen = *self.chosen.borrow();
        let radio = |name: &'static str| {
            let slot = self.chosen.clone();
            Radio::new(name)
                .label(name)
                .checked(chosen == name)
                .disabled(self.disabled == name)
                .on_change(move |_, _, _| *slot.borrow_mut() = name)
        };
        div().p_4().child(
            RadioGroup::new("size")
                .child(radio("small"))
                .child(radio("medium"))
                .child(radio("large")),
        )
    }
}

fn setup(
    cx: &mut TestAppContext,
    disabled: &'static str,
) -> (gpui_kit::WindowHandle<Root>, Rc<RefCell<&'static str>>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let chosen = Rc::new(RefCell::new("small"));
    let slot = chosen.clone();
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            chosen: slot,
            disabled,
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    (handle, chosen)
}

#[gpui_kit::test]
fn the_group_and_its_radios_carry_roles_and_the_ring_is_reference_sized(cx: &mut TestAppContext) {
    let (handle, _) = setup(cx, "");
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("size").role(), Some(Role::RadioGroup));
        let medium = window.find("medium");
        assert_eq!(medium.role(), Some(Role::RadioButton));
        assert_eq!(medium.label(), Some("medium"));
        assert_eq!(medium.bounds().size.height, px(16.), "the ring is size-4");
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle group asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_chooses_the_radio_and_the_group_renders_the_choice_back(cx: &mut TestAppContext) {
    let (handle, chosen) = setup(cx, "");
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("large", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(*chosen.borrow(), "large");
}

#[gpui_kit::test]
fn tab_and_space_choose_by_keyboard(cx: &mut TestAppContext) {
    let (handle, chosen) = setup(cx, "");
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("medium").focused(), Some(true));
        let key = gpui_kit::Keystroke::parse("space").unwrap();
        window.press("space", cx);
        window.dispatch_event(
            gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke: key }),
            cx,
        );
    })
    .unwrap();
    assert_eq!(*chosen.borrow(), "medium");
}

#[gpui_kit::test]
fn a_disabled_radio_ignores_input(cx: &mut TestAppContext) {
    let (handle, chosen) = setup(cx, "large");
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("large", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(*chosen.borrow(), "small");
}

fn focused(
    handle: gpui_kit::WindowHandle<Root>,
    cx: &mut TestAppContext,
    id: &'static str,
) -> bool {
    cx.update_window(handle.into(), |_, window, _| {
        window.find(id).focused() == Some(true)
    })
    .unwrap()
}

fn press(handle: gpui_kit::WindowHandle<Root>, cx: &mut TestAppContext, key: &str) {
    cx.update_window(handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn arrow_keys_move_the_choice_and_the_focus_and_wrap(cx: &mut TestAppContext) {
    let (handle, chosen) = setup(cx, "");
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    press(handle, cx, "tab");
    assert!(
        focused(handle, cx, "small"),
        "Tab lands on the chosen radio"
    );
    for (key, expected) in [
        ("down", "medium"),
        ("right", "large"),
        ("down", "small"),
        ("up", "large"),
        ("left", "medium"),
    ] {
        press(handle, cx, key);
        assert_eq!(*chosen.borrow(), expected, "{key}");
        assert!(focused(handle, cx, expected), "{key}: focus follows");
    }
}

#[gpui_kit::test]
fn the_arrows_skip_a_disabled_radio(cx: &mut TestAppContext) {
    let (handle, chosen) = setup(cx, "medium");
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    press(handle, cx, "tab");
    press(handle, cx, "down");
    assert_eq!(*chosen.borrow(), "large");
}
