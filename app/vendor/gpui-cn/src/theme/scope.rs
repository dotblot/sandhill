//! A subtree drawn with another token set.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, Window,
};

use super::Theme;

/// Draws its child with the tokens of a scope registered by
/// [`Theme::set_scope`], so every gpui-cn component inside reads that
/// scope through `cx.theme()`. A terminal uses one to draw its tab strip in
/// the terminal's colors.
///
/// The scope holds while the child renders, lays out, and paints. A
/// listener that runs later, on an event, reads the window's tokens. An
/// unknown key draws with the window's tokens. Entering a scope changes no
/// global, so it wakes no observer of the theme and costs no frame.
pub struct ThemeScope {
    key: SharedString,
    child: AnyElement,
}

impl ThemeScope {
    /// Draws `child` with the tokens of the scope `key`. The scope takes
    /// the child's layout, so size the child.
    pub fn new(key: impl Into<SharedString>, child: impl IntoElement) -> Self {
        Self {
            key: key.into(),
            child: child.into_any_element(),
        }
    }
}

/// Runs `f` with the scope `key` active, then restores the scope around
/// it, so scopes nest.
fn within<R>(key: &str, cx: &mut App, f: impl FnOnce(&mut App) -> R) -> R {
    let theme = Theme::global(cx);
    let index = theme.scope_index(key);
    let outer = theme
        .active_scope
        .replace(index.or(theme.active_scope.get()));
    let result = f(cx);
    Theme::global(cx).active_scope.set(outer);
    result
}

impl IntoElement for ThemeScope {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ThemeScope {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let child = &mut self.child;
        let layout = within(&self.key, cx, |cx| child.request_layout(window, cx));
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        within(&self.key, cx, |cx| child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        within(&self.key, cx, |cx| child.paint(window, cx));
    }
}
