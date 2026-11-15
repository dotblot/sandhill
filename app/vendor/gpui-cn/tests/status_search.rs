//! UI integration tests for the search list of `StatusSelect`: a popover
//! with a ranked list, opened by a select with three options or more.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    ReduceMotion, StatusOption, StatusSelect, StatusSelectEvent, StatusSelectState, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, Window, assets::IconName, div, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    state: Entity<StatusSelectState>,
    top: f32,
    forced: Option<bool>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut select = StatusSelect::new("sel", &self.state)
            .icon(IconName::GitBranch)
            .empty("No refs found");
        if let Some(forced) = self.forced {
            select = select.searchable(forced);
        }
        div()
            .size_full()
            .pt(px(self.top))
            .pl(px(40.))
            .flex()
            .items_start()
            .child(select)
    }
}

fn refs() -> Vec<StatusOption> {
    let mut refs = vec![
        StatusOption::new("main", "main").trailing("current"),
        StatusOption::new("feat/local-tts", "feat/local-tts").trailing("worktree"),
        StatusOption::new(
            "origin/fix/psmode-current-turn-evidence-x-evidence",
            "origin/fix/psmode-current-turn-evidence-x-evidence",
        )
        .trailing("remote"),
        StatusOption::new("origin/renovate/dotenv-18.x", "origin/renovate/dotenv-18.x")
            .trailing("remote"),
        StatusOption::new("feature/composer", "feature/composer"),
    ];
    for n in 0..20 {
        refs.push(StatusOption::new(
            format!("topic-{n:02}"),
            format!("topic-{n:02}"),
        ));
    }
    refs
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<StatusSelectState>,
    events: Rc<RefCell<Vec<StatusSelectEvent>>>,
}

fn setup(
    cx: &mut TestAppContext,
    options: Vec<StatusOption>,
    top: f32,
    forced: Option<bool>,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut slot = None;
    let handle = cx.open_window(size(px(900.), px(800.)), |window, cx| {
        let state = cx.new(|cx| StatusSelectState::new(options, cx));
        let recorded = events.clone();
        cx.subscribe(&state, move |_, _, event: &StatusSelectEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        slot = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Harness { state, top, forced }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    let setup = Setup {
        handle,
        state: slot.unwrap(),
        events,
    };
    frames(&setup, cx);
    setup
}

fn frames(setup: &Setup, cx: &mut TestAppContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
    }
    cx.run_until_parked();
}

fn named(parent: ElementId, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(parent.into(), name.into())
}

fn sel() -> ElementId {
    ElementId::from("sel")
}

fn select() -> ElementId {
    named(sel(), "select")
}

fn trigger() -> ElementId {
    named(select(), "trigger")
}

/// The scope the menu's rows and search field are named under.
fn palette() -> ElementId {
    select()
}

fn menu() -> ElementId {
    named(select(), "menu")
}

fn click(setup: &Setup, target: ElementId, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(target, cx)
    })
    .unwrap();
    frames(setup, cx);
}

fn input(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.input(text, cx))
        .unwrap();
    frames(setup, cx);
}

fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    frames(setup, cx);
}

fn present(setup: &Setup, target: ElementId, cx: &mut TestAppContext) -> bool {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.try_find(target).is_some()
    })
    .unwrap()
}

fn bounds(
    setup: &Setup,
    target: ElementId,
    cx: &mut TestAppContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.find(target).bounds()
    })
    .unwrap()
}

#[gpui_kit::test]
fn two_options_open_a_plain_menu_and_twelve_open_the_search_list(cx: &mut TestAppContext) {
    let few = setup(
        cx,
        vec![
            StatusOption::new("a", "Local"),
            StatusOption::new("b", "Cloud"),
        ],
        100.,
        None,
    );
    click(&few, trigger(), cx);
    assert!(present(&few, named(palette(), "a"), cx), "a menu item");
    assert!(
        !present(&few, named(palette(), "search"), cx),
        "no search field"
    );

    let many = setup(cx, refs(), 100., None);
    click(&many, trigger(), cx);
    assert!(
        present(&many, named(palette(), "search"), cx),
        "a search field"
    );
    assert!(present(&many, menu(), cx));
}

#[gpui_kit::test]
fn typing_filters_and_ranks_and_enter_picks(cx: &mut TestAppContext) {
    let setup = setup(cx, refs(), 100., None);
    click(&setup, trigger(), cx);
    input(&setup, "cmpsr", cx);
    assert!(present(&setup, named(palette(), "feature/composer"), cx));
    assert!(
        !present(&setup, named(palette(), "main"), cx),
        "a miss is gone"
    );
    press(&setup, "enter", cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().to_string()),
        "feature/composer"
    );
    assert_eq!(
        *setup.events.borrow(),
        [StatusSelectEvent::Changed("feature/composer".into())]
    );
    assert!(!present(&setup, menu(), cx), "picking closes it");
}

#[gpui_kit::test]
fn a_better_match_ranks_above_a_worse_one(cx: &mut TestAppContext) {
    let setup = setup(cx, refs(), 100., None);
    click(&setup, trigger(), cx);
    input(&setup, "orig", cx);
    let first = bounds(&setup, named(palette(), "origin/renovate/dotenv-18.x"), cx);
    let other = bounds(
        &setup,
        named(
            palette(),
            "origin/fix/psmode-current-turn-evidence-x-evidence",
        ),
        cx,
    );
    assert!(first.top() != other.top());
}

#[gpui_kit::test]
fn escape_closes_without_a_change(cx: &mut TestAppContext) {
    let setup = setup(cx, refs(), 100., None);
    click(&setup, trigger(), cx);
    press(&setup, "escape", cx);
    assert!(!present(&setup, menu(), cx));
    assert!(setup.events.borrow().is_empty());
}

#[gpui_kit::test]
fn the_kind_label_is_right_aligned_and_a_long_name_fits_its_row(cx: &mut TestAppContext) {
    let setup = setup(cx, refs(), 100., None);
    click(&setup, trigger(), cx);
    let main = bounds(&setup, named(palette(), "main"), cx);
    let other = bounds(&setup, named(palette(), "feat/local-tts"), cx);
    assert_eq!(main.right(), other.right(), "rows share one right edge");
    let long = named(
        palette(),
        "origin/fix/psmode-current-turn-evidence-x-evidence",
    );
    let row = bounds(&setup, long, cx);
    assert!(row.right() <= bounds(&setup, named(palette(), "search"), cx).right() + px(1.));
}

#[gpui_kit::test]
fn the_panel_hangs_from_its_trigger_and_flips_near_the_bottom(cx: &mut TestAppContext) {
    let low = setup(cx, refs(), 100., None);
    click(&low, trigger(), cx);
    let (trigger_box, panel) = (bounds(&low, trigger(), cx), bounds(&low, menu(), cx));
    assert!(panel.top() >= trigger_box.bottom(), "below the trigger");
    assert!(
        (f32::from(panel.left() - trigger_box.left())).abs() < 1.5,
        "lined up with it"
    );

    let high = setup(cx, refs(), 700., None);
    click(&high, trigger(), cx);
    let (trigger_box, panel) = (bounds(&high, trigger(), cx), bounds(&high, menu(), cx));
    assert!(panel.bottom() <= trigger_box.top(), "above the trigger");
}

#[gpui_kit::test]
fn a_select_can_be_forced_either_way(cx: &mut TestAppContext) {
    let menu = setup(cx, refs(), 100., Some(false));
    click(&menu, trigger(), cx);
    assert!(!present(&menu, named(palette(), "search"), cx));
    let list = setup(
        cx,
        vec![
            StatusOption::new("a", "Local"),
            StatusOption::new("b", "Cloud"),
        ],
        100.,
        Some(true),
    );
    click(&list, trigger(), cx);
    assert!(present(&list, named(palette(), "search"), cx));
}

#[gpui_kit::test]
fn the_menu_follows_options_that_change(cx: &mut TestAppContext) {
    for searchable in [false, true] {
        let first = if searchable {
            refs()
        } else {
            vec![
                StatusOption::new("a", "Local"),
                StatusOption::new("b", "Cloud"),
            ]
        };
        let setup = setup(cx, first, 100., Some(searchable));
        click(&setup, trigger(), cx);
        press(&setup, "escape", cx);
        let events_before = setup.events.borrow().len();
        cx.update_window(setup.handle.into(), |_, _, cx| {
            setup.state.update(cx, |state, cx| {
                state.set_options(
                    [
                        StatusOption::new("x", "Hosted"),
                        StatusOption::new("y", "Remote"),
                        StatusOption::new("z", "Edge"),
                    ],
                    cx,
                )
            });
        })
        .unwrap();
        frames(&setup, cx);
        assert_eq!(
            setup.events.borrow().len(),
            events_before + 1,
            "the choice vanished, so it changed"
        );
        click(&setup, trigger(), cx);
        for id in ["x", "y", "z"] {
            assert!(
                present(&setup, named(palette(), id), cx),
                "searchable={searchable}: {id}"
            );
        }
        assert!(
            !present(&setup, named(palette(), "a"), cx),
            "the old rows are gone"
        );
        press(&setup, "escape", cx);
        let before = setup.events.borrow().len();
        cx.update_window(setup.handle.into(), |_, _, cx| {
            setup.state.update(cx, |state, cx| {
                state.set_options(
                    [
                        StatusOption::new("x", "Hosted 2"),
                        StatusOption::new("y", "Remote"),
                    ],
                    cx,
                )
            });
        })
        .unwrap();
        assert_eq!(
            setup.events.borrow().len(),
            before,
            "a kept choice reports nothing"
        );
    }
}
