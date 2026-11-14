use std::collections::HashMap;

use gpui_kit::{
    App, ElementId, Entity, Global, WeakEntity, Window, WindowId, base::TooltipOverlay,
};

/// The per-window registry of tooltip overlays.
///
/// gpui-cn registers the overlay it adds to each `Root` here. A host with its own
/// root view registers the overlay it owns the same way, and gpui-cn
/// components then show their tooltips through it. With no registration a
/// component falls back to GPUI's native tooltip.
#[derive(Default)]
pub struct TooltipHost {
    overlays: HashMap<WindowId, WeakEntity<TooltipOverlay>>,
    /// The trigger that last asked the window's overlay to show. GPUI
    /// delivers the next trigger's enter before the previous trigger's
    /// leave, and a leave from a trigger that is not the requester must not
    /// cancel the pending show.
    requesters: HashMap<WindowId, ElementId>,
}

impl Global for TooltipHost {}

impl TooltipHost {
    /// Registers `overlay` as the tooltip overlay of `window`.
    ///
    /// The registry holds a weak handle, so dropping the overlay (closing
    /// the window, replacing the root) unregisters it.
    pub fn register(window: &Window, overlay: &Entity<TooltipOverlay>, cx: &mut App) {
        let id = window.window_handle().window_id();
        let overlays = &mut cx.default_global::<Self>().overlays;
        overlays.retain(|_, overlay| overlay.upgrade().is_some());
        overlays.insert(id, overlay.downgrade());
    }

    /// The tooltip overlay registered for `window`, if it is still alive.
    pub fn overlay(window: &Window, cx: &App) -> Option<Entity<TooltipOverlay>> {
        let id = window.window_handle().window_id();
        cx.try_global::<Self>()?.overlays.get(&id)?.upgrade()
    }

    pub(crate) fn note_requester(window: &Window, trigger: &ElementId, cx: &mut App) {
        let id = window.window_handle().window_id();
        cx.default_global::<Self>()
            .requesters
            .insert(id, trigger.clone());
    }

    pub(crate) fn is_requester(window: &Window, trigger: &ElementId, cx: &App) -> bool {
        let id = window.window_handle().window_id();
        cx.try_global::<Self>()
            .and_then(|host| host.requesters.get(&id))
            .is_some_and(|requester| requester == trigger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, Context, IntoElement, Render, TestAppContext, div};

    struct Empty;

    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui_kit::test]
    fn registry_is_per_window_and_weak(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
        });
        let (_, cx) = cx.add_window_view(|_, _| Empty);
        cx.update(|window, cx| {
            assert!(TooltipHost::overlay(window, cx).is_none());
            let overlay = cx.new(|_| TooltipOverlay::new());
            TooltipHost::register(window, &overlay, cx);
            assert_eq!(
                TooltipHost::overlay(window, cx).map(|o| o.entity_id()),
                Some(overlay.entity_id())
            );
            drop(overlay);
        });
        // The entity is released once the update that dropped the handle ends.
        cx.update(|window, cx| {
            assert!(TooltipHost::overlay(window, cx).is_none());
        });
    }
}
