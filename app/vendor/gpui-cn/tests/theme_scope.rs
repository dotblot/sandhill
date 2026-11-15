//! `ThemeScope`: a subtree drawn with another token set.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{ActiveTheme as _, Theme, ThemeConfig, ThemeScope, theme::hex};
use gpui_kit::base::Root;
use gpui_kit::{
    App, AppContext as _, Context, Hsla, IntoElement, ParentElement as _, Render, RenderOnce,
    Styled as _, TestAppContext, Window, div, px, size, test::TestWindowExt as _,
};

/// Records the background `cx.theme()` gives while it renders.
#[derive(IntoElement)]
struct Probe(Rc<RefCell<Vec<Hsla>>>);

impl RenderOnce for Probe {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        self.0.borrow_mut().push(cx.theme().background());
        div().size_full()
    }
}

struct Harness {
    seen: Rc<RefCell<Vec<Hsla>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Probe(self.seen.clone()))
            .child(ThemeScope::new(
                "inner",
                div().size_full().child(Probe(self.seen.clone())),
            ))
            .child(Probe(self.seen.clone()))
    }
}

fn config(surface: &str, ink: &str, cx: &App) -> ThemeConfig {
    let mut config = Theme::global(cx).dark.clone();
    config.surface = hex(surface);
    config.ink = hex(ink);
    config
}

#[gpui_kit::test]
fn a_scope_draws_its_subtree_with_its_own_tokens_and_follows_changes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        let config = config("#282c34", "#ffffff", cx);
        Theme::set_scope(cx, "inner", config);
    });
    let observed = Rc::new(RefCell::new(0));
    cx.update({
        let observed = observed.clone();
        move |cx| {
            cx.observe_global::<Theme>(move |_| *observed.borrow_mut() += 1)
                .detach();
        }
    });
    let seen = Rc::new(RefCell::new(Vec::new()));
    let handle = cx.open_window(size(px(300.), px(200.)), {
        let seen = seen.clone();
        move |window, cx| {
            let harness = cx.new(|_| Harness { seen });
            Root::new(harness, window, cx)
        }
    });
    let frame = |cx: &mut TestAppContext| {
        seen.borrow_mut().clear();
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
        seen.borrow().clone()
    };

    let window = cx.update(|cx| cx.theme().background());
    let [before, inside, after] = frame(cx)[..] else {
        panic!("three probes");
    };
    assert_eq!(before, window);
    assert_eq!(inside, hex("#282c34"), "the scope's surface");
    assert_eq!(after, window, "the scope ends with its subtree");
    assert_eq!(*observed.borrow(), 0, "entering a scope wakes no observer");

    cx.update(|cx| {
        let config = config("#fdf6e3", "#073642", cx);
        Theme::set_scope(cx, "inner", config);
    });
    let inside = frame(cx)[1];
    assert_eq!(inside, hex("#fdf6e3"), "a changed scope draws at once");
    let scoped = cx.update(|cx| Theme::global(cx).scope("inner").map(|t| t.background()));
    assert_eq!(scoped, Some(hex("#fdf6e3")));
}
