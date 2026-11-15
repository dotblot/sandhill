//! UI integration tests for the sidebar: real input through a headless window.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_cn::{
    ReduceMotion, Sidebar, SidebarGroup, SidebarLayout, SidebarMenuButton, SidebarState,
    SidebarTrigger, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, TestAppContext, Window, WindowHandle, base::Selectable as _,
    base::TestSupportExt as _, div, point, px, size, test::TestWindowExt as _,
};

struct Harness {
    sidebar: Entity<SidebarState>,
    clicks: Rc<Cell<usize>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div().size_full().child(
            SidebarLayout::new(&self.sidebar)
                .sidebar(
                    Sidebar::new()
                        .header(
                            div()
                                .h(px(46.))
                                .child(SidebarTrigger::new("trigger", &self.sidebar)),
                        )
                        .child(
                            SidebarGroup::new()
                                .label("Projects")
                                .child(
                                    SidebarMenuButton::new("inbox")
                                        .label("Inbox")
                                        .selected(true)
                                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                                )
                                .child(SidebarMenuButton::new("drafts").label("Drafts")),
                        ),
                )
                .child(div().id("content").test_support().size_full()),
        )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    sidebar: Entity<SidebarState>,
    clicks: Rc<Cell<usize>>,
}

fn setup(cx: &mut TestAppContext, reduce_motion: ReduceMotion) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = reduce_motion);
    });
    // These tests cover the off-canvas mode; the rail has its own below.
    let sidebar = cx.new(|cx| {
        SidebarState::new(cx)
            .with_width_range(px(200.)..px(400.))
            .with_collapsible(gpui_cn::SidebarCollapsible::Offcanvas)
    });
    let clicks = Rc::new(Cell::new(0));
    let handle = cx.open_window(size(px(800.), px(600.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            sidebar: sidebar.clone(),
            clicks: clicks.clone(),
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    Setup {
        handle,
        sidebar,
        clicks,
    }
}

fn panel_width(setup: &Setup, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.find("sidebar-panel").bounds().size.width
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_sidebar_takes_its_width_and_the_content_takes_the_rest(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    assert_eq!(panel_width(&setup, cx), px(300.));
    cx.update_window(setup.handle.into(), |_, window, _| {
        let content = window.find("content").bounds();
        assert_eq!(content.origin.x, px(300.));
        assert_eq!(content.size.width, px(500.));
        // `selected` is visual in base and sets no accessibility flag; the
        // fill is proven in the button's unit tests.
        assert!(window.find("inbox").visible());
        assert_eq!(window.find("inbox").bounds().size.height, px(30.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_trigger_collapses_the_sidebar_and_hides_its_rows(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        assert_eq!(window.find("trigger").label(), Some("Hide sidebar"));
        window.click("trigger", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(!setup.sidebar.read_with(cx, |state, _| state.is_open()));
    assert_eq!(panel_width(&setup, cx), px(0.));
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(
            window.try_find("inbox").is_none(),
            "a collapsed sidebar renders nothing, so nothing in it can take focus"
        );
        assert!(window.try_find("trigger").is_none());
        assert_eq!(window.find("content").bounds().origin.x, px(0.));
    })
    .unwrap();

    setup
        .sidebar
        .update(cx, |state, cx| state.set_open(true, cx));
    assert_eq!(panel_width(&setup, cx), px(300.));
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find("trigger").label(), Some("Hide sidebar"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn collapsing_animates_the_width_unless_motion_is_reduced(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::Off);
    setup
        .sidebar
        .update(cx, |state, cx| state.set_open(false, cx));
    let width = panel_width(&setup, cx);
    assert!(
        width > px(0.) && width <= px(300.),
        "starts sliding: {width:?}"
    );
    cx.executor().advance_clock(Duration::from_millis(100));
    let mid = panel_width(&setup, cx);
    assert!(mid < width, "keeps sliding: {mid:?} < {width:?}");
    assert!(mid > px(0.));
    cx.executor().advance_clock(Duration::from_millis(400));
    assert_eq!(panel_width(&setup, cx), px(0.));
}

#[gpui_kit::test]
fn dragging_the_edge_resizes_within_the_range(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let handle = window.find("sidebar-resize-handle").bounds();
        assert_eq!(handle.origin.x, px(296.));
        assert_eq!(handle.size.width, px(8.));
        window.drag(point(px(300.), px(300.)), point(px(350.), px(300.)), cx);
    })
    .unwrap();
    assert_eq!(
        setup.sidebar.read_with(cx, |state, _| state.width()),
        px(350.)
    );
    assert_eq!(panel_width(&setup, cx), px(350.));

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.drag(point(px(350.), px(300.)), point(px(50.), px(300.)), cx);
    })
    .unwrap();
    assert_eq!(
        setup.sidebar.read_with(cx, |state, _| state.width()),
        px(200.)
    );
    assert_eq!(panel_width(&setup, cx), px(200.));
    assert!(!setup.sidebar.read_with(cx, |state, _| state.is_resizing()));

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.drag(point(px(200.), px(300.)), point(px(700.), px(300.)), cx);
    })
    .unwrap();
    assert_eq!(
        setup.sidebar.read_with(cx, |state, _| state.width()),
        px(400.)
    );
}

#[gpui_kit::test]
fn a_row_clicks_and_is_reachable_by_keyboard(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("inbox", cx);
        window.render_frame(cx);
        assert_eq!(setup.clicks.get(), 1);
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("trigger").focused(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("inbox").focused(), Some(true));
    })
    .unwrap();
}

/// A sidebar with every part: a collapsible group with an action, rows
/// with a badge and a hover action, nested rows, a separator, a skeleton.
struct Parts {
    sidebar: Entity<SidebarState>,
    group_open: bool,
    row_open: bool,
    guide: bool,
    action_clicks: Rc<Cell<usize>>,
    row_action_clicks: Rc<Cell<usize>>,
}

impl Render for Parts {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let action_clicks = self.action_clicks.clone();
        let row_action_clicks = self.row_action_clicks.clone();
        div().size_full().child(
            SidebarLayout::new(&self.sidebar)
                .side(gpui_cn::SidebarSide::Right)
                .sidebar(
                    Sidebar::new()
                        .child(
                            SidebarGroup::new()
                                .id("mail")
                                .label("Mail")
                                .collapsible(self.group_open)
                                .on_toggle(cx.listener(|this, open, _, cx| {
                                    this.group_open = *open;
                                    cx.notify();
                                }))
                                .action(
                                    gpui_cn::Button::new("add")
                                        .ghost()
                                        .size(gpui_cn::ButtonSize::Sm)
                                        .label("Add")
                                        .on_click(move |_, _, _| {
                                            action_clicks.set(action_clicks.get() + 1)
                                        }),
                                )
                                .child(
                                    SidebarMenuButton::new("inbox")
                                        .label("Inbox")
                                        .badge("12")
                                        .action(
                                            gpui_cn::Button::new("inbox-more")
                                                .ghost()
                                                .size(gpui_cn::ButtonSize::Sm)
                                                .label("More")
                                                .on_click(move |_, _, _| {
                                                    row_action_clicks
                                                        .set(row_action_clicks.get() + 1)
                                                }),
                                        ),
                                )
                                .child(
                                    SidebarMenuButton::new("projects")
                                        .label("Projects")
                                        .collapsible(self.row_open)
                                        .on_toggle(cx.listener(|this, open, _, cx| {
                                            this.row_open = *open;
                                            cx.notify();
                                        }))
                                        .submenu(
                                            gpui_cn::SidebarMenuSub::new().guide(self.guide).child(
                                                SidebarMenuButton::new("nested").label("Nested"),
                                            ),
                                        ),
                                ),
                        )
                        .child(gpui_cn::SidebarSeparator::new())
                        .child(gpui_cn::SidebarMenuSkeleton::new("skeleton")),
                )
                .child(div().id("content").test_support().size_full()),
        )
    }
}

fn setup_parts(
    cx: &mut TestAppContext,
    collapsible: gpui_cn::SidebarCollapsible,
) -> (WindowHandle<Root>, Entity<SidebarState>, Entity<Parts>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let sidebar = cx.new(|cx| SidebarState::new(cx).with_collapsible(collapsible));
    let mut parts = None;
    let handle = cx.open_window(size(px(800.), px(600.)), |window, cx| {
        let view = cx.new(|_| Parts {
            sidebar: sidebar.clone(),
            group_open: true,
            row_open: true,
            guide: true,
            action_clicks: Rc::new(Cell::new(0)),
            row_action_clicks: Rc::new(Cell::new(0)),
        });
        parts = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    (handle, sidebar, parts.unwrap())
}

#[gpui_kit::test]
fn a_right_sidebar_sits_at_the_far_edge_with_its_handle_inside(cx: &mut TestAppContext) {
    let (handle, sidebar, _) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(window.find("sidebar-panel").bounds().origin.x, px(500.));
        assert_eq!(window.find("content").bounds().size.width, px(500.));
        let handle = window.find("sidebar-resize-handle").bounds();
        assert_eq!(handle.origin.x, px(496.));
        window.drag(point(px(500.), px(300.)), point(px(450.), px(300.)), cx);
    })
    .unwrap();
    assert_eq!(sidebar.read_with(cx, |state, _| state.width()), px(350.));
}

#[gpui_kit::test]
fn a_collapsible_group_hides_its_rows_and_keeps_its_action(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.try_find("inbox").is_some());
        assert!(window.try_find("nested").is_some());
        window.click(
            gpui_kit::ElementId::NamedChild(
                gpui_kit::ElementId::from("mail").into(),
                "toggle".into(),
            ),
            cx,
        );
        window.render_frame(cx);
        assert!(window.try_find("inbox").is_none(), "rows fold away");
        assert!(window.try_find("nested").is_none());
        window.click("add", cx);
        window.render_frame(cx);
    })
    .unwrap();
    parts.read_with(cx, |parts, _| {
        assert!(!parts.group_open);
        assert_eq!(parts.action_clicks.get(), 1);
    });
}

#[gpui_kit::test]
fn a_row_action_is_reachable_and_fires_on_its_own(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update_window(handle.into(), |_, window, cx| {
        window.hover("inbox", cx);
        window.render_frame(cx);
        window.click("inbox-more", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        parts.read_with(cx, |parts, _| parts.row_action_clicks.get()),
        1
    );
}

#[gpui_kit::test]
fn an_icon_rail_shows_icons_alone_named_by_their_labels(cx: &mut TestAppContext) {
    let (handle, sidebar, _) = setup_parts(cx, gpui_cn::SidebarCollapsible::Icon);
    sidebar.update(cx, |state, cx| state.set_open(false, cx));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find("sidebar-panel").bounds().size.width, px(48.));
        let inbox = window.find("inbox");
        assert_eq!(inbox.label(), Some("Inbox"), "the label is the name");
        assert!(inbox.bounds().size.width < px(40.), "an icon-sized row");
        assert!(
            window.try_find("nested").is_none(),
            "nested rows are hidden"
        );
        assert!(
            window.try_find("sidebar-resize-handle").is_none(),
            "a rail is not resizable"
        );
    })
    .unwrap();
    assert!(sidebar.read_with(cx, |state, _| state.is_icon_only()));
}

#[gpui_kit::test]
fn a_collapsible_row_folds_its_nested_rows(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update_window(handle.into(), |_, window, cx| {
        assert!(window.try_find("nested").is_some());
        window.click("projects", cx);
        window.render_frame(cx);
        assert!(window.try_find("nested").is_none(), "the nested rows fold");
        window.click("projects", cx);
        window.render_frame(cx);
        assert!(window.try_find("nested").is_some());
    })
    .unwrap();
    assert!(parts.read_with(cx, |parts, _| parts.row_open));
}

#[gpui_kit::test]
fn an_icon_rail_shows_an_initial_for_a_row_without_an_icon(cx: &mut TestAppContext) {
    let (handle, sidebar, _) = setup_parts(cx, gpui_cn::SidebarCollapsible::Icon);
    sidebar.update(cx, |state, cx| {
        state.set_icon_width(px(88.), cx);
        state.set_open(false, cx);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find("sidebar-panel").bounds().size.width, px(88.));
        // "Inbox" has no icon in this harness, so its rail form is an "I".
        let inbox = window.find("inbox");
        assert_eq!(inbox.label(), Some("Inbox"));
        assert!(inbox.bounds().size.width > px(0.));
    })
    .unwrap();
    assert_eq!(
        sidebar.read_with(cx, |state, _| state.icon_width()),
        px(88.)
    );
}

#[gpui_kit::test]
fn rows_keep_their_labels_while_the_rail_closes_and_lose_them_at_the_end(cx: &mut TestAppContext) {
    let (handle, sidebar, _) = setup_parts(cx, gpui_cn::SidebarCollapsible::Icon);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    let open_row = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("inbox").bounds()
        })
        .unwrap();
    sidebar.update(cx, |state, cx| state.set_open(false, cx));
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = window.find("sidebar-panel").bounds().size.width;
        assert!(panel > px(48.) && panel < px(300.), "sliding: {panel:?}");
        // The rows still have their open form: full width, labels, nested
        // rows. The panel clips them.
        let inbox = window.find("inbox").bounds();
        assert_eq!(inbox.size.width, open_row.size.width, "still the open row");
        assert!(
            window.try_find("nested").is_some(),
            "nested rows still there"
        );
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("sidebar-panel").bounds().size.width, px(48.));
        let inbox = window.find("inbox").bounds();
        assert!(inbox.size.width < px(40.), "an icon now: {inbox:?}");
        assert!(window.try_find("nested").is_none());
    })
    .unwrap();
    // Opening takes the open form at once, so the labels grow into view.
    sidebar.update(cx, |state, cx| state.set_open(true, cx));
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let panel = window.find("sidebar-panel").bounds().size.width;
        assert!(panel > px(48.) && panel < px(300.), "sliding: {panel:?}");
        assert_eq!(
            window.find("inbox").bounds().size.width,
            open_row.size.width
        );
    })
    .unwrap();
}

/// A layout inside a layout, as the gallery shows the sidebar story inside
/// the shell. Each has its own state.
struct Nested {
    outer: Entity<SidebarState>,
    inner: Entity<SidebarState>,
}

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            SidebarLayout::new(&self.outer)
                .sidebar(Sidebar::new().child(SidebarMenuButton::new("outer-row").label("Outer")))
                .child(
                    div()
                        .id("content")
                        .test_support()
                        .size_full()
                        .p(px(50.))
                        .child(
                            div().size_full().child(
                                SidebarLayout::new(&self.inner)
                                    .sidebar(
                                        Sidebar::new().child(
                                            SidebarMenuButton::new("inner-row").label("Inner"),
                                        ),
                                    )
                                    .child(div().size_full()),
                            ),
                        ),
                ),
        )
    }
}

#[gpui_kit::test]
fn a_drag_on_the_inner_sidebar_leaves_the_outer_one_alone(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let outer = cx.new(|cx| {
        SidebarState::new(cx)
            .with_width(px(300.))
            .with_collapsible(gpui_cn::SidebarCollapsible::Offcanvas)
    });
    let inner = cx.new(|cx| {
        SidebarState::new(cx)
            .with_width(px(240.))
            .with_width_range(px(180.)..px(360.))
            .with_collapsible(gpui_cn::SidebarCollapsible::Offcanvas)
    });
    let handle = cx.open_window(size(px(1000.), px(600.)), |window, cx| {
        let view = cx.new(|_| Nested {
            outer: outer.clone(),
            inner: inner.clone(),
        });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        // The inner layout starts 350px in (outer sidebar plus padding), so
        // its handle is at 590px. Drag it 40px to the right.
        window.drag(point(px(590.), px(300.)), point(px(630.), px(300.)), cx);
    })
    .unwrap();
    assert_eq!(inner.read_with(cx, |state, _| state.width()), px(280.));
    assert_eq!(
        outer.read_with(cx, |state, _| state.width()),
        px(300.),
        "the outer sidebar does not move"
    );
}

#[gpui_kit::test]
fn a_narrow_window_shows_the_sidebar_as_a_sheet_over_the_content(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let sidebar = cx.new(|cx| SidebarState::new(cx));
    let clicks = Rc::new(Cell::new(0));
    // A phone is narrower than the sheet breakpoint.
    let handle = cx.open_window(size(px(400.), px(800.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            sidebar: sidebar.clone(),
            clicks: clicks.clone(),
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        // The sheet starts closed, even though the state opened, and the
        // content has the whole width. No rail shows.
        assert_eq!(window.find("content").bounds().size.width, px(400.));
        assert!(window.try_find("inbox").is_none());
        assert!(window.try_find("sidebar-scrim").is_none());
    })
    .unwrap();
    assert!(sidebar.read_with(cx, |state, _| state.is_sheet()));
    assert!(!sidebar.read_with(cx, |state, _| state.is_open()));

    sidebar.update(cx, |state, cx| state.toggle(cx));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        // The sheet floats over the content at the sheet width, with a
        // scrim behind it. The content did not move.
        let panel = window.find("sidebar-panel").bounds();
        assert_eq!(panel.origin.x, px(0.));
        assert_eq!(panel.size.width, px(288.));
        assert_eq!(window.find("content").bounds().size.width, px(400.));
        assert!(window.find("inbox").visible());
        assert!(window.try_find("sidebar-resize-handle").is_none());
        window.click("inbox", cx);
        assert_eq!(clicks.get(), 1, "rows in the sheet click");
        window.render_frame(cx);
        assert!(
            window.try_find("inbox").is_some(),
            "a row click keeps the sheet open"
        );
        // A click outside the sheet closes it.
        window.click_at("sidebar-scrim", point(px(380.), px(400.)), cx);
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("inbox").is_none());
        assert_eq!(window.find("content").bounds().size.width, px(400.));
    })
    .unwrap();
    assert!(!sidebar.read_with(cx, |state, _| state.is_open()));
}

#[gpui_kit::test]
fn a_group_folds_with_motion_unless_motion_is_reduced(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    let inbox_mounted = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("inbox").is_some()
        })
        .unwrap()
    };
    assert!(inbox_mounted(cx));
    parts.update(cx, |parts, cx| {
        parts.group_open = false;
        cx.notify();
    });
    // While the group folds the rows stay in the tree, clipped to the
    // fold; once it rests they leave it, so a folded row takes no focus.
    cx.executor().advance_clock(Duration::from_millis(30));
    assert!(inbox_mounted(cx), "still mounted mid-fold");
    cx.executor().advance_clock(Duration::from_millis(500));
    assert!(!inbox_mounted(cx), "folded away, and out of the tree");
    // Reduced motion folds at once.
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On));
    parts.update(cx, |parts, cx| {
        parts.group_open = true;
        cx.notify();
    });
    assert!(inbox_mounted(cx));
    parts.update(cx, |parts, cx| {
        parts.group_open = false;
        cx.notify();
    });
    assert!(!inbox_mounted(cx), "gone at once under reduced motion");
}

#[gpui_kit::test]
fn the_nested_rows_guide_line_is_optional(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    let nested_left = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("nested").bounds().origin.x
        })
        .unwrap()
    };
    let with_guide = nested_left(cx);
    parts.update(cx, |parts, cx| {
        parts.guide = false;
        cx.notify();
    });
    // The hairline is one pixel; without it the rows move left by that.
    assert_eq!(nested_left(cx), with_guide - px(1.));
}

#[gpui_kit::test]
fn nested_rows_keep_their_indent_while_they_unfold(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    let nested_left = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.try_find("nested").map(|row| row.bounds().origin.x)
        })
        .unwrap()
    };
    let at_rest = nested_left(cx).expect("open at rest");
    parts.update(cx, |parts, cx| {
        parts.row_open = false;
        cx.notify();
    });
    cx.executor().advance_clock(Duration::from_millis(500));
    assert!(nested_left(cx).is_none(), "folded");
    parts.update(cx, |parts, cx| {
        parts.row_open = true;
        cx.notify();
    });
    cx.executor().advance_clock(Duration::from_millis(30));
    // Mid-unfold the rows sit where they will rest, not at the left edge.
    assert_eq!(nested_left(cx), Some(at_rest));
    cx.executor().advance_clock(Duration::from_millis(500));
    assert_eq!(nested_left(cx), Some(at_rest));
}

#[gpui_kit::test]
fn rows_in_a_group_take_the_sidebar_width_at_rest_and_mid_fold(cx: &mut TestAppContext) {
    let (handle, _, parts) = setup_parts(cx, gpui_cn::SidebarCollapsible::Offcanvas);
    let widths = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("sidebar-content").bounds().size.width,
                window.find("inbox").bounds().size.width,
                window.find("nested").bounds().size.width,
            )
        })
        .unwrap()
    };
    let (content, inbox, nested) = widths(cx);
    // The content pads 8px on each side; a nested row sits 16px further
    // in, past a 6px gutter and the 1px guide.
    assert_eq!(inbox, content - px(16.));
    assert_eq!(nested, content - px(16.) - px(16.) - px(7.));
    cx.update(|cx| Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::Off));
    parts.update(cx, |parts, cx| {
        parts.group_open = false;
        cx.notify();
    });
    cx.executor().advance_clock(Duration::from_millis(30));
    let (_, inbox_folding, nested_folding) = widths(cx);
    assert_eq!(inbox_folding, inbox, "the same width while folding");
    assert_eq!(nested_folding, nested);
}

struct ActionHarness {
    menu: gpui_kit::Entity<gpui_cn::MenuState>,
    open: Rc<std::cell::Cell<bool>>,
    clicks: Rc<std::cell::Cell<usize>>,
}

impl Render for ActionHarness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        let clicks = self.clicks.clone();
        let menu_open = self.menu.read(cx).is_open();
        div().size_full().p_4().child(
            gpui_cn::SidebarMenuButton::new("row")
                .label("Projects")
                .collapsible(open.get())
                .on_toggle(move |value, _, _| open.set(*value))
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1))
                .action(
                    gpui_cn::DropdownMenu::new("row-menu", &self.menu)
                        .trigger(gpui_cn::Button::new("row-more").ghost().label("More"))
                        .items(|_, _| vec![gpui_cn::MenuItem::new("rename", "Rename").into()]),
                )
                .show_action(menu_open),
        )
    }
}

#[gpui_kit::test]
fn a_press_on_the_action_opens_its_menu_and_leaves_the_row_alone(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let open = Rc::new(std::cell::Cell::new(true));
    let clicks = Rc::new(std::cell::Cell::new(0));
    let mut menu = None;
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let state = cx.new(gpui_cn::MenuState::new);
        menu = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            ActionHarness {
                menu: state,
                open: open.clone(),
                clicks: clicks.clone(),
            }
        });
        gpui_kit::base::Root::new(harness, window, cx)
    });
    let menu = menu.unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("row-more", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        menu.read_with(cx, |menu, _| menu.is_open()),
        "the menu opened"
    );
    assert!(open.get(), "the row did not fold");
    assert_eq!(clicks.get(), 0, "the row did not take the press");
}

struct PlainActionHarness {
    open: Rc<std::cell::Cell<bool>>,
    action_clicks: Rc<std::cell::Cell<usize>>,
}

impl Render for PlainActionHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        let action_clicks = self.action_clicks.clone();
        div().size_full().p_4().child(
            gpui_cn::SidebarMenuButton::new("plain-row")
                .label("Projects")
                .collapsible(open.get())
                .on_toggle(move |value, _, _| open.set(*value))
                .action(
                    gpui_cn::Button::new("plain-more")
                        .ghost()
                        .label("More")
                        .on_click(move |_, _, _| action_clicks.set(action_clicks.get() + 1)),
                )
                .show_action(true),
        )
    }
}

#[gpui_kit::test]
fn a_press_on_a_plain_action_does_not_fold_the_row(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let open = Rc::new(std::cell::Cell::new(true));
    let action_clicks = Rc::new(std::cell::Cell::new(0));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|_| PlainActionHarness {
            open: open.clone(),
            action_clicks: action_clicks.clone(),
        });
        gpui_kit::base::Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("plain-more", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(action_clicks.get(), 1, "the action ran");
    assert!(open.get(), "the row did not fold");
}

struct SheetHarness {
    sidebar: Entity<SidebarState>,
}

impl Render for SheetHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            SidebarLayout::new(&self.sidebar)
                .sidebar(
                    Sidebar::new().child(
                        SidebarGroup::new()
                            .child(SidebarMenuButton::new("inbox").label("Inbox"))
                            .child(SidebarMenuButton::new("drafts").label("Drafts")),
                    ),
                )
                .child(
                    div()
                        .id("content")
                        .test_support()
                        .size_full()
                        .child(gpui_cn::Button::new("outside").label("Outside")),
                ),
        )
    }
}

fn sheet_setup(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<SidebarState>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let sidebar = cx.new(|cx| SidebarState::new(cx));
    let handle = cx.open_window(size(px(400.), px(800.)), |window, cx| {
        let harness = cx.new(|_| SheetHarness {
            sidebar: sidebar.clone(),
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    (handle, sidebar)
}

fn frames(handle: WindowHandle<Root>, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn in_sheet(handle: WindowHandle<Root>, cx: &mut TestAppContext) -> bool {
    cx.update_window(handle.into(), |_, window, _| {
        window.find("sheet").focused() == Some(true)
    })
    .unwrap()
}

fn outside_focused(handle: WindowHandle<Root>, cx: &mut TestAppContext) -> bool {
    cx.update_window(handle.into(), |_, window, _| {
        window.find("outside").focused() == Some(true)
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_sheet_takes_focus_when_it_opens_and_gives_it_back_when_it_closes(cx: &mut TestAppContext) {
    let (handle, sidebar) = sheet_setup(cx);
    cx.update_window(handle.into(), |_, window, cx| window.press("tab", cx))
        .unwrap();
    frames(handle, cx);
    assert!(outside_focused(handle, cx), "the page had focus");
    sidebar.update(cx, |state, cx| state.set_open(true, cx));
    frames(handle, cx);
    assert!(in_sheet(handle, cx), "focus moved into the sheet");
    assert!(!outside_focused(handle, cx));
    cx.update_window(handle.into(), |_, window, cx| window.press("escape", cx))
        .unwrap();
    frames(handle, cx);
    assert!(
        !sidebar.read_with(cx, |state, _| state.is_open()),
        "Escape closes"
    );
    cx.update_window(handle.into(), |_, window, _| {
        assert!(window.try_find("sheet").is_none());
    })
    .unwrap();
    assert!(outside_focused(handle, cx), "focus went back to the page");
}

#[gpui_kit::test]
fn tab_keeps_focus_inside_the_open_sheet(cx: &mut TestAppContext) {
    let (handle, sidebar) = sheet_setup(cx);
    sidebar.update(cx, |state, cx| state.set_open(true, cx));
    frames(handle, cx);
    for _ in 0..6 {
        cx.update_window(handle.into(), |_, window, cx| window.press("tab", cx))
            .unwrap();
        frames(handle, cx);
        assert!(in_sheet(handle, cx), "still in the sheet");
        assert!(!outside_focused(handle, cx), "never on the page");
    }
    cx.update_window(handle.into(), |_, window, cx| window.press("shift-tab", cx))
        .unwrap();
    frames(handle, cx);
    assert!(in_sheet(handle, cx));
}

struct BoxedHarness {
    sidebar: Entity<SidebarState>,
    side: gpui_cn::SidebarSide,
}

impl Render for BoxedHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(60.)).w_full())
            .child(
                div().flex_1().min_h_0().p(px(20.)).child(
                    SidebarLayout::new(&self.sidebar)
                        .side(self.side)
                        .sidebar(
                            Sidebar::new().child(
                                SidebarGroup::new()
                                    .child(SidebarMenuButton::new("inbox").label("Inbox")),
                            ),
                        )
                        .child(div().id("content").test_support().size_full()),
                ),
            )
    }
}

#[gpui_kit::test]
fn the_sheet_sits_on_the_layout_box_not_the_window(cx: &mut TestAppContext) {
    for side in [gpui_cn::SidebarSide::Left, gpui_cn::SidebarSide::Right] {
        cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_cn::init(cx);
            Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
        });
        let sidebar = cx.new(|cx| SidebarState::new(cx));
        let handle = cx.open_window(size(px(400.), px(800.)), |window, cx| {
            let harness = cx.new(|_| BoxedHarness {
                sidebar: sidebar.clone(),
                side,
            });
            Root::new(harness, window, cx)
        });
        frames(handle, cx);
        sidebar.update(cx, |state, cx| state.set_open(true, cx));
        frames(handle, cx);
        frames(handle, cx);
        cx.update_window(handle.into(), |_, window, _| {
            let layout = window.find("sidebar-inset").bounds();
            let panel = window.find("sidebar-panel").bounds();
            let scrim = window.find("sidebar-scrim").bounds();
            assert!(layout.top() >= px(60.), "the layout is under the header");
            assert!(
                panel.top() >= layout.top() && panel.bottom() <= layout.bottom(),
                "{side:?}: {panel:?} in {layout:?}"
            );
            assert!(
                panel.left() >= layout.left() && panel.right() <= layout.right(),
                "{side:?}: {panel:?} in {layout:?}"
            );
            assert_eq!(
                scrim, layout,
                "{side:?}: the scrim covers the layout box only"
            );
        })
        .unwrap();
    }
}
