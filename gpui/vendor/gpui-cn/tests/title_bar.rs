//! UI integration tests for `TitleBar`.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{ActiveTheme as _, Button, TitleBar};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, base::TestSupportExt as _, div, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    inset: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            TitleBar::new()
                .inset(self.inset)
                .child(Button::new("action").ghost().label("Action")),
        )
    }
}

fn setup(cx: &mut TestAppContext, inset: bool) -> gpui_kit::WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(600.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness { inset });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    handle
}

#[gpui_kit::test]
fn the_bar_spans_the_window_at_its_height_and_hosts_its_children(cx: &mut TestAppContext) {
    let handle = setup(cx, true);
    cx.update_window(handle.into(), |_, window, cx| {
        let height = cx.theme().metrics.title_bar;
        let bar = window.find("title-bar").bounds();
        assert_eq!(bar.size.width, px(600.));
        assert_eq!(bar.size.height, height);
        assert_eq!(height, px(38.));
        let action = window.find("action").bounds();
        // The child sits after the platform inset, centered on the content
        // line 2px below the bar's top, level with the window controls.
        let expected_left = if cfg!(target_os = "macos") {
            px(88.)
        } else {
            px(12.)
        };
        assert_eq!(action.origin.x, expected_left);
        let offset = cx.theme().metrics.title_bar_content_offset;
        assert_eq!(
            action.origin.y + action.size.height / 2.,
            offset + (height - offset) / 2.
        );
        assert_eq!(offset, px(2.));
        // A press on a control is the control's, so a drag from it does not
        // move the window; the click still lands.
        window.click("action", cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn without_the_inset_the_children_start_at_the_edge(cx: &mut TestAppContext) {
    let handle = setup(cx, false);
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("action").bounds().origin.x, px(12.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn window_options_hide_the_system_title_bar(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        let options = TitleBar::window_options(cx);
        let titlebar = options.titlebar.expect("a title bar configuration");
        assert!(titlebar.appears_transparent);
        assert_eq!(
            titlebar.traffic_light_position,
            Some(gpui_kit::point(px(14.), px(14.)))
        );
        assert!(options.app_owns_titlebar_drag);
    });
}

/// A bar with a button, counting double-clicks the bar itself receives.
struct DoubleClicks {
    bar: Rc<Cell<usize>>,
    button: Rc<Cell<usize>>,
}

impl Render for DoubleClicks {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let bar = self.bar.clone();
        let button = self.button.clone();
        div().size_full().child(
            TitleBar::new()
                .on_double_click(move |_, _, _| bar.set(bar.get() + 1))
                .child(
                    Button::new("action")
                        .ghost()
                        .label("Action")
                        .on_click(move |_, _, _| button.set(button.get() + 1)),
                )
                .child(div().id("blank").test_support().flex_1().h_full()),
        )
    }
}

#[gpui_kit::test]
fn a_double_click_on_a_control_is_not_a_double_click_on_the_bar(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let bar = Rc::new(Cell::new(0));
    let button = Rc::new(Cell::new(0));
    let handle = cx.open_window(size(px(600.), px(400.)), |window, cx| {
        let view = cx.new(|_| DoubleClicks {
            bar: bar.clone(),
            button: button.clone(),
        });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.double_click("action", cx);
        window.render_frame(cx);
        assert_eq!(button.get(), 2, "the button saw both clicks");
        assert_eq!(bar.get(), 0, "the bar did not zoom");
        window.double_click("blank", cx);
        window.render_frame(cx);
        assert_eq!(bar.get(), 1, "a double-click on the bar itself does");
    })
    .unwrap();
}
