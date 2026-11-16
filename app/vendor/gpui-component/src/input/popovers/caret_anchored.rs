//! Caret-anchored popups positioned from the input's current-frame geometry.
//!
//! The input stashes fresh caret geometry in its prepaint
//! ([`PrepaintCaretGeometry`]). Popups live inside `deferred()`, so their
//! prepaint runs after the input's prepaint but before its paint; resolving
//! the anchor there puts the popup at the current frame's caret instead of
//! the previous frame's.

use std::cell::Cell;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Point, Size, Style, WeakEntity, Window, px,
};

use crate::input::{EditorState, PrepaintCaretGeometry};

/// The gap between a menu and the panel beside it.
const SIDE_GAP: Pixels = px(4.);

thread_local! {
    /// Sandhill: the window's bands popups stay clear of (its title bar at
    /// the top, its status bar at the bottom).
    static INSETS: Cell<(Pixels, Pixels)> = const { Cell::new((px(0.), px(0.))) };
    /// Where the side panel was last placed, so a click in it doesn't
    /// count as a click outside the menu.
    static SIDE_BOUNDS: Cell<Option<Bounds<Pixels>>> = const { Cell::new(None) };
}

/// The side panel's bounds in the window, when one is shown.
pub(crate) fn side_bounds() -> Option<Bounds<Pixels>> {
    SIDE_BOUNDS.with(Cell::get)
}

/// Sandhill: keeps caret-anchored popups clear of `top` pixels at the
/// window's top and `bottom` at its bottom, such as a title bar and a
/// status bar drawn over the content.
pub fn set_popup_insets(top: Pixels, bottom: Pixels) {
    INSETS.with(|insets| insets.set((top, bottom)));
}

pub(crate) fn popup_insets() -> (Pixels, Pixels) {
    INSETS.with(Cell::get)
}

/// Computes the caret-anchored popup origin from fresh prepaint geometry.
///
/// Shared by the completion and code-action menus so both anchor to the
/// same point: just below the caret, nudged left. The result is in window
/// coordinates, matching where the caret itself is painted.
pub(crate) fn caret_popup_anchor(geometry: PrepaintCaretGeometry) -> Point<Pixels> {
    Point::new(
        geometry.cursor_bounds().origin.x - px(4.),
        geometry.cursor_bounds().origin.y
            + geometry.scroll_offset().y
            + geometry.line_height()
            + px(4.),
    )
}

/// Builds a panel shown beside the menu, given the most room it may take.
type SideBuilder = Box<dyn Fn(Size<Pixels>, &mut Window, &mut App) -> Option<AnyElement>>;

/// A popup positioned at the caret from the input's current-frame prepaint.
///
/// The content is built in prepaint (not render) from the resolved anchor,
/// so position, hitboxes, accessibility bounds, and width/layout decisions
/// are all consistent with the current frame's caret. Always use inside
/// `deferred()` so the prepaint runs after the input's prepaint.
pub(crate) struct CaretAnchoredPopup {
    editor: WeakEntity<EditorState>,
    build: Box<dyn Fn(Point<Pixels>, &mut Window, &mut App) -> AnyElement>,
    side: Option<SideBuilder>,
    content: Option<AnyElement>,
    side_content: Option<AnyElement>,
}

impl CaretAnchoredPopup {
    pub(crate) fn new(
        editor: WeakEntity<EditorState>,
        build: impl Fn(Point<Pixels>, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            editor,
            build: Box::new(build),
            side: None,
            content: None,
            side_content: None,
        }
    }

    /// Sandhill: a panel placed beside the menu (documentation). The menu is
    /// placed by its own size alone, so a panel that changes never moves it.
    pub(crate) fn side(
        mut self,
        build: impl Fn(Size<Pixels>, &mut Window, &mut App) -> Option<AnyElement> + 'static,
    ) -> Self {
        self.side = Some(Box::new(build));
        self
    }
}

/// Where a menu of `size` goes for a caret: below it, or above when it
/// doesn't fit below and there is more room above, kept inside the window
/// less its `insets`.
fn menu_origin(
    anchor: Point<Pixels>,
    caret_top: Pixels,
    size: Size<Pixels>,
    viewport: Size<Pixels>,
    (top, bottom): (Pixels, Pixels),
) -> (Point<Pixels>, bool) {
    let below = viewport.height - bottom - anchor.y;
    let above = caret_top - px(4.) - top;
    let x = anchor.x.min(viewport.width - size.width).max(px(0.));
    if size.height > below && above > below {
        (Point::new(x, (caret_top - px(4.) - size.height).max(top)), true)
    } else {
        (Point::new(x, anchor.y), false)
    }
}

/// Where the side panel of `side` size goes for a menu at `menu`: to its
/// right, else its left, else under it (over it when the menu opened
/// upward), always inside the window less its `insets`.
fn side_origin(
    menu: Bounds<Pixels>,
    upward: bool,
    side: Size<Pixels>,
    viewport: Size<Pixels>,
    (top, bottom): (Pixels, Pixels),
) -> Point<Pixels> {
    let floor = viewport.height - bottom;
    let fit_y = |y: Pixels| y.min(floor - side.height).max(top);
    let right = menu.origin.x + menu.size.width + SIDE_GAP;
    let left = menu.origin.x - SIDE_GAP - side.width;
    // Beside the menu, lined up with its edge nearest the caret.
    let beside = fit_y(if upward {
        menu.origin.y + menu.size.height - side.height
    } else {
        menu.origin.y
    });
    if right + side.width <= viewport.width {
        Point::new(right, beside)
    } else if left >= px(0.) {
        Point::new(left, beside)
    } else {
        let x = menu
            .origin
            .x
            .min(viewport.width - side.width)
            .max(px(0.));
        let y = if upward {
            menu.origin.y - SIDE_GAP - side.height
        } else {
            menu.origin.y + menu.size.height + SIDE_GAP
        };
        Point::new(x, fit_y(y))
    }
}

impl Element for CaretAnchoredPopup {
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
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The real layout happens in prepaint, once the anchor is resolved.
        let layout_id = window.request_layout(Style::default(), [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let Some(geometry) = editor.read(cx).prepaint_caret_geometry() else {
            return;
        };
        let anchor = caret_popup_anchor(geometry);
        let mut content = (self.build)(anchor, window, cx);
        let viewport = window.viewport_size();
        let insets = popup_insets();
        // Sandhill: at the menu's own width (its `max_w` bounds it); a root
        // laid out in the whole window's width would fill it.
        let room = Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::Definite(viewport.height),
        };
        let size = content.layout_as_root(room, window, cx);
        // Sandhill: open above the caret when the menu doesn't fit below it
        // and there is more room above (an input at the window's bottom).
        let caret_top = geometry.cursor_bounds().origin.y + geometry.scroll_offset().y;
        let (origin, upward) = menu_origin(anchor, caret_top, size, viewport, insets);
        content.prepaint_at(origin, window, cx);
        self.content = Some(content);

        self.side_content = None;
        SIDE_BOUNDS.with(|b| b.set(None));
        if let Some(side) = &self.side {
            let most = Size {
                width: viewport.width,
                height: (viewport.height - insets.0 - insets.1).max(px(0.)),
            };
            if let Some(mut panel) = side(most, window, cx) {
                let side_size = panel.layout_as_root(room, window, cx);
                let at = side_origin(
                    Bounds { origin, size },
                    upward,
                    side_size,
                    viewport,
                    insets,
                );
                panel.prepaint_at(at, window, cx);
                SIDE_BOUNDS.with(|b| {
                    b.set(Some(Bounds {
                        origin: at,
                        size: side_size,
                    }))
                });
                self.side_content = Some(panel);
            }
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = self.content.as_mut() {
            content.paint(window, cx);
        }
        if let Some(side) = self.side_content.as_mut() {
            side.paint(window, cx);
        }
    }
}

#[cfg(not(feature = "gpui-fast"))]
impl gpui::IntoElement for CaretAnchoredPopup {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
