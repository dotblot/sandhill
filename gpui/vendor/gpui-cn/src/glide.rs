//! Panes that glide to a new slot when a layout the application owns
//! changes, with the dock's motion.
//!
//! A [`Glide`] lays its child out at the slot it is given and paints it
//! moving from the rect it was last painted at, so a layout computed
//! elsewhere, such as by a server, moves like a [`DockSkin`] layout. The
//! child is laid out at its final size once, and only moves and clips.
//!
//! [`DockSkin`]: crate::DockSkin

use std::collections::HashMap;

use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, Entity, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, Window,
};

use crate::Theme;
use crate::dock::motion::{MOVING, Track};

/// Where each [`Glide`] of one layout is, by its key. The view that owns
/// the layout keeps one and hands it to every glide.
#[derive(Default)]
pub struct GlideState {
    tracks: HashMap<SharedString, Track>,
}

impl GlideState {
    /// No glides yet: each pane rests where it is first laid out.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the panes whose keys are not in `keys`, so a pane that comes
    /// back starts at rest.
    pub fn retain(&mut self, keys: impl IntoIterator<Item = SharedString>) {
        let keys: Vec<SharedString> = keys.into_iter().collect();
        self.tracks.retain(|key, _| keys.contains(key));
    }

    fn place(
        &mut self,
        key: &SharedString,
        slot: Bounds<Pixels>,
        cx: &App,
    ) -> (Bounds<Pixels>, bool) {
        let now = cx.background_executor().now();
        let motion = &Theme::global(cx).motion;
        let (duration, curve) = (motion.layout, motion.layout_curve.clone());
        let track = self
            .tracks
            .entry(key.clone())
            .or_insert_with(|| Track::still(slot, now));
        if cx.reduce_motion() {
            *track = Track::still(slot, now);
        } else {
            track.retarget(slot, now, duration, &curve);
        }
        track.sample(now, duration, &curve)
    }
}

/// Lays a pane out at its slot and paints it gliding there from where it
/// was, above the panes at rest and clipped to its parent.
pub struct Glide {
    key: SharedString,
    state: Entity<GlideState>,
    child: Option<AnyElement>,
}

impl Glide {
    /// A pane known as `key` across frames, whose motion `state` keeps.
    pub fn new(
        key: impl Into<SharedString>,
        state: &Entity<GlideState>,
        child: impl IntoElement,
    ) -> Self {
        Self {
            key: key.into(),
            state: state.clone(),
            child: Some(child.into_any_element()),
        }
    }
}

impl IntoElement for Glide {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Glide {
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
        let key = self.key.clone();
        let (painted, moving) = self
            .state
            .update(cx, |state, cx| state.place(&key, bounds, cx));
        if !moving {
            if let Some(child) = self.child.as_mut() {
                child.prepaint(window, cx);
            }
            return;
        }
        window.request_animation_frame();
        let child = self.child.take().expect("prepainted once");
        let offset = window.element_offset() + (painted.origin - bounds.origin);
        let mask = painted.intersect(&window.content_mask().bounds);
        window.defer_draw(child, offset, MOVING, Some(ContentMask { bounds: mask }));
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
    use gpui_kit::{TestAppContext, point, px, size};

    use super::*;

    fn rect(x: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(0.)), size(px(100.), px(100.)))
    }

    #[gpui_kit::test]
    fn a_pane_glides_to_a_new_slot_and_rests_there(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
            let mut state = GlideState::new();
            let key = SharedString::from("pane");
            assert_eq!(state.place(&key, rect(0.), cx), (rect(0.), false));
            let (start, moving) = state.place(&key, rect(200.), cx);
            assert!(moving || cx.reduce_motion());
            assert_eq!(
                start.origin.x,
                if cx.reduce_motion() { px(200.) } else { px(0.) }
            );
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.update(|cx| {
            let mut state = GlideState::new();
            let key = SharedString::from("pane");
            state.place(&key, rect(200.), cx);
            assert_eq!(state.place(&key, rect(200.), cx), (rect(200.), false));
        });
    }

    #[gpui_kit::test]
    fn a_pane_that_returns_after_retain_starts_at_rest(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
            let mut state = GlideState::new();
            let key = SharedString::from("pane");
            state.place(&key, rect(0.), cx);
            state.retain([]);
            assert_eq!(state.place(&key, rect(300.), cx), (rect(300.), false));
        });
    }
}
