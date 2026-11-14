//! UI integration tests for `Label`: real input through a headless window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Input, InputState, Label};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, base::Disableable as _, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<InputState>,
    disabled: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Label::new("label", "Email")
                    .for_field(&self.state)
                    .disabled(self.disabled),
            )
            .child(Input::new(&self.state).id("email"))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<InputState>,
}

fn setup(cx: &mut TestAppContext, disabled: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let state = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("you@example.com"));
        state.set(Some(input.clone()));
        let harness = cx.new(|_| Harness {
            state: input,
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
        state: state.take().unwrap(),
    }
}

#[gpui_kit::test]
fn the_label_carries_its_text_at_the_control_line_height(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, _| {
        let label = window.find("label");
        assert_eq!(
            label.bounds().size.height,
            px(16.),
            "the control line height"
        );
        assert!(label.bounds().size.width > px(0.), "the text was laid out");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_on_the_label_focuses_its_field(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("label", cx);
        window.render_frame(cx);
        assert!(
            setup
                .state
                .read(cx)
                .presentation()
                .focus_handle()
                .is_focused(window),
            "the label hands focus to its field"
        );
        window.input("a", cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "a");
}

#[gpui_kit::test]
fn a_disabled_label_does_not_focus_its_field(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("label", cx);
        window.render_frame(cx);
        assert!(
            !setup
                .state
                .read(cx)
                .presentation()
                .focus_handle()
                .is_focused(window)
        );
    })
    .unwrap();
}
