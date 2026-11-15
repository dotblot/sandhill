//! The grab handles and the edit menu a long press leaves on a text
//! field's selection, drawn over what base laid out.
//!
//! Base owns the gesture, the word it selects, the menu's open state, and
//! the drag of either end (`gpui_base::TouchSelectionSnapshot`). This
//! module draws a handle at each end and a row of commands above the
//! selection, in the theme's tokens.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Bounds, ClickEvent, ElementId, Entity, FontWeight, Hitbox, HitboxBehavior,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, RenderOnce, SharedString, Styled as _,
    TestSupportExt as _, TouchDragEvent, TouchPhase, Window,
    base::{
        POPUP_PRIORITY, Placement, Positioner, SelectionEdge, TouchHandle, TouchSelectionSnapshot,
        input::{self, InputBaseState, InputModeKind},
    },
    canvas, deferred, div, px,
};

use crate::{ActiveTheme as _, Button};

/// Reads the touch selection as laid out by the time the handles paint.
type SnapshotSource = Rc<dyn Fn(&Window, &App) -> Option<TouchSelectionSnapshot>>;
/// What a handle's owner gets told as the finger moves it.
type DragHandler = Rc<dyn Fn(SelectionEdge, TouchPhase, Point<Pixels>, &mut Window, &mut App)>;
/// What an edit menu item does when pressed.
type ItemHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One command in the edit menu.
pub(crate) struct EditMenuItem {
    label: SharedString,
    on_click: ItemHandler,
}

impl EditMenuItem {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            on_click: Rc::new(on_click),
        }
    }
}

/// Draws one touch selection: its handles and, when open, its edit menu.
///
/// The handles read the selection's geometry as they paint, so they follow
/// text that scrolls in the same frame. The menu is placed from the
/// snapshot read here.
pub(crate) struct TouchSelectionOverlay {
    id: ElementId,
    source: SnapshotSource,
    items: Vec<EditMenuItem>,
    on_drag: Option<DragHandler>,
}

impl TouchSelectionOverlay {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        source: impl Fn(&Window, &App) -> Option<TouchSelectionSnapshot> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            source: Rc::new(source),
            items: Vec::new(),
            on_drag: None,
        }
    }

    /// Draws floating handles, dragged through `on_drag`.
    pub(crate) fn handles(
        mut self,
        on_drag: impl Fn(SelectionEdge, TouchPhase, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag = Some(Rc::new(on_drag));
        self
    }

    /// The commands the edit menu offers. With none, no menu is drawn.
    pub(crate) fn items(mut self, items: impl IntoIterator<Item = EditMenuItem>) -> Self {
        self.items.extend(items);
        self
    }

    /// The elements to add to the owner: the handles, and the menu when it
    /// is open. Each floats in window coordinates.
    pub(crate) fn into_elements(self, window: &Window, cx: &App) -> Vec<AnyElement> {
        let Some(snapshot) = (self.source)(window, cx) else {
            return Vec::new();
        };
        let mut elements = Vec::with_capacity(2);
        if let Some(on_drag) = self.on_drag {
            elements.push(SelectionHandles::new(self.source, on_drag).into_any_element());
        }
        if let Some(mut anchor) = snapshot
            .bounds()
            .filter(|_| snapshot.is_menu_open() && !self.items.is_empty())
        {
            // Leave the knobs uncovered: the menu anchors to the selection
            // plus the room its handles take above and below.
            if !snapshot.is_empty() {
                anchor.origin.y -= TouchHandle::EXTENT;
                anchor.size.height += TouchHandle::EXTENT * 2.;
            }
            elements.push(
                EditMenu::new(self.id, anchor)
                    .items(self.items)
                    .into_any_element(),
            );
        }
        elements
    }
}

/// The grab handles at the ends of a field's touch selection, floating
/// above the field: their knobs reach past the field's edge, where the
/// field's own clip would cut them off.
///
/// A handle claims the touch drag that begins on it before the window can
/// take the drag for panning, and blocks the mouse so a stray tap on it
/// does not reach the text underneath. A mouse can drag it too, which is
/// how it is exercised without a touch screen.
#[derive(IntoElement)]
struct SelectionHandles {
    source: SnapshotSource,
    on_drag: DragHandler,
}

/// One handle laid out for this frame.
struct LaidOutHandle {
    edge: SelectionEdge,
    caret: Bounds<Pixels>,
    hitbox: Hitbox,
}

impl SelectionHandles {
    fn new(source: SnapshotSource, on_drag: DragHandler) -> Self {
        Self { source, on_drag }
    }

    /// Lays out a handle for each end of a non-empty selection that is in
    /// view.
    fn layout(source: &SnapshotSource, window: &mut Window, cx: &mut App) -> Vec<LaidOutHandle> {
        let Some(snapshot) = source(window, cx) else {
            return Vec::new();
        };
        let window_bounds = Bounds::new(Point::default(), window.viewport_size());
        let mut handles = Vec::with_capacity(2);
        if !snapshot.is_empty() {
            for edge in [SelectionEdge::Start, SelectionEdge::End] {
                let caret = snapshot.edge(edge);
                if !snapshot.is_edge_visible(edge) || !window_bounds.contains(&caret.origin) {
                    continue;
                }
                let hitbox = window.insert_hitbox(
                    TouchHandle::hit_bounds(edge, caret),
                    HitboxBehavior::BlockMouse,
                );
                handles.push(LaidOutHandle {
                    edge,
                    caret,
                    hitbox,
                });
            }
        }
        handles
    }

    /// The drag begins on whichever handle is under the finger or pointer;
    /// its moves and its end are routed by which end is being dragged, so
    /// they keep coming even if that handle is not laid out for a frame.
    fn listen(
        handles: &[LaidOutHandle],
        source: &SnapshotSource,
        on_drag: &DragHandler,
        window: &mut Window,
    ) {
        for handle in handles {
            // Touch: the drag is offered on the first touch, before it can
            // become a tap, a long press or a pan.
            window.on_mouse_event({
                let hitbox = handle.hitbox.clone();
                let edge = handle.edge;
                let on_drag = on_drag.clone();
                move |event: &TouchDragEvent, phase, window, cx| {
                    if !phase.bubble()
                        || event.phase != TouchPhase::Started
                        || window.default_prevented()
                        || !hitbox.is_hovered(window)
                    {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    on_drag(edge, TouchPhase::Started, event.position, window, cx);
                }
            });
            window.on_mouse_event({
                let hitbox = handle.hitbox.clone();
                let edge = handle.edge;
                let on_drag = on_drag.clone();
                move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble()
                        || event.button != MouseButton::Left
                        || !hitbox.is_hovered(window)
                    {
                        return;
                    }
                    cx.stop_propagation();
                    on_drag(edge, TouchPhase::Started, event.position, window, cx);
                }
            });
        }

        // The drag in progress is read live: it may have begun after this
        // frame painted, and its first moves must not be lost.
        let dragging = {
            let source = source.clone();
            move |window: &Window, cx: &App| source(window, cx).and_then(|s| s.dragging())
        };
        window.on_mouse_event({
            let on_drag = on_drag.clone();
            let dragging = dragging.clone();
            move |event: &TouchDragEvent, phase, window, cx| {
                if !phase.bubble() || event.phase == TouchPhase::Started {
                    return;
                }
                let Some(edge) = dragging(window, cx) else {
                    return;
                };
                cx.stop_propagation();
                on_drag(edge, event.phase, event.position, window, cx);
            }
        });
        window.on_mouse_event({
            let on_drag = on_drag.clone();
            let dragging = dragging.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() || event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let Some(edge) = dragging(window, cx) else {
                    return;
                };
                on_drag(edge, TouchPhase::Moved, event.position, window, cx);
            }
        });
        window.on_mouse_event({
            let on_drag = on_drag.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if !phase.bubble() || event.button != MouseButton::Left {
                    return;
                }
                let Some(edge) = dragging(window, cx) else {
                    return;
                };
                on_drag(edge, TouchPhase::Ended, event.position, window, cx);
            }
        });
    }
}

impl RenderOnce for SelectionHandles {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let source = self.source;
        let on_drag = self.on_drag;
        // The accent, as iOS paints its handles system blue.
        let color = cx.theme().ring();
        deferred(
            canvas(
                {
                    let source = source.clone();
                    move |_, window, cx| Self::layout(&source, window, cx)
                },
                move |_, handles, window, _| {
                    for handle in &handles {
                        TouchHandle::paint(handle.edge, handle.caret, color, window);
                    }
                    Self::listen(&handles, &source, &on_drag, window);
                },
            )
            .absolute()
            .size_0(),
        )
        .with_priority(POPUP_PRIORITY)
    }
}

/// The row of commands a touch selection offers: Cut, Copy, Paste, Select
/// All, whichever apply. It floats above the selection, or below it when
/// there is no room above, and stays out of the way of the handles' knobs.
///
/// The look is iOS 26's edit menu, measured from the Simulator's Settings
/// search field at 3x: a pill one control tall on the popover surface
/// with its hairline and shadow, labels at the control text size, and a
/// rule between neighbours inset from the top and bottom.
#[derive(IntoElement)]
struct EditMenu {
    id: ElementId,
    /// The selection, including the room its handles take.
    anchor: Bounds<Pixels>,
    items: Vec<EditMenuItem>,
}

impl EditMenu {
    fn new(id: impl Into<ElementId>, anchor: Bounds<Pixels>) -> Self {
        Self {
            id: id.into(),
            anchor,
            items: Vec::new(),
        }
    }

    fn items(mut self, items: impl IntoIterator<Item = EditMenuItem>) -> Self {
        self.items.extend(items);
        self
    }
}

impl RenderOnce for EditMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        // Two steps of the spacing scale between the bar and the text.
        let gap = gpui_kit::rems(0.5).to_pixels(window.rem_size());
        let height = theme.metrics.control_md;
        let radius = theme.radius_full();
        let (fill, border, separator, foreground) = (
            theme.popover,
            theme.popover_border,
            theme.popover_separator,
            theme.popover_foreground,
        );
        let shadow = theme.base.shadow.md.clone();
        let text = theme.text_control;
        let bar_id = self.id.clone();
        let items = self.items.into_iter().enumerate().flat_map(|(ix, item)| {
            let on_click = item.on_click;
            // Observed under its label, so a test can press "Copy".
            let button = div()
                .id(item.label.clone())
                .test_support()
                .h_full()
                .child(
                    Button::new(ElementId::NamedChild(
                        bar_id.clone().into(),
                        item.label.clone(),
                    ))
                    .ghost()
                    .tab_stop(false)
                    .label(item.label)
                    .h_full()
                    .px_5()
                    .rounded(radius)
                    .text_size(text.size)
                    .font_weight(FontWeight::NORMAL)
                    .text_color(foreground)
                    .on_click(move |_: &ClickEvent, window, cx| on_click(window, cx)),
                )
                .into_any_element();
            (ix > 0)
                .then(|| div().w(px(1.)).my_3().bg(separator).into_any_element())
                .into_iter()
                .chain([button])
        });
        deferred(
            Positioner::side(self.anchor)
                .placement(Placement::Top)
                .offset(gap)
                .occlude()
                .child(
                    div()
                        .id(self.id)
                        .flex()
                        .items_stretch()
                        .h(height)
                        .rounded(radius)
                        .bg(fill)
                        .border_1()
                        .border_color(border)
                        .shadow(shadow)
                        .children(items),
                ),
        )
        .with_priority(POPUP_PRIORITY)
    }
}

/// The handles and the edit menu of the selection a long press made on
/// any text state, for the field and for the select's search box.
///
/// The menu offers what the native context menu would: Cut, Copy, Paste
/// and Select All, leaving out what cannot apply rather than disabling
/// it. Cut, Copy and Paste go through the field's actions, so a custom
/// key binding sees them the same way.
pub(crate) fn for_state<M: InputModeKind>(
    state: &Entity<InputBaseState<M>>,
    window: &Window,
    cx: &App,
) -> Vec<AnyElement> {
    let (capabilities, selectable) = {
        let state = state.read(cx);
        if state.touch_selection().is_none() {
            return Vec::new();
        }
        let length = state.text().len();
        (
            state.context_menu_capabilities(),
            length > 0 && state.selected_range() != (0..length),
        )
    };
    let editable = capabilities.is_editable();
    let copyable = capabilities.is_copyable();
    let focus_handle = state.read(cx).presentation().focus_handle().clone();
    let dispatch = move |action: &dyn gpui_kit::Action, window: &mut Window, cx: &mut App| {
        focus_handle.dispatch_action(action, window, cx);
    };

    let mut items = Vec::with_capacity(4);
    if editable && copyable {
        let dispatch = dispatch.clone();
        items.push(EditMenuItem::new("Cut", move |window, cx| {
            dispatch(&input::Cut, window, cx);
        }));
    }
    if copyable {
        let dispatch = dispatch.clone();
        let state = state.clone();
        items.push(EditMenuItem::new("Copy", move |window, cx| {
            dispatch(&input::Copy, window, cx);
            state.update(cx, |state, cx| state.close_edit_menu(cx));
        }));
    }
    // Offered whenever the text can change, without peeking at the
    // clipboard: on iOS every read of it shows the system's paste banner.
    if editable {
        let dispatch = dispatch.clone();
        items.push(EditMenuItem::new("Paste", move |window, cx| {
            dispatch(&input::Paste, window, cx);
        }));
    }
    if selectable {
        let state = state.clone();
        items.push(EditMenuItem::new("Select All", move |window, cx| {
            state.update(cx, |state, cx| state.select_all_from_edit_menu(window, cx))
        }));
    }

    let drag_state = state.clone();
    let source_state = state.clone();
    TouchSelectionOverlay::new(
        ("input-touch-selection", state.entity_id()),
        move |_, cx| source_state.read(cx).touch_selection(),
    )
    .handles(move |edge, phase, position, _, cx| {
        drag_state.update(cx, |state, cx| match phase {
            TouchPhase::Started => state.begin_edge_drag(edge, position, cx),
            TouchPhase::Moved => state.update_edge_drag(position, cx),
            TouchPhase::Ended | TouchPhase::Cancelled => state.end_edge_drag(cx),
        })
    })
    .items(items)
    .into_elements(window, cx)
}
