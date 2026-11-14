//! UI integration tests for `Textarea`: real input through a headless
//! window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Textarea, TextareaState};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, ElementId, Entity, IntoElement, MouseButton,
    MouseDownEvent, MouseUpEvent, ParentElement as _, PlatformInput, Render, Role, Styled as _,
    TestAppContext, Window, base::Disableable as _, div, point, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<TextareaState>,
    disabled: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child(
            Textarea::new(&self.state)
                .id("notes")
                .disabled(self.disabled)
                .accessibility_label("Notes"),
        )
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<TextareaState>,
}

fn setup(cx: &mut TestAppContext, rows: usize, disabled: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let state = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Notes")
                .auto_grow(rows, rows)
        });
        state.set(Some(textarea.clone()));
        let harness = cx.new(|_| Harness {
            state: textarea,
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
fn the_rows_sit_between_the_reference_padding(cx: &mut TestAppContext) {
    let one = setup(cx, 1, false);
    cx.update_window(one.handle.into(), |_, window, cx| {
        let frame = window.find("notes");
        assert_eq!(frame.label(), Some("Notes"));
        assert_eq!(frame.role(), Some(Role::MultilineTextInput));
        assert_eq!(
            frame.bounds().size.height,
            px(37.),
            "one row: 18.5px rounded up to 19 by the layout, 8px padding and the hairline above and below"
        );
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle field asks for no frame"
        );
    })
    .unwrap();
    let three = setup(cx, 3, false);
    cx.update_window(three.handle.into(), |_, window, _| {
        assert_eq!(
            window.find("notes").bounds().size.height,
            px(75.),
            "three rows: 3 x 18.5px rounded up to 57 by the layout, 8px padding and the hairline above and below"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn enter_starts_a_new_line(cx: &mut TestAppContext) {
    let setup = setup(cx, 3, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("notes", cx);
        window.render_frame(cx);
        assert_eq!(window.find("notes").focused(), Some(true));
        window.input("a", cx);
        window.press("enter", cx);
        window.input("b", cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "a\nb");
}

#[gpui_kit::test]
fn a_disabled_field_ignores_input(cx: &mut TestAppContext) {
    let setup = setup(cx, 3, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("notes", cx);
        window.render_frame(cx);
        assert_ne!(
            window.find("notes").focused(),
            Some(true),
            "a click does not focus a disabled field"
        );
        let mut key = gpui_kit::Keystroke::parse("x").unwrap();
        key.key_char = Some("x".into());
        window.dispatch_keystroke(key, cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "");
}

/// A row of the field's right-click menu.
fn menu_row(name: &'static str) -> ElementId {
    ElementId::NamedChild(
        ElementId::NamedChild(
            ElementId::Name("notes".into()).into(),
            "context-menu".into(),
        )
        .into(),
        name.into(),
    )
}

/// Right-clicks the text just after the caret position `offset`; base
/// opens the menu after the click's update, as between two real events.
fn open_menu_at(setup: &Setup, offset: usize, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let caret = setup
            .state
            .read(cx)
            .range_to_bounds(&(offset..offset))
            .expect("the offset was laid out");
        let at = point(caret.origin.x + px(1.), caret.center().y);
        for event in [
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Right,
                position: at,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Right,
                position: at,
                modifiers: Default::default(),
                click_count: 1,
            }),
        ] {
            window.dispatch_event(event, cx);
        }
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn the_edit_menu_pastes_and_copies_across_lines(cx: &mut TestAppContext) {
    let setup = setup(cx, 3, false);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(" three".into())));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("notes", cx);
        window.input("one", cx);
        window.press("enter", cx);
        window.input("two", cx);
        window.render_frame(cx);
    })
    .unwrap();
    open_menu_at(&setup, 7, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("paste"), cx);
        window.render_frame(cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        assert_eq!(setup.state.read(cx).selected_range(), 0..13);
    })
    .unwrap();
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.value()),
        "one\ntwo three"
    );
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("copy"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("one\ntwo three"));
}
