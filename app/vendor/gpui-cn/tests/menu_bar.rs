//! UI integration tests for `MenuBar`: real input through a headless
//! window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{
    Input, InputState, MenuBar, MenuBarMenu, MenuBarState, MenuEntry, MenuItem, ReduceMotion, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, OwnedMenu, OwnedMenuItem,
    ParentElement as _, Render, Role, SharedString, Styled as _, TestAppContext, Window,
    base::{
        Disableable as _,
        input::{Copy, Paste, SelectAll},
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
    test::TestWindowExt as _,
};

struct Harness {
    bar: Entity<MenuBarState>,
    input: Entity<InputState>,
    plain: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .child(MenuBar::new("bar", &self.bar).when(self.plain, MenuBar::plain))
            .child(div().w(px(240.)).child(Input::new(&self.input).id("name")))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    bar: Entity<MenuBarState>,
}

fn menus() -> Vec<MenuBarMenu> {
    vec![
        MenuBarMenu::new("file", "File").entries([
            MenuItem::new("new", "New File"),
            MenuItem::new("open", "Open"),
        ]),
        MenuBarMenu::new("edit", "Edit").entries([
            MenuEntry::from(MenuItem::new("copy", "Copy").action(Copy)),
            MenuItem::new("paste", "Paste").action(Paste).into(),
            MenuEntry::Separator,
            MenuItem::new("select-all", "Select All")
                .action(SelectAll)
                .into(),
        ]),
        MenuBarMenu::new("go", "Go").disabled(true),
        MenuBarMenu::new("help", "Help").entries([MenuItem::new("about", "About")]),
    ]
}

fn setup_with(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Context<MenuBarState>) -> MenuBarState,
) -> Setup {
    setup_bar(cx, false, build)
}

fn setup_bar(
    cx: &mut TestAppContext,
    plain: bool,
    build: impl FnOnce(&mut Context<MenuBarState>) -> MenuBarState,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let bar = Rc::new(Cell::new(None));
    let handle = cx.open_window(size(px(600.), px(500.)), |window, cx| {
        let state = cx.new(build);
        bar.set(Some(state.clone()));
        let input = cx.new(|cx| InputState::new(window, cx));
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Harness {
                bar: state,
                input,
                plain,
            }
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
        bar: bar.take().unwrap(),
    }
}

fn setup(cx: &mut TestAppContext) -> Setup {
    setup_with(cx, |cx| MenuBarState::new(menus(), cx))
}

fn title(key: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::Name("bar".into()).into(), key.into())
}

fn row(key: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(title("menus").into(), key.into())
}

fn open_menu(setup: &Setup, cx: &mut TestAppContext) -> Option<String> {
    setup
        .bar
        .read_with(cx, |bar, _| bar.open_menu().map(|key| key.to_string()))
}

#[gpui_kit::test]
fn a_click_on_a_title_opens_its_menu_under_it(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find("bar").role(), Some(Role::MenuBar));
        let file = window.find(title("file"));
        assert_eq!(file.role(), Some(Role::MenuItem));
        assert_eq!(file.bounds().size.height, px(28.));
        window.click(title("file"), cx);
        window.render_frame(cx);
        let file = window.find(title("file")).bounds();
        let menu = window.find(row("menu")).bounds();
        assert_eq!(menu.top(), file.bottom() + px(2.), "the menu gap");
        assert_eq!(menu.left(), file.left(), "aligned with the title's start");
        assert!(window.try_find(row("new")).is_some());
        assert_eq!(window.find(title("file")).expanded(), Some(true));
        window.click(title("file"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row("menu")).is_none(),
            "a second click closes"
        );
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx), None);
}

#[gpui_kit::test]
fn hovering_another_title_while_open_switches_to_it(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.hover(title("edit"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row("menu")).is_none(),
            "no menu opens on hover alone"
        );
        window.click(title("file"), cx);
        window.render_frame(cx);
        window.hover(title("edit"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("copy")).is_some());
        assert!(window.try_find(row("new")).is_none());
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx).as_deref(), Some("edit"));
}

#[gpui_kit::test]
fn right_and_left_move_between_menus_skip_disabled_and_wrap(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(title("file"), cx);
        window.render_frame(cx);
        window.press("right", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx).as_deref(), Some("edit"));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("right", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        open_menu(&setup, cx).as_deref(),
        Some("help"),
        "Go is disabled"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("right", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx).as_deref(), Some("file"), "wraps");
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("left", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("about")).is_some());
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
        window.click(title("go"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row("menu")).is_none(),
            "a disabled title does not open"
        );
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx), None);
}

#[gpui_kit::test]
fn edit_copy_acts_on_the_focused_field(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("name", cx);
        window.input("Ada Lovelace", cx);
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.click(title("edit"), cx);
        window.render_frame(cx);
        window.click(row("copy"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
        assert_eq!(
            window.find("name").focused(),
            Some(true),
            "focus returns to the field"
        );
    })
    .unwrap();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("Ada Lovelace"));
}

#[gpui_kit::test]
fn the_applications_menus_convert_with_labels_and_separators(cx: &mut TestAppContext) {
    let setup = setup_with(cx, |cx| {
        let menus = vec![
            OwnedMenu {
                name: "Edit".into(),
                disabled: false,
                items: vec![
                    OwnedMenuItem::Action {
                        name: "Copy".into(),
                        action: Box::new(Copy),
                        os_action: None,
                        checked: false,
                        disabled: false,
                    },
                    OwnedMenuItem::Separator,
                    OwnedMenuItem::Action {
                        name: "Select All".into(),
                        action: Box::new(SelectAll),
                        os_action: None,
                        checked: true,
                        disabled: true,
                    },
                ],
            },
            OwnedMenu {
                name: "Window".into(),
                disabled: true,
                items: Vec::new(),
            },
        ];
        MenuBarState::new(menus, cx)
    });
    let (copy, select_all) = cx.update(|_| {
        use gpui_kit::Action as _;
        (Copy.name(), SelectAll.name())
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find(title("Edit")).label(), Some("Edit"));
        window.click(title("Edit"), cx);
        window.render_frame(cx);
        assert_eq!(window.find(row(copy)).label(), Some("Copy"));
        let select = window.find(row(select_all));
        assert_eq!(select.label(), Some("Select All"));
        assert_eq!(select.checked(), Some(true));
        assert_eq!(
            select.bounds().top(),
            window.find(row(copy)).bounds().bottom() + px(9.),
            "the separator: its hairline with the menu inset above and below"
        );
        window.click(row(select_all), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row("menu")).is_some(),
            "a disabled item stays"
        );
    })
    .unwrap();
    assert!(
        setup
            .bar
            .read_with(cx, |bar, _| bar.menus()[1].is_disabled())
    );
}

#[gpui_kit::test]
fn rows_of_one_action_get_keys_of_their_own(cx: &mut TestAppContext) {
    let setup = setup_with(cx, |cx| {
        let menu = gpui_kit::Menu::new("File").items([
            gpui_kit::MenuItem::action("Save", Paste),
            gpui_kit::MenuItem::action("Save Copy", Paste),
        ]);
        MenuBarState::new([menu.owned()], cx)
    });
    let paste = cx.update(|_| {
        use gpui_kit::Action as _;
        Paste.name()
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(title("File"), cx);
        window.render_frame(cx);
        assert_eq!(window.find(row(paste)).label(), Some("Save"));
        assert_eq!(
            window.find(row(format!("{paste}/Save Copy"))).label(),
            Some("Save Copy")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_bar_is_a_framed_strip_with_its_titles_inset(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let bar = window.find("bar").bounds();
        let file = window.find(title("file")).bounds();
        // The hairline, then the 4px inset.
        assert_eq!(file.left(), bar.left() + px(1.) + px(4.));
        assert_eq!(file.top(), bar.top() + px(1.) + px(4.));
        assert_eq!(file.size.height, px(28.));
        assert_eq!(bar.size.height, px(28. + 8. + 2.));
        let help = window.find(title("help")).bounds();
        assert_eq!(help.right(), bar.right() - px(1.) - px(4.));
        window.click(title("edit"), cx);
        window.render_frame(cx);
        let edit = window.find(title("edit")).bounds();
        let menu = window.find(row("menu")).bounds();
        assert_eq!(
            menu.top(),
            edit.bottom() + px(2.),
            "the menu gap under the title"
        );
        assert_eq!(
            menu.left(),
            edit.left(),
            "lined up with the title, not the frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_plain_bar_has_no_frame(cx: &mut TestAppContext) {
    let setup = setup_bar(cx, true, |cx| MenuBarState::new(menus(), cx));
    cx.update_window(setup.handle.into(), |_, window, _| {
        let bar = window.find("bar").bounds();
        let file = window.find(title("file")).bounds();
        assert_eq!(file.origin, bar.origin);
        assert_eq!(bar.size.height, px(28.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_disabled_title_does_not_open(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(title("go"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(open_menu(&setup, cx), None);
}

#[gpui_kit::test]
fn the_keyboard_walks_the_titles_and_opens_one(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find(title("file")).focused(), Some(true));
        window.press("right", cx);
        assert_eq!(window.find(title("edit")).focused(), Some(true));
        window.press("down", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("copy")).is_some());
        assert_eq!(
            setup.bar.read(cx).menu().read(cx).highlighted().as_deref(),
            Some("copy")
        );
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
        assert_eq!(
            window.find(title("edit")).focused(),
            Some(true),
            "focus returns"
        );
        // A button clicks from the keyboard when the key comes up.
        window.press("enter", cx);
        window.dispatch_event(
            gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent {
                keystroke: gpui_kit::Keystroke::parse("enter").unwrap(),
            }),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        open_menu(&setup, cx).as_deref(),
        Some("edit"),
        "Enter opens it"
    );
}
