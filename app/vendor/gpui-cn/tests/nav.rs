//! UI integration tests for `NavStack` and `NavButtons`.

use gpui_cn::{NavButtons, NavMotion, NavStack, NavStackState, ReduceMotion, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AnyView, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, TestAppContext, Window, WindowHandle,
    base::TestSupportExt as _, div, px, size, test::TestWindowExt as _,
};

/// The id `NavButtons` gives one of its arrows.
fn arrow(name: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::from("nav").into(), name.into())
}

struct Page {
    name: &'static str,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.name)
            .test_support()
            .size_full()
            .child(self.name)
    }
}

struct Harness {
    stack: Entity<NavStackState>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(NavButtons::new("nav", &self.stack))
            .child(NavStack::new(&self.stack).flex_1())
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    stack: Entity<NavStackState>,
    home: AnyView,
    settings: AnyView,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let stack = cx.new(|_| NavStackState::new());
    let home: AnyView = cx.new(|_| Page { name: "home" }).into();
    let settings: AnyView = cx.new(|_| Page { name: "settings" }).into();
    stack.update(cx, |stack, cx| {
        stack.push(home.clone(), NavMotion::Immediate, cx);
    });
    let handle = cx.open_window(size(px(600.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            stack: stack.clone(),
        });
        Root::new(harness, window, cx)
    });
    Setup {
        handle,
        stack,
        home,
        settings,
    }
}

fn current(setup: &Setup, cx: &mut TestAppContext) -> Option<AnyView> {
    setup
        .stack
        .read_with(cx, |stack, _| stack.current().cloned())
}

fn click_arrow(setup: &Setup, cx: &mut TestAppContext, name: &'static str) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.click(arrow(name), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_pages_follow_the_stack(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("home").is_some());
        assert!(window.try_find("settings").is_none());
        // Base exposes no disabled flag in snapshots; the arrows are proven
        // by what a click does below.
        assert!(window.find(arrow("back")).visible());
        assert!(window.find(arrow("forward")).visible());
    })
    .unwrap();

    setup.stack.update(cx, |stack, cx| {
        stack.push(setup.settings.clone(), NavMotion::Animated, cx);
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings").is_some());
        assert!(
            window.try_find("home").is_none(),
            "under reduced motion the outgoing page is gone at once"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_arrows_move_the_stack_and_are_inert_at_its_ends(cx: &mut TestAppContext) {
    let setup = setup(cx);
    // Only the root: neither arrow does anything.
    click_arrow(&setup, cx, "back");
    click_arrow(&setup, cx, "forward");
    assert_eq!(current(&setup, cx), Some(setup.home.clone()));
    assert_eq!(setup.stack.read_with(cx, |stack, _| stack.depth()), 1);

    setup.stack.update(cx, |stack, cx| {
        stack.push(setup.settings.clone(), NavMotion::Immediate, cx);
    });
    click_arrow(&setup, cx, "back");
    assert_eq!(current(&setup, cx), Some(setup.home.clone()));
    assert_eq!(
        setup
            .stack
            .read_with(cx, |stack, _| stack.forward_views().len()),
        1
    );

    click_arrow(&setup, cx, "forward");
    assert_eq!(current(&setup, cx), Some(setup.settings.clone()));
    assert_eq!(
        setup
            .stack
            .read_with(cx, |stack, _| stack.forward_views().len()),
        0
    );
    // Forward with nothing to bring back changes nothing.
    click_arrow(&setup, cx, "forward");
    assert_eq!(current(&setup, cx), Some(setup.settings.clone()));
}

#[gpui_kit::test]
fn pop_and_discard_frees_the_page(cx: &mut TestAppContext) {
    use gpui_cn::NavStackExt as _;
    let setup = setup(cx);
    let page = cx.new(|_| Page { name: "extra" });
    let weak = page.downgrade();
    setup.stack.update(cx, |stack, cx| {
        stack.push(page, NavMotion::Immediate, cx);
    });
    cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();

    let popped = cx.update(|cx| setup.stack.pop_and_discard(NavMotion::Immediate, cx));
    assert!(popped.is_some());
    drop(popped);
    cx.update(|cx| {
        let stack = setup.stack.read(cx);
        assert_eq!(stack.forward_views().len(), 0, "nothing to bring back");
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.current(), Some(&setup.home));
    });
    cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(
        weak.upgrade().is_none(),
        "the page is dropped once nothing holds it"
    );
}

#[gpui_kit::test]
fn an_animated_pop_and_discard_waits_for_the_transition(cx: &mut TestAppContext) {
    use gpui_cn::NavStackExt as _;
    use std::time::Duration;
    let setup = setup(cx);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    setup.stack.update(cx, |stack, cx| {
        stack.push(setup.settings.clone(), NavMotion::Immediate, cx);
    });
    cx.update(|cx| {
        setup.stack.pop_and_discard(NavMotion::Animated, cx);
    });
    assert_eq!(
        setup
            .stack
            .read_with(cx, |stack, _| stack.forward_views().len()),
        1,
        "the page waits while its exit runs"
    );
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.executor().run_until_parked();
    assert_eq!(
        setup
            .stack
            .read_with(cx, |stack, _| stack.forward_views().len()),
        0
    );
    assert_eq!(
        setup
            .stack
            .read_with(cx, |stack, _| stack.current().cloned()),
        Some(setup.home.clone())
    );
}

#[gpui_kit::test]
fn a_push_crossfades_and_then_drops_the_outgoing_page(cx: &mut TestAppContext) {
    use std::time::Duration;
    let setup = setup(cx);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    setup.stack.update(cx, |stack, cx| {
        stack.push(setup.settings.clone(), NavMotion::Animated, cx);
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("home").is_some(),
            "the outgoing page is still mounted"
        );
        assert!(
            window.try_find("settings").is_some(),
            "the incoming page is mounted"
        );
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("home").is_some(),
            "mid-transition both are mounted"
        );
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            window.try_find("home").is_none(),
            "the outgoing page is dropped"
        );
        assert!(window.try_find("settings").is_some());
    })
    .unwrap();
    // And a pop right after: the settings page exits, home returns.
    setup.stack.update(cx, |stack, cx| {
        stack.pop(NavMotion::Animated, cx);
    });
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("settings").is_none());
        assert!(window.try_find("home").is_some());
    })
    .unwrap();
}
