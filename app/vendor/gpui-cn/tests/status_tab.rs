//! UI integration tests for `ComposerStatusTab`, `ContextMeter`, and
//! `StatusSelect`.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{
    ComposerStatusTab, ReduceMotion, StatusOption, StatusSelect, StatusSelectEvent,
    StatusSelectState, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, TestAppContext, Window, assets::IconName, div, px, size,
    test::TestWindowExt as _,
};

struct Harness {
    percent: f32,
    bare: bool,
    project: Entity<StatusSelectState>,
    branch: Entity<StatusSelectState>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(500.)).child(if self.bare {
            ComposerStatusTab::new("status")
        } else {
            ComposerStatusTab::new("status")
                .select(
                    StatusSelect::new("project", &self.project)
                        .icon(IconName::Folder)
                        .searchable(false),
                )
                .select(
                    StatusSelect::new("branch", &self.branch)
                        .icon(IconName::GitBranch)
                        .searchable(false),
                )
                .context(self.percent)
        })
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    branch: Entity<StatusSelectState>,
    events: Rc<RefCell<Vec<StatusSelectEvent>>>,
}

fn options(names: &[&str]) -> Vec<StatusOption> {
    names
        .iter()
        .map(|name| StatusOption::new(name.to_string(), name.to_string()))
        .collect()
}

fn start(cx: &mut TestAppContext, bare: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut slot = None;
    let handle = cx.open_window(size(px(600.), px(500.)), |window, cx| {
        let project =
            cx.new(|cx| StatusSelectState::new(options(&["sample-app", "northwind"]), cx));
        let branch = cx.new(|cx| StatusSelectState::new(options(&["main", "dev", "fix"]), cx));
        let recorded = events.clone();
        cx.subscribe(&branch, move |_, _, event: &StatusSelectEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        slot = Some(branch.clone());
        let harness = cx.new(|cx| {
            cx.observe(&project, |_, _, cx| cx.notify()).detach();
            cx.observe(&branch, |_, _, cx| cx.notify()).detach();
            Harness {
                percent: 57.,
                bare,
                project,
                branch,
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
        branch: slot.unwrap(),
        events,
    }
}

fn setup(cx: &mut TestAppContext, bare: bool) -> gpui_kit::WindowHandle<Root> {
    start(cx, bare).handle
}

fn meter() -> ElementId {
    ElementId::NamedChild(ElementId::from("status").into(), "context".into())
}

/// A part of a select, under the select its item shows.
fn part(parent: &str, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(
        ElementId::NamedChild(
            ElementId::from(SharedString::from(parent.to_string())).into(),
            "select".into(),
        )
        .into(),
        name.into(),
    )
}

#[gpui_kit::test]
fn the_tab_is_thirty_eight_high_and_inset_from_both_sides(cx: &mut TestAppContext) {
    let handle = setup(cx, false);
    cx.update_window(handle.into(), |_, window, cx| {
        let tab = window.find("status").bounds();
        assert_eq!(tab.size.height, px(38.), "exactly the visible strip");
        assert_eq!(tab.origin.x, px(14.), "inset from the left");
        assert_eq!(tab.size.width, px(500. - 28.), "inset from both sides");
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle tab asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_meter_sits_at_the_far_right_and_holds_a_sixteen_pixel_ring(cx: &mut TestAppContext) {
    let handle = setup(cx, false);
    cx.update_window(handle.into(), |_, window, _| {
        let tab = window.find("status").bounds();
        let bounds = window.find(meter()).bounds();
        assert_eq!(
            bounds.right(),
            tab.right() - px(16.),
            "the tab's right padding"
        );
        assert!(
            (bounds.center().y - (tab.top() + px(19.))).abs() <= px(0.5),
            "centered in the 38px tab"
        );
        let ring = window
            .find(ElementId::NamedChild(meter().into(), "ring".into()))
            .bounds();
        assert_eq!(ring.size, size(px(16.), px(16.)));
        assert!(ring.left() >= bounds.left());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_tab_without_items_is_still_a_strip(cx: &mut TestAppContext) {
    let handle = setup(cx, true);
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(window.find("status").bounds().size.height, px(38.));
        assert!(window.try_find(meter()).is_none(), "no meter was given");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_dropdown_shows_the_chosen_option_and_picking_another_changes_it(cx: &mut TestAppContext) {
    let setup = start(cx, false);
    let tab_height = |cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("status").bounds().size.height
        })
        .unwrap()
    };
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("branch", "trigger"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            window.try_find(part("branch", "menu")).is_some(),
            "one menu"
        );
        assert!(
            window.try_find(part("project", "menu")).is_none(),
            "only this one"
        );
        assert!(window.try_find(part("branch", "main")).is_some());
        assert!(window.try_find(part("branch", "dev")).is_some());
    })
    .unwrap();
    assert_eq!(
        tab_height(cx),
        px(38.),
        "the tab keeps its height while open"
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("branch", "dev"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(part("branch", "menu")).is_none(), "closed");
    })
    .unwrap();
    assert_eq!(
        setup
            .branch
            .read_with(cx, |state, _| state.selected().clone()),
        "dev"
    );
    assert_eq!(
        setup.events.borrow().as_slice(),
        [StatusSelectEvent::Changed("dev".into())]
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(ElementId::NamedChild(
                    ElementId::from("branch").into(),
                    "select".into()
                ))
                .value(),
            Some("dev"),
            "the trigger shows the new label"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_closes_a_dropdown_without_a_change_and_the_keyboard_opens_it(cx: &mut TestAppContext) {
    let setup = start(cx, false);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("tab", cx);
        window.render_frame(cx);
        window.press("tab", cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let open = window.try_find(part("project", "menu")).is_some()
            || window.try_find(part("branch", "menu")).is_some();
        assert!(open, "Enter on a focused item opens its menu");
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(window.try_find(part("project", "menu")).is_none());
        assert!(window.try_find(part("branch", "menu")).is_none());
    })
    .unwrap();
    assert!(setup.events.borrow().is_empty());
}

struct Narrow {
    width: f32,
    project: Entity<StatusSelectState>,
    device: Entity<StatusSelectState>,
    branch: Entity<StatusSelectState>,
}

impl Render for Narrow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(self.width)).child(
            ComposerStatusTab::new("status")
                .select(StatusSelect::new("project", &self.project).icon(IconName::Folder))
                .select(StatusSelect::new("device", &self.device).icon(IconName::Laptop))
                .select(StatusSelect::new("branch", &self.branch).icon(IconName::GitBranch))
                .context(57.),
        )
    }
}

fn child(parent: &str, name: &str) -> ElementId {
    if name == "context" {
        return ElementId::NamedChild(
            ElementId::Name(parent.to_string().into()).into(),
            name.to_string().into(),
        );
    }
    part(parent, name.to_string())
}

#[gpui_kit::test]
fn items_never_overlap_and_the_meter_stays_at_the_right(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    for width in [280., 360., 480., 720.] {
        let handle = cx.open_window(size(px(800.), px(200.)), |window, cx| {
            let project = cx.new(|cx| {
                StatusSelectState::new(options(&["sample-application-with-a-long-name"]), cx)
            });
            let device =
                cx.new(|cx| StatusSelectState::new(options(&["Local machine in the office"]), cx));
            let branch = cx.new(|cx| {
                StatusSelectState::new(options(&["feature/composer-redesign-long-branch-name"]), cx)
            });
            let harness = cx.new(|_| Narrow {
                width,
                project,
                device,
                branch,
            });
            Root::new(harness, window, cx)
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.activate_window();
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let tab = window.find("status").bounds();
            assert_eq!(tab.size.height, px(38.), "width {width}");
            let items: Vec<_> = ["project", "device", "branch"]
                .iter()
                .map(|name| window.find(child(name, "trigger")).bounds())
                .collect();
            for (at, item) in items.iter().enumerate() {
                assert!(
                    item.left() >= tab.left() && item.right() <= tab.right(),
                    "item {at} is inside the tab at {width}: {item:?} {tab:?}"
                );
                for other in &items[at + 1..] {
                    assert!(
                        !item.intersects(other),
                        "items overlap at {width}: {item:?} {other:?}"
                    );
                }
            }
            let meter = window.find(child("status", "context")).bounds();
            assert!(
                meter.left() >= tab.left() && meter.right() <= tab.right(),
                "the meter is inside the tab at {width}"
            );
            let last = items.last().unwrap();
            assert!(
                last.right() <= meter.left(),
                "the last item ends before the meter at {width}"
            );
            let ring = window
                .find(ElementId::NamedChild(
                    child("status", "context").into(),
                    "ring".into(),
                ))
                .bounds();
            assert!(ring.left() >= meter.left() && ring.right() <= meter.right());
            let inset = f32::from(tab.right() - meter.right());
            assert!(
                (inset - 12.).abs() < 6.,
                "the meter keeps the tab's trailing padding at {width}: {inset}"
            );
        })
        .unwrap();
    }
}
