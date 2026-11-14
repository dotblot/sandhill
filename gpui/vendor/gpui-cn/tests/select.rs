//! UI integration tests for `Select`: real input through a headless
//! window.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui_cn::{ReduceMotion, Select, SelectEntry, SelectEvent, SelectItem, SelectState, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window,
    base::{Disableable as _, TestSupportExt as _},
    div,
    prelude::FluentBuilder as _,
    px, size,
    test::TestWindowExt as _,
};

struct Harness {
    state: Entity<State>,
    disabled: bool,
    /// Whether the select sits at the bottom of the window, so its menu
    /// has to open upward.
    at_bottom: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .when(self.at_bottom, |this| this.flex_col_reverse())
            .when(!self.at_bottom, |this| this.justify_between())
            .child(
                Select::new("pick", &self.state)
                    .placeholder("Pick one")
                    .accessibility_label("Pick")
                    .disabled(self.disabled),
            )
            .child(div().id("elsewhere").test_support().size(px(40.)))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<State>,
    events: Rc<RefCell<Vec<SelectEvent<&'static str>>>>,
}

fn entries() -> Vec<SelectEntry<&'static str>> {
    vec![
        SelectEntry::label("Type"),
        SelectItem::new("all", "All chats").into(),
        SelectItem::new("local", "Local").disabled(true).into(),
        SelectItem::new("cloud", "Cloud").into(),
        SelectEntry::Separator,
        SelectItem::new("de", "Deutsch").keywords(["german"]).into(),
    ]
}

type State = SelectState<&'static str>;

fn setup(
    cx: &mut TestAppContext,
    disabled: bool,
    build: impl FnOnce(&mut Window, &mut Context<State>) -> State,
) -> Setup {
    setup_with(cx, disabled, false, ReduceMotion::On, build)
}

fn setup_with(
    cx: &mut TestAppContext,
    disabled: bool,
    at_bottom: bool,
    motion: ReduceMotion,
    build: impl FnOnce(&mut Window, &mut Context<State>) -> State,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut state = None;
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| build(window, cx));
        let recorded = events.clone();
        cx.subscribe(
            &select,
            move |_, _, event: &SelectEvent<&'static str>, _| {
                recorded.borrow_mut().push(event.clone());
            },
        )
        .detach();
        state = Some(select.clone());
        let harness = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Harness {
                state: select,
                disabled,
                at_bottom,
            }
        });
        Root::new(harness, window, cx)
    });
    // A window that is not active reports no focus paths, so focus
    // events never fire in it; a real window is active when clicked.
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        state: state.unwrap(),
        events,
    }
}

/// A leaked number, since the harness keys rows by `&'static str`.
fn number(n: usize) -> &'static str {
    Box::leak(n.to_string().into_boxed_str())
}

fn child(name: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::Name("pick".into()).into(), name.into())
}

fn selected(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    setup.state.read_with(cx, |state, _| {
        state.selected().iter().map(|v| v.to_string()).collect()
    })
}

#[gpui_kit::test]
fn the_trigger_is_a_control_and_a_click_opens_a_menu_of_rows(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| SelectState::new(entries(), cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let trigger = window.find(child("trigger"));
        assert_eq!(trigger.bounds().size.height, px(28.));
        assert!(window.try_find(child("menu")).is_none());
        assert_eq!(window.find("pick").label(), Some("Pick"));
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        let menu = window.find(child("menu"));
        let trigger = window.find(child("trigger"));
        assert!(
            menu.bounds().size.width >= trigger.bounds().size.width,
            "never narrower than the trigger"
        );
        assert!(
            menu.bounds().origin.y >= trigger.bounds().bottom(),
            "opens under the trigger"
        );
        let row = window.find(child("all"));
        assert_eq!(row.bounds().size.height, px(28.));
        assert!(window.try_find(child("local")).is_some());
        assert_eq!(
            window.simulate_next_frame(cx),
            1,
            "the first opening asks for one frame, to scroll to the row once the list has a height"
        );
        window.render_frame(cx);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an open menu at rest asks for no frame"
        );
    })
    .unwrap();
    assert!(setup.state.read_with(cx, |state, _| state.is_open()));
    assert_eq!(&*setup.events.borrow(), &[SelectEvent::OpenChanged(true)]);
}

#[gpui_kit::test]
fn clicking_a_row_selects_it_and_closes(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| SelectState::new(entries(), cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.click(child("cloud"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
        assert_eq!(window.find("pick").focused(), Some(true), "focus returns");
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["cloud"]);
    assert_eq!(
        &*setup.events.borrow(),
        &[
            SelectEvent::OpenChanged(true),
            SelectEvent::Changed(vec!["cloud"]),
            SelectEvent::OpenChanged(false),
        ]
    );
}

#[gpui_kit::test]
fn a_disabled_row_ignores_the_click_and_a_press_outside_closes(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| SelectState::new(entries(), cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.click(child("local"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_some(), "still open");
        window.click("elsewhere", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_some());
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
    })
    .unwrap();
    assert!(selected(&setup, cx).is_empty());
}

#[gpui_kit::test]
fn the_keyboard_opens_walks_chooses_and_escapes(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| {
        SelectState::new(entries(), cx).with_selected(["all"])
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(window.find("pick").focused(), Some(true));
        window.press("down", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_some());
        assert_eq!(
            setup
                .state
                .read(cx)
                .highlighted()
                .map(|item| item.value().to_string()),
            Some("all".into()),
            "opens on the selected row"
        );
        window.press("down", cx);
        assert_eq!(
            setup
                .state
                .read(cx)
                .highlighted()
                .map(|item| item.value().to_string()),
            Some("cloud".into()),
            "skips the disabled row"
        );
        window.press("end", cx);
        assert_eq!(
            setup
                .state
                .read(cx)
                .highlighted()
                .map(|item| item.value().to_string()),
            Some("de".into())
        );
        window.press("home", cx);
        window.press("down", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
    })
    .unwrap();
    // Focus returns to the trigger once base's own confirm handling is
    // done, after the key's update, as between two real key presses.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("pick").focused(), Some(true));
        window.press("space", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_some(), "Space opens too");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
        assert_eq!(window.find("pick").focused(), Some(true));
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["cloud"]);
}

#[gpui_kit::test]
fn a_search_narrows_the_rows_and_clears_on_close(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("search")).is_some());
        window.input("germ", cx);
    })
    .unwrap();
    // The field reports its change as an event, which lands after the
    // keystroke's own update, as it does between two real key presses.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(child("de")).is_some(), "a keyword matches");
        assert!(window.try_find(child("all")).is_none());
        assert_eq!(setup.state.read(cx).query(cx).as_ref(), "germ");
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
        assert_eq!(setup.state.read(cx).query(cx).as_ref(), "");
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("zzz", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(child("de")).is_none(), "nothing matches");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["de"]);
}

#[gpui_kit::test]
fn a_space_typed_in_the_search_is_a_character_not_a_confirm(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("all ", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(setup.state.read(cx).query(cx).as_ref(), "all ");
        assert!(window.try_find(child("menu")).is_some(), "still open");
        assert!(window.try_find(child("all")).is_some());
    })
    .unwrap();
    assert!(selected(&setup, cx).is_empty());
}

#[gpui_kit::test]
fn a_multiple_select_toggles_rows_and_stays_open(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| {
        SelectState::multiple(entries(), cx).with_selected(["all"])
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.click(child("cloud"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_some(), "stays open");
        window.click(child("all"), cx);
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["cloud"]);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.click(child("de"), cx);
        window.render_frame(cx);
        window.press("escape", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["cloud", "de"], "menu order");
}

#[gpui_kit::test]
fn a_disabled_select_ignores_input(cx: &mut TestAppContext) {
    let setup = setup(cx, true, |_, cx| SelectState::new(entries(), cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
        window.press("tab", cx);
        assert_ne!(window.find("pick").focused(), Some(true));
    })
    .unwrap();
    assert!(setup.events.borrow().is_empty());
}

#[gpui_kit::test]
fn a_long_list_lays_out_only_the_rows_in_view(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| {
        SelectState::new(
            (1..=10_000).map(|n| SelectItem::new(number(n), format!("Item {n}"))),
            cx,
        )
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("1")).is_some());
        assert!(window.try_find(child("9999")).is_none());
        let menu = window.find(child("menu"));
        assert!(
            menu.bounds().size.height <= px(390.),
            "capped at the menu height"
        );
        window.press("end", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child("10000")).is_some(),
            "End scrolls to the last row"
        );
        assert!(window.try_find(child("1")).is_none());
        window.press("home", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("1")).is_some());
        window.scroll(
            child("rows"),
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-300.))),
            cx,
        );
        window.render_frame(cx);
        assert!(
            window.try_find(child("1")).is_none(),
            "the wheel scrolls the rows"
        );
        assert!(window.try_find(child("12")).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_menu_fades_in_and_goes_out_at_once(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| SelectState::new(entries(), cx));
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the fade asks for frames until it lands"
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(settles(window, cx), "landed, at rest");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child("menu")).is_none(),
            "gone in the frame it closed: GPUI cannot fade a panel over its own shadow"
        );
        assert_eq!(window.simulate_next_frame(cx), 0);
    })
    .unwrap();
}

/// Whether the window stops asking for frames within a few: the frame
/// that sees a fade land still asks for one more to paint the rest.
fn settles(window: &mut Window, cx: &mut gpui_kit::App) -> bool {
    (0..4).any(|_| {
        window.render_frame(cx);
        window.simulate_next_frame(cx) == 0
    })
}

/// Stands in for a request: dropped before it answered means the request
/// was cancelled, not merely its answer ignored.
struct Request {
    query: String,
    answered: bool,
    dropped_unanswered: Arc<Mutex<Vec<String>>>,
}

impl Request {
    /// Marks the request answered. A method, so the async block captures
    /// the whole request rather than the one field it assigns.
    fn answer(&mut self) {
        self.answered = true;
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if !self.answered {
            self.dropped_unanswered
                .lock()
                .unwrap()
                .push(self.query.clone());
        }
    }
}

#[gpui_kit::test]
fn a_search_handler_answers_later_and_a_newer_query_cancels_it(cx: &mut TestAppContext) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let setup = setup(cx, false, {
        let asked = asked.clone();
        let cancelled = cancelled.clone();
        move |window, cx| {
            SelectState::new(entries(), cx)
                .with_selected(["cloud"])
                .with_search_handler(
                    "Search",
                    move |query, cx| {
                        asked.borrow_mut().push(query.to_string());
                        let timer = cx
                            .background_executor()
                            .timer(std::time::Duration::from_millis(200));
                        let mut request = Request {
                            query: query.to_string(),
                            answered: false,
                            dropped_unanswered: cancelled.clone(),
                        };
                        cx.background_spawn(async move {
                            timer.await;
                            request.answer();
                            vec![
                                SelectItem::new(number(query.len()), format!("Hit for {query}"))
                                    .into(),
                            ]
                        })
                    },
                    window,
                    cx,
                )
        }
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(setup.state.read(cx).is_searching(), "asked on open");
        assert!(
            window.try_find(child("cloud")).is_some(),
            "the given rows show until the first answer"
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!setup.state.read(cx).is_searching());
        assert!(
            window.try_find(child("0")).is_some(),
            "the empty query's answer"
        );
        assert!(window.try_find(child("cloud")).is_none());
        assert_eq!(
            setup.state.read(cx).selected(),
            ["cloud"],
            "the selection outlives the rows"
        );
        window.input("a", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.input("b", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(150));
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(child("1")).is_none(),
            "the answer to \"a\" was cancelled by \"ab\""
        );
        assert!(setup.state.read(cx).is_searching(), "\"ab\" is still out");
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(
            setup.state.read(cx).is_open(),
            "Enter while the answer is out chooses nothing: the rows it is on are stale"
        );
    })
    .unwrap();
    assert_eq!(
        &*cancelled.lock().unwrap(),
        &["a"],
        "the request for \"a\" was dropped mid-flight, not left to finish"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(child("2")).is_some(),
            "the answer to \"ab\""
        );
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(&*asked.borrow(), &["", "a", "ab"]);
    assert_eq!(selected(&setup, cx), ["2"]);
    assert!(!setup.state.read_with(cx, |state, _| state.is_open()));
    assert_eq!(&*cancelled.lock().unwrap(), &["a"], "the rest answered");
}

#[gpui_kit::test]
fn closing_cancels_a_running_search(cx: &mut TestAppContext) {
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let setup = setup(cx, false, {
        let cancelled = cancelled.clone();
        move |window, cx| {
            SelectState::new(entries(), cx).with_search_handler(
                "Search",
                move |query, cx| {
                    let timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(200));
                    let mut request = Request {
                        query: query.to_string(),
                        answered: false,
                        dropped_unanswered: cancelled.clone(),
                    };
                    cx.background_spawn(async move {
                        timer.await;
                        request.answer();
                        Vec::new()
                    })
                },
                window,
                cx,
            )
        }
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.press("escape", cx);
        assert!(!setup.state.read(cx).is_searching());
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(&*cancelled.lock().unwrap(), &[""]);
}

#[gpui_kit::test]
fn a_custom_row_renderer_draws_the_rows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    struct Custom {
        state: Entity<State>,
    }
    impl Render for Custom {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .p_4()
                .child(
                    Select::new("pick", &self.state).render_item(|item, row, _, _| {
                        div()
                            .id(ElementId::NamedChild(
                                ElementId::Name("custom".into()).into(),
                                item.key(),
                            ))
                            .test_support()
                            .h(px(40.))
                            .child(if row.is_selected() {
                                format!("[{}]", item.label())
                            } else {
                                item.label().to_string()
                            })
                    }),
                )
        }
    }
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| SelectState::new(entries(), cx).with_selected(["cloud"]));
        let custom = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Custom { state: select }
        });
        Root::new(custom, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        let custom = |name: &'static str| {
            ElementId::NamedChild(ElementId::Name("custom".into()).into(), name.into())
        };
        assert!(
            window.try_find(custom("all")).is_some(),
            "the renderer drew the row"
        );
        assert_eq!(
            window.find(child("all")).bounds().size.height,
            px(52.),
            "the row grows around what the renderer drew"
        );
        window.click(child("all"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child("menu")).is_none(),
            "the row still chooses"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_menu_that_opened_upward_stays_up_while_a_search_shortens_it(cx: &mut TestAppContext) {
    let setup = setup_with(cx, false, true, ReduceMotion::On, |window, cx| {
        SelectState::new(
            (1..=30).map(|n| SelectItem::new(number(n), format!("Item {n}"))),
            cx,
        )
        .with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        let trigger = window.find(child("trigger")).bounds();
        let menu = window.find(child("menu")).bounds();
        assert!(
            menu.bottom() <= trigger.top(),
            "no room below a full menu, so it opens above: {menu:?} vs {trigger:?}"
        );
        assert!(menu.size.height > px(300.), "a full menu");
        window.input("30", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let trigger = window.find(child("trigger")).bounds();
        let menu = window.find(child("menu")).bounds();
        assert!(menu.size.height < px(120.), "one row and the field");
        assert!(
            menu.bottom() <= trigger.top(),
            "still above, though it would now fit below: {menu:?} vs {trigger:?}"
        );
        assert!(window.try_find(child("30")).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_next_opening_after_a_search_shows_every_row(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(
            (1..=30).map(|n| SelectItem::new(number(n), format!("Item {n}"))),
            cx,
        )
        .with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("30", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(child("1")).is_none());
        window.click(child("30"), cx);
        window.render_frame(cx);
        assert!(!setup.state.read(cx).is_open());
        assert!(window.try_find(child("menu")).is_none());
        assert_eq!(setup.state.read(cx).query(cx).as_ref(), "");
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["30"]);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(child("25")).is_some(),
            "the next opening shows every row, scrolled to the chosen one"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_menu_that_never_opened_does_not_show_on_the_first_frames(cx: &mut TestAppContext) {
    let setup = setup_with(cx, false, false, ReduceMotion::Off, |_, cx| {
        SelectState::new(entries(), cx)
    });
    for _ in 0..3 {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(child("menu")).is_none());
            assert_eq!(window.simulate_next_frame(cx), 0, "nothing to animate");
        })
        .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(50));
    }
}

#[gpui_kit::test]
fn choosing_from_a_searchable_menu_returns_focus_to_the_trigger(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.click(child("cloud"), cx);
        window.render_frame(cx);
        assert!(!setup.state.read(cx).is_open());
        assert_eq!(
            window.focused(cx),
            Some(setup.state.read(cx).focus_handle(cx)),
            "the search field gives focus back"
        );
        window.click("elsewhere", cx);
        window.press("tab", cx);
        assert_eq!(window.find("pick").focused(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn focus_leaving_a_searchable_menu_closes_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(setup.state.read(cx).is_open());
    })
    .unwrap();
    // The focus watch arms after the update that opened the menu, as it
    // does between two real events.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.blur(cx);
        window.render_frame(cx);
        assert!(!setup.state.read(cx).is_open(), "focus left the field");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_handlers_answer_keeps_the_chosen_row_under_the_keyboard(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx)
            .with_selected(["cloud"])
            .with_search_handler(
                "Search",
                |query, cx| {
                    let timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(50));
                    cx.background_spawn(async move {
                        timer.await;
                        entries()
                            .into_iter()
                            .filter(|entry| entry.item().is_none_or(|item| item.matches(&query)))
                            .collect()
                    })
                },
                window,
                cx,
            )
    });
    let highlighted = |setup: &Setup, cx: &mut TestAppContext| {
        setup.state.read_with(cx, |state, _| {
            state.highlighted().map(|item| item.value().to_string())
        })
    };
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(highlighted(&setup, cx), Some("cloud".into()), "on opening");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        highlighted(&setup, cx),
        Some("cloud".into()),
        "the answer to the empty query does not move it to the first row"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.input("a", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(
        highlighted(&setup, cx),
        Some("all".into()),
        "a typed query starts at its first match"
    );
}

#[gpui_kit::test]
fn a_fixed_row_height_sizes_every_row_and_lands_a_far_scroll(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    struct Fixed {
        state: Entity<State>,
    }
    impl Render for Fixed {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_4().child(
                Select::new("pick", &self.state)
                    .render_item(|item, _, _, _| div().child(item.label().clone())),
            )
        }
    }
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| {
            SelectState::new(
                (1..=5_000).map(|n| SelectItem::new(number(n), format!("Person {n}"))),
                cx,
            )
            .with_row_height(px(44.))
            .with_selected([number(4_000)])
        });
        let fixed = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Fixed { state: select }
        });
        Root::new(fixed, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        let rows = window.find(child("rows")).bounds();
        let selected = window.find(child("4000")).bounds();
        assert!(
            selected.top() >= rows.top() && selected.bottom() <= rows.bottom(),
            "the first opening scrolls the selected row into view: {selected:?} in {rows:?}"
        );
        assert_eq!(selected.size.height, px(44.));
        assert!(window.try_find(child("1")).is_none(), "virtual");
        window.press("end", cx);
        window.render_frame(cx);
        let last = window.find(child("5000")).bounds();
        let rows = window.find(child("rows")).bounds();
        assert!(
            (last.bottom() - rows.bottom()).abs() <= px(1.),
            "the last row sits at the bottom edge: {last:?} in {rows:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_value_labels_empty_and_loading_can_each_be_drawn_by_the_application(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    struct Custom {
        state: Entity<State>,
    }
    fn part(name: &'static str) -> impl IntoElement {
        div()
            .id(ElementId::NamedChild(
                ElementId::Name("part".into()).into(),
                name.into(),
            ))
            .test_support()
            .child(name)
    }
    impl Render for Custom {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_4().child(
                Select::new("pick", &self.state)
                    .render_value(|items, _, _| {
                        div()
                            .child(part("value"))
                            .child(format!("{} chosen", items.len()))
                    })
                    .render_label(|text, _, _| div().child(part("label")).child(text.clone()))
                    .render_empty(|_, _| part("empty"))
                    .render_loading(|_, _| part("loading")),
            )
        }
    }
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| {
            SelectState::new(entries(), cx).with_search_handler(
                "Search",
                |query, cx| {
                    let timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(50));
                    cx.background_spawn(async move {
                        timer.await;
                        entries()
                            .into_iter()
                            .filter(|entry| entry.item().is_none_or(|item| item.matches(&query)))
                            .collect()
                    })
                },
                window,
                cx,
            )
        });
        let custom = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Custom { state: select }
        });
        Root::new(custom, window, cx)
    });
    let part_id = |name: &'static str| {
        ElementId::NamedChild(ElementId::Name("part".into()).into(), name.into())
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(part_id("value")).is_some(), "the trigger");
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(
            window.try_find(part_id("loading")).is_some(),
            "asked on opening"
        );
        assert!(
            window.try_find(part_id("label")).is_none(),
            "the loading element stands in for the rows"
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(part_id("loading")).is_none(), "answered");
        assert!(
            window.try_find(part_id("label")).is_some(),
            "the group heading"
        );
        window.input("zzz", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(part_id("empty")).is_some(),
            "nothing matched"
        );
        assert!(window.try_find(part_id("loading")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn on_touch_a_long_press_in_the_search_box_opens_the_edit_menu(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("germ", cx);
        window.render_frame(cx);
        // The query sits at the left of the search row, after the icon.
        let search = window.find(child("search")).bounds();
        let at = gpui_kit::point(search.origin.x + px(48.), search.center().y);
        // A finger comes down before it presses long; the harness has only
        // typed so far, and a hitbox ignores the pointer after a keystroke.
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Default::default(),
            }),
            cx,
        );
        for phase in [gpui_kit::TouchPhase::Started, gpui_kit::TouchPhase::Ended] {
            window.dispatch_event(
                gpui_kit::PlatformInput::LongPress(gpui_kit::LongPressEvent {
                    phase,
                    start_position: at,
                    position: at,
                }),
                cx,
            );
            window.render_frame(cx);
        }
        assert!(window.try_find("Copy").is_some(), "the menu offers Copy");
        assert!(
            window.try_find(child("menu")).is_some(),
            "the select stays open"
        );
        window.click("Copy", cx);
        window.render_frame(cx);
    })
    .unwrap();
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("germ"));
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, cx| state.query(cx))
            .as_ref(),
        "germ"
    );
}

/// A row of the search field's right-click menu.
fn search_menu_row(name: &'static str) -> ElementId {
    ElementId::NamedChild(child("search-menu").into(), name.into())
}

#[gpui_kit::test]
fn paste_from_the_search_fields_menu_runs_the_search(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("germ".into())));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        assert!(window.try_find(child("all")).is_some());
        window.right_click(child("search"), cx);
        window.render_frame(cx);
        let search = window.find(child("search")).bounds();
        let menu = window.find(search_menu_row("menu")).bounds();
        assert_eq!(
            menu.origin,
            search.center(),
            "the menu opens at the pointer"
        );
        assert!(
            window.try_find(child("menu")).is_some(),
            "the select stays open under it"
        );
        assert!(window.try_find(search_menu_row("copy")).is_some());
        window.click(search_menu_row("paste"), cx);
        window.render_frame(cx);
        assert!(window.try_find(search_menu_row("menu")).is_none());
        assert!(window.try_find(child("menu")).is_some(), "still open");
    })
    .unwrap();
    // The field reports its change as an event, which lands after the
    // click's own update.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(setup.state.read(cx).query(cx).as_ref(), "germ");
        assert!(window.try_find(child("de")).is_some(), "a keyword matches");
        assert!(window.try_find(child("all")).is_none());
        window.input("an", cx);
    })
    .unwrap();
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, cx| state.query(cx))
            .as_ref(),
        "german",
        "focus is back in the search field"
    );
}

#[gpui_kit::test]
fn escape_closes_the_search_fields_menu_before_the_select(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(entries(), cx).with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.right_click(child("search"), cx);
        window.render_frame(cx);
        assert!(window.try_find(search_menu_row("menu")).is_some());
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(search_menu_row("menu")).is_none());
        assert!(window.try_find(child("menu")).is_some(), "the select stays");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(child("menu")).is_none());
    })
    .unwrap();
    assert!(!setup.state.read_with(cx, |state, _| state.is_open()));
}

#[gpui_kit::test]
fn a_rows_icon_lines_up_with_the_label_not_the_description(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| {
        SelectState::new(
            [
                SelectItem::new("plain", "Plain").icon(gpui_kit::assets::IconName::File),
                SelectItem::new("described", "Described")
                    .icon(gpui_kit::assets::IconName::File)
                    .description("A line under the label"),
            ],
            cx,
        )
    });
    let line = cx.update(|cx| {
        use gpui_cn::ActiveTheme as _;
        cx.theme().text_control.line_height
    });
    let part = |key: &'static str, name: &'static str| {
        ElementId::NamedChild(child(key).into(), name.into())
    };
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        let leading = window.find(part("described", "leading")).bounds();
        let label = window.find(part("described", "label")).bounds();
        let expected = label.top() + line / 2.;
        assert!((leading.center().y - expected).abs() <= px(0.5));
        let row = window.find(child("plain")).bounds();
        let leading = window.find(part("plain", "leading")).bounds();
        assert!((leading.center().y - row.center().y).abs() <= px(0.5));
    })
    .unwrap();
}

/// A searchable select whose search field's menu is replaced, or off.
struct SearchMenuHarness {
    state: Entity<State>,
    menu_enabled: bool,
}

impl Render for SearchMenuHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        div().size_full().p_4().child(
            Select::new("pick", &self.state)
                .search_context_menu_enabled(self.menu_enabled)
                .search_context_menu(move |_, _, _, _| {
                    let state = state.clone();
                    vec![
                        gpui_cn::MenuItem::new("pick-first", "Pick First")
                            .on_select(move |window, cx| {
                                state.update(cx, |state, cx| {
                                    state.set_selected(["all"], cx);
                                    state.close(window, cx);
                                });
                            })
                            .into(),
                    ]
                }),
        )
    }
}

fn setup_search_menu(cx: &mut TestAppContext, menu_enabled: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut state = None;
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| SelectState::new(entries(), cx).with_search("Search", window, cx));
        state = Some(select.clone());
        let harness = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            SearchMenuHarness {
                state: select,
                menu_enabled,
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
        state: state.unwrap(),
        events,
    }
}

#[gpui_kit::test]
fn the_search_fields_menu_can_be_replaced(cx: &mut TestAppContext) {
    let setup = setup_search_menu(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.right_click(child("search"), cx);
        window.render_frame(cx);
        assert!(window.try_find(search_menu_row("menu")).is_some());
        assert!(
            window.try_find(search_menu_row("cut")).is_none(),
            "no default rows"
        );
        window.click(search_menu_row("pick-first"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(selected(&setup, cx), ["all"]);
}

#[gpui_kit::test]
fn a_search_field_without_a_menu_ignores_the_right_click(cx: &mut TestAppContext) {
    let setup = setup_search_menu(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.right_click(child("search"), cx);
        window.render_frame(cx);
        assert!(window.try_find(search_menu_row("menu")).is_none());
        assert!(
            window.try_find(child("menu")).is_some(),
            "the select stays open"
        );
    })
    .unwrap();
    assert!(setup.state.read_with(cx, |state, _| state.is_open()));
}

#[gpui_kit::test]
fn on_touch_a_pointer_over_a_row_leaves_the_highlight(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |_, cx| SelectState::new(entries(), cx));
    let highlighted = |cx: &mut TestAppContext| {
        setup
            .state
            .read_with(cx, |state, _| state.highlighted().map(|item| *item.value()))
    };
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.hover(child("cloud"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(highlighted(cx), Some("all"), "a finger has no hover");
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = false));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(child("all"), cx);
        window.hover(child("cloud"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(highlighted(cx), Some("cloud"), "a mouse highlights the row");
}

#[gpui_kit::test]
fn a_fuzzy_search_keeps_the_rows_it_matches_and_marks_the_characters(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    type Seen = std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<std::ops::Range<usize>>)>>>;
    struct Marked {
        state: Entity<State>,
        seen: Seen,
    }
    impl Render for Marked {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let seen = self.seen.clone();
            div()
                .size_full()
                .p_4()
                .child(
                    Select::new("pick", &self.state).render_item(move |item, row, _, _| {
                        seen.borrow_mut()
                            .push((item.label().to_string(), row.matched().to_vec()));
                        div().h(px(30.)).child(item.label().clone())
                    }),
                )
        }
    }
    let seen: Seen = Default::default();
    let record = seen.clone();
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| SelectState::new(entries(), cx).with_search("Search", window, cx));
        let marked = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Marked {
                state: select,
                seen: record,
            }
        });
        Root::new(marked, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("dtsh", cx);
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(child("de")).is_some(), "a subsequence");
        assert!(window.try_find(child("all")).is_none());
        seen.borrow_mut().clear();
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        seen.borrow().last(),
        Some(&("Deutsch".to_string(), vec![0..1, 3..5, 6..7])),
        "d, t, s, h are marked in Deutsch"
    );
}

#[gpui_kit::test]
fn a_query_highlights_the_best_match_not_the_first_listed(cx: &mut TestAppContext) {
    let setup = setup(cx, false, |window, cx| {
        SelectState::new(
            [
                SelectItem::new("de", "Deutsch").keywords(["German"]),
                SelectItem::new("en", "English"),
            ],
            cx,
        )
        .with_search("Search", window, cx)
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.input("en", cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let en = window.find(child("en")).bounds();
        let de = window.find(child("de")).bounds();
        assert!(en.top() < de.top(), "English is listed first");
        assert_eq!(
            setup
                .state
                .read(cx)
                .highlighted()
                .map(|item| item.value().to_string()),
            Some("en".into()),
            "and takes the highlight"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_row_height_below_the_default_row_applies(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    struct Fixed {
        state: Entity<State>,
    }
    impl Render for Fixed {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_4().child(
                Select::new("pick", &self.state)
                    .render_item(|item, _, _, _| div().child(item.label().clone())),
            )
        }
    }
    let handle = cx.open_window(size(px(500.), px(600.)), |window, cx| {
        let select = cx.new(|cx| {
            SelectState::new(
                (1..=50).map(|n| SelectItem::new(number(n), format!("Person {n}"))),
                cx,
            )
            .with_row_height(px(20.))
        });
        let fixed = cx.new(|cx| {
            cx.observe(&select, |_, _, cx| cx.notify()).detach();
            Fixed { state: select }
        });
        Root::new(fixed, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(child("3")).bounds().size.height, px(20.));
    })
    .unwrap();
}
