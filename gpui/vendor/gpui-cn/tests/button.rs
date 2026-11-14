//! UI integration tests for `Button`: real input through a headless window.

use std::{cell::Cell, rc::Rc};

use gpui_cn::{Button, ButtonSize, ButtonVariant, Icon, Theme, ThemeMode};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window,
    base::{Disableable as _, Selectable as _},
    div,
    prelude::FluentBuilder as _,
    px, size,
    test::TestWindowExt as _,
};

const DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8"/></svg>"#;

/// A view with one configurable button and a counter it drives.
struct Harness {
    focus: gpui_kit::FocusHandle,
    clicks: Rc<Cell<usize>>,
    keyboard_clicks: Rc<Cell<usize>>,
    disabled: bool,
    loading: bool,
    icon_only: bool,
    variant: ButtonVariant,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        let keyboard_clicks = self.keyboard_clicks.clone();
        let mut button = Button::new("save")
            .track_focus(&self.focus)
            .variant(self.variant)
            .disabled(self.disabled)
            .loading(self.loading)
            .on_click(move |event, _, _| {
                clicks.set(clicks.get() + 1);
                if matches!(event, gpui_kit::ClickEvent::Keyboard(_)) {
                    keyboard_clicks.set(keyboard_clicks.get() + 1);
                }
            });
        button = if self.icon_only {
            button
                .icon(Icon::from_bytes(DOT))
                .accessibility_label("Save")
                .tooltip("Save")
        } else {
            button.label("Save")
        };
        div()
            .size_full()
            .p_4()
            .child(button)
            .child(Button::new("other").outline().label("Other"))
    }
}

struct Setup {
    view: gpui_kit::Entity<Harness>,
    handle: gpui_kit::WindowHandle<Root>,
    clicks: Rc<Cell<usize>>,
    keyboard_clicks: Rc<Cell<usize>>,
}

/// Presses and releases a key on the focused element. `press` sends only
/// the key down; keyboard activation needs the matching key up.
fn tap(window: &mut Window, key: &'static str, cx: &mut gpui_kit::App) {
    let keystroke = gpui_kit::Keystroke::parse(key).unwrap();
    window.press(key, cx);
    window.dispatch_event(
        gpui_kit::PlatformInput::KeyUp(gpui_kit::KeyUpEvent { keystroke }),
        cx,
    );
    window.render_frame(cx);
}

fn focus_save(setup: &Setup, cx: &mut TestAppContext) {
    let focus = setup.view.read_with(cx, |view, _| view.focus.clone());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.focus(&focus, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn setup(cx: &mut TestAppContext, configure: impl FnOnce(&mut Harness)) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let clicks = Rc::new(Cell::new(0));
    let keyboard_clicks = Rc::new(Cell::new(0));
    let mut view = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|cx| {
            let mut harness = Harness {
                focus: cx.focus_handle(),
                clicks: clicks.clone(),
                keyboard_clicks: keyboard_clicks.clone(),
                disabled: false,
                loading: false,
                icon_only: false,
                variant: ButtonVariant::Default,
            };
            configure(&mut harness);
            harness
        });
        view = Some(harness.clone());
        Root::new(harness, window, cx)
    });
    Setup {
        view: view.unwrap(),
        handle,
        clicks,
        keyboard_clicks,
    }
}

#[gpui_kit::test]
fn click_fires_once_and_label_is_the_accessible_name(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let save = window.find("save");
        assert_eq!(save.label(), Some("Save"));
        assert!(save.bounds().size.height > px(0.));
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 1);
    assert_eq!(setup.keyboard_clicks.get(), 0);
}

#[gpui_kit::test]
fn enter_and_space_activate_the_focused_button(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // A pointer press does not move focus onto a button, as on macOS.
        window.click("save", cx);
        assert_eq!(window.find("save").focused(), Some(false));
    })
    .unwrap();
    focus_save(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find("save").focused(), Some(true));
        tap(window, "enter", cx);
        tap(window, "space", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 3);
    assert_eq!(setup.keyboard_clicks.get(), 2);
}

#[gpui_kit::test]
fn tab_moves_focus_between_buttons(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    focus_save(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find("save").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("other").focused(), Some(true));
        assert_eq!(window.find("save").focused(), Some(false));
        window.press("shift-tab", cx);
        assert_eq!(window.find("save").focused(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn disabled_button_ignores_pointer_and_keyboard(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| harness.disabled = true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save", cx);
    })
    .unwrap();
    focus_save(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        // Base keeps focus that a disabled button already holds, so Tab can
        // leave it, but Enter and Space do not activate it.
        tap(window, "enter", cx);
        tap(window, "space", cx);
        window.press("tab", cx);
        assert_eq!(window.find("other").focused(), Some(true));
        // It is not a Tab stop, so Shift-Tab does not come back to it.
        window.press("shift-tab", cx);
        assert_ne!(window.find("save").focused(), Some(true));
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 0);
}

#[gpui_kit::test]
fn a_loading_button_keeps_focus_and_ignores_activation(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| harness.loading = true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(gpui_kit::ElementId::NamedChild(
                    gpui_kit::ElementId::from("save").into(),
                    "spinner".into()
                ))
                .role(),
            Some(gpui_kit::Role::Status)
        );
        window.click("save", cx);
    })
    .unwrap();
    focus_save(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find("save").focused(), Some(true));
        tap(window, "enter", cx);
        tap(window, "space", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 0);

    setup.view.update(cx, |view, cx| {
        view.loading = false;
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save", cx);
        tap(window, "enter", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 2);
}

#[gpui_kit::test]
fn a_loading_icon_only_button_stays_square(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| {
        harness.icon_only = true;
        harness.loading = true;
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("save").bounds();
        assert_eq!(bounds.size.width, px(28.));
        assert_eq!(bounds.size.height, px(28.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn icon_only_button_exposes_its_accessibility_label(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| harness.icon_only = true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let save = window.find("save");
        assert_eq!(save.label(), Some("Save"));
        let bounds = save.bounds();
        assert_eq!(
            bounds.size.width, bounds.size.height,
            "icon-only buttons are square"
        );
        assert_eq!(bounds.size.width, px(28.));
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 1);
}

#[gpui_kit::test]
fn sizes_follow_the_reference_heights_and_scale_with_the_font_size(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("save").bounds().size.height, px(28.));
        assert_eq!(window.find("other").bounds().size.height, px(28.));
    })
    .unwrap();
    cx.update(|cx| Theme::set_ui_font_size(cx, px(20.)));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("save").bounds().size.height, px(35.));
    })
    .unwrap();
}

/// One button per size tier, plus one with a style override.
struct Sizes;

impl Render for Sizes {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(Button::new("xs").size(ButtonSize::Xs).label("Save"))
            .child(Button::new("sm").size(ButtonSize::Sm).label("Save"))
            .child(Button::new("default").label("Save"))
            .child(Button::new("lg").size(ButtonSize::Lg).label("Save"))
            .child(Button::new("tall").h(px(40.)).w(px(200.)).label("Save"))
    }
}

#[gpui_kit::test]
fn size_tiers_match_the_reference_and_styled_overrides_win(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(600.), px(200.)), |window, cx| {
        let view = cx.new(|_| Sizes);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for (id, height) in [("xs", 20.), ("sm", 24.), ("default", 28.), ("lg", 32.)] {
            assert_eq!(window.find(id).bounds().size.height, px(height), "{id}");
        }
        let tall = window.find("tall").bounds().size;
        assert_eq!(tall.height, px(40.), "h() overrides the tier height");
        assert_eq!(tall.width, px(200.), "w() overrides the content width");
    })
    .unwrap();
}

#[gpui_kit::test]
fn tab_reaches_the_first_button_when_nothing_is_focused(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("save").focused(), Some(false));
        window.press("tab", cx);
        assert_eq!(window.find("save").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("other").focused(), Some(true));
        window.press("tab", cx);
        assert_eq!(window.find("save").focused(), Some(true), "wraps around");
    })
    .unwrap();
}

/// Two buttons with id-keyed focus handles; the first can be removed.
struct Removable {
    show_first: bool,
}

impl Render for Removable {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .when(self.show_first, |this| {
                this.child(Button::new("first").label("First"))
            })
            .child(Button::new("second").label("Second"))
    }
}

#[gpui_kit::test]
fn tab_recovers_after_the_focused_button_disappears(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let mut view = None;
    let handle = cx.open_window(size(px(400.), px(200.)), |window, cx| {
        let removable = cx.new(|_| Removable { show_first: true });
        view = Some(removable.clone());
        Root::new(removable, window, cx)
    });
    let view = view.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("tab", cx);
        assert_eq!(window.find("first").focused(), Some(true));
    })
    .unwrap();
    view.update(cx, |view, cx| {
        view.show_first = false;
        cx.notify();
    });
    // The removed button's focus handle goes with its element state. GPUI
    // Fast releases it when effects flush, as an app does after each event,
    // so the window has no focus and Root takes it back on the next frame.
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("tab", cx);
        assert_eq!(window.find("second").focused(), Some(true));
    })
    .unwrap();
}

/// One button per variant, all at the default size.
struct Variants;

impl Render for Variants {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(Button::new("default").label("Save"))
            .child(Button::new("primary").primary().label("Save"))
            .child(Button::new("destructive").destructive().label("Save"))
            .child(Button::new("outline").outline().label("Save"))
            .child(Button::new("ghost").ghost().label("Save"))
            .child(Button::new("link").link().label("Save"))
    }
}

#[gpui_kit::test]
fn every_variant_shares_one_height_and_one_width(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(600.), px(200.)), |window, cx| {
        let view = cx.new(|_| Variants);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let reference = window.find("default").bounds().size;
        for id in ["primary", "destructive", "outline", "ghost", "link"] {
            let bounds = window.find(id).bounds().size;
            assert_eq!(bounds.height, reference.height, "{id} height");
            assert_eq!(bounds.width, reference.width, "{id} width");
        }
        assert_eq!(reference.height, px(28.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn link_variant_has_the_link_role(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| harness.variant = ButtonVariant::Link);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("save").role(), Some(gpui_kit::Role::Link));
        assert_eq!(window.find("other").role(), Some(gpui_kit::Role::Button));
    })
    .unwrap();
}

#[gpui_kit::test]
fn theme_changes_redraw_the_button(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("save").visible());
    })
    .unwrap();
    cx.update(|cx| Theme::change(ThemeMode::Dark, cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("save").visible());
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 1);
    drop(setup.view);
}

/// A wide and a narrow button with the same long label.
struct Truncated;

impl Render for Truncated {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Button::new("disabled-with-reason")
                    .disabled(true)
                    .tooltip("Select a file first")
                    .label("Upload"),
            )
            .child(Button::new("wide").w(px(360.)).label("A long label"))
            .child(Button::new("narrow").w(px(60.)).label("A long label"))
            .child(
                Button::new("opted-out")
                    .w(px(60.))
                    .truncation_tooltip(false)
                    .label("A long label"),
            )
    }
}

#[gpui_kit::test]
fn a_truncated_label_shows_itself_as_a_tooltip(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(200.)), |window, cx| {
        let view = cx.new(|_| Truncated);
        Root::new(view, window, cx)
    });
    let hover_and_settle = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.hover(id, cx);
        })
        .unwrap();
        // The overlay's show delay is a timer created when its task first
        // polls, so park once before advancing the clock past it.
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find("gpui-cn-tooltip")
                .is_some_and(|tooltip| tooltip.visible())
        })
        .unwrap()
    };
    assert!(
        !hover_and_settle(cx, "wide"),
        "a label that fits has no tooltip"
    );
    assert!(
        hover_and_settle(cx, "narrow"),
        "a truncated label shows one"
    );
    assert!(
        !hover_and_settle(cx, "opted-out"),
        "unless the button opted out"
    );
}

/// Buttons whose accessibility state is set by the caller.
struct States;

impl Render for States {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(Button::new("plain").label("Plain"))
            .child(Button::new("selected").selected(true).label("Selected"))
            .child(Button::new("open").open(true).label("Open"))
            .child(Button::new("toggled").toggled(true).label("Bold"))
            .child(Button::new("untoggled").toggled(false).label("Bold"))
            .child(Button::new("off").disabled(true).label("Off"))
    }
}

#[gpui_kit::test]
fn toggled_reaches_assistive_technology_and_other_states_render(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(600.), px(200.)), |window, cx| {
        let view = cx.new(|_| States);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // `selected` and `open` are visual in base and set no accessibility
        // flag; `toggled` is the channel assistive technology reads.
        assert!(window.find("selected").visible());
        assert!(window.find("open").visible());
        assert_eq!(window.find("plain").checked(), None, "not a toggle");
        assert_eq!(window.find("toggled").checked(), Some(true));
        assert_eq!(window.find("untoggled").checked(), Some(false));
        // Base does not expose a disabled flag in snapshots; disabled is
        // proven by behavior in `disabled_button_ignores_pointer_and_keyboard`.
        assert!(window.find("off").visible());
    })
    .unwrap();
}

/// Two buttons that report hover to the test.
struct Hovers {
    entered: Rc<Cell<usize>>,
    left: Rc<Cell<usize>>,
}

impl Render for Hovers {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let entered = self.entered.clone();
        let left = self.left.clone();
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(
                Button::new("first")
                    .label("First")
                    .on_hover(move |hovered, _, _| {
                        if *hovered {
                            entered.set(entered.get() + 1);
                        } else {
                            left.set(left.get() + 1);
                        }
                    }),
            )
            .child(Button::new("second").label("Second"))
    }
}

#[gpui_kit::test]
fn on_hover_reports_enter_and_leave(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let entered = Rc::new(Cell::new(0));
    let left = Rc::new(Cell::new(0));
    let (entered_for_view, left_for_view) = (entered.clone(), left.clone());
    let handle = cx.open_window(size(px(400.), px(200.)), |window, cx| {
        let view = cx.new(|_| Hovers {
            entered: entered_for_view,
            left: left_for_view,
        });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover("first", cx);
        assert_eq!((entered.get(), left.get()), (1, 0));
        window.hover("second", cx);
        assert_eq!((entered.get(), left.get()), (1, 1));
        window.hover("first", cx);
        assert_eq!((entered.get(), left.get()), (2, 1));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_press_dragged_off_the_button_does_not_click(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.drag_to("save", "other", cx);
        window.render_frame(cx);
        // The button is still interactive afterwards.
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 1, "only the real click counts");
}

#[gpui_kit::test]
fn a_button_disabled_while_hovered_stays_inert_and_recovers(cx: &mut TestAppContext) {
    let setup = setup(cx, |_| {});
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover("save", cx);
    })
    .unwrap();
    setup.view.update(cx, |view, cx| {
        view.disabled = true;
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 0);
    setup.view.update(cx, |view, cx| {
        view.disabled = false;
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(setup.clicks.get(), 1);
}

#[gpui_kit::test]
fn an_explicit_tooltip_shows_and_hides_with_the_pointer(cx: &mut TestAppContext) {
    let setup = setup(cx, |harness| harness.icon_only = true);
    let handle = setup.handle;
    let tooltip_visible = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find("gpui-cn-tooltip")
                .is_some_and(|tooltip| tooltip.visible())
        })
        .unwrap()
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover("save", cx);
    })
    .unwrap();
    assert!(tooltip_visible(cx), "shown after the delay");
    cx.update_window(handle.into(), |_, window, cx| {
        // "other" has no tooltip, so moving there hides it.
        window.hover("other", cx);
    })
    .unwrap();
    assert!(!tooltip_visible(cx), "hidden after leaving");
    cx.update_window(handle.into(), |_, window, cx| {
        window.hover("save", cx);
        window.click("save", cx);
    })
    .unwrap();
    assert!(
        !tooltip_visible(cx),
        "a press hides it and does not reschedule"
    );
}

/// A button in a plain window with no `gpui_kit::base::Root`.
struct Bare {
    clicks: Rc<Cell<usize>>,
}

impl Render for Bare {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div().size_full().p_4().child(
            Button::new("save")
                .label("Save")
                .tooltip("Save the file")
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
        )
    }
}

#[gpui_kit::test]
fn works_without_root(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let clicks = Rc::new(Cell::new(0));
    let clicks_for_view = clicks.clone();
    let handle = cx.open_window(size(px(400.), px(200.)), |_, _| Bare {
        clicks: clicks_for_view,
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            gpui_cn::TooltipHost::overlay(window, cx).is_none(),
            "no root, no overlay"
        );
        let save = window.find("save");
        assert_eq!(save.label(), Some("Save"));
        assert_eq!(save.bounds().size.height, px(28.));
        window.click("save", cx);
        // Without an overlay the tooltip is GPUI's native one; hovering
        // must not panic and the button stays interactive.
        window.hover("save", cx);
        window.click("save", cx);
    })
    .unwrap();
    assert_eq!(clicks.get(), 2);
}

/// The Link variant with its own on_click, next to a plain button.
struct Links;

impl Render for Links {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p_4()
            .flex()
            .gap_2()
            .child(Button::new("docs").link().label("Read the docs"))
            .child(
                Button::new("icon-link")
                    .link()
                    .icon(Icon::from_bytes(DOT))
                    .accessibility_label("Open"),
            )
    }
}

#[gpui_kit::test]
fn link_variant_exposes_its_name_and_keeps_the_tier_height(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(200.)), |window, cx| {
        let view = cx.new(|_| Links);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let docs = window.find("docs");
        assert_eq!(docs.role(), Some(gpui_kit::Role::Link));
        assert_eq!(docs.label(), Some("Read the docs"));
        assert_eq!(docs.bounds().size.height, px(28.));
        let icon = window.find("icon-link");
        assert_eq!(icon.role(), Some(gpui_kit::Role::Link));
        assert_eq!(icon.label(), Some("Open"));
        assert_eq!(
            icon.bounds().size.width,
            px(28.),
            "icon-only links are square too"
        );
    })
    .unwrap();
}
