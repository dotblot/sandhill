use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, TestSupportExt as _,
    Window, base::StyledExt as _, div,
};

use crate::Progress;

/// A shadcn-style spinner: an arc turning in place, for work whose length
/// is not known. Use a circular [`Progress`] for a known percentage; this
/// is that ring with no value.
///
/// The look is shadcn's `Spinner`: `size-4` (16px at the default font
/// size), painted with the inherited text color, and turning once a
/// second at a linear pace, Tailwind's `animate-spin`. `Styled` sizes and
/// colors it:
///
/// ```
/// use gpui_cn::Spinner;
/// use gpui_kit::Styled as _;
///
/// let _ = Spinner::new("saving");
/// let _ = Spinner::new("sync").size_6().accessibility_label("Syncing");
/// ```
///
/// Assistive technology reads it as a status named "Loading", as shadcn's
/// `role="status" aria-label="Loading"`.
///
/// The turn repaints at 30 fps on the app's shared loop clock, not at the
/// display rate: a 16px arc turning once a second moves 12 degrees a
/// step, which reads as smooth, and every spinner on screen repaints on
/// the same tick. Under reduced motion it holds still and asks for no
/// frames. A spinner is a thin wrapper: a [`Progress`] ring with no value
/// draws the arc.
#[derive(IntoElement)]
pub struct Spinner {
    id: ElementId,
    style: StyleRefinement,
    accessibility_label: Option<SharedString>,
}

impl Spinner {
    /// A spinner with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            accessibility_label: None,
        }
    }

    /// The name a screen reader announces. The default is "Loading".
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
}

impl Styled for Spinner {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The accessible name when none is given, as shadcn's.
const DEFAULT_LABEL: &str = "Loading";

impl RenderOnce for Spinner {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = self
            .accessibility_label
            .unwrap_or_else(|| DEFAULT_LABEL.into());
        div()
            .id(self.id.clone())
            .role(Role::Status)
            .aria_label(label.clone())
            .test_support()
            .flex_shrink_0()
            .size_4()
            .refine_style(&self.style)
            .child(
                Progress::new(ElementId::NamedChild(self.id.into(), "arc".into()))
                    .circular()
                    .accessibility_label(label)
                    .size_full(),
            )
    }
}
