use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, AppContext as _, AvailableSpace, Bounds, CursorStyle, DispatchPhase, Element,
    ElementId, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, Size,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, deferred, div, point, px,
};

use crate::{
    ActiveTheme as _, StyledExt, ThemeStyled as _,
    input::{
        EditorState,
        popovers::{DOC_TEXT, render_markdown},
    },
};

pub struct HoverPopover {
    editor: Entity<EditorState>,
    /// The symbol range byte of the hover trigger.
    pub(crate) symbol_range: Range<usize>,
    pub(crate) hover: Rc<lsp_types::Hover>,
}

impl HoverPopover {
    pub fn new(
        editor: Entity<EditorState>,
        symbol_range: Range<usize>,
        hover: &lsp_types::Hover,
        cx: &mut App,
    ) -> Entity<Self> {
        let hover = Rc::new(hover.clone());

        cx.new(|_| Self {
            editor,
            symbol_range,
            hover,
        })
    }
}

impl Render for HoverPopover {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let contents = match self.hover.contents.clone() {
            lsp_types::HoverContents::Scalar(scalar) => match scalar {
                lsp_types::MarkedString::String(s) => s,
                lsp_types::MarkedString::LanguageString(ls) => ls.value,
            },
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(|item| match item {
                    lsp_types::MarkedString::String(s) => s,
                    lsp_types::MarkedString::LanguageString(ls) => ls.value,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            lsp_types::HoverContents::Markup(markup) => markup.value,
        };

        Popover::new(
            "hover-popover",
            self.editor.clone(),
            self.symbol_range.clone(),
            move |window, cx| render_markdown("message", contents.clone(), window, cx),
        )
        .resizable()
        .into_any_element()
    }
}

pub(crate) struct Popover {
    id: ElementId,
    style: StyleRefinement,
    editor: Entity<EditorState>,
    range: Range<usize>,
    width_limit: Range<Pixels>,
    /// Sandhill: whether a corner drags to resize it.
    resizable: bool,
    content_builder: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

/// Sandhill: a corner drag in progress: where it began, the size then, and
/// which way each axis grows (1 toward right/bottom, -1 toward left/top).
#[derive(Clone, Copy)]
struct Drag {
    start: Point<Pixels>,
    from: Size<Pixels>,
    grow: Point<f32>,
}

thread_local! {
    /// Sandhill: the hover popup's size once the user has dragged its
    /// corner, kept for the session; until then it fits its text.
    static SIZE: Cell<Option<Size<Pixels>>> = const { Cell::new(None) };
    static DRAG: Cell<Option<Drag>> = const { Cell::new(None) };
}

const MIN_SIZE: Size<Pixels> = Size {
    width: px(200.),
    height: px(60.),
};
const HANDLE: Pixels = px(14.);
const SNAP_TO_EDGE: Pixels = px(8.);

impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Popover {
    pub fn new<F, E>(
        id: impl Into<ElementId>,
        editor: Entity<EditorState>,
        range: Range<usize>,
        f: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        Self {
            id: id.into(),
            editor,
            range,
            style: StyleRefinement::default(),
            width_limit: px(200.)..px(500.),
            resizable: false,
            content_builder: Box::new(move |window, cx| (f)(window, cx).into_any_element()),
        }
    }

    /// Sandhill: the corner away from the text drags to resize it.
    pub fn resizable(mut self) -> Self {
        self.resizable = true;
        self
    }

    /// Get the bounds of the range in the editor, if it is visible.
    fn trigger_bounds(&self, cx: &App) -> Option<Bounds<Pixels>> {
        let editor = self.editor.read(cx);
        editor.range_to_bounds(&self.range)
    }
}

#[cfg(not(feature = "gpui-fast"))]
impl IntoElement for Popover {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct PopoverLayoutState {
    bounds: Bounds<Pixels>,
    element: Option<AnyElement>,
    /// Sandhill: the resize corner and where it sits.
    handle: Option<(AnyElement, Point<Pixels>)>,
}

impl Element for Popover {
    type RequestLayoutState = PopoverLayoutState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let trigger_bounds = match self.trigger_bounds(cx) {
            Some(bounds) => bounds,
            None => {
                return (
                    div().into_any_element().request_layout(window, cx),
                    PopoverLayoutState {
                        bounds: Bounds::default(),
                        element: None,
                        handle: None,
                    },
                );
            }
        };

        let window_size = window.bounds().size;
        let top_space = trigger_bounds.top() - SNAP_TO_EDGE;
        let bottom_space = window_size.height - trigger_bounds.bottom() - SNAP_TO_EDGE;
        let right_space = window_size.width - trigger_bounds.left() - SNAP_TO_EDGE;
        let left_space = trigger_bounds.right() - SNAP_TO_EDGE;
        // Sandhill: while its corner is dragged it keeps its side of the
        // text, and grows only into the room on that side.
        let drag = self.resizable.then(|| DRAG.with(Cell::get)).flatten();
        let chosen = self
            .resizable
            .then(|| SIZE.with(Cell::get))
            .flatten()
            .map(|size| {
                let (width_room, height_room) = match drag {
                    Some(drag) => (
                        if drag.grow.x > 0. {
                            right_space
                        } else {
                            left_space
                        },
                        if drag.grow.y > 0. {
                            bottom_space
                        } else {
                            top_space
                        },
                    ),
                    None => (
                        window_size.width - SNAP_TO_EDGE * 2,
                        window_size.height - SNAP_TO_EDGE * 2,
                    ),
                };
                Size {
                    width: size.width.min(width_room).max(MIN_SIZE.width),
                    height: size.height.min(height_room).max(MIN_SIZE.height),
                }
            });

        let max_width = self
            .width_limit
            .end
            .min(window_size.width - SNAP_TO_EDGE * 2)
            .max(px(200.));
        let max_height = (window_size.height - SNAP_TO_EDGE * 2).min(px(320.));

        // Sandhill: prose in the interface font (not the editor's code
        // font) at the popups' size; code blocks set their own.
        let content = div()
            .id("hover-popover-content")
            .flex_none()
            .occlude()
            .p_1()
            .text_size(DOC_TEXT)
            .font_family(cx.theme().font_family.clone())
            .popover_style(cx)
            .shadow_md();
        let content = match chosen {
            Some(size) => content.w(size.width).h(size.height),
            None => content.max_w(max_width).max_h(max_height),
        };
        let mut popover = deferred(
            content
                .overflow_y_scroll()
                .refine_style(&self.style)
                .child((self.content_builder)(window, cx)),
        )
        .with_priority(gpui_base::POPUP_PRIORITY)
        .into_any_element();

        let popover_size = popover.layout_as_root(AvailableSpace::min_size(), window, cx);

        let above = match drag {
            Some(drag) => drag.grow.y < 0.,
            None => popover_size.height <= top_space,
        };
        let flipped = match drag {
            Some(drag) => drag.grow.x < 0.,
            None => popover_size.width > right_space,
        };
        let pos = point(
            if flipped {
                trigger_bounds.right() - popover_size.width
            } else {
                trigger_bounds.left()
            },
            if above {
                trigger_bounds.top() - popover_size.height
            } else {
                trigger_bounds.bottom()
            },
        );
        let bounds = Bounds {
            origin: pos,
            size: popover_size,
        };

        // Sandhill: the corner away from the text, so it grows away from it.
        let handle = self.resizable.then(|| {
            let grow = point(if flipped { -1. } else { 1. }, if above { -1. } else { 1. });
            let (glyph, cursor) = match (flipped, above) {
                (false, false) => ("◢", CursorStyle::ResizeUpLeftDownRight),
                (false, true) => ("◥", CursorStyle::ResizeUpRightDownLeft),
                (true, false) => ("◣", CursorStyle::ResizeUpRightDownLeft),
                (true, true) => ("◤", CursorStyle::ResizeUpLeftDownRight),
            };
            let mut handle = deferred(
                div()
                    .id("hover-popover-resize")
                    .occlude()
                    .size(HANDLE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.))
                    .text_color(cx.theme().muted_foreground)
                    .cursor(cursor)
                    .child(glyph)
                    .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        DRAG.with(|drag| {
                            drag.set(Some(Drag {
                                start: event.position,
                                from: popover_size,
                                grow,
                            }))
                        });
                        cx.stop_propagation();
                        window.refresh();
                    }),
            )
            .with_priority(gpui_base::POPUP_PRIORITY + 1)
            .into_any_element();
            handle.layout_as_root(AvailableSpace::min_size(), window, cx);
            let at = point(
                if flipped {
                    bounds.left()
                } else {
                    bounds.right() - HANDLE
                },
                if above {
                    bounds.top()
                } else {
                    bounds.bottom() - HANDLE
                },
            );
            (handle, at)
        });

        let mut empty = div().into_any_element();
        let layout_id = empty.request_layout(window, cx);
        (
            layout_id,
            PopoverLayoutState {
                bounds,
                element: Some(popover),
                handle,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        window.with_absolute_element_offset(bounds.origin, |window| {
            popover.prepaint(window, cx);
        });
        if let Some((handle, at)) = request_layout.handle.as_mut() {
            window.with_absolute_element_offset(*at, |window| handle.prepaint(window, cx));
        }
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        popover.paint(window, cx);
        if let Some((handle, _)) = request_layout.handle.as_mut() {
            handle.paint(window, cx);
        }

        let editor = self.editor.clone();
        // Mouse down out to hide.
        window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
            if !bounds.contains(&event.position) {
                let _ = editor.update(cx, |editor, cx| {
                    editor.clear_hover_state(cx);
                });
            }
        });

        // Mouse out of trigger + popover bounds
        let editor = self.editor.clone();
        let trigger_bounds = self.trigger_bounds(cx).unwrap_or(bounds);
        let keep_open_region = trigger_bounds.union(&bounds);
        window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
            // Sandhill: a corner drag may pass outside it.
            if DRAG.with(Cell::get).is_some() {
                return;
            }
            if !keep_open_region.contains(&event.position) {
                let _ = editor.update(cx, |editor, cx| {
                    editor.clear_hover_state(cx);
                });
            }
        });

        if self.resizable {
            window.on_mouse_event(|event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let Some(drag) = DRAG.with(Cell::get) else {
                    return;
                };
                if !event.dragging() {
                    DRAG.with(|drag| drag.set(None));
                    return;
                }
                let delta = event.position - drag.start;
                SIZE.with(|size| {
                    size.set(Some(Size {
                        width: (drag.from.width + delta.x * drag.grow.x).max(MIN_SIZE.width),
                        height: (drag.from.height + delta.y * drag.grow.y).max(MIN_SIZE.height),
                    }))
                });
                cx.stop_propagation();
                window.refresh();
            });
            window.on_mouse_event(|event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Capture
                    && event.button == MouseButton::Left
                    && DRAG.with(|drag| drag.take()).is_some()
                {
                    cx.stop_propagation();
                }
            });
        }
    }
}
