//! Plain output text with its own retained rendering invalidation boundary.

use gpui_kit::base::{SelectableText, TextSelectionEvent, TextSelectionHandle};
use gpui_kit::*;
use std::ops::Range;
use std::rc::Rc;

/// Colours over ranges of the text, such as code's syntax colours.
pub(crate) type Highlights = Rc<[(Range<usize>, HighlightStyle)]>;

/// R and Python code's syntax colours in `theme`, the editor's; none for
/// other languages.
pub(crate) fn code_highlights(
    code: &str,
    language: &str,
    theme: &gpui_kit::component::highlighter::HighlightTheme,
) -> Highlights {
    use gpui_kit::component::highlighter::SyntaxHighlighter;
    use gpui_kit::component::input::Rope;
    if !matches!(language, "python" | "r") {
        return Rc::from([]);
    }
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &Rope::from_str(code), None);
    highlighter.styles(&(0..code.len()), theme).into()
}

/// [`code_highlights`] kept for code drawn every frame (the AI pane's step
/// cards): one parse per code, language and theme.
pub(crate) fn cached_code_highlights(
    code: &str,
    language: &str,
    theme: &std::sync::Arc<gpui_kit::component::highlighter::HighlightTheme>,
) -> Highlights {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    type Key = (u64, String, usize);
    thread_local! {
        static CACHE: RefCell<HashMap<Key, Highlights>> = RefCell::new(HashMap::new());
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hash);
    let key = (
        hash.finish(),
        language.to_string(),
        std::sync::Arc::as_ptr(theme) as usize,
    );
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&key) {
            return found.clone();
        }
        if cache.len() >= 256 {
            cache.clear();
        }
        let made = code_highlights(code, language, theme);
        cache.insert(key, made.clone());
        made
    })
}

/// Keeps selection changes local to the output, rather than rebuilding its
/// notebook or Console. The standard selectable text still owns hit testing,
/// Unicode projection, selection painting and copy behavior.
pub(crate) struct OutputText {
    id: ElementId,
    text: SharedString,
    highlights: Option<Highlights>,
}

impl OutputText {
    pub(crate) fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            highlights: None,
        }
    }

    /// Colours ranges of the text (UTF-8 offsets, in order, not overlapping).
    pub(crate) fn highlights(mut self, highlights: Highlights) -> Self {
        self.highlights = Some(highlights);
        self
    }
}

struct SelectionOwner {
    text: SharedString,
    highlights: Option<Highlights>,
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
            highlights: None,
            selection,
            _subscription: subscription,
        }
    }
}

impl Render for SelectionOwner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let text = SelectableText::with_handle("text", self.selection.clone(), self.text.clone());
        match &self.highlights {
            Some(highlights) => text.highlights(highlights.iter().cloned()),
            None => text,
        }
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
            let same = match (&owner.highlights, &self.highlights) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                owner.highlights = self.highlights.clone();
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
