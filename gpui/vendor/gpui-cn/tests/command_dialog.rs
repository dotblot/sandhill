//! UI integration tests for `CommandDialog`: a palette centered near the
//! top of the window, driven by real input in a headless window.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use gpui_cn::{
    ActiveTheme as _, CommandDialog, CommandEvent, CommandItem, CommandState, ReduceMotion, Theme,
};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, WindowHandle, base::Root, div, point, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<CommandState>,
    open: Rc<Cell<bool>>,
    before: FocusHandle,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        div()
            .id("before")
            .track_focus(&self.before)
            .size_full()
            .child(
                CommandDialog::new("cd", &self.state)
                    .open(self.open.get())
                    .on_open_change(move |value, _, _| open.set(value)),
            )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    state: Entity<CommandState>,
    open: Rc<Cell<bool>>,
    before: FocusHandle,
    events: Rc<RefCell<Vec<CommandEvent>>>,
}

fn setup(cx: &mut TestAppContext, width: f32, items: usize) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let open = Rc::new(Cell::new(false));
    let (mut state, mut before) = (None, None);
    let handle = cx.open_window(size(px(width), px(800.)), |window, cx| {
        let entity = cx.new(|cx| {
            CommandState::new("Search", window, cx).with_entries(
                (0..items)
                    .map(|n| CommandItem::new(format!("item-{n:02}"), format!("Item {n:02}"))),
            )
        });
        let recorded = events.clone();
        cx.subscribe(&entity, move |_, _, event: &CommandEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        state = Some(entity.clone());
        let harness = cx.new(|cx| {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
            let focus = cx.focus_handle();
            before = Some(focus.clone());
            Harness {
                state: entity,
                open: open.clone(),
                before: focus,
            }
        });
        Root::new(harness, window, cx)
    });
    let setup = Setup {
        handle,
        state: state.unwrap(),
        open,
        before: before.unwrap(),
        events,
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        setup.before.focus(window, cx);
    })
    .unwrap();
    settle(&setup, cx);
    setup
}

fn settle(setup: &Setup, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn open(setup: &Setup, cx: &mut TestAppContext) {
    setup.open.set(true);
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.refresh();
    })
    .unwrap();
    settle(setup, cx);
    settle(setup, cx);
}

fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    settle(setup, cx);
}

fn part(name: &str) -> ElementId {
    ElementId::NamedChild(ElementId::Name("cd".into()).into(), name.to_string().into())
}

fn popup(setup: &Setup, cx: &mut TestAppContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.find(part("popup")).bounds()
    })
    .unwrap()
}

fn within(a: gpui_kit::Pixels, b: gpui_kit::Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() <= 1.
}

#[gpui_kit::test]
fn it_sits_centered_across_and_a_token_below_the_top(cx: &mut TestAppContext) {
    let setup = setup(cx, 800., 20);
    open(&setup, cx);
    let bounds = popup(&setup, cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    assert!(within(bounds.center().x, px(400.)), "{bounds:?}");
    assert!(
        within(bounds.top(), metrics.command_dialog_top),
        "{bounds:?}"
    );
    assert!(
        within(bounds.size.width, metrics.command_dialog_width),
        "{bounds:?}"
    );
}

#[gpui_kit::test]
fn a_narrow_window_clamps_the_width_with_a_margin(cx: &mut TestAppContext) {
    let setup = setup(cx, 400., 20);
    open(&setup, cx);
    let bounds = popup(&setup, cx);
    assert!(bounds.left() >= px(15.), "{bounds:?}");
    assert!(bounds.right() <= px(385.), "{bounds:?}");
    assert!(within(bounds.center().x, px(200.)), "{bounds:?}");
}

#[gpui_kit::test]
fn the_search_field_takes_focus_and_escape_gives_it_back(cx: &mut TestAppContext) {
    let setup = setup(cx, 800., 20);
    open(&setup, cx);
    let in_search = cx.update_window(setup.handle.into(), |_, window, cx| {
        setup.state.read(cx).focus_handle(cx).is_focused(window)
    });
    assert!(in_search.unwrap(), "the search field has focus");
    press(&setup, "escape", cx);
    assert!(!setup.open.get());
    let back = cx
        .update_window(setup.handle.into(), |_, window, _| {
            setup.before.is_focused(window)
        })
        .unwrap();
    assert!(back, "focus returned");
}

#[gpui_kit::test]
fn typing_filters_and_enter_chooses_and_closes(cx: &mut TestAppContext) {
    let setup = setup(cx, 800., 20);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.input("item 07", cx)
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.matched_count()),
        1
    );
    press(&setup, "enter", cx);
    assert!(
        setup
            .events
            .borrow()
            .contains(&CommandEvent::Confirmed("item-07".into()))
    );
    assert!(!setup.open.get(), "choosing closes it");
}

#[gpui_kit::test]
fn a_press_on_the_scrim_closes(cx: &mut TestAppContext) {
    let setup = setup(cx, 800., 20);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click_at(part("backdrop"), point(px(10.), px(10.)), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert!(!setup.open.get());
}

#[gpui_kit::test]
fn the_height_does_not_change_with_the_number_of_matches(cx: &mut TestAppContext) {
    let setup = setup(cx, 800., 40);
    open(&setup, cx);
    let many = popup(&setup, cx).size.height;
    for (text, expected) in [("item 07", 1), ("zzzz", 0)] {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-a"
                } else {
                    "ctrl-a"
                },
                cx,
            );
            window.input(text, cx);
        })
        .unwrap();
        settle(&setup, cx);
        assert_eq!(
            setup.state.read_with(cx, |state, _| state.matched_count()),
            expected
        );
        assert!(within(popup(&setup, cx).size.height, many), "{text}");
    }
    assert!(many < px(800.));
}
