use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement, Length,
    MouseButton, ParentElement, RenderOnce, StyleRefinement, Styled, Window,
    base::{StyledExt as _, TestSupportExt as _},
    div,
};

use super::{MenuAnchor, MenuEntry, MenuState, dropdown::EntriesBuilder, panel::MenuPanels};

/// A menu of commands that a right click on its content opens at the
/// pointer, as shadcn's `ContextMenu`.
///
/// The rows are built as the menu opens, so their enabled state is
/// current. The menu's top-left corner sits on the pointer, moved into
/// the window when it would not fit. A right click inside the content
/// does not reach a context menu around it.
///
/// ```
/// use gpui_cn::{ContextMenu, MenuItem, MenuState};
/// use gpui_kit::{Entity, IntoElement, ParentElement as _};
///
/// fn file(state: &Entity<MenuState>) -> impl IntoElement {
///     ContextMenu::new("file", state)
///         .items(|_, _| vec![MenuItem::new("reveal", "Reveal in Finder").into()])
///         .child("notes.txt")
/// }
/// ```
///
/// The id keys the rows and the menu's motion, so it must be stable
/// across frames.
#[derive(IntoElement)]
pub struct ContextMenu {
    id: ElementId,
    state: Entity<MenuState>,
    items: Option<EntriesBuilder>,
    action_context: Option<FocusHandle>,
    menu_width: Option<Length>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
}

impl ContextMenu {
    /// A context menu on `state` around its children, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<MenuState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            items: None,
            action_context: None,
            menu_width: None,
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    /// Builds the rows each time the menu opens.
    pub fn items(
        mut self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> Self {
        self.items = Some(Rc::new(items));
        self
    }

    /// The element the items' actions go to, and whose key bindings their
    /// shortcuts show. Without one they go to the element focused when
    /// the menu opens, as its key binding would, or up from this element
    /// when nothing is focused.
    pub fn action_context(mut self, focus_handle: &FocusHandle) -> Self {
        self.action_context = Some(focus_handle.clone());
        self
    }

    /// A fixed menu width. Without one the menu is as wide as its rows,
    /// between the theme's menu minimum and maximum.
    pub fn menu_width(mut self, width: impl Into<Length>) -> Self {
        self.menu_width = Some(width.into());
        self
    }
}

impl Styled for ContextMenu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for ContextMenu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let state = self.state;
        let items = self.items;
        let context = self.action_context;
        div()
            .id(self.id.clone())
            .test_support()
            .refine_style(&self.style)
            .on_mouse_down(MouseButton::Right, {
                let state = state.clone();
                move |event, window, cx| {
                    cx.stop_propagation();
                    let entries = items
                        .as_ref()
                        .map(|items| items(window, cx))
                        .unwrap_or_default();
                    if entries.is_empty() {
                        return;
                    }
                    let anchor = MenuAnchor::Point(event.position);
                    state.update(cx, |state, cx| match &context {
                        Some(context) => state.open_for(entries, anchor, context, window, cx),
                        None => state.open(entries, anchor, window, cx),
                    });
                }
            })
            .children(self.children)
            .child(MenuPanels::new(self.id, &state).width(self.menu_width))
    }
}
