//! The drag session of a skinned dock: which pane is lifted, where it
//! would land, the layout to go back to, and where each pane is painted.

use std::{collections::HashMap, sync::Arc, time::Instant};

use gpui_kit::{
    App, Axis, Bounds, Context, Pixels, Point, Task, WeakEntity, Window,
    base::{
        Interpolate as _, Placement,
        dock::{
            DockArea, DockLayout, DockPlacement, InsertTarget, NodeId, PaneNode, PaneRef, PaneTree,
            PanelId, PanelView,
        },
    },
    size,
};

use super::motion::{MOVING, SETTLING, Track};
use crate::Theme;

/// Where a lifted pane would land: beside the group `node`, on the side
/// `placement`. There is no center: a pane is never merged into a tab
/// group by a drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target {
    node: NodeId,
    placement: Placement,
}

/// What a lifted pane carries: itself, the layout it came from, and the
/// card it is drawn as.
#[derive(Clone)]
pub(crate) struct Lift {
    panel: PanelId,
    view: Arc<dyn PanelView>,
    /// The layout when the pane lifted, put back on a cancel.
    origin: PaneTree,
    /// The pane's slot when it lifted. The card keeps this size for its
    /// content, so a terminal inside does not resize while it is dragged.
    slot: Bounds<Pixels>,
    /// Where in the slot the pointer took hold.
    grab: Point<Pixels>,
    pointer: Point<Pixels>,
    lifted_at: Instant,
}

/// The card a lifted pane floats as.
pub(crate) struct Card {
    pub(crate) view: Arc<dyn PanelView>,
    /// Where the card is this frame.
    pub(crate) rect: Bounds<Pixels>,
    /// The pane's slot when it lifted, the size its content keeps.
    pub(crate) content: Bounds<Pixels>,
    /// Whether the card is still shrinking from the slot.
    pub(crate) growing: bool,
}

/// One pane drag, from the press on its grab handle to the moment it rests.
#[derive(Clone, Default)]
pub(crate) enum PaneDrag {
    #[default]
    Idle,
    /// The pointer went down on a grab handle and has not yet moved past
    /// the drag threshold.
    Pressed {
        panel: PanelId,
        grab: Point<Pixels>,
        start: Point<Pixels>,
    },
    /// The pane is a floating card and its slot has closed: it waits as a
    /// hidden tab of its nearest neighbor, which grows into the room.
    Lifted { lift: Lift, pending: Option<Target> },
    /// The layout shows the pane at `target`, where a drop would leave it.
    Previewing {
        lift: Lift,
        target: Target,
        pending: Option<Target>,
    },
    /// Dropped or cancelled: the pane flies into its slot.
    Settling { panel: PanelId },
}

/// What the pointer is over while a pane is lifted.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hover {
    /// No pane: a divider, or outside the dock.
    Nothing,
    /// The lifted pane's own slot.
    Own,
    Target(Target),
}

/// The state a [`DockSkin`](super::DockSkin) shares between the area, its
/// groups, and their panes.
pub(crate) struct DockSkinState {
    area: WeakEntity<DockArea>,
    drag: PaneDrag,
    /// The one retarget timer. Replacing it cancels the one before, and
    /// nothing runs while no pane is lifted.
    debounce: Option<Task<()>>,
    settle: Option<Task<()>>,
    /// The area's bounds, the clip of a moving pane.
    frame: Bounds<Pixels>,
    /// Each group's content bounds, the drop hit areas.
    groups: HashMap<NodeId, Bounds<Pixels>>,
    tracks: HashMap<PanelId, Track>,
    /// The pane whose grab handle the pointer reveals.
    revealed: Option<PanelId>,
    /// The pane whose grab handle the pointer is on.
    hovered: Option<PanelId>,
    /// Whether the center holds more than one group.
    split: bool,
}

impl DockSkinState {
    pub(crate) fn new(area: WeakEntity<DockArea>) -> Self {
        Self {
            area,
            drag: PaneDrag::Idle,
            debounce: None,
            settle: None,
            frame: Bounds::default(),
            groups: HashMap::new(),
            tracks: HashMap::new(),
            revealed: None,
            hovered: None,
            split: false,
        }
    }

    /// Whether a drag owns the pointer: from the press until the drop.
    pub(crate) fn is_dragging(&self) -> bool {
        matches!(
            self.drag,
            PaneDrag::Pressed { .. } | PaneDrag::Lifted { .. } | PaneDrag::Previewing { .. }
        )
    }

    /// The pane drawn as a card instead of in its slot.
    pub(crate) fn lifted(&self) -> Option<PanelId> {
        self.lift().map(|lift| lift.panel)
    }

    pub(crate) fn split(&self) -> bool {
        self.split
    }

    pub(crate) fn revealed(&self) -> Option<PanelId> {
        self.revealed
    }

    pub(crate) fn hovered(&self) -> Option<PanelId> {
        self.hovered
    }

    fn lift(&self) -> Option<&Lift> {
        match &self.drag {
            PaneDrag::Lifted { lift, .. } | PaneDrag::Previewing { lift, .. } => Some(lift),
            _ => None,
        }
    }

    pub(crate) fn frame(&self) -> Bounds<Pixels> {
        self.frame
    }

    pub(crate) fn record_frame(&mut self, bounds: Bounds<Pixels>) {
        self.frame = bounds;
    }

    pub(crate) fn record_group(&mut self, node: NodeId, bounds: Bounds<Pixels>) {
        self.groups.insert(node, bounds);
    }

    /// Where pane `panel`, laid out at `slot`, paints this frame: the rect,
    /// whether it moves, its paint order while it does, and its clip.
    pub(crate) fn place(
        &mut self,
        panel: PanelId,
        slot: Bounds<Pixels>,
        cx: &App,
    ) -> (Bounds<Pixels>, bool, usize, Bounds<Pixels>) {
        let now = cx.background_executor().now();
        let motion = &Theme::global(cx).motion;
        let (duration, curve) = (motion.layout, motion.layout_curve.clone());
        let track = self
            .tracks
            .entry(panel)
            .or_insert_with(|| Track::still(slot, now));
        if cx.reduce_motion() {
            *track = Track::still(slot, now);
        } else {
            track.retarget(slot, now, duration, &curve);
        }
        let (painted, moving) = track.sample(now, duration, &curve);
        let priority = match self.drag {
            PaneDrag::Settling { panel: settling } if settling == panel => SETTLING,
            _ => MOVING,
        };
        (painted, moving, priority, self.frame)
    }

    /// The card the lifted pane is drawn as this frame.
    pub(crate) fn card(&self, cx: &App) -> Option<Card> {
        let lift = self.lift()?;
        let (rect, growing) = card_rect(lift, cx);
        Some(Card {
            view: lift.view.clone(),
            rect,
            content: lift.slot,
            growing,
        })
    }

    /// Shows or hides the grab handle of `panel` as the pointer enters or
    /// leaves the top band of its pane.
    pub(crate) fn reveal(&mut self, panel: PanelId, inside: bool, cx: &mut Context<Self>) {
        if toggle(&mut self.revealed, panel, inside) {
            cx.notify();
        }
    }

    pub(crate) fn hover_handle(&mut self, panel: PanelId, hovered: bool, cx: &mut Context<Self>) {
        if toggle(&mut self.hovered, panel, hovered) {
            cx.notify();
        }
    }

    /// Follows a layout change: whether the dock is split, and which panes
    /// still have motion to keep.
    pub(crate) fn layout_changed(&mut self, tree: &PaneTree, cx: &mut Context<Self>) {
        let mut groups = 0;
        tree.root().walk(&mut |node| {
            if matches!(node.kind(), PaneRef::Tabs { .. }) {
                groups += 1;
            }
        });
        let split = groups > 1;
        if split != self.split {
            self.split = split;
            cx.notify();
        }
        self.tracks.retain(|panel, _| tree.contains_panel(*panel));
        self.groups
            .retain(|node, _| tree.find_node(*node).is_some());
    }

    /// The pointer went down on the grab handle of `panel`.
    pub(crate) fn press(
        &mut self,
        panel: PanelId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.is_dragging() {
            return;
        }
        let Some(slot) = self.tracks.get(&panel).map(Track::target) else {
            return;
        };
        self.settle = None;
        self.drag = PaneDrag::Pressed {
            panel,
            grab: position - slot.origin,
            start: position,
        };
        cx.notify();
    }

    /// The pointer moved while a drag holds it.
    pub(crate) fn moved(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &mut self.drag {
            PaneDrag::Pressed { panel, grab, start } => {
                let threshold = f32::from(Theme::global(cx).tokens().metrics.dock_drag_threshold);
                if (position - *start).magnitude() > f64::from(threshold) {
                    let (panel, grab) = (*panel, *grab);
                    self.lift_pane(panel, grab, position, window, cx);
                }
            }
            PaneDrag::Lifted { lift, .. } | PaneDrag::Previewing { lift, .. } => {
                lift.pointer = position;
                self.retarget(position, window, cx);
                cx.notify();
            }
            PaneDrag::Idle | PaneDrag::Settling { .. } => {}
        }
    }

    fn lift_pane(
        &mut self,
        panel: PanelId,
        grab: Point<Pixels>,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(area) = self.area.upgrade() else {
            return;
        };
        let (origin, view) = {
            let area = area.read(cx);
            (
                area.layout(DockPlacement::Center).cloned(),
                area.panel(panel).cloned(),
            )
        };
        let (Some(origin), Some(view), Some(track)) = (origin, view, self.tracks.get(&panel))
        else {
            self.drag = PaneDrag::Idle;
            return;
        };
        let neighbor = neighbor_group(origin.root(), panel);
        self.drag = PaneDrag::Lifted {
            lift: Lift {
                panel,
                view: view.clone(),
                origin,
                slot: track.target(),
                grab,
                pointer,
                lifted_at: cx.background_executor().now(),
            },
            pending: None,
        };
        if let Some(node) = neighbor {
            area.update(cx, |area, cx| {
                area.move_panel(
                    panel,
                    InsertTarget::Tabs {
                        node,
                        ix: None,
                        activate: false,
                    },
                    window,
                    cx,
                );
            });
        }
        view.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    /// What the pointer at `position` is over.
    fn hover_at(&self, position: Point<Pixels>, cx: &App) -> Hover {
        let (Some(lift), Some(area)) = (self.lift(), self.area.upgrade()) else {
            return Hover::Nothing;
        };
        let area = area.read(cx);
        let Some(tree) = area.layout(DockPlacement::Center) else {
            return Hover::Nothing;
        };
        // While the pane waits in a neighbor's group, that group is a
        // target; only a group holding nothing else is its own slot.
        let own = tree.find_panel_node(lift.panel).filter(|node| {
            tree.find_node(*node).is_some_and(
                |found| matches!(found.kind(), PaneRef::Tabs { panels, .. } if panels.len() == 1),
            )
        });
        let hit = self.groups.iter().find(|(node, bounds)| {
            bounds.contains(&position)
                && tree
                    .find_node(**node)
                    .is_some_and(|found| matches!(found.kind(), PaneRef::Tabs { .. }))
        });
        match hit {
            None => Hover::Nothing,
            Some((node, _)) if Some(*node) == own => Hover::Own,
            Some((node, bounds)) => Hover::Target(Target {
                node: *node,
                placement: nearest_edge(*bounds, position),
            }),
        }
    }

    /// Waits out the retarget delay on a new target, or forgets a pending
    /// one the pointer left.
    fn retarget(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let hover = self.hover_at(position, cx);
        let (applied, pending) = match &mut self.drag {
            PaneDrag::Lifted { pending, .. } => (None, pending),
            PaneDrag::Previewing {
                target, pending, ..
            } => (Some(*target), pending),
            _ => return,
        };
        let wanted = match hover {
            Hover::Target(target) if Some(target) != applied => Some(target),
            _ => None,
        };
        if *pending == wanted {
            return;
        }
        *pending = wanted;
        self.debounce = wanted.map(|_| {
            let delay = Theme::global(cx).motion.retarget;
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(delay).await;
                _ = this.update_in(cx, |this, window, cx| {
                    // The timer is this task, so let it finish.
                    if let Some(task) = this.debounce.take() {
                        task.detach();
                    }
                    this.apply_pending(window, cx);
                });
            })
        });
    }

    fn apply_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = match &self.drag {
            PaneDrag::Lifted { pending, .. } | PaneDrag::Previewing { pending, .. } => *pending,
            _ => None,
        };
        if let Some(target) = pending {
            self.apply(target, window, cx);
        }
    }

    /// Shows the lifted pane at `target`.
    fn apply(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lift) = self.lift().cloned() else {
            return;
        };
        let Some(area) = self.area.upgrade() else {
            return;
        };
        let live = area
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|tree| tree.find_node(target.node))
            .is_some();
        if live {
            area.update(cx, |area, cx| {
                area.move_panel(
                    lift.panel,
                    InsertTarget::Split {
                        node: target.node,
                        placement: target.placement,
                        size: None,
                    },
                    window,
                    cx,
                );
            });
        }
        self.debounce = None;
        self.drag = PaneDrag::Previewing {
            lift,
            target,
            pending: None,
        };
        cx.notify();
    }

    /// The pointer let go over the dock: the pane lands where the pointer
    /// is, or goes back where it came from when that is no pane at all.
    pub(crate) fn drop_at(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.drag {
            PaneDrag::Pressed { .. } => self.rest(cx),
            PaneDrag::Lifted { .. } | PaneDrag::Previewing { .. } => {
                match self.hover_at(position, cx) {
                    Hover::Nothing => self.cancel(window, cx),
                    Hover::Own => self.commit(window, cx),
                    Hover::Target(target) => {
                        let applied = match &self.drag {
                            PaneDrag::Previewing { target, .. } => Some(*target),
                            _ => None,
                        };
                        if applied != Some(target) {
                            self.apply(target, window, cx);
                        }
                        self.commit(window, cx);
                    }
                }
            }
            PaneDrag::Idle | PaneDrag::Settling { .. } => {}
        }
    }

    /// The pointer let go and no drop took it: outside the dock, or a press
    /// that never became a drag.
    pub(crate) fn released(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.drag {
            PaneDrag::Pressed { .. } => self.rest(cx),
            PaneDrag::Lifted { .. } | PaneDrag::Previewing { .. } => self.cancel(window, cx),
            PaneDrag::Idle | PaneDrag::Settling { .. } => {}
        }
    }

    /// Escape: a lifted pane goes back. Reports whether a drag took the key.
    pub(crate) fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.drag {
            PaneDrag::Pressed { .. } => {
                cx.stop_active_drag(window);
                self.rest(cx);
                true
            }
            PaneDrag::Lifted { .. } | PaneDrag::Previewing { .. } => {
                self.cancel(window, cx);
                true
            }
            PaneDrag::Idle | PaneDrag::Settling { .. } => false,
        }
    }

    /// Keeps the layout the pane previews, and flies the card into its slot.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lift) = self.lift().cloned() else {
            return;
        };
        self.settle_from_card(&lift, window, cx);
    }

    /// Puts back the layout the pane lifted from, and flies the card home.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lift) = self.lift().cloned() else {
            return;
        };
        if let Some(area) = self.area.upgrade() {
            let layout = {
                let area = area.read(cx);
                layout_of(lift.origin.root(), area, cx)
            };
            area.update(cx, |area, cx| area.set_center(layout, window, cx));
        }
        cx.stop_active_drag(window);
        self.settle_from_card(&lift, window, cx);
    }

    fn settle_from_card(&mut self, lift: &Lift, window: &mut Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let (card, _) = card_rect(lift, cx);
        self.tracks.insert(lift.panel, Track::still(card, now));
        self.debounce = None;
        self.drag = PaneDrag::Settling { panel: lift.panel };
        lift.view.focus_handle(cx).focus(window, cx);
        let duration = Theme::global(cx).motion.layout;
        self.settle = (!cx.reduce_motion() && !duration.is_zero()).then(|| {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(duration).await;
                _ = this.update(cx, |this, cx| {
                    if let Some(task) = this.settle.take() {
                        task.detach();
                    }
                    this.rest(cx);
                });
            })
        });
        if self.settle.is_none() {
            self.drag = PaneDrag::Idle;
        }
        cx.notify();
    }

    fn rest(&mut self, cx: &mut Context<Self>) {
        self.drag = PaneDrag::Idle;
        self.debounce = None;
        cx.notify();
    }
}

/// Sets `slot` to `panel` when `on`, or clears it when it holds `panel`.
/// Reports whether it changed.
fn toggle(slot: &mut Option<PanelId>, panel: PanelId, on: bool) -> bool {
    let next = match (on, *slot) {
        (true, _) => Some(panel),
        (false, Some(shown)) if shown == panel => None,
        (false, shown) => shown,
    };
    let changed = next != *slot;
    *slot = next;
    changed
}

/// The card of a lifted pane at this moment, and whether it is still
/// shrinking from the pane's slot toward the compact card size.
fn card_rect(lift: &Lift, cx: &App) -> (Bounds<Pixels>, bool) {
    let theme = Theme::global(cx);
    let compact = theme.tokens().metrics.dock_card;
    let compact = size(
        compact.width.min(lift.slot.size.width),
        compact.height.min(lift.slot.size.height),
    );
    let appear = theme.motion.appear;
    let elapsed = cx
        .background_executor()
        .now()
        .saturating_duration_since(lift.lifted_at);
    let (progress, growing) = if cx.reduce_motion() || appear.is_zero() || elapsed >= appear {
        (1., false)
    } else {
        let linear = elapsed.as_secs_f32() / appear.as_secs_f32();
        (theme.motion.layout_curve.sample(linear), true)
    };
    let card = lift.slot.size.interpolate(&compact, progress);
    let scale = |part: Pixels, of: Pixels, to: Pixels| {
        if of > Pixels::ZERO {
            to * (part / of)
        } else {
            Pixels::ZERO
        }
    };
    let grab = Point::new(
        scale(lift.grab.x, lift.slot.size.width, card.width),
        scale(lift.grab.y, lift.slot.size.height, card.height),
    );
    (Bounds::new(lift.pointer - grab, card), growing)
}

/// The side of `bounds` nearest `position`, relative to its size: the
/// four triangles between the diagonals, as Ghostty's
/// `TerminalSplitDropZone.calculate` divides a pane.
pub(crate) fn nearest_edge(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Placement {
    let x = (position.x - bounds.left()) / bounds.size.width;
    let y = (position.y - bounds.top()) / bounds.size.height;
    let edges = [
        (x, Placement::Left),
        (1. - x, Placement::Right),
        (y, Placement::Top),
        (1. - y, Placement::Bottom),
    ];
    edges
        .into_iter()
        .fold((f32::MAX, Placement::Left), |best, (distance, edge)| {
            if distance < best.0 {
                (distance, edge)
            } else {
                best
            }
        })
        .1
}

/// The group a lifted `panel` waits in: the nearest group of the sibling
/// beside its own in the parent split, on the side that touches it.
fn neighbor_group(root: &PaneNode, panel: PanelId) -> Option<NodeId> {
    let PaneRef::Split { children, .. } = root.kind() else {
        return None;
    };
    let index = children.iter().position(|child| holds(child, panel))?;
    let holds_only = matches!(
        children[index].kind(),
        PaneRef::Tabs { panels, .. } if panels.len() == 1
    );
    if !holds_only {
        return neighbor_group(&children[index], panel);
    }
    let (sibling, from_start) = match index.checked_sub(1) {
        Some(before) => (&children[before], false),
        None => (children.get(index + 1)?, true),
    };
    Some(edge_group(sibling, from_start))
}

fn holds(node: &PaneNode, panel: PanelId) -> bool {
    match node.kind() {
        PaneRef::Split { children, .. } => children.iter().any(|child| holds(child, panel)),
        PaneRef::Tabs { panels, .. } => panels.contains(&panel),
    }
}

/// The group of `node` at its start or its end.
fn edge_group(node: &PaneNode, from_start: bool) -> NodeId {
    match node.kind() {
        PaneRef::Split { children, .. } => {
            let child = if from_start {
                children.first()
            } else {
                children.last()
            };
            child.map_or(node.id(), |child| edge_group(child, from_start))
        }
        PaneRef::Tabs { .. } => node.id(),
    }
}

/// The layout `node` describes, with the area's own panel handles, so
/// installing it moves panels rather than making new ones.
fn layout_of(node: &PaneNode, area: &DockArea, cx: &App) -> DockLayout {
    match node.kind() {
        PaneRef::Split {
            axis,
            children,
            sizes,
        } => {
            let split = match axis {
                Axis::Horizontal => DockLayout::h_split(),
                Axis::Vertical => DockLayout::v_split(),
            };
            children
                .iter()
                .zip(sizes.iter())
                .fold(split, |split, (child, size)| {
                    split.child(layout_of(child, area, cx), *size)
                })
        }
        PaneRef::Tabs { panels, active_ix } => panels
            .iter()
            .filter_map(|panel| area.panel(*panel))
            .fold(DockLayout::tabs(), |tabs, view| {
                tabs.panel_view(view.clone(), cx)
            })
            .active_index(active_ix),
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{point, px};

    use super::*;

    #[test]
    fn the_nearest_edge_wins_and_there_is_no_center() {
        let bounds = Bounds::new(point(px(100.), px(0.)), size(px(400.), px(200.)));
        let at = |x: f32, y: f32| nearest_edge(bounds, point(px(x), px(y)));
        assert_eq!(at(110., 100.), Placement::Left);
        assert_eq!(at(490., 100.), Placement::Right);
        assert_eq!(at(300., 10.), Placement::Top);
        assert_eq!(at(300., 190.), Placement::Bottom);
        assert_eq!(at(320., 100.), Placement::Right);
        assert_eq!(at(280., 100.), Placement::Left);
    }
}
