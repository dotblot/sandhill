//! UI integration tests for `DropdownMenu` and `ContextMenu`: real input
//! through a headless window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    Button, ContextMenu, DropdownMenu, MenuEntry, MenuEvent, MenuItem, MenuState, MenuSubmenu,
    ReduceMotion, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, Role, SharedString, Styled as _, TestAppContext,
    Window, actions,
    base::{Disableable as _, TestSupportExt as _},
    div, px, size,
    test::TestWindowExt as _,
};

actions!(
    menu_test,
    [
        /// The command an action item of the test menu dispatches.
        NewFile
    ]
);

/// What the menus' items did, in order.
type Log = Rc<RefCell<Vec<&'static str>>>;

struct Harness {
    dropdown: Entity<MenuState>,
    context: Entity<MenuState>,
    log: Log,
    /// The page's focus, which holds the keyboard before a menu opens and
    /// so receives the menus' actions, as it would their key bindings.
    focus: FocusHandle,
}

fn logged(log: &Log, name: &'static str) -> impl Fn(&mut Window, &mut gpui_kit::App) + 'static {
    let log = log.clone();
    move |_, _| log.borrow_mut().push(name)
}

fn dropdown_entries(log: &Log) -> Vec<MenuEntry> {
    vec![
        MenuItem::new("new", "New File").action(NewFile).into(),
        MenuItem::new("copy-link", "Copy Link")
            .disabled(true)
            .on_select(logged(log, "copy-link"))
            .into(),
        MenuItem::new("rename", "Rename")
            .on_select(logged(log, "rename"))
            .into(),
        MenuEntry::Separator,
        MenuEntry::label("More"),
        MenuSubmenu::new("share", "Share")
            .entries([
                MenuItem::new("email", "Email").on_select(logged(log, "email")),
                MenuItem::new("chat", "Chat").on_select(logged(log, "chat")),
            ])
            .into(),
        MenuItem::new("wrap", "Word Wrap")
            .checked(true)
            .on_select(logged(log, "wrap"))
            .into(),
        MenuItem::new("delete", "Delete")
            .destructive()
            .on_select(logged(log, "delete"))
            .into(),
    ]
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dropdown_log = self.log.clone();
        let context_log = self.log.clone();
        let action_log = self.log.clone();
        div()
            .track_focus(&self.focus)
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .on_action(move |_: &NewFile, _, _| action_log.borrow_mut().push("new"))
            .child(
                DropdownMenu::new("actions", &self.dropdown)
                    .trigger(Button::new("open").label("Actions"))
                    .items(move |_, _| dropdown_entries(&dropdown_log)),
            )
            .child(
                ContextMenu::new("area", &self.context)
                    .size(px(200.))
                    .items(move |_, _| {
                        vec![
                            MenuItem::new("reveal", "Reveal")
                                .on_select(logged(&context_log, "reveal"))
                                .into(),
                        ]
                    })
                    .child(div().size_full()),
            )
            .child(div().id("elsewhere").test_support().size(px(40.)))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    dropdown: Entity<MenuState>,
    log: Log,
    events: Rc<RefCell<Vec<MenuEvent>>>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
        cx.bind_keys([KeyBinding::new("cmd-n", NewFile, None)]);
    });
    let log: Log = Rc::default();
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut dropdown = None;
    let handle = cx.open_window(size(px(600.), px(600.)), |window, cx| {
        let menu = cx.new(MenuState::new);
        let context = cx.new(MenuState::new);
        let recorded = events.clone();
        cx.subscribe(&menu, move |_, _, event: &MenuEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        dropdown = Some(menu.clone());
        let harness = cx.new(|cx| {
            cx.observe(&menu, |_, _, cx| cx.notify()).detach();
            cx.observe(&context, |_, _, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            Harness {
                dropdown: menu,
                context,
                log: log.clone(),
                focus,
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
        dropdown: dropdown.unwrap(),
        log,
        events,
    }
}

fn child(parent: &'static str, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(ElementId::Name(parent.into()).into(), name.into())
}

fn row(name: &'static str) -> ElementId {
    child("actions", name)
}

fn submenu(name: &'static str) -> ElementId {
    ElementId::NamedChild(row(name).into(), "menu".into())
}

fn log(setup: &Setup) -> Vec<&'static str> {
    setup.log.borrow().clone()
}

#[gpui_kit::test]
fn the_dropdown_opens_under_its_trigger_with_rows_of_the_reference_height(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(row("menu")).is_none());
        window.click("open", cx);
        window.render_frame(cx);
        let trigger = window.find("actions").bounds();
        let menu = window.find(row("menu"));
        assert_eq!(menu.role(), Some(Role::Menu));
        assert_eq!(
            menu.bounds().top(),
            trigger.bottom() + px(2.),
            "the menu gap under the trigger"
        );
        let new = window.find(row("new"));
        assert_eq!(new.bounds().size.height, px(28.));
        assert_eq!(new.role(), Some(Role::MenuItem));
        assert_eq!(new.label(), Some("New File"));
        assert_eq!(window.find(row("wrap")).checked(), Some(true));
        assert_eq!(window.find(row("share")).expanded(), Some(false));
        assert_eq!(menu.focused(), Some(true), "the menu holds the keyboard");
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an open menu at rest asks for no frame"
        );
    })
    .unwrap();
    assert!(setup.dropdown.read_with(cx, |state, _| state.is_open()));
}

#[gpui_kit::test]
fn clicking_an_item_closes_the_menu_then_runs_it(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.click(row("rename"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(log(&setup), ["rename"]);
    assert_eq!(
        &*setup.events.borrow(),
        &[
            MenuEvent::Opened,
            MenuEvent::Activated("rename".into()),
            MenuEvent::Closed,
        ]
    );
}

#[gpui_kit::test]
fn an_action_item_dispatches_its_action(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.click(row("new"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(log(&setup), ["new"]);
}

#[gpui_kit::test]
fn a_disabled_item_ignores_the_click_and_an_outside_press_closes(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.click(row("copy-link"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_some(), "still open");
        window.click("elsewhere", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
        window.click("open", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_some());
        window.click("open", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(row("menu")).is_none(),
            "a press on the trigger closes it"
        );
    })
    .unwrap();
    assert!(log(&setup).is_empty());
}

#[gpui_kit::test]
fn the_keyboard_opens_walks_past_disabled_rows_and_chooses(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find("open").focused(), Some(true));
        window.press("down", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_some());
        assert_eq!(
            setup.dropdown.read(cx).highlighted().as_deref(),
            Some("new")
        );
        // Copy Link is disabled, so Down lands on Rename.
        window.press("down", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(log(&setup), ["rename"]);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("open").focused(),
            Some(true),
            "focus returns to the trigger"
        );
        window.press("space", cx);
        window.render_frame(cx);
        window.press("end", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(log(&setup), ["rename", "delete"]);
}

#[gpui_kit::test]
fn escape_closes_and_gives_focus_back(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_some());
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
        assert_eq!(window.find("open").focused(), Some(true));
    })
    .unwrap();
    assert!(log(&setup).is_empty());
}

#[gpui_kit::test]
fn right_opens_a_submenu_left_closes_it_and_its_items_run(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("down", cx);
        window.press("down", cx);
        window.press("down", cx);
        assert_eq!(
            setup.dropdown.read(cx).highlighted().as_deref(),
            Some("share")
        );
        window.press("right", cx);
        window.render_frame(cx);
        let parent = window.find(row("share")).bounds();
        let panel = window.find(submenu("share")).bounds();
        assert!(
            panel.left() >= parent.right(),
            "the submenu opens beside its row"
        );
        assert_eq!(window.find(row("share")).expanded(), Some(true));
        assert_eq!(
            setup.dropdown.read(cx).highlighted().as_deref(),
            Some("email")
        );
        window.press("left", cx);
        window.render_frame(cx);
        assert!(window.try_find(submenu("share")).is_none());
        assert_eq!(
            setup.dropdown.read(cx).highlighted().as_deref(),
            Some("share")
        );
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(submenu("share")).is_some(),
            "Enter opens it too"
        );
        window.press("down", cx);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(submenu("share")).is_none(),
            "Escape closes the innermost level"
        );
        assert!(window.try_find(row("menu")).is_some());
        window.press("right", cx);
        window.press("down", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(log(&setup), ["chat"]);
}

#[gpui_kit::test]
fn hovering_a_submenu_row_opens_it_and_a_click_chooses_inside(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.hover(row("share"), cx);
        window.render_frame(cx);
        assert!(window.try_find(submenu("share")).is_some());
        window.click(row("email"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(log(&setup), ["email"]);
}

#[gpui_kit::test]
fn a_right_click_opens_the_context_menu_at_the_pointer(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.right_click("area", cx);
        window.render_frame(cx);
        let area = window.find("area").bounds();
        let menu = window.find(child("area", "menu")).bounds();
        assert_eq!(menu.origin, area.center(), "the corner sits on the pointer");
        let reveal = window.find(child("area", "reveal")).bounds();
        assert_eq!(
            reveal.origin,
            area.center() + gpui_kit::point(px(5.), px(5.)),
            "inside the border and the padding"
        );
        window.click(child("area", "reveal"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("area", "menu")).is_none());
    })
    .unwrap();
    assert_eq!(log(&setup), ["reveal"]);
}

#[gpui_kit::test]
fn a_submenu_opens_level_with_its_row_a_gap_from_the_panel(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.hover(row("share"), cx);
        window.render_frame(cx);
        let parent_row = window.find(row("share")).bounds();
        let parent_panel = window.find(row("menu")).bounds();
        let panel = window.find(submenu("share")).bounds();
        let first = window.find(row("email")).bounds();
        assert_eq!(first.top(), parent_row.top(), "the first row is level");
        assert_eq!(
            panel.left(),
            parent_panel.right() + px(2.),
            "the menu gap beside the panel"
        );
    })
    .unwrap();
    assert!(log(&setup).is_empty());
}

#[gpui_kit::test]
fn a_check_takes_the_leading_slot_and_labels_line_up(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        let leading = |key: &'static str| {
            window
                .find(ElementId::NamedChild(row(key).into(), "leading".into()))
                .bounds()
        };
        let label = |key: &'static str| {
            window
                .find(ElementId::NamedChild(row(key).into(), "label".into()))
                .bounds()
        };
        let wrap = window.find(row("wrap")).bounds();
        assert_eq!(
            leading("wrap").left(),
            wrap.left() + px(10.),
            "after the row padding"
        );
        assert!(
            leading("wrap").right() < label("wrap").left(),
            "the check is left of the label"
        );
        assert_eq!(
            label("rename").left(),
            label("wrap").left(),
            "labels line up"
        );
        assert!(
            label("wrap").right() > wrap.right() - px(11.),
            "no trailing slot"
        );
    })
    .unwrap();
}
