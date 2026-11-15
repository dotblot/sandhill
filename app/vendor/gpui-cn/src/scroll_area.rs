//! A vertical scroll region that stretches past its ends and springs
//! back, on every platform, with an overlay scrollbar.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    AnyElement, App, Bounds, ElementId, InteractiveElement as _, IntoElement, ListSizingBehavior,
    ListState, ParentElement, Pixels, RenderOnce, ScrollHandle, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window,
    base::{
        self, ElementExt as _, ScrollBounce, ScrollbarHandle, StyledExt as _, TestSupportExt as _,
    },
    div, list, point,
    prelude::FluentBuilder as _,
    px,
};

type RenderItem = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement>;
type OnScroll = Box<dyn Fn(&mut Window, &mut App)>;

/// What the region scrolls: children laid out in full, or a virtual list
/// that lays out only the rows in view.
enum Content {
    Children(Vec<AnyElement>),
    List {
        state: ListState,
        row_hint: Pixels,
        render_item: RenderItem,
    },
}

/// A vertical scroll region with the overscroll bounce of a `UIScrollView`
/// and an overlay scrollbar.
///
/// `gpui_base::ScrollBounce` plays the stretch and the return, and turns
/// itself on for iOS and Android alone. gpui-cn turns it on everywhere,
/// so a trackpad on a desktop feels the same end of the page as a finger
/// on a phone. Reduced motion turns the bounce off; the region still
/// scrolls. The scroll position lives in window state under `id`, so it
/// survives a re-render.
///
/// `gpui_base::Scrollbar` draws the bar over the region's right edge: it
/// shows while the region scrolls, fades when it rests, and drags. Its
/// look comes from the theme, which projects it onto base for every
/// scrolling element.
///
/// A focused field inside the region asks GPUI to reveal it, and the
/// region scrolls until the field is in view, as GPUI's `list` does.
///
/// The region fills its parent. Give it a sized parent, and put fixed
/// chrome such as a toolbar outside it.
///
/// ```
/// use gpui_cn::ScrollArea;
/// use gpui_kit::{IntoElement, ParentElement as _, div};
///
/// fn page() -> impl IntoElement {
///     ScrollArea::new("page").child(div().child("A long page"))
/// }
/// ```
///
/// A long collection uses [`list`](Self::list) instead of children: GPUI's
/// `list` element lays out only the rows in view, and the caller's
/// `ListState` owns the scroll position and the row measurements.
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    content: Content,
    handle: Option<ScrollHandle>,
    on_scroll: Option<OnScroll>,
    style: StyleRefinement,
}

impl ScrollArea {
    /// An empty scroll region. The id owns the scroll position and the
    /// bounce state.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            content: Content::Children(Vec::new()),
            handle: None,
            on_scroll: None,
            style: StyleRefinement::default(),
        }
    }

    /// A scroll region over a virtual list: `render_item` builds the row
    /// at an index when it comes into view, and `state` (the caller's, so
    /// it can scroll to a row and reset the count) owns the scroll
    /// position. The region sizes itself to its rows up to the height its
    /// style allows, so a short list is short and a long one scrolls.
    ///
    /// `row_hint` is the height a row is taken to have before it is laid
    /// out, so the scroll range and a scroll to a far row are right from
    /// the first frame. GPUI drops every unmeasured row's hint whenever
    /// the list's width changes; the region puts it back.
    pub fn list(
        id: impl Into<ElementId>,
        state: &ListState,
        row_hint: Pixels,
        render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            content: Content::List {
                state: state.clone(),
                row_hint,
                render_item: Box::new(render_item),
            },
            handle: None,
            on_scroll: None,
            style: StyleRefinement::default(),
        }
    }
}

impl ScrollArea {
    /// Scrolls with `handle`, the caller's, so the caller can read how far
    /// the region has moved. A region over a list has the list state for
    /// that. Without one the region keeps its own, keyed by its id.
    pub fn track(mut self, handle: &ScrollHandle) -> Self {
        self.handle = Some(handle.clone());
        self
    }

    /// Runs after a wheel step or a pan moves the region. The region
    /// moves its content itself, so a list's own scroll handler does not
    /// run for these steps; read the new position from the list state.
    pub fn on_scroll(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_scroll = Some(Box::new(handler));
        self
    }
}

impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for ScrollArea {
    /// Children go to a region made with [`new`](Self::new). A region over a
    /// list has no children of its own; the list's rows are the content.
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Content::Children(children) = &mut self.content {
            children.extend(elements);
        }
    }
}

/// The bounce and the scrollbar around a viewport, and whether the
/// region has room to scroll, for any handle base can drive.
fn chrome<H: ScrollbarHandle + Clone>(
    id: &ElementId,
    handle: &H,
    viewport: impl IntoElement,
    on_scroll: Option<OnScroll>,
) -> (AnyElement, AnyElement) {
    let mut bounce = ScrollBounce::new(
        ElementId::NamedChild(id.clone().into(), "bounce".into()),
        handle,
        viewport,
    )
    .enabled(true);
    if let Some(handler) = on_scroll {
        bounce = bounce.on_scroll(handler);
    }
    let scrollbar = base::Scrollbar::vertical(handle)
        .id(ElementId::NamedChild(id.clone().into(), "scrollbar".into()));
    (bounce.into_any_element(), scrollbar.into_any_element())
}

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // The caller styles one element, but there are two: an outer box
        // that sits in the caller's layout, and the viewport inside it
        // that lays out the content. The box takes what places it (size,
        // flex, margin, position); the viewport takes the rest (padding,
        // direction, gap, colors).
        let (frame_style, viewport_style) = split_style(self.style);
        let on_scroll = self.on_scroll;
        let mut revealer = None;
        let (bounce, scrollbar) = match self.content {
            Content::Children(children) => {
                let handle = match self.handle {
                    Some(handle) => handle,
                    None => window
                        .use_keyed_state(
                            ElementId::NamedChild(self.id.clone().into(), "scroll".into()),
                            cx,
                            |_, _| ScrollHandle::new(),
                        )
                        .read(cx)
                        .clone(),
                };
                let viewport = div()
                    .id(self.id.clone())
                    .test_support()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&handle)
                    .refine_style(&viewport_style)
                    .children(children);
                revealer = Some(handle.clone());
                chrome(&self.id, &handle, viewport, on_scroll)
            }
            Content::List {
                state,
                row_hint,
                render_item,
            } => {
                // GPUI drops the unmeasured rows' hints when the list's
                // width changes, and with them the scroll range. The
                // width the rows were last hinted at is kept here, and the
                // hint goes back when the width is seen to change. A
                // measured row keeps its measured height.
                let hinted_width = window.use_keyed_state(
                    ElementId::NamedChild(self.id.clone().into(), "hinted".into()),
                    cx,
                    |_, _| None::<Pixels>,
                );
                let width = state.viewport_bounds().size.width;
                if *hinted_width.read(cx) != Some(width) {
                    state.clone().with_uniform_item_height(row_hint);
                    hinted_width.update(cx, |hinted, _| *hinted = Some(width));
                }
                // `Infer` sizes the list to its rows, so the region is as
                // tall as its content until the style's height caps it;
                // `Auto` would fill a parent that has no height of its own.
                let viewport = div()
                    .id(self.id.clone())
                    .test_support()
                    .size_full()
                    .refine_style(&viewport_style)
                    .child(
                        list(state.clone(), render_item)
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .size_full(),
                    );
                chrome(&self.id, &state, viewport, on_scroll)
            }
        };
        // GPUI's `list` takes a reveal request itself. For children, the
        // request made before the content prepaints belongs to something
        // outside, so it is set aside and put back unless the content made
        // its own.
        let outside = Rc::new(Cell::new(None));
        // The bounce owns the wheel: the innermost region under the
        // pointer with room to scroll takes the step in the capture phase
        // and stops it there, and a region whose content fits passes it on.
        div()
            .id(ElementId::NamedChild(self.id.into(), "frame".into()))
            .relative()
            .refine_style(&frame_style)
            .when(revealer.is_some(), |this| {
                let outside = outside.clone();
                this.on_prepaint(move |_, window, _| outside.set(window.take_autoscroll()))
            })
            .child(bounce)
            .child(scrollbar)
            .when_some(revealer, |this, handle| {
                this.on_prepaint(move |visible, window, _| match window.take_autoscroll() {
                    Some(request) => reveal(&handle, request, visible, window),
                    None => {
                        if let Some(request) = outside.take() {
                            window.request_autoscroll(request);
                        }
                    }
                })
            })
    }
}

/// Scrolls `handle` so `request`, an area a focused element inside the
/// region asked to see, lies inside `visible`, the region's bounds. An
/// area taller than the region shows from its top. The part that shows
/// is asked of the region outside, so nested regions reveal it in turn.
fn reveal(
    handle: &ScrollHandle,
    request: Bounds<Pixels>,
    visible: Bounds<Pixels>,
    window: &mut Window,
) {
    let offset = handle.offset();
    let shift = if request.top() < visible.top() || request.size.height > visible.size.height {
        visible.top() - request.top()
    } else if request.bottom() > visible.bottom() {
        visible.bottom() - request.bottom()
    } else {
        px(0.)
    };
    let y = (offset.y + shift).clamp(-handle.max_offset().y, px(0.));
    let moved = y - offset.y;
    if moved != px(0.) {
        handle.set_offset(point(offset.x, y));
        window.request_animation_frame();
    }
    let shown =
        Bounds::new(request.origin + point(px(0.), moved), request.size).intersect(&visible);
    if shown.size.height > px(0.) {
        window.request_autoscroll(shown);
    }
}

/// Splits a refinement into what places an element in its parent and what
/// lays out its content.
fn split_style(style: StyleRefinement) -> (StyleRefinement, StyleRefinement) {
    let mut frame = StyleRefinement::default();
    let mut viewport = style;
    frame.size = std::mem::take(&mut viewport.size);
    frame.min_size = std::mem::take(&mut viewport.min_size);
    frame.max_size = std::mem::take(&mut viewport.max_size);
    frame.flex_grow = viewport.flex_grow.take();
    frame.flex_shrink = viewport.flex_shrink.take();
    frame.flex_basis = viewport.flex_basis.take();
    frame.align_self = viewport.align_self.take();
    frame.margin = std::mem::take(&mut viewport.margin);
    frame.position = viewport.position.take();
    frame.inset = std::mem::take(&mut viewport.inset);
    (frame, viewport)
}
