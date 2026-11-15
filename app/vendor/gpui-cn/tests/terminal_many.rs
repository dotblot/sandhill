//! Several terminals in one window: each has its own source and state,
//! and keys and actions reach only the focused one.

#![cfg(feature = "ghostty")]

use std::sync::{Arc, Mutex};

use gpui_cn::terminal::input::KeyAction;
use gpui_cn::terminal::{FixtureSource, Terminal, TerminalConfig, TerminalInput, TerminalState};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Pixels, Render, Styled as _,
    TestAppContext, Window, WindowHandle, div, px, size, test::TestWindowExt as _,
};

struct Pair {
    left: Entity<TerminalState>,
    right: Entity<TerminalState>,
}

impl Render for Pair {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .w_1_2()
                    .h_full()
                    .child(Terminal::new("left", &self.left)),
            )
            .child(
                div()
                    .w_1_2()
                    .h_full()
                    .child(Terminal::new("right", &self.right)),
            )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    left: Entity<TerminalState>,
    right: Entity<TerminalState>,
    left_inputs: Arc<Mutex<Vec<TerminalInput>>>,
    right_inputs: Arc<Mutex<Vec<TerminalInput>>>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        cx.bind_keys(gpui_cn::terminal::default_key_bindings());
    });
    let left = FixtureSource::new("left\r\n");
    let right = FixtureSource::new("right\r\n");
    let (left_inputs, right_inputs) = (left.inputs(), right.inputs());
    let sources = Arc::new(Mutex::new(Some((left, right))));
    let states = Arc::new(Mutex::new(None));
    let handle = cx.open_window(size(px(800.), px(300.)), {
        let states = states.clone();
        move |window, cx| {
            let (left, right) = sources.lock().unwrap().take().expect("one window");
            let left = cx.new(|cx| TerminalState::new(left, TerminalConfig::default(), window, cx));
            let right =
                cx.new(|cx| TerminalState::new(right, TerminalConfig::default(), window, cx));
            *states.lock().unwrap() = Some((left.clone(), right.clone()));
            let pair = cx.new(|cx| {
                cx.observe(&left, |_, _, cx| cx.notify()).detach();
                cx.observe(&right, |_, _, cx| cx.notify()).detach();
                Pair { left, right }
            });
            Root::new(pair, window, cx)
        }
    });
    let (left, right) = states.lock().unwrap().take().expect("the terminals");
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    Setup {
        handle,
        left,
        right,
        left_inputs,
        right_inputs,
    }
}

fn keys(inputs: &Mutex<Vec<TerminalInput>>) -> Vec<String> {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter_map(|input| match input {
            TerminalInput::Key(key) if key.action == KeyAction::Press => Some(key.key.clone()),
            _ => None,
        })
        .collect()
}

fn font_size(state: &Entity<TerminalState>, cx: &mut TestAppContext) -> Pixels {
    state.read_with(cx, |state, cx| state.font_size(cx))
}

#[gpui_kit::test]
fn each_terminal_has_its_own_screen(cx: &mut TestAppContext) {
    let setup = setup(cx);
    assert!(
        setup
            .left
            .read_with(cx, |s, _| s.frame().text().starts_with("left"))
    );
    assert!(
        setup
            .right
            .read_with(cx, |s, _| s.frame().text().starts_with("right"))
    );
}

#[gpui_kit::test]
fn keys_and_actions_reach_only_the_focused_terminal(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let zoom = if cfg!(target_os = "macos") {
        "cmd-="
    } else {
        "ctrl-="
    };
    let before = font_size(&setup.left, cx);

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("left", cx);
        window.press("a", cx);
        // Root's Tab, which the focused terminal takes.
        window.press("tab", cx);
        window.press(zoom, cx);
    })
    .unwrap();
    assert_eq!(keys(&setup.left_inputs), ["a", "tab"]);
    assert!(keys(&setup.right_inputs).is_empty());
    assert!(font_size(&setup.left, cx) > before);
    assert_eq!(font_size(&setup.right, cx), before);

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("right", cx);
        window.press("b", cx);
        window.press("tab", cx);
    })
    .unwrap();
    assert_eq!(keys(&setup.left_inputs), ["a", "tab"], "left heard nothing");
    assert_eq!(keys(&setup.right_inputs), ["b", "tab"]);
    assert_eq!(font_size(&setup.right, cx), before);
}
