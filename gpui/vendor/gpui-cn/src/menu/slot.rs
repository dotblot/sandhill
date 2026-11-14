//! A trigger the application builds, which a dropdown or popover marks open.

use gpui_kit::{AnyElement, App, IntoElement, RenderOnce, Window, base::Selectable};

/// A trigger built by the caller, which a dropdown or a popover marks open
/// through [`Selectable`]. It is built with the open flag the host gives, or
/// holds an element already built with the flag the caller keeps itself.
#[derive(IntoElement)]
pub(crate) struct OpenSlot {
    build: Box<dyn FnOnce(bool) -> AnyElement>,
    open: bool,
    /// Whether the host's open flag is ignored.
    pinned: bool,
}

impl OpenSlot {
    /// A trigger built when the host knows whether it is open.
    pub(crate) fn built(build: impl FnOnce(bool) -> AnyElement + 'static) -> Self {
        Self {
            build: Box::new(build),
            open: false,
            pinned: false,
        }
    }

    /// A trigger already built, with the open flag the caller keeps.
    pub(crate) fn fixed(element: AnyElement, open: bool) -> Self {
        Self {
            build: Box::new(move |_| element),
            open,
            pinned: true,
        }
    }
}

impl Selectable for OpenSlot {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        false
    }

    fn open(mut self, open: bool) -> Self {
        if !self.pinned {
            self.open = open;
        }
        self
    }

    fn is_open(&self) -> bool {
        self.open
    }
}

impl RenderOnce for OpenSlot {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        (self.build)(self.open)
    }
}
