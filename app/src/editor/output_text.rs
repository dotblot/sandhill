//! Plain output text with its own retained rendering invalidation boundary.

use gpui_kit::base::{SelectableText, TextSelectionEvent, TextSelectionHandle};
use gpui_kit::*;

/// Keeps selection changes local to the output, rather than rebuilding its
/// notebook or Console. The standard selectable text still owns hit testing,
/// Unicode projection, selection painting and copy behavior.
pub(crate) struct OutputText {
    id: ElementId,
    text: SharedString,
}

impl OutputText {
    pub(crate) fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

struct SelectionOwner {
    text: SharedString,
    selection: TextSelectionHandle,
    _subscription: Subscription,
}

impl SelectionOwner {
    fn new(text: SharedString, cx: &mut Context<Self>) -> Self {
        let selection = TextSelectionHandle::new(text.to_string(), cx);
        let owner = cx.entity_id();
        let subscription = selection.subscribe(
            move |event, cx| {
                if matches!(event, TextSelectionEvent::SelectionChanged(_)) {
                    // Notify without updating the model: only this rendering
                    // boundary needs rebuilding, not views that retain it.
                    cx.notify(owner);
                }
            },
            cx,
        );
        Self {
            text,
            selection,
            _subscription: subscription,
        }
    }
}

impl Render for SelectionOwner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SelectableText::with_handle("text", self.selection.clone(), self.text.clone())
    }
}

impl Element for OutputText {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // Unlike use_keyed_state, this does not subscribe the enclosing
        // Workbench to the owner's notifications. The entity and subscription
        // live only as long as this output's retained element state.
        let owner = window.with_element_state(
            global_id.expect("OutputText must have a stable element id"),
            |retained: Option<Entity<SelectionOwner>>, _| {
                let owner = retained
                    .unwrap_or_else(|| cx.new(|cx| SelectionOwner::new(self.text.clone(), cx)));
                (owner.clone(), owner)
            },
        );
        owner.update(cx, |owner, cx| {
            if owner.text != self.text {
                owner.text = self.text.clone();
                owner
                    .selection
                    .set_fallback_copy_text(self.text.to_string(), cx);
                cx.notify();
            }
        });
        let mut element = owner.into_any_element();
        let layout = element.request_layout(window, cx);
        (layout, element)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}
