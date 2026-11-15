//! UI integration tests for `Command` and `Popover`: real input through a
//! headless window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    Button, Command, CommandEvent, CommandGroup, CommandItem, CommandState, Popover, ReduceMotion,
    Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, TestAppContext, Window,
    WindowHandle, actions, base::TestSupportExt as _, div, px, size, test::TestWindowExt as _,
};

actions!(
    command_test,
    [
        /// The command a test item dispatches.
        OpenTerminal
    ]
);

struct Harness {
    command: Entity<CommandState>,
    open: Rc<RefCell<bool>>,
    focus: gpui_kit::FocusHandle,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let command = self.command.clone();
        let focus = command.focus_handle(cx);
        let open = self.open.clone();
        let log = self.log.clone();
        div()
            .track_focus(&self.focus)
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .on_action(move |_: &OpenTerminal, _, _| log.borrow_mut().push("terminal"))
            .child(
                Popover::new("add")
                    .trigger(Button::new("open").label("Add"))
                    .open(*self.open.borrow())
                    .on_open_change(move |value, _, _| *open.borrow_mut() = value)
                    .track_focus(&focus)
                    .content(move |_, _| Command::new("palette", &command).bordered(false)),
            )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    command: Entity<CommandState>,
    open: Rc<RefCell<bool>>,
    events: Rc<RefCell<Vec<CommandEvent>>>,
    log: Rc<RefCell<Vec<&'static str>>>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let open = Rc::new(RefCell::new(false));
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut command = None;
    let handle = cx.open_window(size(px(600.), px(600.)), |window, cx| {
        let focus = cx.focus_handle();
        let state = cx.new(|cx| {
            CommandState::new("Open any file", window, cx).with_entries([
                gpui_cn::CommandEntry::from(CommandItem::new("project", "Project")),
                CommandGroup::new()
                    .heading("Open")
                    .items([
                        CommandItem::new("terminal", "Terminal")
                            .keywords(["shell"])
                            .action(OpenTerminal)
                            .action_context(&focus),
                        CommandItem::new("browser", "Browser"),
                    ])
                    .into(),
            ])
        });
        let recorded = events.clone();
        let closer = open.clone();
        cx.subscribe(&state, move |_, _, event: &CommandEvent, _| {
            if matches!(
                event,
                CommandEvent::Confirmed(_) | CommandEvent::Submitted(_)
            ) {
                *closer.borrow_mut() = false;
            }
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        command = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            focus.focus(window, cx);
            Harness {
                command: state,
                open: open.clone(),
                focus,
                log: log.clone(),
            }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        command: command.unwrap(),
        open,
        events,
        log,
    }
}

fn child(name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(ElementId::Name("palette".into()).into(), name.into())
}

fn panel() -> ElementId {
    ElementId::NamedChild(ElementId::Name("add".into()).into(), "panel".into())
}

/// Opens the popover and types `text`; the field reports its change as an
/// event, which lands after the keystroke's own update.
fn open_and_type(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn the_popover_opens_under_its_trigger_with_the_search_focused(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(panel()).is_none());
        window.click("open", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let trigger = window.find("open").bounds();
        let panel = window.find(panel()).bounds();
        assert_eq!(panel.top(), trigger.bottom() + px(2.), "the menu gap");
        assert_eq!(
            panel.left(),
            trigger.left(),
            "lined up with the leading edge"
        );
        assert_eq!(window.find(child("search")).bounds().size.height, px(28.));
        assert_eq!(window.find(child("project")).bounds().size.height, px(28.));
        assert_eq!(
            setup.command.read(cx).highlighted().as_deref(),
            Some("project"),
            "the first command takes the highlight"
        );
        let focused = setup.command.focus_handle(cx).is_focused(window);
        assert!(focused, "the search field takes the keyboard");
    })
    .unwrap();
    assert!(*setup.open.borrow());
}

#[gpui_kit::test]
fn a_keyword_matches_and_enter_runs_the_action_and_closes(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open_and_type(&setup, "shell", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(child("project")).is_none());
        assert!(window.try_find(child("terminal")).is_some());
        assert_eq!(setup.command.read(cx).matched_count(), 1);
        window.press("enter", cx);
    })
    .unwrap();
    // The palette's events land once the key press's update ends.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(panel()).is_none());
        assert_eq!(setup.command.read(cx).query(cx).as_ref(), "");
    })
    .unwrap();
    assert_eq!(*setup.log.borrow(), ["terminal"]);
    assert_eq!(
        setup.events.borrow().last(),
        Some(&CommandEvent::Confirmed("terminal".into()))
    );
}

#[gpui_kit::test]
fn down_moves_past_the_heading_and_a_click_chooses(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("open", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        window.press("down", cx);
        assert_eq!(
            setup.command.read(cx).highlighted().as_deref(),
            Some("terminal")
        );
        window.press("up", cx);
        window.press("up", cx);
        assert_eq!(
            setup.command.read(cx).highlighted().as_deref(),
            Some("browser"),
            "Up wraps to the last command"
        );
        window.render_frame(cx);
        window.click(child("browser"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        setup.events.borrow().last(),
        Some(&CommandEvent::Confirmed("browser".into()))
    );
    assert!(!*setup.open.borrow());
}

#[gpui_kit::test]
fn enter_with_no_match_submits_the_query(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open_and_type(&setup, "example.com/a b", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(child("rows")).is_none(), "no rows match");
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(
        setup.events.borrow().last(),
        Some(&CommandEvent::Submitted("example.com/a b".into())),
        "a space is a character in the field"
    );
}

#[gpui_kit::test]
fn escape_clears_the_query_then_closes_and_the_next_opening_is_fresh(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open_and_type(&setup, "brow", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(
            window.try_find(panel()).is_some(),
            "the first Escape clears"
        );
        assert_eq!(setup.command.read(cx).query(cx).as_ref(), "");
        assert!(
            window.try_find(child("project")).is_some(),
            "every row is back"
        );
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(panel()).is_none(), "the second closes");
    })
    .unwrap();
    assert!(!*setup.open.borrow());
}

struct InlineHarness {
    command: Entity<CommandState>,
}

impl Render for InlineHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .child(Command::new("palette", &self.command))
    }
}

fn inline(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<CommandState>) -> CommandState + 'static,
) -> (WindowHandle<Root>, Entity<CommandState>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let mut command = None;
    let handle = cx.open_window(size(px(600.), px(600.)), |window, cx| {
        let state = cx.new(|cx| build(window, cx));
        command = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            state.focus_handle(cx).focus(window, cx);
            InlineHarness { command: state }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
    })
    .unwrap();
    (handle, command.unwrap())
}

#[gpui_kit::test]
fn a_search_handler_answers_later_and_a_newer_query_cancels_it(cx: &mut TestAppContext) {
    let asked = Rc::new(RefCell::new(Vec::<String>::new()));
    let (handle, command) = inline(cx, {
        let asked = asked.clone();
        move |window, cx| {
            CommandState::new("Open any file", window, cx).with_search_handler(
                move |query, cx| {
                    asked.borrow_mut().push(query.to_string());
                    let timer = cx
                        .background_executor()
                        .timer(std::time::Duration::from_millis(100));
                    cx.spawn(async move |_, _| {
                        timer.await;
                        vec![
                            CommandItem::new(format!("file-{query}"), format!("{query}.rs")).into(),
                        ]
                    })
                },
                cx,
            )
        }
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(child("file-")).is_some(),
            "the empty query's answer"
        );
        window.input("ma", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(command.read(cx).is_searching());
        window.input("in", cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(!command.read(cx).is_searching());
        assert!(window.try_find(child("file-main")).is_some());
        assert!(
            window.try_find(child("file-ma")).is_none(),
            "the older answer was cancelled"
        );
        assert!(window.try_find(child("file-m")).is_none());
    })
    .unwrap();
    assert_eq!(asked.borrow().first().map(String::as_str), Some(""));
    assert_eq!(asked.borrow().last().map(String::as_str), Some("main"));
}

#[gpui_kit::test]
fn a_custom_row_draws_inside_the_row_frame(cx: &mut TestAppContext) {
    let (handle, command) = inline(cx, |window, cx| {
        CommandState::new("Search", window, cx).with_entries([
            CommandItem::new("custom", "Custom").render(|row, _, _| {
                div()
                    .id(if row.is_highlighted() {
                        "custom-on"
                    } else {
                        "custom-off"
                    })
                    .test_support()
                    .h(px(40.))
                    .child("Custom")
            }),
            CommandItem::new("plain", "Plain"),
        ])
    });
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(
            window.try_find("custom-on").is_some(),
            "the renderer sees the highlight"
        );
        let row = window.find(child("custom")).bounds();
        assert_eq!(
            row.size.height,
            px(52.),
            "the 40px content inside the row's 6px padding above and below"
        );
        window.press("down", cx);
        window.render_frame(cx);
        assert!(window.try_find("custom-off").is_some());
        assert_eq!(command.read(cx).highlighted().as_deref(), Some("plain"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn enter_before_the_handler_answers_submits_the_query(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let (handle, command) = inline(cx, |window, cx| {
        CommandState::new("Open any file", window, cx).with_search_handler(
            |query, cx| {
                let timer = cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(100));
                cx.spawn(async move |_, _| {
                    timer.await;
                    vec![CommandItem::new(format!("file-{query}"), format!("{query}.rs")).into()]
                })
            },
            cx,
        )
    });
    cx.update({
        let events = events.clone();
        let command = command.clone();
        move |cx| {
            cx.subscribe(&command, move |_, event: &CommandEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        }
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(command.read(cx).highlighted().as_deref(), Some("file-"));
        window.input("main", cx);
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(command.read(cx).is_searching());
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        events.borrow().last(),
        Some(&CommandEvent::Submitted("main".into())),
        "the stale row is not chosen"
    );
}

#[gpui_kit::test]
fn set_entries_drops_an_answer_still_on_its_way(cx: &mut TestAppContext) {
    let (handle, command) = inline(cx, |window, cx| {
        CommandState::new("Open any file", window, cx).with_search_handler(
            |query, cx| {
                let timer = cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(100));
                cx.spawn(async move |_, _| {
                    timer.await;
                    vec![CommandItem::new(format!("file-{query}"), format!("{query}.rs")).into()]
                })
            },
            cx,
        )
    });
    command.update(cx, |command, cx| {
        command.set_entries([CommandItem::new("app", "From the app")], cx)
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(child("app")).is_some());
        assert!(window.try_find(child("file-")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_palette_matches_a_subsequence_of_a_label_or_a_keyword(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open_and_type(&setup, "trml", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.try_find(child("terminal")).is_some(), "label");
        assert!(window.try_find(child("project")).is_none());
        assert_eq!(setup.command.read(cx).matched_count(), 1);
    })
    .unwrap();
    let setup = self::setup(cx);
    open_and_type(&setup, "shl", cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(child("terminal")).is_some(), "keyword");
        assert!(window.try_find(child("browser")).is_none());
    })
    .unwrap();
}
