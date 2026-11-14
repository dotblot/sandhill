//! UI integration tests for `Tabs`: real input through a headless window.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use gpui_cn::{ActiveTheme as _, Tab, Tabs, TabsEvent, TabsState, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Pixels, Render,
    SharedString, Styled as _, TestAppContext, Window, WindowHandle, div, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    tabs: Entity<TabsState>,
    addable: bool,
    leading: Rc<Cell<Pixels>>,
    confirm: Rc<Cell<bool>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.theme().metrics.title_bar;
        div()
            .size_full()
            .child(
                div().w_full().h(bar).child(
                    Tabs::new("tabs", &self.tabs)
                        .addable(self.addable)
                        .confirm_close(self.confirm.get())
                        .with_leading(self.leading.get()),
                ),
            )
            .child(div().flex_1())
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    tabs: Entity<TabsState>,
    events: Rc<RefCell<Vec<TabsEvent>>>,
    leading: Rc<Cell<Pixels>>,
    confirm: Rc<Cell<bool>>,
}

fn setup(cx: &mut TestAppContext, width: f32, count: usize, addable: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let tabs = cx.new(|_| TabsState::new((0..count).map(tab)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let leading = Rc::new(Cell::new(px(0.)));
    let confirm = Rc::new(Cell::new(false));
    cx.update({
        let tabs = tabs.clone();
        let events = events.clone();
        move |cx| {
            cx.subscribe(&tabs, move |_, event: &TabsEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        }
    });
    let handle = cx.open_window(size(px(width), px(300.)), |window, cx| {
        let tabs = tabs.clone();
        let leading = leading.clone();
        let confirm = confirm.clone();
        let harness = cx.new(|cx| {
            cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
            Harness {
                tabs,
                addable,
                leading,
                confirm,
            }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        // The scroll controls come a frame after the layout that overflows.
        window.render_frame(cx);
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        tabs,
        events,
        leading,
        confirm,
    }
}

/// Lets every tab motion land: the clock passes the glide, deferred
/// settles run, and the strip renders again.
fn settle(setup: &Setup, cx: &mut TestAppContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
}

/// The nth tab: id `t<n>`.
fn tab(n: usize) -> Tab {
    Tab::new(format!("t{n}"), format!("Tab {n}"))
}

/// A child of the strip, as the strip names it.
fn child(name: &str) -> ElementId {
    ElementId::NamedChild(Arc::new("tabs".into()), SharedString::from(name.to_owned()))
}

/// The nth tab's item: the strip's child named by the tab id.
fn tab_id(n: usize) -> ElementId {
    child(&format!("t{n}"))
}

/// A part of a tab: its close control or its label box.
fn part(tab: &str, name: &str) -> ElementId {
    ElementId::NamedChild(Arc::new(child(tab)), SharedString::from(name.to_owned()))
}

fn selected(setup: &Setup, cx: &TestAppContext) -> Option<String> {
    setup
        .tabs
        .read_with(cx, |state, _| state.selected().map(|id| id.to_string()))
}

#[gpui_kit::test]
fn a_click_selects_the_tab_and_reports_it(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    assert_eq!(selected(&setup, cx).as_deref(), Some("t0"));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find(tab_id(0)).selected(), Some(true));
        assert_eq!(window.find(tab_id(2)).selected(), Some(false));
        assert_eq!(window.find(tab_id(2)).label(), Some("Tab 2"));
        window.click(tab_id(2), cx);
        assert_eq!(window.find(tab_id(2)).selected(), Some(true));
        assert_eq!(window.find(tab_id(0)).selected(), Some(false));
    })
    .unwrap();
    assert_eq!(selected(&setup, cx).as_deref(), Some("t2"));
    assert_eq!(*setup.events.borrow(), [TabsEvent::Selected("t2".into())]);
}

#[gpui_kit::test]
fn a_strip_that_confirms_close_asks_and_keeps_the_tab(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    setup.confirm.set(true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(part("t0", "close"), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        *setup.events.borrow(),
        [TabsEvent::CloseRequested("t0".into())]
    );
    assert_eq!(setup.tabs.read_with(cx, |tabs, _| tabs.tabs().len()), 3);
    setup.tabs.update(cx, |tabs, cx| tabs.remove("t0", cx));
    assert_eq!(setup.tabs.read_with(cx, |tabs, _| tabs.tabs().len()), 2);
}

#[gpui_kit::test]
fn the_close_control_removes_the_tab_and_moves_the_selection(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.find(part("t0", "close")).visible());
        assert!(
            !window.find(part("t1", "close")).visible(),
            "an unselected tab hides its close until hovered"
        );
        window.click(part("t0", "close"), cx);
        cx.background_executor()
            .advance_clock(std::time::Duration::from_millis(90));
        window.render_frame(cx);
        let leaving = window.find(tab_id(0)).bounds().size.width;
        assert!(
            leaving > px(0.) && leaving < px(160.),
            "the closed tab shrinks out: {leaving:?}"
        );
        assert_eq!(window.find(tab_id(1)).selected(), Some(true));
    })
    .unwrap();
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(tab_id(0)).is_none(), "and is gone");
        assert_eq!(window.find(tab_id(1)).bounds().origin.x, px(8.));
    })
    .unwrap();
    assert_eq!(selected(&setup, cx).as_deref(), Some("t1"));
    // Select the last tab and close it: the one before it takes over.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(tab_id(2), cx);
        window.click(part("t2", "close"), cx);
        assert_eq!(window.find(tab_id(1)).selected(), Some(true));
    })
    .unwrap();
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(tab_id(2)).is_none());
    })
    .unwrap();
    assert_eq!(selected(&setup, cx).as_deref(), Some("t1"));
    assert_eq!(
        *setup.events.borrow(),
        [
            TabsEvent::Closed("t0".into()),
            TabsEvent::Selected("t1".into()),
            TabsEvent::Selected("t2".into()),
            TabsEvent::Closed("t2".into()),
            TabsEvent::Selected("t1".into()),
        ]
    );
}

#[gpui_kit::test]
fn the_new_tab_control_asks_the_application(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find(child("add")).label(), Some("New tab"));
        window.click(child("add"), cx);
    })
    .unwrap();
    assert_eq!(*setup.events.borrow(), [TabsEvent::AddRequested]);
    assert_eq!(
        selected(&setup, cx).as_deref(),
        Some("t0"),
        "nothing was added"
    );
}

#[gpui_kit::test]
fn without_the_new_tab_control_there_is_no_add(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, false);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(child("add")).is_none());
        assert!(window.find(tab_id(0)).visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn tabs_that_fit_take_their_preferred_width_and_no_scroll_controls(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        for n in 0..3 {
            let bounds = window.find(tab_id(n)).bounds();
            assert_eq!(bounds.size.width, px(160.), "tab {n}");
            assert_eq!(bounds.size.height, px(38.), "tab {n} fills the bar");
            assert_eq!(
                bounds.origin.x,
                px(8. + 160. * n as f32),
                "tab {n}, after the strip's edge room"
            );
        }
        assert_eq!(
            window.find(tab_id(0)).bounds().bottom(),
            px(38.),
            "the tabs sit at the bottom of the bar"
        );
        let add = window.find(child("add")).bounds();
        assert_eq!(
            add.origin.x,
            px(8. + 480. + 8.),
            "the new-tab control follows the last tab after the same room"
        );
        assert!(window.try_find(child("back")).is_none());
        assert!(window.try_find(child("forward")).is_none());
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle strip asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn tabs_that_overflow_shrink_to_the_minimum_and_show_scroll_controls(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    cx.update_window(setup.handle.into(), |_, window, _| {
        for n in 0..8 {
            assert_eq!(
                window.find(tab_id(n)).bounds().size.width,
                px(112.),
                "tab {n}"
            );
        }
        let back = window.find(child("back"));
        let forward = window.find(child("forward"));
        assert_eq!(
            back.bounds().origin.x,
            px(8.),
            "the controls lead the strip, after its room"
        );
        assert_eq!(forward.bounds().origin.x, px(8. + 24. + 4.), "a gap apart");
        assert_eq!(
            window.find(tab_id(0)).bounds().origin.x,
            px(8. + 24. + 4. + 24. + 8. + 8.),
            "room after the controls, then the strip's lead-in"
        );
        let add = window.find(child("add")).bounds();
        assert!(
            add.right() <= px(500. - 8.),
            "the new-tab control keeps off the edge: {add:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn pushing_tabs_into_a_narrow_strip_brings_the_scroll_controls_by_itself(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 2, true);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(child("back")).is_none());
    })
    .unwrap();
    setup.tabs.update(cx, |state, cx| {
        for n in 2..8 {
            state.push(tab(n), cx);
        }
    });
    cx.update_window(setup.handle.into(), |_, window, _| {
        let entering = window.find(tab_id(7)).bounds().size.width;
        assert!(entering < px(112.), "a pushed tab grows in: {entering:?}");
    })
    .unwrap();
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find(tab_id(7)).bounds().size.width, px(112.));
        assert!(window.find(child("back")).visible());
        assert!(window.find(child("forward")).visible());
    })
    .unwrap();
    // The landed widths change the overflow once more; then nothing moves.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.simulate_next_frame(cx), 0, "the strip rests");
    })
    .unwrap();
    setup.tabs.update(cx, |state, cx| {
        for n in 2..8 {
            state.remove(format!("t{n}"), cx);
        }
    });
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(child("back")).is_none());
        assert!(window.try_find(child("forward")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_scroll_controls_glide_the_strip_one_tab_width(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    let first_tab_x = |cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find(tab_id(0)).bounds().origin.x
        })
        .unwrap()
    };
    let click = |cx: &mut TestAppContext, name: &str| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.click(child(name), cx)
        })
        .unwrap();
    };
    let wait = |cx: &mut TestAppContext, millis: u64| {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(millis));
    };
    let start = first_tab_x(cx);
    click(cx, "back");
    wait(cx, 400);
    assert_eq!(first_tab_x(cx), start, "nothing to scroll back to");
    click(cx, "forward");
    wait(cx, 90);
    let midway = first_tab_x(cx);
    assert!(
        midway < start && midway > start - px(160.),
        "the strip is on its way: {midway:?}"
    );
    wait(cx, 400);
    assert_eq!(first_tab_x(cx), start - px(160.));
    click(cx, "back");
    wait(cx, 400);
    assert_eq!(first_tab_x(cx), start);
}

#[gpui_kit::test]
fn selecting_a_tab_scrolls_it_into_view(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    let viewport_right = cx
        .update_window(setup.handle.into(), |_, window, _| {
            window.find(child("scroll")).bounds().right()
        })
        .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert!(window.find(tab_id(7)).bounds().right() > viewport_right);
        window.click(tab_id(3), cx);
        window.render_frame(cx);
    })
    .unwrap();
    setup.tabs.update(cx, |state, cx| state.select("t7", cx));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(tab_id(7)).bounds().right(), viewport_right);
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_arrow_keys_select_the_neighbor_and_wrap(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        assert_eq!(
            window.find("tabs").focused(),
            Some(true),
            "the strip is one tab stop"
        );
        window.press("right", cx);
        assert_eq!(window.find(tab_id(1)).selected(), Some(true));
        window.press("right", cx);
        window.press("right", cx);
        assert_eq!(window.find(tab_id(0)).selected(), Some(true), "wraps");
        window.press("left", cx);
        assert_eq!(window.find(tab_id(2)).selected(), Some(true), "wraps back");
        window.press("cmd-left", cx);
        assert_eq!(
            window.find(tab_id(2)).selected(),
            Some(true),
            "a modified arrow is not the strip's"
        );
    })
    .unwrap();
    assert_eq!(selected(&setup, cx).as_deref(), Some("t2"));
}

#[gpui_kit::test]
fn a_tab_shows_its_label_as_a_tooltip(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 2, true);
    setup.tabs.update(cx, |state, cx| {
        state.push(
            Tab::new("long", "A label far too long for a tab of its width"),
            cx,
        );
    });
    let hover_and_settle = |cx: &mut TestAppContext, id: ElementId| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.hover(id, cx);
        })
        .unwrap();
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
    };
    assert!(
        hover_and_settle(cx, part("t0", "label")),
        "every tab names itself on hover"
    );
    assert!(hover_and_settle(cx, part("long", "label")));
}

/// A page that scrolls, with the strip in its bar.
struct Page {
    tabs: Entity<TabsState>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.theme().metrics.title_bar;
        gpui_cn::ScrollArea::new("page")
            .size_full()
            .child(div().w_full().h(bar).child(Tabs::new("tabs", &self.tabs)))
            .child(div().w_full().h(px(2000.)))
    }
}

#[gpui_kit::test]
fn a_wheel_step_over_an_overflowing_strip_does_not_scroll_the_page(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let tabs = cx.new(|_| TabsState::new((0..8).map(tab)));
    let handle = cx.open_window(size(px(500.), px(300.)), |window, cx| {
        let tabs = tabs.clone();
        let page = cx.new(|cx| {
            cx.observe(&tabs, |_, _, cx| cx.notify()).detach();
            Page { tabs }
        });
        Root::new(page, window, cx)
    });
    let strip_top = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("tabs").bounds().origin.y
        })
        .unwrap()
    };
    let wheel = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.scroll(
                tab_id(1),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-40.), px(-6.))),
                cx,
            );
        })
        .unwrap();
    };
    let top = strip_top(cx);
    wheel(cx);
    assert_eq!(strip_top(cx), top, "the page stayed under the strip");
    // A vertical mouse line is the strip's too, though the page scrolls
    // vertically: the strip glides it sideways.
    let first_tab_x = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find(tab_id(0)).bounds().origin.x
        })
        .unwrap()
    };
    let before = first_tab_x(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.scroll(
            tab_id(1),
            gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -1.)),
            cx,
        );
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    assert_eq!(strip_top(cx), top, "the page stayed under the strip");
    assert!(first_tab_x(cx) < before, "the strip glided the line");
    tabs.update(cx, |state, cx| {
        for n in 3..8 {
            state.remove(format!("t{n}"), cx);
        }
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    strip_top(cx);
    cx.run_until_parked();
    strip_top(cx);
    let top = strip_top(cx);
    wheel(cx);
    assert!(
        strip_top(cx) < top,
        "a strip whose tabs fit passes the step to the page"
    );
}

#[gpui_kit::test]
fn only_a_tab_with_a_separator_gives_up_the_gutter(cx: &mut TestAppContext) {
    let setup = setup(cx, 900., 3, true);
    let label_width = |cx: &mut TestAppContext, n: usize| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .find(part(&format!("t{n}"), "label"))
                .bounds()
                .size
                .width
        })
        .unwrap()
    };
    // t0 selected: t1 separates from t2, t2 is last.
    assert_eq!(label_width(cx, 0), px(146.), "160 minus two shoulders");
    assert_eq!(label_width(cx, 1), px(139.), "minus the separator gutter");
    assert_eq!(label_width(cx, 2), px(146.));
    setup.tabs.update(cx, |state, cx| state.select("t1", cx));
    assert_eq!(label_width(cx, 0), px(146.), "before the selected tab");
    assert_eq!(label_width(cx, 1), px(146.));
    assert_eq!(label_width(cx, 2), px(146.));
}

#[gpui_kit::test]
fn a_push_that_starts_the_overflow_keeps_the_new_tab_in_view(cx: &mut TestAppContext) {
    // Four tabs fit a 500px strip beside the new-tab control; the fifth
    // brings the scroll controls, which shift the tabs by their width.
    let setup = setup(cx, 500., 4, true);
    setup.tabs.update(cx, |state, cx| state.push(tab(4), cx));
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        for _ in 0..3 {
            window.render_frame(cx);
        }
        let viewport = window.find(child("scroll")).bounds();
        assert!(window.find(child("back")).visible());
        assert_eq!(
            window.find(tab_id(4)).bounds().right(),
            viewport.right(),
            "the pushed tab ends at the viewport's edge"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_controls_fit_their_slots_at_a_larger_font_size(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    cx.update(|cx| Theme::set_ui_font_size(cx, px(20.)));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let back = window.find(child("back")).bounds();
        let forward = window.find(child("forward")).bounds();
        assert_eq!(back.origin.x, px(8.));
        assert_eq!(forward.origin.x, back.right() + px(4.));
        assert_eq!(
            window.find(tab_id(0)).bounds().origin.x,
            forward.right() + px(16.)
        );
        let close = window.find(part("t0", "close")).bounds();
        assert_eq!(close.size.width, px(25.), "an Xs control at 20px text");
        let label = window.find(part("t0", "label")).bounds();
        assert!(
            close.right() <= label.right(),
            "the close control stays inside its tab"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn closing_every_tab_takes_the_scroll_controls_with_them(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(child("forward"), cx);
    })
    .unwrap();
    settle(&setup, cx);
    setup.tabs.update(cx, |state, cx| {
        for n in 0..8 {
            state.remove(format!("t{n}"), cx);
        }
    });
    settle(&setup, cx);
    settle(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(tab_id(0)).is_none());
        assert!(
            window.try_find(child("back")).is_none(),
            "nothing to scroll"
        );
        assert!(window.try_find(child("forward")).is_none());
        assert!(window.find(child("add")).visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_leading_room_moves_the_scroll_controls_and_the_tabs_after_it(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    setup.leading.set(px(100.));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(child("back")).bounds().origin.x, px(100. + 8.));
        assert_eq!(
            window.find(tab_id(0)).bounds().origin.x,
            px(100. + 8. + 24. + 4. + 24. + 8. + 8.)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_mouse_wheel_step_glides_and_a_trackpad_step_is_direct(cx: &mut TestAppContext) {
    let setup = setup(cx, 500., 8, true);
    let first_tab_x = |cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find(tab_id(0)).bounds().origin.x
        })
        .unwrap()
    };
    let wheel = |cx: &mut TestAppContext, delta: gpui_kit::ScrollDelta| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.scroll(tab_id(1), delta, cx);
        })
        .unwrap();
    };
    let wait = |cx: &mut TestAppContext, millis: u64| {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(millis));
    };
    let line = cx
        .update_window(setup.handle.into(), |_, window, _| window.line_height())
        .unwrap();
    let start = first_tab_x(cx);
    // A wheel line: the offset goes back and the strip glides the line.
    wheel(cx, gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -2.)));
    let right_after = first_tab_x(cx);
    assert!(
        right_after > start - line * 2. && right_after <= start,
        "the step glides instead of jumping: {right_after:?}"
    );
    wait(cx, 400);
    assert_eq!(first_tab_x(cx), start - line * 2.);
    // Two lines while a glide runs stack on its end.
    wheel(cx, gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -1.)));
    wheel(cx, gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -1.)));
    wait(cx, 400);
    assert_eq!(first_tab_x(cx), start - line * 4.);
    // A trackpad step lands at once.
    wheel(
        cx,
        gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-10.), px(0.))),
    );
    assert_eq!(first_tab_x(cx), start - line * 4. - px(10.));
}

struct TriggerHarness {
    tabs: Entity<TabsState>,
}

impl Render for TriggerHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.theme().metrics.title_bar;
        div().size_full().child(div().w_full().h(bar).child(
            Tabs::new("tabs", &self.tabs).add_trigger(|button| {
                gpui_cn::Popover::new("add-popover")
                    .trigger(button)
                    .content(|_, _| div().size(px(40.)))
            }),
        ))
    }
}

#[gpui_kit::test]
fn an_add_trigger_takes_the_control_in_place_of_the_request(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = gpui_cn::ReduceMotion::On);
    });
    let tabs = cx.new(|_| TabsState::new((0..2).map(tab)));
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update({
        let tabs = tabs.clone();
        let events = events.clone();
        move |cx| {
            cx.subscribe(&tabs, move |_, event: &TabsEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        }
    });
    let handle = cx.open_window(size(px(600.), px(300.)), |window, cx| {
        let tabs = tabs.clone();
        let harness = cx.new(|_| TriggerHarness { tabs });
        Root::new(harness, window, cx)
    });
    let panel = ElementId::NamedChild(ElementId::Name("add-popover".into()).into(), "panel".into());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let add = window.find(child("add")).bounds();
        window.click(child("add"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let panel = window.find(panel).bounds();
        assert_eq!(panel.left(), add.left(), "lined up with the leading edge");
        assert_eq!(
            panel.top(),
            add.bottom() + px(2.),
            "the menu gap under the control"
        );
    })
    .unwrap();
    assert!(
        events.borrow().is_empty(),
        "the control opens the popover instead of requesting a tab"
    );
}
