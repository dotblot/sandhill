//! UI integration tests for the touch selection over an `Input`: a long
//! press through a headless window, then the handles and the edit menu.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Input, InputState, Textarea, TextareaState, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, LongPressEvent, ParentElement as _, Pixels,
    PlatformInput, Point, Render, Styled as _, TestAppContext, TouchPhase, Window,
    base::SelectionEdge, div, point, px, size, test::TestWindowExt as _,
};

struct Harness(Entity<InputState>);

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .child(Input::new(&self.0).id("name"))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<InputState>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.touch = true);
    });
    let state = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your name")
                .default_value("quick select value")
        });
        state.set(Some(input.clone()));
        let harness = cx.new(|_| Harness(input));
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

/// The window position of the caret before `offset`, in the field's line.
fn caret_at(state: &Entity<InputState>, offset: usize, cx: &gpui_kit::App) -> Point<Pixels> {
    let state = state.read(cx);
    let bounds = state.text_bounds().expect("the text was laid out");
    let caret = state
        .range_to_bounds(&(offset..offset))
        .expect("the offset is in the line");
    point(caret.origin.x, bounds.center().y)
}

fn long_press(window: &mut Window, phase: TouchPhase, at: Point<Pixels>, cx: &mut gpui_kit::App) {
    window.dispatch_event(
        PlatformInput::LongPress(LongPressEvent {
            phase,
            start_position: at,
            position: at,
        }),
        cx,
    );
    window.render_frame(cx);
}

/// Selects "select" with a long press and lifts the finger, which opens
/// the menu.
fn select_word(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let at = caret_at(&setup.state, 8, cx);
        long_press(window, TouchPhase::Started, at, cx);
        long_press(window, TouchPhase::Ended, at, cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_press_selects_a_word_and_opens_the_edit_menu(cx: &mut TestAppContext) {
    let setup = setup(cx);
    select_word(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(
            setup.state.read(cx).selected_value(),
            "select",
            "the word under the finger"
        );
        assert!(window.try_find("Copy").is_some(), "the menu offers Copy");
        assert!(window.try_find("Cut").is_some());
        assert!(window.try_find("Paste").is_some());
        assert!(window.try_find("Select All").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn select_all_from_the_menu_selects_the_whole_value(cx: &mut TestAppContext) {
    let setup = setup(cx);
    select_word(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("Select All", cx);
        window.render_frame(cx);
        let state = setup.state.read(cx);
        assert_eq!(state.selected_range(), 0..state.text().len());
        assert!(
            window.try_find("Select All").is_none(),
            "nothing is left to select"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn copy_from_the_menu_puts_the_selection_on_the_clipboard(cx: &mut TestAppContext) {
    let setup = setup(cx);
    select_word(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("Copy", cx);
        window.render_frame(cx);
        assert!(window.try_find("Copy").is_none(), "Copy closes the menu");
    })
    .unwrap();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("select"));
}

#[gpui_kit::test]
fn dragging_the_end_handle_extends_the_selection(cx: &mut TestAppContext) {
    let setup = setup(cx);
    select_word(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let snapshot = setup
            .state
            .read(cx)
            .touch_selection()
            .expect("the selection is live");
        let end = snapshot.edge(SelectionEdge::End);
        // The knob hangs below the caret; grab it there.
        let from = point(end.center().x, end.bottom() + px(6.));
        let to = point(caret_at(&setup.state, 18, cx).x + px(2.), from.y);
        window.drag(from, to, cx);
        window.render_frame(cx);
        assert_eq!(setup.state.read(cx).selected_value(), "select value");
    })
    .unwrap();
}

struct TextareaHarness(Entity<TextareaState>);

impl Render for TextareaHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .child(Textarea::new(&self.0).id("notes"))
    }
}

#[gpui_kit::test]
fn a_textarea_gets_the_same_handles_and_menu(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.touch = true);
    });
    let state = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 4)
                .default_value("quick select value")
        });
        state.set(Some(notes.clone()));
        let harness = cx.new(|_| TextareaHarness(notes));
        Root::new(harness, window, cx)
    });
    let state = state.take().unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let at = {
            let state = state.read(cx);
            let bounds = state.text_bounds().expect("the text was laid out");
            let caret = state
                .range_to_bounds(&(8..8))
                .expect("the offset is in the first line");
            point(
                caret.origin.x,
                bounds.origin.y + caret.center().y - caret.origin.y,
            )
        };
        long_press(window, TouchPhase::Started, at, cx);
        long_press(window, TouchPhase::Ended, at, cx);
        assert_eq!(state.read(cx).selected_value(), "select");
        assert!(window.try_find("Copy").is_some(), "the menu offers Copy");
        window.click("Select All", cx);
        window.render_frame(cx);
        let state = state.read(cx);
        assert_eq!(state.selected_range(), 0..state.text().len());
    })
    .unwrap();
}
