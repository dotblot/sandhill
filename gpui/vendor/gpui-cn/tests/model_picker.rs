//! UI integration tests for `ModelPicker` and `EffortMenu`: real input
//! through a headless window.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    ActiveTheme as _, EffortMenu, ModelEntry, ModelPicker, ModelPickerEvent, ModelPickerState,
    ModelProvider, ModelView, ReduceMotion, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, Window, assets::IconName, div, point,
    prelude::FluentBuilder as _, px, size, test::TestWindowExt as _,
};

const SECONDARY: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

struct Harness {
    state: Entity<ModelPickerState>,
    bottom: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .when(self.bottom, |this| this.items_end())
            .when(!self.bottom, |this| this.items_start())
            .justify_end()
            .gap_2()
            .child(ModelPicker::new("model", &self.state))
            .child(EffortMenu::new("effort", &self.state))
    }
}

fn catalog() -> Vec<ModelProvider> {
    vec![
        ModelProvider::new("acme", "Acme")
            .icon(IconName::Bot)
            .models([
                ModelEntry::new("acme-fast", "Alpha Fast"),
                ModelEntry::new("acme-deep", "Alpha Deep").effort(true),
                ModelEntry::new("acme-old", "Alpha Old").legacy(true),
                ModelEntry::new("acme-older", "Alpha Older").legacy(true),
            ]),
        ModelProvider::new("zed", "Zed Labs")
            .icon(IconName::Cpu)
            .models([
                ModelEntry::new("zed-one", "Beta One"),
                ModelEntry::new("zed-alpha", "Alpha Z"),
            ]),
        ModelProvider::new("moon", "Moon")
            .icon(IconName::Moon)
            .models([ModelEntry::new("moon-1", "Gamma")]),
    ]
}

fn big_catalog() -> Vec<ModelProvider> {
    vec![
        ModelProvider::new("acme", "Acme")
            .models((0..30).map(|n| ModelEntry::new(format!("m{n:02}"), format!("Model {n:02}")))),
        ModelProvider::new("zed", "Zed").models([ModelEntry::new("z", "Zed One")]),
    ]
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<ModelPickerState>,
    events: Rc<RefCell<Vec<ModelPickerEvent>>>,
}

fn setup(cx: &mut TestAppContext) -> Setup {
    start(cx, catalog(), |state| state)
}

fn start(
    cx: &mut TestAppContext,
    providers: Vec<ModelProvider>,
    configure: impl FnOnce(ModelPickerState) -> ModelPickerState,
) -> Setup {
    start_with_motion(cx, providers, configure, ReduceMotion::On)
}

fn start_with_motion(
    cx: &mut TestAppContext,
    providers: Vec<ModelProvider>,
    configure: impl FnOnce(ModelPickerState) -> ModelPickerState,
    motion: ReduceMotion,
) -> Setup {
    start_placed(cx, providers, configure, motion, false)
}

/// A picker whose trigger sits at the top of the window, or at the bottom.
fn start_placed(
    cx: &mut TestAppContext,
    providers: Vec<ModelProvider>,
    configure: impl FnOnce(ModelPickerState) -> ModelPickerState,
    motion: ReduceMotion,
    bottom: bool,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut slot = None;
    let handle = cx.open_window(size(px(900.), px(700.)), |window, cx| {
        let state = cx.new(|cx| configure(ModelPickerState::new(providers, window, cx)));
        let recorded = events.clone();
        cx.subscribe(&state, move |_, _, event: &ModelPickerEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        slot = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Harness { state, bottom }
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
        state: slot.unwrap(),
        events,
    }
}

fn named(parent: ElementId, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(parent.into(), name.into())
}

fn id() -> ElementId {
    ElementId::from("model")
}

fn model(name: &'static str) -> ElementId {
    named(id(), name)
}

fn star(name: &'static str) -> ElementId {
    named(named(id(), "star"), name)
}

fn provider(name: &'static str) -> ElementId {
    named(named(id(), "provider"), name)
}

fn frames(setup: &Setup, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn open(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(named(id(), "trigger"), cx);
    })
    .unwrap();
    frames(setup, cx);
    frames(setup, cx);
}

fn click(setup: &Setup, target: ElementId, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(target, cx)
    })
    .unwrap();
    frames(setup, cx);
}

fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    frames(setup, cx);
}

fn rows(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    setup.state.read_with(cx, |state, _| {
        state
            .visible()
            .into_iter()
            .map(|(_, model)| model.id().to_string())
            .collect()
    })
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
fn the_panel_is_the_measured_size_and_lists_the_first_providers_models(cx: &mut TestAppContext) {
    let setup = setup(cx);
    assert!(!present(&setup, named(id(), "body"), cx));
    open(&setup, cx);
    let panel = bounds(&setup, named(id(), "body"), cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    let border = px(2.);
    assert_eq!(panel.size.width + border, metrics.model_picker_width);
    assert_eq!(panel.size.height + border, metrics.model_picker_height);
    assert_eq!(
        rows(&setup, cx),
        ["acme-fast", "acme-deep"],
        "legacy is folded"
    );
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.has_legacy_group())
    );
    let rail = bounds(&setup, named(id(), "rail-box"), cx);
    assert_eq!(rail.size.width, metrics.model_picker_rail);
}

#[gpui_kit::test]
fn a_rail_entry_swaps_the_models(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    click(&setup, provider("zed"), cx);
    assert_eq!(rows(&setup, cx), ["zed-one", "zed-alpha"]);
    assert_eq!(
        setup.events.borrow().last(),
        Some(&ModelPickerEvent::ViewChanged(ModelView::Provider(
            "zed".into()
        )))
    );
    click(&setup, named(id(), "favorites"), cx);
    assert!(rows(&setup, cx).is_empty());
}

#[gpui_kit::test]
fn choosing_a_model_selects_it_and_closes_the_panel(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    click(&setup, model("acme-fast"), cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        Some("acme-fast".into())
    );
    assert!(
        setup
            .events
            .borrow()
            .contains(&ModelPickerEvent::Selected("acme-fast".into()))
    );
    assert!(!present(&setup, named(id(), "body"), cx));
}

#[gpui_kit::test]
fn search_filters_across_every_provider_and_dims_the_rail(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.input("alpha", cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert_eq!(
        rows(&setup, cx),
        [
            "zed-alpha",
            "acme-old",
            "acme-fast",
            "acme-deep",
            "acme-older"
        ],
        "best match first"
    );
    assert!(
        !setup
            .state
            .read_with(cx, |state, _| state.has_legacy_group()),
        "a search is flat"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.input("zzzz", cx);
    })
    .unwrap();
    frames(&setup, cx);
    assert!(
        present(&setup, named(id(), "empty"), cx),
        "an empty search says so"
    );
}

#[gpui_kit::test]
fn the_favorites_view_lists_the_starred_models_or_says_how_to_star(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| {
        state.with_favorites(["zed-alpha", "acme-deep", "nope"])
    });
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.favorites().to_vec()),
        ["zed-alpha", "acme-deep"],
        "an unknown id is ignored"
    );
    open(&setup, cx);
    click(&setup, named(id(), "favorites"), cx);
    assert_eq!(rows(&setup, cx), ["zed-alpha", "acme-deep"]);
    click(&setup, star("zed-alpha"), cx);
    click(&setup, star("acme-deep"), cx);
    assert!(rows(&setup, cx).is_empty());
    assert!(present(&setup, named(id(), "empty"), cx));
}

#[gpui_kit::test]
fn a_star_toggles_a_favorite_and_does_not_choose(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    click(&setup, star("acme-fast"), cx);
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.is_favorite("acme-fast"))
    );
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        None,
        "the star did not choose"
    );
    assert!(
        present(&setup, named(id(), "body"), cx),
        "and the panel stays open"
    );
    click(&setup, star("acme-fast"), cx);
    assert!(
        !setup
            .state
            .read_with(cx, |state, _| state.is_favorite("acme-fast"))
    );
    let favorite_events: Vec<_> = setup
        .events
        .borrow()
        .iter()
        .filter(|event| matches!(event, ModelPickerEvent::FavoriteChanged { .. }))
        .cloned()
        .collect();
    assert_eq!(
        favorite_events,
        [
            ModelPickerEvent::FavoriteChanged {
                model: "acme-fast".into(),
                favorite: true
            },
            ModelPickerEvent::FavoriteChanged {
                model: "acme-fast".into(),
                favorite: false
            },
        ]
    );
}

#[gpui_kit::test]
fn favorites_survive_a_provider_switch(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    click(&setup, star("acme-fast"), cx);
    click(&setup, provider("zed"), cx);
    click(&setup, provider("acme"), cx);
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.is_favorite("acme-fast"))
    );
}

#[gpui_kit::test]
fn the_number_shortcuts_choose_the_nth_row(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    assert!(present(
        &setup,
        named(named(id(), "shortcut"), "acme-deep"),
        cx
    ));
    press(&setup, &format!("{SECONDARY}-2"), cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        Some("acme-deep".into())
    );
    assert!(!present(&setup, named(id(), "body"), cx));
}

#[gpui_kit::test]
fn only_the_first_nine_rows_get_a_number_shortcut(cx: &mut TestAppContext) {
    let setup = start(cx, big_catalog(), |state| state);
    open(&setup, cx);
    let badge = |name: &'static str| named(named(id(), "shortcut"), name);
    assert!(
        present(&setup, badge("m00"), cx),
        "the first row has its badge"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.scroll(
            named(id(), "models"),
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
    })
    .unwrap();
    frames(&setup, cx);
    assert!(
        present(&setup, model("m09"), cx),
        "the tenth row is on screen"
    );
    assert!(
        present(&setup, badge("m08"), cx),
        "the ninth row has the last badge"
    );
    assert!(!present(&setup, badge("m09"), cx), "the tenth row has none");
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup
            .state
            .update(cx, |state, cx| state.select_number(10, window, cx));
    })
    .unwrap();
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        None,
        "there is no tenth shortcut"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup
            .state
            .update(cx, |state, cx| state.select_number(9, window, cx));
    })
    .unwrap();
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        Some("m08".into())
    );
}

fn panel_height(setup: &Setup, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    bounds(setup, named(id(), "body"), cx).size.height
}

#[gpui_kit::test]
fn the_legacy_group_is_folded_until_opened_and_the_panel_keeps_its_height(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| state.with_favorites(["zed-one"]));
    open(&setup, cx);
    let height = panel_height(&setup, cx);
    assert!(!setup.state.read_with(cx, |state, _| state.is_legacy_open()));
    assert_eq!(rows(&setup, cx), ["acme-fast", "acme-deep"]);
    click(&setup, named(id(), "legacy"), cx);
    assert_eq!(
        rows(&setup, cx),
        ["acme-old", "acme-older", "acme-fast", "acme-deep"]
    );
    assert_eq!(panel_height(&setup, cx), height, "legacy open");
    click(&setup, named(id(), "legacy"), cx);
    assert_eq!(rows(&setup, cx), ["acme-fast", "acme-deep"]);
    click(&setup, named(id(), "favorites"), cx);
    assert_eq!(panel_height(&setup, cx), height, "favorites");
    click(&setup, star("zed-one"), cx);
    assert_eq!(panel_height(&setup, cx), height, "favorites empty");
    click(&setup, provider("zed"), cx);
    assert_eq!(panel_height(&setup, cx), height, "another provider");
    for (text, expected) in [("beta one", 1usize), ("zzzz", 0)] {
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
        frames(&setup, cx);
        assert_eq!(rows(&setup, cx).len(), expected, "search {text}");
        assert_eq!(panel_height(&setup, cx), height, "search {text}");
    }
}

#[gpui_kit::test]
fn chosen_and_highlighted_rows_have_one_size_and_a_gap_between_them(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| state.with_selected("acme-fast"));
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.hover(model("acme-deep"), cx);
    })
    .unwrap();
    frames(&setup, cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("acme-deep".into())
    );
    let chosen = bounds(&setup, model("acme-fast"), cx);
    let hovered = bounds(&setup, model("acme-deep"), cx);
    assert_eq!(chosen.size, hovered.size, "the same size and radius slot");
    assert_eq!(chosen.left(), hovered.left(), "the same inset");
    let gap = f32::from(hovered.top() - chosen.bottom());
    assert!(gap >= 1., "the fills do not touch: {gap}");
}

#[gpui_kit::test]
fn the_keyboard_moves_the_highlight_wraps_and_enter_chooses(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    let highlighted = |setup: &Setup, cx: &mut TestAppContext| {
        setup.state.read_with(cx, |state, _| state.highlighted())
    };
    assert_eq!(highlighted(&setup, cx), Some("acme-fast".into()));
    press(&setup, "down", cx);
    assert_eq!(highlighted(&setup, cx), Some("acme-deep".into()));
    press(&setup, "down", cx);
    assert_eq!(
        highlighted(&setup, cx),
        None,
        "the legacy header is a row too"
    );
    press(&setup, "down", cx);
    assert_eq!(
        highlighted(&setup, cx),
        Some("acme-fast".into()),
        "wraps at the end"
    );
    press(&setup, "up", cx);
    assert_eq!(
        highlighted(&setup, cx),
        None,
        "wraps at the start, on the header"
    );
    press(&setup, "up", cx);
    assert_eq!(highlighted(&setup, cx), Some("acme-deep".into()));
    press(&setup, "down", cx);
    press(&setup, "down", cx);
    assert_eq!(highlighted(&setup, cx), Some("acme-fast".into()));
    press(&setup, "enter", cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        Some("acme-fast".into())
    );
    assert!(!present(&setup, named(id(), "body"), cx));
}

#[gpui_kit::test]
fn home_and_end_jump_and_escape_closes(cx: &mut TestAppContext) {
    let setup = start(cx, big_catalog(), |state| state);
    open(&setup, cx);
    press(&setup, "end", cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("m29".into())
    );
    press(&setup, "home", cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("m00".into())
    );
    press(&setup, "escape", cx);
    assert!(!present(&setup, named(id(), "body"), cx));
    assert!(!setup.state.read_with(cx, |state, _| state.is_open()));
}

#[gpui_kit::test]
fn space_chooses_the_highlighted_row_while_the_search_is_empty(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    press(&setup, "space", cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.selected().cloned()),
        Some("acme-fast".into())
    );
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, cx| state.query(cx).to_string()),
        ""
    );
}

#[gpui_kit::test]
fn the_highlight_scrolls_into_view(cx: &mut TestAppContext) {
    let setup = start(cx, big_catalog(), |state| state);
    open(&setup, cx);
    press(&setup, "end", cx);
    for _ in 0..4 {
        frames(&setup, cx);
    }
    let list = bounds(&setup, named(id(), "list"), cx);
    let row = bounds(&setup, model("m29"), cx);
    assert!(
        row.top() >= list.top() && row.bottom() <= list.bottom(),
        "{row:?} in {list:?}"
    );
}

#[gpui_kit::test]
fn the_highlight_follows_the_pointer_only_when_it_moves_and_not_on_touch(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    press(&setup, "down", cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("acme-deep".into()),
        "keys move it with the pointer at rest"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.hover(model("acme-fast"), cx);
    })
    .unwrap();
    frames(&setup, cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("acme-fast".into()),
        "a move puts it under the pointer"
    );
    click(&setup, model("acme-deep"), cx);

    let touch = setup_touch(cx);
    open(&touch, cx);
    cx.update_window(touch.handle.into(), |_, window, cx| {
        window.hover(model("acme-deep"), cx);
    })
    .unwrap();
    frames(&touch, cx);
    assert_eq!(
        touch.state.read_with(cx, |state, _| state.highlighted()),
        Some("acme-fast".into()),
        "on touch a hover is not a highlight"
    );
}

fn setup_touch(cx: &mut TestAppContext) -> Setup {
    let touch = setup(cx);
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    frames(&touch, cx);
    touch
}

#[gpui_kit::test]
fn typing_a_letter_in_the_panel_goes_to_the_search(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    click(&setup, provider("zed"), cx);
    cx.update_window(setup.handle.into(), |_, window, cx| window.press("b", cx))
        .unwrap();
    frames(&setup, cx);
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, cx| state.query(cx).to_string()),
        "b"
    );
}

#[gpui_kit::test]
fn the_effort_menu_shows_for_a_model_with_effort_and_reports_a_change(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| state.with_selected("acme-fast"));
    frames(&setup, cx);
    assert!(
        !present(&setup, named(id_effort(), "trigger"), cx),
        "no effort on this model"
    );
    setup
        .state
        .update(cx, |state, cx| state.select("acme-deep", cx));
    frames(&setup, cx);
    assert!(present(&setup, named(id_effort(), "trigger"), cx));
    click(&setup, named(id_effort(), "trigger"), cx);
    frames(&setup, cx);
    click(&setup, named(id_effort(), "3"), cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.effort()),
        Some(3)
    );
    assert!(
        setup
            .events
            .borrow()
            .contains(&ModelPickerEvent::EffortChanged(3))
    );
}

fn id_effort() -> ElementId {
    ElementId::from("effort")
}

#[gpui_kit::test]
fn the_trigger_shows_the_open_state(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let closed = bounds(&setup, named(id(), "trigger"), cx);
    open(&setup, cx);
    let opened = bounds(&setup, named(id(), "trigger"), cx);
    assert_eq!(
        closed.size, opened.size,
        "the pill keeps its size while open"
    );
}

#[gpui_kit::test]
fn the_list_pane_insets_its_rows_equally_and_keeps_a_gap_between_them(cx: &mut TestAppContext) {
    let setup = start(cx, big_catalog(), |state| state);
    open(&setup, cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    let inset = f32::from(px(4.).max(metrics.model_row * 0. + px(4.)));
    let list = bounds(&setup, named(id(), "list"), cx);
    let first = bounds(&setup, model("m00"), cx);
    let second = bounds(&setup, model("m01"), cx);
    let left = f32::from(first.left() - list.left());
    let right = f32::from(list.right() - first.right());
    assert!((left - inset).abs() <= 0.5, "left {left}");
    assert!((right - inset).abs() <= 0.5, "right {right}");
    let gap = f32::from(second.top() - first.bottom());
    assert!((gap - 4.).abs() <= 0.5, "row gap {gap}");
    let underline = bounds(&setup, named(id(), "underline"), cx);
    let step = f32::from(first.top() - underline.bottom());
    assert!(step >= 0., "the list starts under the divider: {step}");
}

#[gpui_kit::test]
fn the_rail_has_one_active_entry_with_names_and_symmetric_insets(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    let rail = bounds(&setup, named(id(), "rail-box"), cx);
    for entry in [
        named(id(), "favorites"),
        provider("acme"),
        provider("zed"),
        provider("moon"),
    ] {
        let tile = bounds(&setup, entry, cx);
        let left = f32::from(tile.left() - rail.left());
        let right = f32::from(rail.right() - tile.right());
        assert!((left - right).abs() <= 1.5, "tile insets {left} {right}");
    }
    let order: Vec<f32> = [
        named(id(), "favorites"),
        provider("acme"),
        provider("zed"),
        provider("moon"),
    ]
    .into_iter()
    .map(|entry| f32::from(bounds(&setup, entry, cx).top()))
    .collect();
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "Favorites, then providers in order"
    );
    cx.update_window(setup.handle.into(), |_, window, _| {
        let labels: Vec<_> = ["acme", "zed", "moon"]
            .iter()
            .map(|name| window.find(provider(name)).label().map(str::to_string))
            .collect();
        assert_eq!(
            labels,
            [
                Some("Acme".into()),
                Some("Zed Labs".into()),
                Some("Moon".into())
            ]
        );
        assert_eq!(
            window.find(named(id(), "favorites")).label(),
            Some("Favorites")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_legacy_header_has_the_row_inset(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    let header = bounds(&setup, named(id(), "legacy"), cx);
    let row = bounds(&setup, model("acme-fast"), cx);
    assert_eq!(header.left(), row.left());
    assert_eq!(header.right(), row.right());
    assert_eq!(header.size.height, row.size.height);
}

#[gpui_kit::test]
fn the_panel_hangs_from_its_trigger_and_flips_when_it_has_no_room(cx: &mut TestAppContext) {
    let high = setup(cx);
    open(&high, cx);
    let trigger_box = bounds(&high, named(id(), "trigger"), cx);
    let panel = bounds(&high, named(id(), "body"), cx);
    assert!(
        panel.top() >= trigger_box.bottom(),
        "a trigger near the top hangs the panel below it: {trigger_box:?} {panel:?}"
    );
    let low = start_placed(cx, catalog(), |state| state, ReduceMotion::On, true);
    open(&low, cx);
    let trigger_box = bounds(&low, named(id(), "trigger"), cx);
    let panel = bounds(&low, named(id(), "body"), cx);
    assert!(
        panel.bottom() <= trigger_box.top(),
        "a trigger near the bottom flips the panel above it: {trigger_box:?} {panel:?}"
    );
}

fn legacy_span(setup: &Setup, cx: &mut TestAppContext) -> f32 {
    let header = bounds(setup, named(id(), "legacy"), cx);
    let next = bounds(setup, model("acme-fast"), cx);
    f32::from(next.origin.y - header.origin.y)
}

fn tick(setup: &Setup, ms: u64, cx: &mut TestAppContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(ms));
    frames(setup, cx);
}

#[gpui_kit::test]
fn the_legacy_group_folds_open_and_shut_with_the_fold_transition(cx: &mut TestAppContext) {
    let setup = start_with_motion(cx, catalog(), |state| state, ReduceMotion::Off);
    open(&setup, cx);
    let height = panel_height(&setup, cx);
    let header = named(id(), "legacy");
    let expanded =
        |cx: &mut TestAppContext| setup.state.read_with(cx, |state, _| state.is_legacy_open());
    click(&setup, header.clone(), cx);
    assert!(expanded(cx));
    let rest = legacy_span(&setup, cx);
    let mut last = rest;
    let mut seen_partial = false;
    for _ in 0..40 {
        tick(&setup, 20, cx);
        let now = legacy_span(&setup, cx);
        assert!(now >= last - 0.5, "monotonic open: {last} then {now}");
        seen_partial |= now > rest + 1. && now < rest + 90.;
        last = now;
        assert_eq!(panel_height(&setup, cx), height, "panel height holds");
    }
    assert!(seen_partial, "the body passed through partial heights");
    assert!(last > rest + 80., "open at rest, got {last}");
    let open_span = last;
    click(&setup, header, cx);
    assert!(!expanded(cx));
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.has_legacy_group())
    );
    let mut last = open_span;
    for _ in 0..40 {
        tick(&setup, 20, cx);
        let now = legacy_span(&setup, cx);
        assert!(now <= last + 0.5, "monotonic close: {last} then {now}");
        last = now;
    }
    assert!((last - rest).abs() < 1., "closed at rest, got {last}");
    assert!(!present(&setup, model("acme-old"), cx), "rows left");
}

#[gpui_kit::test]
fn reduced_motion_jumps_the_legacy_group(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| state);
    open(&setup, cx);
    let rest = legacy_span(&setup, cx);
    click(&setup, named(id(), "legacy"), cx);
    assert!(legacy_span(&setup, cx) > rest + 80.);
    click(&setup, named(id(), "legacy"), cx);
    frames(&setup, cx);
    assert!(!present(&setup, model("acme-old"), cx));
}

#[gpui_kit::test]
fn the_legacy_header_reports_expanded_and_the_group_closes_on_reopen(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| state);
    open(&setup, cx);
    click(&setup, named(id(), "legacy"), cx);
    assert_eq!(
        setup.events.borrow().last(),
        Some(&ModelPickerEvent::LegacyToggled(true))
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("left", cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert!(!setup.state.read_with(cx, |state, _| state.is_legacy_open()));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("right", cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert!(setup.state.read_with(cx, |state, _| state.is_legacy_open()));
    cx.update(|cx| {
        setup
            .state
            .update(cx, |state, cx| state.set_legacy_open(false, cx))
    });
    click(&setup, named(id(), "legacy"), cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup
            .state
            .update(cx, |state, cx| state.set_open(false, window, cx));
    })
    .unwrap();
    open(&setup, cx);
    assert!(!setup.state.read_with(cx, |state, _| state.is_legacy_open()));
    assert_eq!(rows(&setup, cx), ["acme-fast", "acme-deep"]);
}

fn tooltip_visible(setup: &Setup, cx: &mut TestAppContext) -> bool {
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(700));
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find("gpui-cn-tooltip")
            .is_some_and(|tooltip| tooltip.visible())
    })
    .unwrap()
}

fn hover(setup: &Setup, target: ElementId, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.hover(target, cx)
    })
    .unwrap();
}

#[gpui_kit::test]
fn each_rail_entry_is_named_and_shows_its_name_as_a_tooltip(cx: &mut TestAppContext) {
    let setup = setup(cx);
    open(&setup, cx);
    let entries = [
        (named(id(), "favorites"), "Favorites"),
        (provider("acme"), "Acme"),
        (provider("zed"), "Zed Labs"),
    ];
    for (entry, name) in entries {
        let label = cx
            .update_window(setup.handle.into(), |_, window, _| {
                window.find(entry.clone()).label().map(str::to_string)
            })
            .unwrap();
        assert_eq!(label.as_deref(), Some(name), "the accessible name");
        hover(&setup, entry, cx);
        assert!(tooltip_visible(&setup, cx), "{name} shows a tooltip");
        hover(&setup, model("acme-fast"), cx);
        assert!(!tooltip_visible(&setup, cx), "leaving {name} hides it");
    }
}

fn visible_model(
    setup: &Setup,
    prefix: &str,
    cx: &mut TestAppContext,
) -> Option<(String, gpui_kit::Pixels)> {
    let top = bounds(setup, named(id(), "list"), cx).top();
    (0..60).find_map(|n| {
        let name = format!("{prefix}{n:02}");
        let target = model(Box::leak(name.clone().into_boxed_str()));
        present(setup, target.clone(), cx)
            .then(|| (name, bounds(setup, target, cx).top()))
            .filter(|(_, y)| *y >= top)
    })
}

#[gpui_kit::test]
fn starring_a_model_keeps_the_scroll_place_and_the_highlight(cx: &mut TestAppContext) {
    let setup = start(cx, big_catalog(), |state| state);
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.scroll(
            named(id(), "models"),
            gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
    })
    .unwrap();
    frames(&setup, cx);
    let (name, top) = visible_model(&setup, "m", cx).expect("a row is on screen");
    assert_ne!(name, "m00", "the list scrolled");
    let target: &'static str = Box::leak(name.clone().into_boxed_str());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.hover(model(target), cx)
    })
    .unwrap();
    frames(&setup, cx);
    let highlighted = setup.state.read_with(cx, |state, _| state.highlighted());
    assert_eq!(highlighted.as_deref(), Some(target));
    click(&setup, star(target), cx);
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.is_favorite(&name)),
        "starred"
    );
    assert_eq!(
        bounds(&setup, model(target), cx).top(),
        top,
        "the row did not move"
    );
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        highlighted,
        "the highlight stayed"
    );
}

fn legacy_catalog() -> Vec<ModelProvider> {
    vec![
        ModelProvider::new("acme", "Acme").models(
            (0..12)
                .map(|n| ModelEntry::new(format!("old{n:02}"), format!("Old {n:02}")).legacy(true))
                .chain((0..5).map(|n| ModelEntry::new(format!("new{n}"), format!("New {n}")))),
        ),
    ]
}

#[gpui_kit::test]
fn opening_the_legacy_group_keeps_the_header_in_place_and_highlighted(cx: &mut TestAppContext) {
    let setup = start(cx, legacy_catalog(), |state| state);
    open(&setup, cx);
    let before = bounds(&setup, named(id(), "legacy"), cx).top();
    click(&setup, named(id(), "legacy"), cx);
    assert!(setup.state.read_with(cx, |state, _| state.is_legacy_open()));
    assert_eq!(
        bounds(&setup, named(id(), "legacy"), cx).top(),
        before,
        "the header stays"
    );
    press(&setup, "enter", cx);
    assert!(
        !setup.state.read_with(cx, |state, _| state.is_legacy_open()),
        "the header was still highlighted, so Enter closed the group"
    );
}

#[gpui_kit::test]
fn number_shortcuts_skip_rows_that_are_folding_away(cx: &mut TestAppContext) {
    let setup = start_with_motion(cx, catalog(), |state| state, ReduceMotion::Off);
    open(&setup, cx);
    click(&setup, named(id(), "legacy"), cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    frames(&setup, cx);
    click(&setup, named(id(), "legacy"), cx);
    assert!(
        present(&setup, model("acme-old"), cx),
        "the rows are still folding"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(&format!("{SECONDARY}-1"), cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert!(
        setup
            .events
            .borrow()
            .contains(&ModelPickerEvent::Selected("acme-fast".into())),
        "Cmd+1 chose the first row that is not folding away"
    );
}

fn chose(setup: &Setup) -> Vec<String> {
    setup
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ModelPickerEvent::Selected(id) => Some(id.to_string()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn number_shortcuts_count_the_rows_of_a_search_and_of_the_favorites(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| {
        state.with_favorites(["zed-alpha", "acme-deep"])
    });
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.input("alpha", cx)
    })
    .unwrap();
    frames(&setup, cx);
    let second = rows(&setup, cx)[1].clone();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(&format!("{SECONDARY}-2"), cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert_eq!(
        chose(&setup),
        [second],
        "Cmd+2 chose the second row of the search"
    );

    let setup = start(cx, catalog(), |state| {
        state.with_favorites(["zed-alpha", "acme-deep"])
    });
    open(&setup, cx);
    click(&setup, named(id(), "favorites"), cx);
    assert_eq!(rows(&setup, cx), ["zed-alpha", "acme-deep"]);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(&format!("{SECONDARY}-2"), cx)
    })
    .unwrap();
    frames(&setup, cx);
    assert_eq!(
        chose(&setup),
        ["acme-deep"],
        "Cmd+2 chose the second favorite"
    );
}

#[gpui_kit::test]
fn the_effort_levels_can_be_the_applications_own(cx: &mut TestAppContext) {
    let setup = start(cx, catalog(), |state| {
        state.with_effort_levels(["Fast", "Balanced", "Deep"])
    });
    cx.update_window(setup.handle.into(), |_, _, cx| {
        setup.state.update(cx, |state, cx| {
            state.select("acme-deep", cx);
            state.set_effort(9, cx);
        });
    })
    .unwrap();
    let (effort, label, levels) = setup.state.read_with(cx, |state, _| {
        (
            state.effort(),
            state.effort_label(),
            state.effort_levels().to_vec(),
        )
    });
    assert_eq!(levels.len(), 3);
    assert_eq!(effort, Some(2), "clamped to the last of three levels");
    assert_eq!(label.as_deref(), Some("Deep"));
    assert!(
        setup
            .events
            .borrow()
            .contains(&ModelPickerEvent::EffortChanged(2))
    );
    let default = start(cx, catalog(), |state| state);
    assert_eq!(
        default.state.read_with(cx, |s, _| s.effort_levels().len()),
        6
    );
}
