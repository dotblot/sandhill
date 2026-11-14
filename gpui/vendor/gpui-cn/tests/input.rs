//! UI integration tests for `Input`: real input through a headless window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Icon, Input, InputEvent, InputState, MenuEntry, MenuItem, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, ElementId, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, PlatformInput, Point, Render, Styled as _,
    TestAppContext, TestSupportExt as _, Window, base::Disableable as _, div, point,
    prelude::FluentBuilder as _, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<InputState>,
    options: Options,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let options = &self.options;
        div().size_full().p_4().child(
            Input::new(&self.state)
                .id("name")
                .disabled(options.disabled)
                .readonly(options.readonly)
                .when(options.label, |this| this.accessibility_label("Name"))
                .when(options.affixes, |this| {
                    this.prefix(Icon::new("icons/search.svg"))
                        .suffix(div().id("suffix").test_support().size_4())
                })
                .cleanable(options.cleanable)
                .mask_toggle(options.masked)
                .context_menu_enabled(options.context_menu)
                .when(options.replace_menu, |this| {
                    let state = self.state.clone();
                    this.context_menu(move |_, _, _, _| {
                        let state = state.clone();
                        vec![
                            MenuItem::new("only", "Only")
                                .on_select(move |window, cx| {
                                    state.update(cx, |state, cx| {
                                        state.set_value("REPLACED", window, cx)
                                    });
                                })
                                .into(),
                        ]
                    })
                })
                .when(options.extend_menu, |this| {
                    let state = self.state.clone();
                    this.context_menu(move |mut entries, _, _, _| {
                        let state = state.clone();
                        entries.push(MenuEntry::Separator);
                        entries.push(
                            MenuItem::new("shout", "Shout")
                                .on_select(move |window, cx| {
                                    state.update(cx, |state, cx| {
                                        let loud = state.value().to_uppercase();
                                        state.set_value(loud, window, cx);
                                    });
                                })
                                .into(),
                        );
                        entries
                    })
                }),
        )
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<InputState>,
    enters: Rc<Cell<usize>>,
}

struct Options {
    disabled: bool,
    readonly: bool,
    affixes: bool,
    /// Whether the field gets an accessibility label of its own.
    label: bool,
    cleanable: bool,
    /// The state is masked and the field shows the toggle.
    masked: bool,
    /// Whether a right click opens the field's menu.
    context_menu: bool,
    /// Whether the menu gets an item of the application's.
    extend_menu: bool,
    /// Whether the application's one item replaces the menu.
    replace_menu: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            disabled: false,
            readonly: false,
            affixes: false,
            label: true,
            cleanable: false,
            masked: false,
            context_menu: true,
            extend_menu: false,
            replace_menu: false,
        }
    }
}

fn setup(cx: &mut TestAppContext, options: Options) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let enters = Rc::new(Cell::new(0));
    let state = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your name")
                .masked(options.masked)
        });
        state.set(Some(input.clone()));
        let harness = cx.new(|cx| {
            let enters = enters.clone();
            cx.subscribe(&input, move |_, _, event: &InputEvent, _| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    enters.set(enters.get() + 1);
                }
            })
            .detach();
            Harness {
                state: input,
                options,
            }
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
        enters,
    }
}

#[gpui_kit::test]
fn the_frame_is_the_reference_field_height_and_carries_the_name(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let input = window.find("name");
        assert_eq!(input.label(), Some("Name"));
        assert_eq!(input.bounds().size.height, px(32.));
        assert_eq!(
            input.bounds().size.width,
            px(400. - 32.),
            "the frame fills its parent"
        );
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle field asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_placeholder_names_the_field_when_no_label_is_given(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            label: false,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find("name").label(), Some("Your name"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_on_the_padding_puts_the_caret_in_the_text(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let padding = point(px(3.), px(16.));
        window.click_at("name", padding, cx);
        window.input("hi", cx);
        window.click_at("name", padding, cx);
        window.render_frame(cx);
        assert!(
            setup
                .state
                .read(cx)
                .presentation()
                .focus_handle()
                .is_focused(window),
            "a second click on the padding keeps the caret inside"
        );
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "hi");
}

#[gpui_kit::test]
fn a_click_focuses_the_field_and_typing_changes_the_value(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("name").focused(),
            Some(true),
            "the frame reports focus while the caret is inside"
        );
        window.input("hello", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("name").value(),
            Some("hello"),
            "assistive technology reads the value from the frame"
        );
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "hello");
}

#[gpui_kit::test]
fn select_all_takes_the_whole_value(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.input("Ada", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.selected_range()),
        0..3
    );
}

#[gpui_kit::test]
fn tab_reaches_the_field(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("name").focused(), Some(true));
        window.input("a", cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "a");
}

#[gpui_kit::test]
fn enter_reports_a_submit(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.input("go", cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(setup.enters.get(), 1);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.value()),
        "go",
        "a single line keeps Enter out of the text"
    );
}

#[gpui_kit::test]
fn a_disabled_field_ignores_input(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            disabled: true,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.render_frame(cx);
        assert_ne!(
            window.find("name").focused(),
            Some(true),
            "a click does not focus a disabled field"
        );
        assert!(!setup.state.read(cx).focus_handle(cx).is_focused(window));
        let mut key = gpui_kit::Keystroke::parse("x").unwrap();
        key.key_char = Some("x".into());
        window.dispatch_keystroke(key, cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "");
}

#[gpui_kit::test]
fn a_read_only_field_focuses_but_keeps_its_value(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            readonly: true,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.render_frame(cx);
        assert_eq!(window.find("name").focused(), Some(true));
        window.input("x", cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "");
}

#[gpui_kit::test]
fn on_touch_the_frame_is_a_hit_target_tall(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("name").bounds().size.height,
            px(44.),
            "the HIG hit target"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_prefix_and_suffix_sit_inside_the_frame(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            affixes: true,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let frame = window.find("name").bounds();
        let suffix = window.find("suffix").bounds();
        assert_eq!(frame.size.height, px(32.), "affixes do not grow the frame");
        assert!(frame.contains(&suffix.origin));
        assert!(frame.contains(&suffix.bottom_right()));
        assert!(
            suffix.right() < frame.right() - px(11.),
            "the suffix sits inside the horizontal padding"
        );
        window.click("name", cx);
        window.input("hi", cx);
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "hi");
}

#[gpui_kit::test]
fn the_clear_button_empties_the_value_and_keeps_focus(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            cleanable: true,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(
            window.try_find("clear").is_none(),
            "no button while the value is empty"
        );
        window.click("name", cx);
        window.input("hi", cx);
        window.click("clear", cx);
        window.render_frame(cx);
        assert!(
            setup
                .state
                .read(cx)
                .presentation()
                .focus_handle()
                .is_focused(window),
            "the caret stays in the field"
        );
        assert!(window.try_find("clear").is_none());
    })
    .unwrap();
    assert_eq!(setup.state.read_with(cx, |state, _| state.value()), "");
}

#[gpui_kit::test]
fn the_mask_toggle_shows_and_hides_the_value(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            masked: true,
            ..Options::default()
        },
    );
    let masked = |cx: &mut TestAppContext| {
        setup
            .state
            .read_with(cx, |state, _| state.presentation().is_masked())
    };
    assert!(masked(cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("mask-toggle", cx);
    })
    .unwrap();
    assert!(!masked(cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("mask-toggle", cx);
    })
    .unwrap();
    assert!(masked(cx));
}

/// A row of the field's right-click menu.
fn menu_row(name: &'static str) -> ElementId {
    ElementId::NamedChild(
        ElementId::NamedChild(ElementId::Name("name".into()).into(), "context-menu".into()).into(),
        name.into(),
    )
}

/// The window position of the caret before `offset`.
fn caret_at(state: &Entity<InputState>, offset: usize, cx: &gpui_kit::App) -> Point<Pixels> {
    let state = state.read(cx);
    let bounds = state.text_bounds().expect("the text was laid out");
    let caret = state
        .range_to_bounds(&(offset..offset))
        .expect("the offset is in the line");
    point(caret.origin.x, bounds.center().y)
}

/// A right click at `at`, as a mouse sends it.
fn right_click_at(window: &mut Window, at: Point<Pixels>, cx: &mut gpui_kit::App) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: Default::default(),
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Right,
            position: at,
            modifiers: Default::default(),
            click_count: 1,
        }),
        cx,
    );
}

/// Types `text` into the field and selects its last `selected`
/// characters.
fn type_and_select(setup: &Setup, text: &str, selected: usize, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.input(text, cx);
        for _ in 0..selected {
            window.press("shift-left", cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
}

/// Right-clicks the field inside its text at `offset`.
fn open_menu_at(setup: &Setup, offset: usize, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let at = caret_at(&setup.state, offset, cx) + point(px(1.), px(0.));
        right_click_at(window, at, cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn value(setup: &Setup, cx: &mut TestAppContext) -> String {
    setup
        .state
        .read_with(cx, |state, _| state.value().to_string())
}

#[gpui_kit::test]
fn a_right_click_opens_the_edit_menu_while_another_popup_is_open(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    let popup = cx.update(gpui_kit::base::GlobalState::register_deferred_popover);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.right_click("name", cx);
        window.render_frame(cx);
        assert!(window.try_find(menu_row("copy")).is_some());
    })
    .unwrap();
    drop(popup);
}

#[gpui_kit::test]
fn a_right_click_opens_the_edit_menu_at_the_pointer(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.right_click("name", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let field = window.find("name").bounds();
        let menu = window.find(menu_row("menu")).bounds();
        assert_eq!(
            menu.origin,
            field.center(),
            "the corner sits on the pointer"
        );
        let cut = window.find(menu_row("cut")).bounds();
        assert_eq!(cut.origin, field.center() + point(px(5.), px(5.)));
        assert_eq!(cut.size.height, px(28.));
        for row in ["copy", "paste", "select-all"] {
            assert!(window.try_find(menu_row(row)).is_some(), "{row}");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn copy_puts_the_selection_on_the_clipboard(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_and_select(&setup, "Ada Lovelace", 8, cx);
    open_menu_at(&setup, 6, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("copy"), cx);
        window.render_frame(cx);
        assert!(window.try_find(menu_row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(clipboard(cx).as_deref(), Some("Lovelace"));
    assert_eq!(value(&setup, cx), "Ada Lovelace");
}

#[gpui_kit::test]
fn cut_moves_the_selection_to_the_clipboard(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_and_select(&setup, "Ada Lovelace", 8, cx);
    open_menu_at(&setup, 6, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("cut"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(clipboard(cx).as_deref(), Some("Lovelace"));
    assert_eq!(value(&setup, cx), "Ada ");
}

#[gpui_kit::test]
fn paste_inserts_the_clipboard_text(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("Grace".into())));
    type_and_select(&setup, "Hi ", 0, cx);
    open_menu_at(&setup, 3, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("paste"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), "Hi Grace");
}

#[gpui_kit::test]
fn select_all_then_copy_takes_the_whole_value(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_and_select(&setup, "Ada Lovelace", 0, cx);
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("select-all"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(menu_row("copy"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(clipboard(cx).as_deref(), Some("Ada Lovelace"));
}

#[gpui_kit::test]
fn without_a_selection_copy_and_paste_ignore_the_click(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_and_select(&setup, "Ada", 0, cx);
    open_menu_at(&setup, 3, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        // Nothing is selected and the clipboard is empty.
        window.click(menu_row("copy"), cx);
        window.click(menu_row("paste"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(menu_row("menu")).is_some(),
            "a disabled row keeps the menu open"
        );
    })
    .unwrap();
    assert_eq!(clipboard(cx), None);
    assert_eq!(value(&setup, cx), "Ada");
}

#[gpui_kit::test]
fn a_masked_field_offers_no_cut_or_copy(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            masked: true,
            ..Options::default()
        },
    );
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("before".into())));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.input("hunter2", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        assert_eq!(setup.state.read(cx).selected_range(), 0..7);
        window.right_click("name", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(menu_row("menu")).is_some());
        assert!(window.try_find(menu_row("copy")).is_none());
        assert!(window.try_find(menu_row("cut")).is_none());
        assert!(window.try_find(menu_row("paste")).is_some());
    })
    .unwrap();
    assert_eq!(clipboard(cx).as_deref(), Some("before"));
}

#[gpui_kit::test]
fn escape_closes_the_menu_and_typing_goes_back_into_the_field(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_and_select(&setup, "Ada", 0, cx);
    open_menu_at(&setup, 3, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find(menu_row("menu")).focused(), Some(true));
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(menu_row("menu")).is_none());
        window.input("!", cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), "Ada!");
}

#[gpui_kit::test]
fn the_application_extends_the_menu_with_its_own_item(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            extend_menu: true,
            ..Options::default()
        },
    );
    type_and_select(&setup, "quiet", 0, cx);
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(
            window.try_find(menu_row("copy")).is_some(),
            "the defaults stay"
        );
        window.click(menu_row("shout"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), "QUIET");
}

#[gpui_kit::test]
fn a_field_without_a_menu_ignores_the_right_click(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            context_menu: false,
            ..Options::default()
        },
    );
    type_and_select(&setup, "Ada", 0, cx);
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(menu_row("menu")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_application_replaces_the_menu_with_its_own_items(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            replace_menu: true,
            ..Options::default()
        },
    );
    type_and_select(&setup, "quiet", 0, cx);
    open_menu_at(&setup, 2, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(
            window.try_find(menu_row("cut")).is_none(),
            "no default rows"
        );
        assert!(window.try_find(menu_row("copy")).is_none());
        window.click(menu_row("only"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(value(&setup, cx), "REPLACED");
}
