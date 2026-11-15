//! UI integration tests for `Dialog`: real input through a headless window.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use gpui_cn::{Button, Dialog, ReduceMotion, Theme};
use gpui_kit::{
    AppContext as _, Context, ElementId, FocusHandle, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, WindowHandle, base::Root, point, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    open: Rc<Cell<bool>>,
    changes: Rc<RefCell<Vec<bool>>>,
    opener: FocusHandle,
    inner: FocusHandle,
    close_button: bool,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        let changes = self.changes.clone();
        let log = self.log.clone();
        gpui_kit::div()
            .size_full()
            .child(
                Button::new("opener")
                    .label("Open")
                    .track_focus(&self.opener)
                    .on_click({
                        let open = self.open.clone();
                        move |_, _, _| open.set(true)
                    }),
            )
            .child(
                Dialog::new("dlg")
                    .open(self.open.get())
                    .title("Rename")
                    .description("Pick a new name.")
                    .show_close_button(self.close_button)
                    .track_focus(&self.inner)
                    .on_open_change(move |value, _, _| {
                        changes.borrow_mut().push(value);
                        open.set(value);
                    })
                    .child(
                        Button::new("inner")
                            .label("Inner")
                            .track_focus(&self.inner)
                            .on_click(|_, _, _| {}),
                    )
                    .footer(
                        Button::new("footer-ok")
                            .primary()
                            .label("Save")
                            .on_click(move |_, _, _| log.borrow_mut().push("save")),
                    ),
            )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    open: Rc<Cell<bool>>,
    changes: Rc<RefCell<Vec<bool>>>,
    opener: FocusHandle,
    inner: FocusHandle,
    log: Rc<RefCell<Vec<&'static str>>>,
}

fn setup(cx: &mut TestAppContext, close_button: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let open = Rc::new(Cell::new(false));
    let changes = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::new(RefCell::new(Vec::new()));
    let (mut opener, mut inner) = (None, None);
    let handle = cx.open_window(size(px(800.), px(700.)), |window, cx| {
        let view = cx.new(|cx| {
            let harness = Harness {
                open: open.clone(),
                changes: changes.clone(),
                opener: cx.focus_handle(),
                inner: cx.focus_handle(),
                close_button,
                log: log.clone(),
            };
            opener = Some(harness.opener.clone());
            inner = Some(harness.inner.clone());
            harness
        });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        open,
        changes,
        opener: opener.unwrap(),
        inner: inner.unwrap(),
        log,
    }
}

fn part(name: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::Name("dlg".into()).into(), name.into())
}

fn open_it(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("opener", cx);
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn it_draws_nothing_while_closed_and_a_centered_surface_when_open(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(part("popup")).is_none());
        assert!(window.try_find(part("backdrop")).is_none());
    })
    .unwrap();
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        let popup = window.find(part("popup")).bounds();
        let backdrop = window.find(part("backdrop")).bounds();
        assert_eq!(popup.size.width, px(480.));
        assert_eq!(backdrop.size, size(px(800.), px(700.)));
        assert_eq!(popup.center(), point(px(400.), px(350.)));
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_closes_it_and_reports_the_change(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(part("popup")).is_none());
    })
    .unwrap();
    assert_eq!(*setup.changes.borrow(), vec![false]);
    assert!(!setup.open.get());
}

#[gpui_kit::test]
fn a_press_on_the_backdrop_closes_it_and_one_inside_does_not(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("popup"), cx);
        window.render_frame(cx);
        assert!(window.try_find(part("popup")).is_some());
        window.click_at(part("backdrop"), point(px(5.), px(5.)), cx);
        window.render_frame(cx);
        assert!(window.try_find(part("popup")).is_none());
    })
    .unwrap();
    assert_eq!(*setup.changes.borrow(), vec![false]);
}

#[gpui_kit::test]
fn the_close_button_closes_it_and_can_be_hidden(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("close"), cx);
        window.render_frame(cx);
        assert!(window.try_find(part("popup")).is_none());
    })
    .unwrap();
    assert_eq!(*setup.changes.borrow(), vec![false]);

    let hidden = self::setup(cx, false);
    open_it(&hidden, cx);
    cx.update_window(hidden.handle.into(), |_, window, _| {
        assert!(window.try_find(part("popup")).is_some());
        assert!(window.try_find(part("close")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn focus_moves_in_stays_in_and_returns(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup.opener.focus(window, cx);
        window.render_frame(cx);
    })
    .unwrap();
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(
            setup.inner.is_focused(window),
            "focus moved into the dialog"
        );
        for _ in 0..6 {
            window.press("tab", cx);
            window.render_frame(cx);
            assert!(
                !setup.opener.is_focused(window),
                "Tab does not leave the dialog"
            );
        }
        window.press("escape", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(setup.opener.is_focused(window), "focus went back");
    })
    .unwrap();
}

#[gpui_kit::test]
fn footer_buttons_are_clickable(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    open_it(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("footer-ok", cx);
        window.render_frame(cx);
        let popup = window.find(part("popup")).bounds();
        let ok = window.find("footer-ok").bounds();
        assert!(popup.contains(&ok.center()));
        assert!(ok.right() < popup.right());
    })
    .unwrap();
    assert_eq!(*setup.log.borrow(), vec!["save"]);
}
