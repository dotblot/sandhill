//! UI integration tests for `Popover`: real input through a headless window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Button, Popover, ReduceMotion, Theme};
use gpui_kit::{
    AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, TestAppContext, Window, WindowHandle, base::Root,
    base::TestSupportExt as _, div, px, size, test::TestWindowExt as _,
};

/// A popover at the top or the bottom of the window, controlled by the
/// harness or keeping its own open state.
struct Harness {
    at_bottom: bool,
    open: Option<Rc<Cell<bool>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut popover = Popover::new("pop")
            .trigger(Button::new("pop-open").label("Open"))
            .content(|_, _| div().id("pop-body").test_support().size(px(200.)));
        if let Some(open) = &self.open {
            let setter = open.clone();
            popover = popover
                .open(open.get())
                .on_open_change(move |value, _, _| setter.set(value));
        }
        div()
            .relative()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .when_bottom(self.at_bottom)
            .child(div().flex().child(popover))
            .child(
                div()
                    .id("outside")
                    .test_support()
                    .absolute()
                    .top_4()
                    .right_4()
                    .size(px(40.)),
            )
    }
}

trait BottomExt {
    fn when_bottom(self, bottom: bool) -> Self;
}

impl BottomExt for gpui_kit::Div {
    fn when_bottom(self, bottom: bool) -> Self {
        if bottom { self.justify_end() } else { self }
    }
}

fn setup(
    cx: &mut TestAppContext,
    at_bottom: bool,
    open: Option<Rc<Cell<bool>>>,
) -> WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let handle = cx.open_window(size(px(600.), px(600.)), |window, cx| {
        let view = cx.new(|_| Harness { at_bottom, open });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })
    .unwrap();
    handle
}

fn panel() -> ElementId {
    ElementId::NamedChild(ElementId::Name("pop".into()).into(), "panel".into())
}

#[gpui_kit::test]
fn a_click_opens_it_under_the_trigger_and_an_outside_press_closes_it(cx: &mut TestAppContext) {
    let handle = setup(cx, false, None);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("pop-open", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let trigger = window.find("pop-open").bounds();
        let panel_bounds = window.find(panel()).bounds();
        assert_eq!(
            panel_bounds.top(),
            trigger.bottom() + px(2.),
            "the menu gap"
        );
        assert_eq!(
            panel_bounds.left(),
            trigger.left(),
            "the leading edges line up"
        );
        window.click("outside", cx);
        window.render_frame(cx);
        assert!(window.try_find(panel()).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_closes_it(cx: &mut TestAppContext) {
    let handle = setup(cx, false, None);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("pop-open", cx);
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(panel()).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn without_room_below_it_opens_above_its_trigger(cx: &mut TestAppContext) {
    let handle = setup(cx, true, None);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("pop-open", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let trigger = window.find("pop-open").bounds();
        let panel_bounds = window.find(panel()).bounds();
        assert_eq!(
            panel_bounds.bottom(),
            trigger.top() - px(2.),
            "the menu gap above"
        );
        assert_eq!(panel_bounds.left(), trigger.left());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_controlled_popover_reports_and_follows_the_application(cx: &mut TestAppContext) {
    let open = Rc::new(Cell::new(false));
    let handle = setup(cx, false, Some(open.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("pop-open", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(open.get(), "the trigger reports the change");
    open.set(false);
    cx.update_window(handle.into(), |_, window, cx| {
        window.refresh();
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            window.try_find(panel()).is_none(),
            "it follows the application"
        );
    })
    .unwrap();
}
