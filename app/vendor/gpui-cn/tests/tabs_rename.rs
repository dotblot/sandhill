//! `Tabs`: inline rename on a double click, and a context menu per tab.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui_cn::{ActiveTheme as _, MenuItem, MenuState, Tab, Tabs, TabsEvent, TabsState};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, Window, WindowHandle, div, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    tabs: Entity<TabsState>,
    menu: Entity<MenuState>,
    renamable: bool,
    asked: Rc<RefCell<Vec<SharedString>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.theme().metrics.title_bar;
        let asked = self.asked.clone();
        div().size_full().child(
            div().w_full().h(bar).child(
                Tabs::new("tabs", &self.tabs)
                    .renamable(self.renamable)
                    .context_menu(&self.menu, move |id, _, _| {
                        asked.borrow_mut().push(id.clone());
                        vec![MenuItem::new("close", "Close Tab").into()]
                    }),
            ),
        )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    tabs: Entity<TabsState>,
    menu: Entity<MenuState>,
    events: Rc<RefCell<Vec<TabsEvent>>>,
    asked: Rc<RefCell<Vec<SharedString>>>,
}

fn setup(cx: &mut TestAppContext, renamable: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let tabs = cx.new(|_| {
        TabsState::new([
            Tab::new("logs", "Logs"),
            Tab::new("shell", "Shell"),
            Tab::new("notes", "Notes"),
        ])
    });
    let menu = cx.new(MenuState::new);
    let events = Rc::new(RefCell::new(Vec::new()));
    let asked = Rc::new(RefCell::new(Vec::new()));
    cx.update({
        let events = events.clone();
        let tabs = tabs.clone();
        move |cx| {
            cx.subscribe(&tabs, move |_, event: &TabsEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        }
    });
    let handle = cx.open_window(size(px(900.), px(300.)), {
        let tabs = tabs.clone();
        let menu = menu.clone();
        let asked = asked.clone();
        move |window, cx| {
            let harness = cx.new(|cx| {
                cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
                Harness {
                    tabs,
                    menu,
                    renamable,
                    asked,
                }
            });
            Root::new(harness, window, cx)
        }
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        tabs,
        menu,
        events,
        asked,
    }
}

fn tab(id: &str) -> ElementId {
    ElementId::NamedChild(Arc::new("tabs".into()), id.to_owned().into())
}

fn part(id: &str, name: &str) -> ElementId {
    ElementId::NamedChild(Arc::new(tab(id)), name.to_owned().into())
}

fn label(setup: &Setup, id: &str, cx: &TestAppContext) -> String {
    setup.tabs.read_with(cx, |tabs, _| {
        tabs.tabs()
            .iter()
            .find(|tab| tab.id() == id)
            .map(|tab| tab.label().to_string())
            .unwrap_or_default()
    })
}

fn renames(setup: &Setup) -> usize {
    setup
        .events
        .borrow()
        .iter()
        .filter(|event| matches!(event, TabsEvent::Renamed(_)))
        .count()
}

#[gpui_kit::test]
fn rename_sets_a_label_and_reports_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update(|cx| {
        setup.tabs.update(cx, |tabs, cx| {
            tabs.rename("shell", "Build", cx);
            tabs.rename("shell", "Build", cx);
            tabs.rename("missing", "Nothing", cx);
        })
    });
    assert_eq!(label(&setup, "shell", cx), "Build");
    assert_eq!(
        *setup.events.borrow(),
        [TabsEvent::Renamed("shell".into())],
        "an unchanged label or an unknown tab reports nothing"
    );
}

#[gpui_kit::test]
fn a_double_click_renames_inline_and_enter_commits(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.double_click(tab("shell"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        setup.tabs.read_with(cx, |tabs, _| tabs.renaming().cloned()),
        Some("shell".into())
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(part("shell", "rename")).is_some());
        // The label starts selected, so typing replaces it.
        window.input("Server", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(label(&setup, "shell", cx), "Server");
    assert_eq!(renames(&setup), 1);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(part("shell", "rename")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_cancels_an_inline_rename(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.double_click(tab("notes"), cx);
        window.render_frame(cx);
        window.input("Scratch", cx);
        window.press("escape", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(label(&setup, "notes", cx), "Notes");
    assert_eq!(renames(&setup), 0);
    assert!(
        setup
            .tabs
            .read_with(cx, |tabs, _| tabs.renaming().is_none())
    );
}

#[gpui_kit::test]
fn a_strip_that_is_not_renamable_ignores_a_double_click(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.double_click(tab("shell"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        setup
            .tabs
            .read_with(cx, |tabs, _| tabs.renaming().is_none())
    );
    assert_eq!(
        setup.tabs.read_with(cx, |tabs, _| tabs.selected().cloned()),
        Some("shell".into()),
        "the click still selects"
    );
}

#[gpui_kit::test]
fn a_right_click_opens_the_tab_menu_for_that_tab(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.right_click(tab("notes"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(*setup.asked.borrow(), [SharedString::from("notes")]);
    assert!(setup.menu.read_with(cx, |menu, _| menu.is_open()));
}
