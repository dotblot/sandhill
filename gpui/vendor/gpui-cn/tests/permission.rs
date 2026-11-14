//! UI integration tests for `PermissionMenu`: real input through a
//! headless window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    PermissionEvent, PermissionMenu, PermissionMode, PermissionState, ReduceMotion, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render, Role,
    SharedString, Styled as _, TestAppContext, Window, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<PermissionState>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .child(PermissionMenu::new("permission", &self.state))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<PermissionState>,
    events: Rc<RefCell<Vec<PermissionEvent>>>,
}

fn setup(cx: &mut TestAppContext, modes: Option<Vec<PermissionMode>>) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut state_slot = None;
    let handle = cx.open_window(size(px(600.), px(600.)), |window, cx| {
        let state = cx.new(|cx| {
            let state = PermissionState::new(cx);
            match modes {
                Some(modes) => state.with_modes(modes),
                None => state,
            }
        });
        let recorded = events.clone();
        cx.subscribe(&state, move |_, _, event: &PermissionEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        state_slot = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Harness { state }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    Setup {
        handle,
        state: state_slot.unwrap(),
        events,
    }
}

fn row(name: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::from("permission").into(), name.into())
}

fn trigger() -> ElementId {
    row("trigger")
}

fn open(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(trigger(), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn selected(setup: &Setup, cx: &mut TestAppContext) -> SharedString {
    setup
        .state
        .read_with(cx, |state, _| state.selected().clone())
}

#[gpui_kit::test]
fn the_default_modes_are_listed_with_descriptions_and_auto_is_selected(cx: &mut TestAppContext) {
    let setup = setup(cx, None);
    assert_eq!(selected(&setup, cx), "auto");
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        for (key, label) in [
            ("auto", "Auto"),
            ("manual", "Manual"),
            ("plan", "Plan"),
            ("bypass", "Bypass all"),
        ] {
            let row = window.find(row(key));
            assert_eq!(row.role(), Some(Role::MenuItem));
            assert_eq!(row.label(), Some(label));
        }
        assert!(
            window.find(row("manual")).bounds().size.height
                > window.find(row("learn-more")).bounds().size.height,
            "a mode row holds a description line"
        );
        assert!(
            window.find(row("learn-more")).bounds().top() < window.find(row("auto")).bounds().top(),
            "the header comes first"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn choosing_a_mode_selects_it_closes_the_menu_and_reports_once(cx: &mut TestAppContext) {
    let setup = setup(cx, None);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(row("bypass"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none(), "the menu closed");
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), "bypass");
    assert_eq!(
        setup.events.borrow().as_slice(),
        [PermissionEvent::Changed("bypass".into())]
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(row("bypass"), cx);
    })
    .unwrap();
    assert_eq!(setup.events.borrow().len(), 1, "the same mode is no change");
}

#[gpui_kit::test]
fn learn_more_reports_without_changing_the_mode(cx: &mut TestAppContext) {
    let setup = setup(cx, None);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(row("learn-more"), cx);
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), "auto");
    assert_eq!(
        setup.events.borrow().as_slice(),
        [PermissionEvent::LearnMore]
    );
}

#[gpui_kit::test]
fn an_application_defines_its_own_modes(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Some(vec![
            PermissionMode::new("read", "Read only").description("Look, never touch"),
            PermissionMode::new("write", "Read and write"),
        ]),
    );
    assert_eq!(
        selected(&setup, cx),
        "read",
        "the first mode when auto is gone"
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(row("auto")).is_none());
        window.click(row("write"), cx);
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), "write");
}

#[gpui_kit::test]
fn escape_closes_the_menu_without_a_change(cx: &mut TestAppContext) {
    let setup = setup(cx, None);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert!(setup.events.borrow().is_empty());
}
