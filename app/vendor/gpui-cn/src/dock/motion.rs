//! Layout motion for dock panes: each pane glides from the rect it was
//! last painted at to the slot the layout gives it.

use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Window,
    base::{Easing, Interpolate as _, dock::PanelId},
};

use super::session::DockSkinState;

/// Paint order of a pane while it moves, above the panes at rest. A
/// pane flying in from a drop takes `SETTLING` so it passes over the
/// panes that make room for it.
pub(crate) const MOVING: usize = 1;
pub(crate) const SETTLING: usize = 2;

/// One pane's motion: from the rect it was at to the slot it goes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Track {
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    started: Instant,
}

impl Track {
    /// A pane resting at `bounds`.
    pub(crate) fn still(bounds: Bounds<Pixels>, now: Instant) -> Self {
        Self {
            from: bounds,
            to: bounds,
            started: now,
        }
    }

    /// The slot the pane goes to, which is where it rests once the motion
    /// ends.
    pub(crate) fn target(&self) -> Bounds<Pixels> {
        self.to
    }

    /// Where the pane is at `now`, and whether it is still moving.
    pub(crate) fn sample(
        &self,
        now: Instant,
        duration: Duration,
        curve: &Easing,
    ) -> (Bounds<Pixels>, bool) {
        let elapsed = now.saturating_duration_since(self.started);
        if self.from == self.to || duration.is_zero() || elapsed >= duration {
            return (self.to, false);
        }
        let progress = curve.sample(elapsed.as_secs_f32() / duration.as_secs_f32());
        (self.from.interpolate(&self.to, progress), true)
    }

    /// Heads for `slot` from wherever the pane is at `now`. A slot that is
    /// already the target changes nothing, so a frame that lays the pane
    /// out where it was going does not restart the motion.
    pub(crate) fn retarget(
        &mut self,
        slot: Bounds<Pixels>,
        now: Instant,
        duration: Duration,
        curve: &Easing,
    ) {
        if self.to == slot {
            return;
        }
        let (at, _) = self.sample(now, duration, curve);
        *self = Self {
            from: at,
            to: slot,
            started: now,
        };
    }
}

/// Lays a pane out at its slot and paints it where its [`Track`] says.
///
/// The child is laid out at the slot's final size, so a terminal inside
/// resizes its grid once per layout change rather than on every frame.
/// While the pane moves it is drawn later than the tree around it, at the
/// sampled origin, clipped to the sampled rect, so it can pass over its
/// neighbors and out of its old group's clip. At rest it is drawn in
/// place like any other child. Frames are asked for only while it moves.
pub(crate) struct LayoutMotion {
    panel: PanelId,
    state: Entity<DockSkinState>,
    child: Option<AnyElement>,
}

impl LayoutMotion {
    pub(crate) fn new(
        panel: PanelId,
        state: Entity<DockSkinState>,
        child: impl IntoElement,
    ) -> Self {
        Self {
            panel,
            state,
            child: Some(child.into_any_element()),
        }
    }
}

impl IntoElement for LayoutMotion {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LayoutMotion {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = self.child.as_mut().expect("laid out once");
        (child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let (painted, moving, priority, clip) = self
            .state
            .update(cx, |state, cx| state.place(self.panel, bounds, cx));
        if !moving {
            if let Some(child) = self.child.as_mut() {
                child.prepaint(window, cx);
            }
            return;
        }
        window.request_animation_frame();
        let child = self.child.take().expect("prepainted once");
        let offset = window.element_offset() + (painted.origin - bounds.origin);
        let mask = painted.intersect(&clip);
        window.defer_draw(child, offset, priority, Some(ContentMask { bounds: mask }));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = self.child.as_mut() {
            child.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gpui_kit::{point, px, size};

    use super::*;

    fn rect(x: f32, w: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(0.)), size(px(w), px(100.)))
    }

    #[test]
    fn a_track_eases_to_its_slot_and_rests_there() {
        let now = Instant::now();
        let curve = Easing::Custom(Rc::new(|t| 1. - (1. - t).powi(5)));
        let duration = Duration::from_millis(460);
        let mut track = Track::still(rect(0., 100.), now);
        track.retarget(rect(200., 100.), now, duration, &curve);
        let (start, moving) = track.sample(now, duration, &curve);
        assert!(moving);
        assert_eq!(start.origin.x, px(0.));
        let (half, _) = track.sample(now + duration / 2, duration, &curve);
        assert_eq!(
            half.origin.x,
            px(193.75),
            "ease-out quint at half time is 1 - 0.5^5 = 0.96875 of the way"
        );
        let (end, moving) = track.sample(now + duration, duration, &curve);
        assert!(!moving);
        assert_eq!(end, rect(200., 100.));
    }

    #[test]
    fn a_new_slot_mid_motion_starts_from_where_the_pane_is() {
        let now = Instant::now();
        let curve = Easing::Linear;
        let duration = Duration::from_millis(400);
        let mut track = Track::still(rect(0., 100.), now);
        track.retarget(rect(400., 100.), now, duration, &curve);
        let later = now + Duration::from_millis(100);
        track.retarget(rect(0., 100.), later, duration, &curve);
        let (at, _) = track.sample(later, duration, &curve);
        assert_eq!(at.origin.x, px(100.));
        track.retarget(rect(0., 100.), later + duration / 2, duration, &curve);
        let (at, _) = track.sample(later + duration / 2, duration, &curve);
        assert_eq!(at.origin.x, px(50.));
    }
}
