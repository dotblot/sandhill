//! UI integration tests for the kinds of menu rows: links, descriptions,
//! custom content, radio items, per-item action targets, widths, rows
//! replaced while open, tidied separators, and submenus of a long menu.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    Button, DropdownMenu, MenuEntry, MenuItem, MenuState, MenuSubmenu, ReduceMotion, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, Role, SharedString, Styled as _, TestAppContext, Window,
    actions, div,
    prelude::FluentBuilder as _,
    px, size,
    test::{TestSupportExt as _, TestWindowExt as _},
};

actions!(
    menu_items_test,
    [
        /// The command the test menus' action items dispatch.
        Ping
    ]
);

type Log = Rc<RefCell<Vec<&'static str>>>;

struct Harness {
    menu: Entity<MenuState>,
    entries: Rc<RefCell<Vec<MenuEntry>>>,
    width: Option<Pixels>,
    focus: FocusHandle,
    target: FocusHandle,
    log: Log,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let entries = self.entries.clone();
        let (page_log, target_log) = (self.log.clone(), self.log.clone());
        div()
            .track_focus(&self.focus)
            .on_action(move |_: &Ping, _, _| page_log.borrow_mut().push("page"))
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .gap_4()
            .child(
                DropdownMenu::new("m", &self.menu)
                    .align(gpui_kit::base::Align::Start)
                    .trigger(Button::new("open").label("Open"))
                    .when_some(self.width, |this, width| this.menu_width(width))
                    .items(move |_, _| entries.borrow().clone()),
            )
            .child(
                div()
                    .id("target")
                    .test_support()
                    .track_focus(&self.target)
                    .on_action(move |_: &Ping, _, _| target_log.borrow_mut().push("target"))
                    .size(px(20.)),
            )
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    menu: Entity<MenuState>,
    target: FocusHandle,
    log: Log,
}

fn setup(cx: &mut TestAppContext, entries: Vec<MenuEntry>, width: Option<Pixels>) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let log: Log = Rc::default();
    let mut out = None;
    let handle = cx.open_window(size(px(600.), px(700.)), |window, cx| {
        let menu = cx.new(MenuState::new);
        let harness = cx.new(|cx| {
            cx.observe(&menu, |_, _, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            let target = cx.focus_handle();
            out = Some((menu.clone(), target.clone()));
            Harness {
                menu,
                entries: Rc::new(RefCell::new(entries)),
                width,
                focus,
                target,
                log: log.clone(),
            }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let (menu, target) = out.unwrap();
    Setup {
        handle,
        menu,
        target,
        log,
    }
}

fn row(key: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(ElementId::Name("m".into()).into(), key.into())
}

fn part(key: &'static str, name: &'static str) -> ElementId {
    ElementId::NamedChild(row(key).into(), name.into())
}

fn open(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_link_item_opens_its_url(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        vec![
            MenuItem::new("docs", "Docs")
                .link("https://example.com/docs")
                .into(),
            MenuItem::new("plain", "Plain link")
                .link("https://example.com")
                .external_link_icon(false)
                .into(),
        ],
        None,
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let docs = window.find(part("docs", "label")).bounds();
        let plain = window.find(part("plain", "label")).bounds();
        assert!(
            docs.right() < plain.right(),
            "the external-link icon takes room after the label"
        );
        window.click(row("docs"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/docs"));
}

#[gpui_kit::test]
fn a_description_sits_under_the_label_and_a_custom_row_keeps_its_behavior(cx: &mut TestAppContext) {
    let log: Log = Rc::default();
    let custom_log = log.clone();
    let setup = setup(
        cx,
        vec![
            MenuItem::new("plain", "Plain").into(),
            MenuItem::new("described", "Described")
                .description("A line under the label")
                .into(),
            MenuItem::new("custom", "Custom")
                .render(|row, _, _| {
                    div()
                        .id("custom-content")
                        .test_support()
                        .size(px(12.))
                        .when(row.is_highlighted(), |this| this.w(px(24.)))
                })
                .on_select(move |_, _| custom_log.borrow_mut().push("custom"))
                .into(),
        ],
        None,
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find(row("plain")).bounds().size.height, px(28.));
        assert!(
            window.find(row("described")).bounds().size.height > px(28.),
            "a description makes the row taller"
        );
        assert_eq!(window.find(row("custom")).bounds().size.height, px(28.));
        assert_eq!(window.find("custom-content").bounds().size.width, px(12.));
        window.hover(row("custom"), cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("custom-content").bounds().size.width,
            px(24.),
            "the renderer sees the highlight"
        );
        window.click(row("custom"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(&*log.borrow(), &["custom"]);
    assert!(setup.log.borrow().is_empty());
}

#[gpui_kit::test]
fn radio_items_show_the_selected_one_in_the_leading_slot(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        vec![
            MenuEntry::label("Theme"),
            MenuItem::new("light", "Light").radio(false).into(),
            MenuItem::new("dark", "Dark").radio(true).into(),
        ],
        None,
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        let dark = window.find(row("dark"));
        assert_eq!(dark.role(), Some(Role::MenuItemRadio));
        assert_eq!(dark.checked(), Some(true));
        assert_eq!(window.find(row("light")).checked(), Some(false));
        assert!(
            window.find(part("dark", "leading")).bounds().right()
                < window.find(part("dark", "label")).bounds().left()
        );
        assert_eq!(
            window.find(part("light", "label")).bounds().left(),
            window.find(part("dark", "label")).bounds().left(),
            "labels line up whether the dot shows or not"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn actions_go_to_the_focus_from_before_or_to_an_items_own_target(cx: &mut TestAppContext) {
    let setup = setup(cx, Vec::new(), None);
    let entries: Vec<MenuEntry> = vec![
        MenuItem::new("page", "To the page").action(Ping).into(),
        MenuItem::new("target", "To the target")
            .action(Ping)
            .action_context(&setup.target)
            .into(),
    ];
    for key in ["page", "target"] {
        let entries = entries.clone();
        cx.update_window(setup.handle.into(), |_, window, cx| {
            setup.menu.update(cx, |menu, cx| {
                menu.open(
                    entries,
                    gpui_cn::MenuAnchor::Point(gpui_kit::point(px(100.), px(100.))),
                    window,
                    cx,
                )
            });
            window.render_frame(cx);
            window.click(row(key), cx);
            window.render_frame(cx);
        })
        .unwrap();
    }
    assert_eq!(&*setup.log.borrow(), &["page", "target"]);
}

#[gpui_kit::test]
fn a_menu_width_fixes_the_panel(cx: &mut TestAppContext) {
    let setup = setup(cx, vec![MenuItem::new("a", "A").into()], Some(px(300.)));
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find(row("menu")).bounds().size.width, px(300.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn rows_replaced_while_open_keep_the_highlighted_key(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        vec![
            MenuItem::new("a", "A").into(),
            MenuItem::new("b", "B").into(),
        ],
        None,
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("down", cx);
        window.press("down", cx);
        assert_eq!(setup.menu.read(cx).highlighted().as_deref(), Some("b"));
        setup.menu.update(cx, |menu, cx| {
            menu.set_entries(
                [
                    MenuItem::new("loading", "Loaded later"),
                    MenuItem::new("a", "A"),
                    MenuItem::new("b", "B"),
                ],
                cx,
            )
        });
        window.render_frame(cx);
        assert!(window.try_find(row("loading")).is_some());
        assert_eq!(setup.menu.read(cx).highlighted().as_deref(), Some("b"));
        window.press("up", cx);
        assert_eq!(setup.menu.read(cx).highlighted().as_deref(), Some("a"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn separators_are_tidied(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        vec![
            MenuEntry::Separator,
            MenuItem::new("a", "A").into(),
            MenuEntry::Separator,
            MenuEntry::Separator,
            MenuItem::new("b", "B").into(),
            MenuEntry::Separator,
            MenuEntry::label("Nothing under it"),
            MenuEntry::Separator,
        ],
        None,
    );
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        let menu = window.find(row("menu")).bounds();
        let a = window.find(row("a")).bounds();
        let b = window.find(row("b")).bounds();
        assert_eq!(
            a.top(),
            menu.top() + px(5.),
            "no separator above the first row"
        );
        assert_eq!(
            b.top(),
            a.bottom() + px(9.),
            "one separator between the rows"
        );
        assert_eq!(
            menu.bottom(),
            b.bottom() + px(5.),
            "none after the last row"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_submenu_of_a_scrolled_menu_is_not_clipped(cx: &mut TestAppContext) {
    let keys: Vec<&'static str> = (0..30)
        .map(|n| &*Box::leak(format!("item-{n}").into_boxed_str()))
        .collect();
    let log: Log = Rc::default();
    let chosen = log.clone();
    let mut entries: Vec<MenuEntry> = keys
        .iter()
        .map(|key| MenuItem::new(*key, *key).into())
        .collect();
    entries.push(
        MenuSubmenu::new("more", "More")
            .entries([MenuItem::new("deep", "Deep").on_select(move |_, _| {
                chosen.borrow_mut().push("deep");
            })])
            .into(),
    );
    let setup = setup(cx, entries, None);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.press("down", cx);
        window.press("end", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        window.press("right", cx);
        window.render_frame(cx);
        let parent = window.find(row("menu")).bounds();
        assert!(
            parent.size.height <= px(390.),
            "the menu scrolls at its height"
        );
        let more = window.find(row("more")).bounds();
        assert!(
            more.bottom() <= parent.bottom(),
            "the row scrolled into view"
        );
        let panel = window
            .find(ElementId::NamedChild(row("more").into(), "menu".into()))
            .bounds();
        assert!(
            panel.left() >= parent.right(),
            "beside the parent, not inside it"
        );
        window.click(row("deep"), cx);
        window.render_frame(cx);
        assert!(window.try_find(row("menu")).is_none());
    })
    .unwrap();
    assert_eq!(&*log.borrow(), &["deep"]);
}

#[gpui_kit::test]
fn a_rows_icon_lines_up_with_the_label_not_the_description(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        vec![
            MenuItem::new("plain", "Plain")
                .icon(gpui_cn::gpui_kit::assets::IconName::File)
                .into(),
            MenuItem::new("described", "Described")
                .icon(gpui_cn::gpui_kit::assets::IconName::File)
                .description("A line under the label")
                .into(),
        ],
        None,
    );
    open(&setup, cx);
    let line = cx.update(|cx| {
        use gpui_cn::ActiveTheme as _;
        cx.theme().text_control.line_height
    });
    cx.update_window(setup.handle.into(), |_, window, _| {
        let leading = window.find(part("described", "leading")).bounds();
        let label = window.find(part("described", "label")).bounds();
        let expected = label.top() + line / 2.;
        assert!(
            (leading.center().y - expected).abs() <= px(0.5),
            "{:?} against {:?}",
            leading.center().y,
            expected
        );
        let row = window.find(row("plain")).bounds();
        let leading = window.find(part("plain", "leading")).bounds();
        assert!((leading.center().y - row.center().y).abs() <= px(0.5));
    })
    .unwrap();
}
