//! UI integration tests for `ThemeModePicker`: real input through a
//! headless window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{ThemeMode, ThemeModePicker};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, base::Disableable as _, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    value: Rc<Cell<ThemeMode>>,
    changes: Rc<Cell<usize>>,
    disabled: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let value = self.value.clone();
        let changes = self.changes.clone();
        div().size_full().p_4().child(
            ThemeModePicker::new("mode")
                .value(self.value.get())
                .disabled(self.disabled)
                .on_change(move |mode, _, _| {
                    value.set(*mode);
                    changes.set(changes.get() + 1);
                }),
        )
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    value: Rc<Cell<ThemeMode>>,
    changes: Rc<Cell<usize>>,
}

fn option(mode: ThemeMode) -> ElementId {
    ThemeModePicker::option_id(&"mode".into(), mode)
}

fn setup(cx: &mut TestAppContext, disabled: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let value = Rc::new(Cell::new(ThemeMode::System));
    let changes = Rc::new(Cell::new(0));
    let handle = cx.open_window(size(px(900.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            value: value.clone(),
            changes: changes.clone(),
            disabled,
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        value,
        changes,
    }
}

fn tap(window: &mut Window, key: &'static str, cx: &mut gpui_kit::App) {
    let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
    window.press(key, cx);
    window.dispatch_event(
        gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }),
        cx,
    );
    window.render_frame(cx);
}

#[gpui_kit::test]
fn three_cards_in_a_row_with_the_chosen_one_selected(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("mode").is_some(),
            "the picker carries its id"
        );
        let system = window.find(option(ThemeMode::System));
        let light = window.find(option(ThemeMode::Light));
        let dark = window.find(option(ThemeMode::Dark));
        assert_eq!(system.label(), Some("System"));
        assert_eq!(light.label(), Some("Light"));
        assert_eq!(dark.label(), Some("Dark"));
        assert_eq!(system.selected(), Some(true));
        assert_eq!(light.selected(), Some(false));
        assert_eq!(dark.selected(), Some(false));
        let (a, b, c) = (system.bounds(), light.bounds(), dark.bounds());
        assert_eq!(a.origin.y, b.origin.y);
        assert_eq!(b.origin.y, c.origin.y);
        assert!(a.right() < b.origin.x && b.right() < c.origin.x);
        assert!(
            (a.size.width - b.size.width).abs() <= px(1.),
            "the cards share the width, up to layout rounding"
        );
        assert!(a.size.height >= px(44.), "a card is a touch target");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_card_keeps_the_reference_ratio_from_the_first_frame(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let card = window.find(option(ThemeMode::Light)).bounds();
        let expected_width = (px(900.) - px(16.) * 2. - px(12.) * 2.) / 3.;
        assert!(
            (card.size.width - expected_width).abs() < px(1.),
            "{card:?}"
        );
        let picture_height = card.size.width * (175. / 248.);
        assert!(card.size.height > picture_height, "{card:?}");
        assert!(card.size.height < picture_height + px(40.), "{card:?}");
        let before = window.find(option(ThemeMode::Light)).bounds();
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(option(ThemeMode::Light)).bounds(), before);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle picker asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_changes_the_value_and_the_chosen_card_is_inert(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(option(ThemeMode::System), cx);
        assert_eq!(setup.changes.get(), 0, "the chosen card does nothing");
        window.click(option(ThemeMode::Dark), cx);
    })
    .unwrap();
    assert_eq!(setup.value.get(), ThemeMode::Dark);
    assert_eq!(setup.changes.get(), 1);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(option(ThemeMode::Dark)).selected(), Some(true));
        assert_eq!(
            window.find(option(ThemeMode::System)).selected(),
            Some(false)
        );
        window.click(option(ThemeMode::Light), cx);
    })
    .unwrap();
    assert_eq!(setup.value.get(), ThemeMode::Light);
    assert_eq!(setup.changes.get(), 2);
}

#[gpui_kit::test]
fn tab_reaches_a_card_and_space_chooses_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find(option(ThemeMode::System)).focused(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find(option(ThemeMode::Light)).focused(), Some(true));
        tap(window, "space", cx);
    })
    .unwrap();
    assert_eq!(setup.value.get(), ThemeMode::Light);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        tap(window, "enter", cx);
    })
    .unwrap();
    assert_eq!(setup.value.get(), ThemeMode::Dark);
    assert_eq!(setup.changes.get(), 2);
}

#[gpui_kit::test]
fn disabled_cards_ignore_input(cx: &mut TestAppContext) {
    let setup = setup(cx, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(option(ThemeMode::Light), cx);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_ne!(window.find(option(ThemeMode::System)).focused(), Some(true));
        tap(window, "space", cx);
    })
    .unwrap();
    assert_eq!(setup.value.get(), ThemeMode::System);
    assert_eq!(setup.changes.get(), 0);
}
