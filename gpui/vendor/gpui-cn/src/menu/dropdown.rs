use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement, Length,
    MouseButton, ParentElement as _, RenderOnce, StyleRefinement, Styled, Window,
    base::{
        Align, ElementExt as _, GlobalState, Selectable, StyledExt as _, TestSupportExt as _,
        actions::{Confirm, SelectDown},
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{MenuAnchor, MenuEntry, MenuState, TRIGGER_CONTEXT, panel::MenuPanels};

/// Builds the trigger, marked open while the menu is.
type TriggerBuilder = Box<dyn FnOnce(bool) -> AnyElement>;
pub(crate) type EntriesBuilder = Rc<dyn Fn(&mut Window, &mut App) -> Vec<MenuEntry>>;
/// Opens the menu, from the keyboard when the flag is set.
type Opener = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A menu of commands that opens under a trigger, as shadcn's
/// `DropdownMenu`.
///
/// A press on the trigger opens the menu; so do Enter, Space, and Down
/// while the trigger has focus, which also put the keyboard on the first
/// row. The rows are built as the menu opens, so their enabled state is
/// current. The menu opens under the trigger, a gap away, lined up with
/// its trailing edge unless [`align`](Self::align) says otherwise, and
/// over it when there is no room below.
///
/// ```
/// use gpui_cn::{Button, DropdownMenu, MenuItem, MenuState};
/// use gpui_kit::{Entity, IntoElement};
///
/// fn actions(state: &Entity<MenuState>) -> impl IntoElement {
///     DropdownMenu::new("actions", state)
///         .trigger(Button::new("actions-trigger").label("Actions"))
///         .items(|_, _| {
///             vec![
///                 MenuItem::new("rename", "Rename").into(),
///                 MenuItem::new("delete", "Delete").destructive().into(),
///             ]
///         })
/// }
/// ```
///
/// The id keys the rows and the menu's motion, so it must be stable
/// across frames.
#[derive(IntoElement)]
pub struct DropdownMenu {
    id: ElementId,
    state: Entity<MenuState>,
    trigger: Option<TriggerBuilder>,
    items: Option<EntriesBuilder>,
    align: Align,
    action_context: Option<FocusHandle>,
    menu_width: Option<Length>,
    style: StyleRefinement,
}

impl DropdownMenu {
    /// A dropdown menu on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<MenuState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            trigger: None,
            items: None,
            align: Align::End,
            action_context: None,
            menu_width: None,
            style: StyleRefinement::default(),
        }
    }

    /// The element that opens the menu, such as a [`Button`](crate::Button)
    /// without a click handler of its own. It is marked open while the
    /// menu shows.
    pub fn trigger(mut self, trigger: impl Selectable + IntoElement + 'static) -> Self {
        self.trigger = Some(Box::new(move |open| trigger.open(open).into_any_element()));
        self
    }

    /// Builds the rows each time the menu opens.
    pub fn items(
        mut self,
        items: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> Self {
        self.items = Some(Rc::new(items));
        self
    }

    /// Which edge of the trigger the menu lines up with. The default is
    /// `End`, as a select's menu opens.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
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

impl Styled for DropdownMenu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for DropdownMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state;
        let (open, trigger_bounds) = {
            let state = state.read(cx);
            (state.is_open(), state.trigger_bounds().clone())
        };
        let open_menu: Opener = {
            let state = state.clone();
            let items = self.items;
            let align = self.align;
            let context = self.action_context;
            let trigger_bounds = trigger_bounds.clone();
            Rc::new(move |keyboard, window, cx| {
                let entries = items
                    .as_ref()
                    .map(|items| items(window, cx))
                    .unwrap_or_default();
                let anchor = MenuAnchor::Trigger(trigger_bounds.get(), align);
                state.update(cx, |state, cx| {
                    match &context {
                        Some(context) => state.open_for(entries, anchor, context, window, cx),
                        None => state.open(entries, anchor, window, cx),
                    }
                    if keyboard {
                        state.highlight_edge(false, cx);
                    }
                });
            })
        };
        div()
            .id(self.id.clone())
            .test_support()
            .key_context(TRIGGER_CONTEXT)
            .flex_shrink_0()
            .refine_style(&self.style)
            .relative()
            .on_prepaint(move |bounds, _, _| trigger_bounds.set(bounds))
            .on_mouse_down(MouseButton::Left, {
                let state = state.clone();
                let open_menu = open_menu.clone();
                move |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    GlobalState::suppress_text_selection(cx);
                    // A press on the trigger while the menu is open reaches
                    // the menu's outside press first, which closes it; that
                    // press must not open it again.
                    if state.read(cx).is_open() != open {
                        return;
                    }
                    if open {
                        state.update(cx, |state, cx| state.close(window, cx));
                    } else {
                        open_menu(false, window, cx);
                    }
                }
            })
            .when(!open, |this| {
                let confirm = open_menu.clone();
                this.on_action(move |_: &Confirm, window, cx| confirm(true, window, cx))
                    .on_action(move |_: &SelectDown, window, cx| open_menu(true, window, cx))
            })
            .children(self.trigger.map(|trigger| trigger(open)))
            .child(MenuPanels::new(self.id, &state).width(self.menu_width))
    }
}
