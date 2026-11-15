//! UI integration tests for `DockSkin`: panes dragged by their grab
//! handles through a headless window.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_cn::dock::{
    DockArea, DockLayout, DockPlacement, PaneNode, PaneRef, Panel, PanelEvent, PanelId,
};
use gpui_cn::{DockSkin, ReduceMotion, Theme};
use gpui_kit::{
    App, AppContext as _, Axis, Bounds, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, Styled as _, TestAppContext, Window,
    WindowHandle,
    base::{Root, TestSupportExt as _},
    div, point, px, size,
    test::TestWindowExt as _,
};

/// A plain pane that counts how often the dock told it it left.
struct Pane {
    name: &'static str,
    focus: FocusHandle,
    removed: Rc<Cell<usize>>,
    pressed: Rc<Cell<usize>>,
}

impl Panel for Pane {
    fn panel_name(&self) -> &'static str {
        "pane"
    }

    fn on_removed(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.removed.set(self.removed.get() + 1);
    }
}

impl EventEmitter<PanelEvent> for Pane {}

impl Focusable for Pane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(ElementId::Name(format!("pane-{}", self.name).into()))
            .test_support()
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, {
                let pressed = self.pressed.clone();
                move |_, _, _| pressed.set(pressed.get() + 1)
            })
            .size_full()
            .child(self.name)
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    area: Entity<DockArea>,
    a: Entity<Pane>,
    b: Entity<Pane>,
    removed: Rc<Cell<usize>>,
    pressed: Rc<Cell<usize>>,
}

/// An 800x400 window holding one dock: pane `a` left of pane `b`.
fn setup(cx: &mut TestAppContext, motion: bool, touch: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| {
            theme.reduce_motion = if motion {
                ReduceMotion::Off
            } else {
                ReduceMotion::On
            };
            theme.touch = touch;
        });
    });
    let removed = Rc::new(Cell::new(0));
    let pressed = Rc::new(Cell::new(0));
    let mut made = None;
    let handle = cx.open_window(size(px(800.), px(400.)), |window, cx| {
        let pane = |name, cx: &mut App| {
            let removed = removed.clone();
            let pressed = pressed.clone();
            cx.new(|cx| Pane {
                name,
                focus: cx.focus_handle(),
                removed,
                pressed,
            })
        };
        let a = pane("a", cx);
        let b = pane("b", cx);
        let area = DockSkin::area("dock", window, cx);
        area.update(cx, |area, cx| {
            area.set_center(
                DockLayout::h_split()
                    .child(DockLayout::tabs().panel(a.clone()), None)
                    .child(DockLayout::tabs().panel(b.clone()), None),
                window,
                cx,
            );
        });
        made = Some((area.clone(), a, b));
        Root::new(area, window, cx)
    });
    let (area, a, b) = made.expect("the window was built");
    let setup = Setup {
        handle,
        area,
        a,
        b,
        removed,
        pressed,
    };
    frames(&setup, cx, 2);
    setup
}

fn frames(setup: &Setup, cx: &mut TestAppContext, count: usize) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        for _ in 0..count {
            window.render_frame(cx);
        }
    })
    .unwrap();
}

fn id(panel: &Entity<Pane>) -> PanelId {
    PanelId::from(panel.entity_id())
}

fn grab(panel: &Entity<Pane>) -> ElementId {
    ElementId::NamedInteger("dock-grab".into(), id(panel).as_u64())
}

fn pane_id(name: &str) -> ElementId {
    ElementId::Name(format!("pane-{name}").into())
}

fn bounds(setup: &Setup, cx: &mut TestAppContext, element: ElementId) -> Bounds<Pixels> {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.find(element).bounds()
    })
    .unwrap()
}

fn present(setup: &Setup, cx: &mut TestAppContext, element: ElementId) -> bool {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.try_find(element).is_some()
    })
    .unwrap()
}

fn mouse(setup: &Setup, cx: &mut TestAppContext, event: impl gpui_kit::InputEvent) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.dispatch_event(event.to_platform_input(), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn move_to(setup: &Setup, cx: &mut TestAppContext, position: Point<Pixels>, held: bool) {
    mouse(
        setup,
        cx,
        MouseMoveEvent {
            position,
            pressed_button: held.then_some(MouseButton::Left),
            modifiers: Default::default(),
        },
    );
}

fn down(setup: &Setup, cx: &mut TestAppContext, position: Point<Pixels>) {
    mouse(
        setup,
        cx,
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        },
    );
}

fn up(setup: &Setup, cx: &mut TestAppContext, position: Point<Pixels>) {
    mouse(
        setup,
        cx,
        MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Default::default(),
            click_count: 1,
        },
    );
}

/// Lets `millis` pass: timers fire and a frame is drawn.
fn wait(setup: &Setup, cx: &mut TestAppContext, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    cx.run_until_parked();
    frames(setup, cx, 2);
}

/// Presses `a`'s grab handle, after the pointer revealed it, and returns
/// where the press was.
fn press_a(setup: &Setup, cx: &mut TestAppContext) -> Point<Pixels> {
    let pane = bounds(setup, cx, pane_id("a"));
    move_to(setup, cx, pane.origin + point(px(20.), px(5.)), false);
    let handle = bounds(setup, cx, grab(&setup.a)).center();
    move_to(setup, cx, handle, false);
    down(setup, cx, handle);
    handle
}

/// Drags `a` by its handle and rests over `to` past the retarget delay.
fn lift_a_to(setup: &Setup, cx: &mut TestAppContext, to: Point<Pixels>) {
    let start = press_a(setup, cx);
    move_to(setup, cx, start + point(px(3.), px(0.)), true);
    move_to(setup, cx, start + point(px(12.), px(0.)), true);
    move_to(setup, cx, to, true);
    move_to(setup, cx, to + point(px(1.), px(0.)), true);
    wait(setup, cx, 200);
}

/// The center layout as text: `h[..]` and `v[..]` for splits, a group as
/// its panes joined by `+`.
fn shape(setup: &Setup, cx: &mut TestAppContext) -> String {
    let names = [(id(&setup.a), "a"), (id(&setup.b), "b")];
    setup.area.read_with(cx, |area, _| {
        fn walk(node: &PaneNode, names: &[(PanelId, &str)]) -> String {
            match node.kind() {
                PaneRef::Split { axis, children, .. } => {
                    let parts: Vec<String> = children.iter().map(|c| walk(c, names)).collect();
                    let axis = if axis == Axis::Horizontal { "h" } else { "v" };
                    format!("{axis}[{}]", parts.join(","))
                }
                PaneRef::Tabs { panels, .. } => panels
                    .iter()
                    .map(|p| {
                        names
                            .iter()
                            .find(|(id, _)| id == p)
                            .map_or("?", |(_, name)| *name)
                    })
                    .collect::<Vec<_>>()
                    .join("+"),
            }
        }
        walk(
            area.layout(DockPlacement::Center).expect("a center").root(),
            &names,
        )
    })
}

fn focused(setup: &Setup, cx: &mut TestAppContext, pane: &Entity<Pane>) -> bool {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        pane.read(cx).focus.is_focused(window)
    })
    .unwrap()
}

fn focus(setup: &Setup, cx: &mut TestAppContext, pane: &Entity<Pane>) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let handle = pane.read(cx).focus.clone();
        window.focus(&handle, cx);
    })
    .unwrap();
    frames(setup, cx, 1);
}

fn dim(panel: &Entity<Pane>) -> ElementId {
    ElementId::NamedInteger("dock-dim".into(), id(panel).as_u64())
}

#[gpui_kit::test]
fn a_press_on_the_handle_does_not_reach_the_pane_under_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let handle = press_a(&setup, cx);
    up(&setup, cx, handle);
    assert_eq!(setup.pressed.get(), 0, "a terminal would start a selection");
    let pane = bounds(&setup, cx, pane_id("a")).center();
    down(&setup, cx, pane);
    up(&setup, cx, pane);
    assert_eq!(setup.pressed.get(), 1);
}

#[gpui_kit::test]
fn panes_without_focus_are_dimmed_and_focus_resizes_nothing(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    focus(&setup, cx, &setup.a);
    let a = bounds(&setup, cx, pane_id("a"));
    assert!(!present(&setup, cx, dim(&setup.a)));
    assert!(present(&setup, cx, dim(&setup.b)));
    assert_eq!(
        bounds(&setup, cx, dim(&setup.b)),
        bounds(&setup, cx, pane_id("b")),
        "the fill covers the whole pane"
    );
    focus(&setup, cx, &setup.b);
    assert!(present(&setup, cx, dim(&setup.a)));
    assert!(!present(&setup, cx, dim(&setup.b)));
    assert_eq!(bounds(&setup, cx, pane_id("a")), a);
}

#[gpui_kit::test]
fn a_pane_dropped_on_the_right_edge_of_another_lands_right_of_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    assert_eq!(shape(&setup, cx), "h[a,b]");
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.right() - px(10.), b.center().y);
    lift_a_to(&setup, cx, to);
    assert!(present(&setup, cx, ElementId::Name("dock-card".into())));
    assert_eq!(shape(&setup, cx), "h[b,a]", "the preview after the delay");
    up(&setup, cx, to + point(px(1.), px(0.)));
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[b,a]");
    assert!(!present(&setup, cx, ElementId::Name("dock-card".into())));
    assert!(focused(&setup, cx, &setup.a));
    assert_eq!(setup.removed.get(), 0);
    assert_eq!(
        bounds(&setup, cx, pane_id("a")),
        Bounds::new(point(px(400.), px(0.)), size(px(400.), px(400.)))
    );
}

#[gpui_kit::test]
fn a_drop_near_the_center_takes_the_nearest_edge_and_never_merges(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let b = bounds(&setup, cx, pane_id("b"));
    lift_a_to(&setup, cx, point(b.right() - px(10.), b.center().y));
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.center().x, b.center().y + px(30.));
    move_to(&setup, cx, to, true);
    wait(&setup, cx, 200);
    up(&setup, cx, to);
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[v[b,a]]", "below b, not in b's group");
}

#[gpui_kit::test]
fn a_lifted_pane_closes_its_slot_and_its_neighbor_fills_it(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let start = press_a(&setup, cx);
    move_to(&setup, cx, start + point(px(3.), px(0.)), true);
    move_to(&setup, cx, start + point(px(12.), px(0.)), true);
    assert_eq!(shape(&setup, cx), "h[b+a]", "a waits as a hidden tab of b");
    assert_eq!(
        bounds(&setup, cx, pane_id("b")),
        Bounds::new(point(px(0.), px(0.)), size(px(800.), px(400.)))
    );
    assert!(!present(
        &setup,
        cx,
        ElementId::NamedInteger("dock-tab".into(), id(&setup.a).as_u64())
    ));
}

#[gpui_kit::test]
fn a_drop_on_its_own_slot_changes_nothing(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let a = bounds(&setup, cx, pane_id("a"));
    let to = a.center();
    lift_a_to(&setup, cx, to);
    up(&setup, cx, to);
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[a,b]");
    assert_eq!(bounds(&setup, cx, pane_id("a")), a);
}

#[gpui_kit::test]
fn escape_mid_drag_puts_back_the_layout_and_the_panes(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let a_before = bounds(&setup, cx, pane_id("a"));
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.right() - px(10.), b.center().y);
    lift_a_to(&setup, cx, to);
    assert_eq!(shape(&setup, cx), "h[b,a]");
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[a,b]");
    up(&setup, cx, to);
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[a,b]");
    assert_eq!(setup.removed.get(), 0, "no pane left the dock");
    setup.area.read_with(cx, |area, _| {
        let a = area.panel(id(&setup.a)).expect("a stays");
        assert_eq!(a.view().entity_id(), setup.a.entity_id());
        let b = area.panel(id(&setup.b)).expect("b stays");
        assert_eq!(b.view().entity_id(), setup.b.entity_id());
    });
    assert_eq!(bounds(&setup, cx, pane_id("a")), a_before);
}

#[gpui_kit::test]
fn a_move_under_the_threshold_lifts_nothing(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let start = press_a(&setup, cx);
    move_to(&setup, cx, start + point(px(3.), px(0.)), true);
    move_to(&setup, cx, start + point(px(5.), px(0.)), true);
    wait(&setup, cx, 200);
    assert!(!present(&setup, cx, ElementId::Name("dock-card".into())));
    up(&setup, cx, start + point(px(5.), px(0.)));
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[a,b]");
}

#[gpui_kit::test]
fn a_dropped_pane_glides_into_its_slot(cx: &mut TestAppContext) {
    let setup = setup(cx, true, false);
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.right() - px(10.), b.center().y);
    lift_a_to(&setup, cx, to);
    wait(&setup, cx, 600);
    let card = bounds(&setup, cx, ElementId::Name("dock-card".into()));
    assert_eq!(
        card,
        Bounds::new(point(px(601.), px(196.)), size(px(380.), px(260.)))
    );
    up(&setup, cx, to);
    wait(&setup, cx, 60);
    assert_eq!(
        bounds(&setup, cx, pane_id("a")),
        Bounds::new(point(px(500.), px(97.5)), size(px(400.), px(400.))),
        "60ms of 460ms on ease-out quint is 0.5025 of the way from the card to the slot"
    );
    wait(&setup, cx, 400);
    let slot = Bounds::new(point(px(400.), px(0.)), size(px(400.), px(400.)));
    assert_eq!(bounds(&setup, cx, pane_id("a")), slot);
    wait(&setup, cx, 100);
    assert_eq!(bounds(&setup, cx, pane_id("a")), slot, "at rest it stays");
}

#[gpui_kit::test]
fn with_reduced_motion_a_drop_lands_in_one_frame(cx: &mut TestAppContext) {
    let setup = setup(cx, false, false);
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.right() - px(10.), b.center().y);
    lift_a_to(&setup, cx, to);
    up(&setup, cx, to);
    let landed = bounds(&setup, cx, pane_id("a"));
    wait(&setup, cx, 1000);
    assert_eq!(bounds(&setup, cx, pane_id("a")), landed);
}

#[gpui_kit::test]
fn on_touch_the_handle_shows_without_a_hover_and_drags(cx: &mut TestAppContext) {
    let setup = setup(cx, false, true);
    let handle = bounds(&setup, cx, grab(&setup.a));
    assert_eq!(handle.size, size(px(80.), px(44.)));
    assert!(present(
        &setup,
        cx,
        ElementId::NamedInteger("dock-grab-icon".into(), id(&setup.a).as_u64())
    ));
    let start = handle.center();
    down(&setup, cx, start);
    let b = bounds(&setup, cx, pane_id("b"));
    let to = point(b.right() - px(10.), b.center().y);
    move_to(&setup, cx, start + point(px(12.), px(0.)), true);
    move_to(&setup, cx, to, true);
    up(&setup, cx, to);
    wait(&setup, cx, 10);
    assert_eq!(shape(&setup, cx), "h[b,a]");
}
