//! A shadcn-style select: a trigger that opens a menu of items, with an
//! optional search field (local, or answered by the application on a
//! background thread or over the network), group labels, separators,
//! descriptions, and a single or multiple selection, over a virtual list.

mod item;
mod menu;
mod state;

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, Entity, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    Length, ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window,
    assets::IconName,
    base::{
        self, Align, Disableable, ElementExt as _, GlobalState, Placement, Positioner,
        StyledExt as _, TestSupportExt as _,
        actions::{Confirm, SelectDown, SelectFirst, SelectLast, SelectUp},
        h_flex,
        input::InputContextMenuCapabilities,
    },
    deferred, div, point,
    prelude::FluentBuilder as _,
    px,
};

pub use item::{SelectEntry, SelectItem, SelectValue};
pub use menu::SelectRow;
pub use state::{SearchHandler, SelectEvent, SelectState};

use menu::{ItemRenderer, LabelRenderer, Menu, PartRenderer, Rows};

use crate::{
    ActiveTheme as _, ButtonSize, Icon, MenuEntry, Theme, ThemeTokens,
    button::ControlGeometry,
    menu::{MenuLook, MenuMotion, TextMenuBuilder, corner},
    theme::mix,
};

/// The key context of a select's trigger and menu, outside base's own.
/// Base binds the arrows, Enter, and Escape; this adds the keys base
/// leaves to the styled layer. Space confirms only outside the search
/// field, where it is a character.
const CONTEXT: &str = "GpuiCnSelect";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "space",
            Confirm { secondary: false },
            Some("GpuiCnSelect && !Input"),
        ),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
    ]);
}

type ValueFormatter<V> = Rc<dyn Fn(&[SelectItem<V>]) -> SharedString>;
type ValueRenderer<V> = Rc<dyn Fn(&[SelectItem<V>], &mut Window, &mut App) -> AnyElement>;

/// A shadcn-style select on `gpui_base::Select`: a trigger that shows the
/// selection and opens a menu of items.
///
/// Base owns the combobox role, Tab order, the arrows and Enter that open
/// the menu, Escape that closes it, and the focus transfer between the
/// trigger and the menu. This type owns the look and the menu: the
/// trigger surface, the popover, its rows with icons, descriptions, group
/// labels and separators, the search field, the check beside a selected
/// row, the highlight that follows the keyboard and the pointer, and the
/// virtual list the rows scroll in.
///
/// The rows and the selection live in a [`SelectState`] the view owns and
/// observes. Both are generic over the item value, any [`SelectValue`]:
/// a string or integer, or the application's own enum or record.
///
/// ```
/// use gpui_cn::{Select, SelectItem, SelectState};
/// use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Render, Window};
///
/// struct Settings {
///     shortcut: Entity<SelectState<&'static str>>,
/// }
///
/// impl Settings {
///     fn new(cx: &mut Context<Self>) -> Self {
///         let shortcut = cx.new(|cx| {
///             SelectState::new(
///                 [
///                     SelectItem::new("enter", "Enter"),
///                     SelectItem::new("cmd-enter", "Cmd + Enter always"),
///                 ],
///                 cx,
///             )
///             .with_selected(["enter"])
///         });
///         cx.observe(&shortcut, |_, _, cx| cx.notify()).detach();
///         Self { shortcut }
///     }
/// }
///
/// impl Render for Settings {
///     fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
///         Select::new("shortcut", &self.shortcut).accessibility_label("Send shortcut")
///     }
/// }
/// ```
///
/// The id keys focus, motion, and the rows, so it must be stable across
/// frames and unique under the nearest stateful ancestor.
#[derive(IntoElement)]
pub struct Select<V: SelectValue> {
    id: ElementId,
    state: Entity<SelectState<V>>,
    style: StyleRefinement,
    size: ButtonSize,
    placeholder: Option<SharedString>,
    empty_text: SharedString,
    accessibility_label: Option<SharedString>,
    icon: Option<Icon>,
    menu_width: Option<Length>,
    align: Align,
    format_value: Option<ValueFormatter<V>>,
    render_value: Option<ValueRenderer<V>>,
    render_item: Option<ItemRenderer<V>>,
    render_label: Option<LabelRenderer>,
    render_empty: Option<PartRenderer>,
    render_loading: Option<PartRenderer>,
    search_context_menu: Option<TextMenuBuilder>,
    search_context_menu_enabled: bool,
    disabled: bool,
    tab_index: isize,
    ghost: bool,
    chevron: bool,
}

impl<V: SelectValue> Select<V> {
    /// A select over `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<SelectState<V>>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            style: StyleRefinement::default(),
            size: ButtonSize::Default,
            placeholder: None,
            empty_text: "No results".into(),
            accessibility_label: None,
            icon: None,
            menu_width: None,
            align: Align::End,
            format_value: None,
            render_value: None,
            render_item: None,
            render_label: None,
            render_empty: None,
            render_loading: None,
            search_context_menu: None,
            search_context_menu_enabled: true,
            disabled: false,
            tab_index: 0,
            ghost: false,
            chevron: true,
        }
    }

    /// A quiet trigger for a toolbar: no fill and no hairline at rest, and
    /// the surface of a ghost button while the pointer is on it or the menu
    /// is open. The menu is the same.
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }

    /// Whether the trigger shows its chevron. On by default. A trigger
    /// that shows only an icon (see [`render_value`](Self::render_value))
    /// turns it off.
    pub fn chevron(mut self, chevron: bool) -> Self {
        self.chevron = chevron;
        self
    }

    /// The trigger's size tier, shared with [`Button`](crate::Button).
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// The text the trigger shows while nothing is selected, in the muted
    /// color.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// The text the menu shows when the search matches no row, or while
    /// a search handler is still answering with no rows to show. The
    /// default is "No results".
    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// The name a screen reader announces for the control. The selection
    /// is its value, not its name, so every select should have one.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// An icon before the trigger's text, in place of the selected item's
    /// own.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// A fixed menu width. Without one the menu is as wide as its rows,
    /// never narrower than the trigger, and no wider than the theme's
    /// menu width. A menu with many rows should set one: the virtual list
    /// measures only the rows in view.
    pub fn menu_width(mut self, width: impl Into<Length>) -> Self {
        self.menu_width = Some(width.into());
        self
    }

    /// Which edge of the trigger the menu lines up with. The default is
    /// `End`, as the reference app opens its menus.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Builds the trigger's text from the selected items. The default is
    /// the item's label, "N selected" for more than one, and the
    /// placeholder for none.
    pub fn format_value(
        mut self,
        format: impl Fn(&[SelectItem<V>]) -> SharedString + 'static,
    ) -> Self {
        self.format_value = Some(Rc::new(format));
        self
    }

    /// Draws the inside of every item row: `render` gets the item and
    /// the row's state and returns what goes between the row's padding, in
    /// place of the icon, label, description, and check. The row itself
    /// (its height, highlight, hover, and click) stays the menu's, so a
    /// custom row still behaves like the others. The trigger keeps
    /// showing the label; see [`format_value`](Self::format_value).
    ///
    /// ```
    /// use gpui_cn::{Select, SelectItem, SelectState};
    /// use gpui_kit::{Entity, IntoElement, ParentElement as _, Styled as _, div};
    ///
    /// fn assignee(state: &Entity<SelectState<usize>>) -> Select<usize> {
    ///     Select::new("assignee", state).render_item(|item, row, _, _| {
    ///         div()
    ///             .flex()
    ///             .gap_2()
    ///             .child(item.label().clone())
    ///             .when_some(item.description_text().cloned(), |this, email| this.child(email))
    ///             .when(row.is_selected(), |this| this.child("*"))
    ///     })
    /// }
    /// # use gpui_kit::prelude::FluentBuilder as _;
    /// ```
    pub fn render_item<E: IntoElement>(
        mut self,
        render: impl Fn(&SelectItem<V>, SelectRow, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render_item = Some(Rc::new(move |item, row, window, cx| {
            render(item, row, window, cx).into_any_element()
        }));
        self
    }

    /// Draws the trigger's content from the selected items, in place of
    /// the icon and text; the chevron stays. For a text that differs, see
    /// [`format_value`](Self::format_value).
    pub fn render_value<E: IntoElement>(
        mut self,
        render: impl Fn(&[SelectItem<V>], &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render_value = Some(Rc::new(move |items, window, cx| {
            render(items, window, cx).into_any_element()
        }));
        self
    }

    /// Draws a group heading from its text, in place of the muted row.
    pub fn render_label<E: IntoElement>(
        mut self,
        render: impl Fn(&SharedString, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render_label = Some(Rc::new(move |text, window, cx| {
            render(text, window, cx).into_any_element()
        }));
        self
    }

    /// Draws the menu's body when no row matches, in place of
    /// [`empty_text`](Self::empty_text): an illustration, a hint, an
    /// action that adds what was typed.
    pub fn render_empty<E: IntoElement>(
        mut self,
        render: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render_empty = Some(Rc::new(move |window, cx| {
            render(window, cx).into_any_element()
        }));
        self
    }

    /// Draws the menu's body while a search handler is still answering,
    /// in place of the rows: skeleton rows the height of the ones to
    /// come keep the menu's size, so the answer fills it in. Without it
    /// the last rows stay until the answer.
    pub fn render_loading<E: IntoElement>(
        mut self,
        render: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.render_loading = Some(Rc::new(move |window, cx| {
            render(window, cx).into_any_element()
        }));
        self
    }

    /// Shapes the menu a right click opens on the search field, as
    /// [`Field::context_menu`](crate::Field::context_menu) does for a field:
    /// `build` gets the default rows (Cut, Copy, Paste, a separator, Select
    /// All) and returns the rows to show, the defaults extended or
    /// replaced. No rows, no menu.
    pub fn search_context_menu(
        mut self,
        build: impl Fn(
            Vec<MenuEntry>,
            InputContextMenuCapabilities,
            &mut Window,
            &mut App,
        ) -> Vec<MenuEntry>
        + 'static,
    ) -> Self {
        self.search_context_menu = Some(Rc::new(build));
        self
    }

    /// Whether a right click on the search field opens its menu. On by
    /// default.
    pub fn search_context_menu_enabled(mut self, enabled: bool) -> Self {
        self.search_context_menu_enabled = enabled;
        self
    }

    /// The focus traversal index. The default is `0`. Tab always reaches
    /// an enabled trigger; base owns that.
    pub fn tab_index(mut self, tab_index: isize) -> Self {
        self.tab_index = tab_index;
        self
    }
}

impl<V: SelectValue> Disableable for Select<V> {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<V: SelectValue> Styled for Select<V> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Everything the trigger reads from the theme, in one borrow.
struct TriggerLook {
    fill: Hsla,
    fill_hovered: Hsla,
    ghost_hovered: Hsla,
    ghost_open: Hsla,
    border: Hsla,
    foreground: Hsla,
    placeholder: Hsla,
    indicator: Hsla,
    ring: Hsla,
    ring_spread: Pixels,
    geometry: ControlGeometry,
    disabled_opacity: f32,
    touch: bool,
}

impl TriggerLook {
    fn of(size: ButtonSize, theme: &ThemeTokens) -> Self {
        Self {
            fill: theme.select_trigger,
            fill_hovered: mix(theme.select_trigger, theme.popover_foreground, 0.05),
            ghost_hovered: theme.secondary(),
            ghost_open: theme.selected,
            border: theme.select_trigger_border,
            foreground: theme.popover_foreground,
            placeholder: theme.muted_foreground(),
            indicator: theme.select_indicator,
            ring: theme.focus_ring(),
            ring_spread: theme.metrics.focus_ring,
            geometry: size.geometry(theme),
            disabled_opacity: theme.disabled_opacity,
            touch: theme.touch,
        }
    }
}

/// What the trigger shows for the selection.
struct Value {
    text: SharedString,
    leading: Option<AnyElement>,
    is_placeholder: bool,
}

impl<V: SelectValue> Select<V> {
    /// The trigger's text and leading element: the selection, else the
    /// placeholder.
    fn value(&self, window: &mut Window, cx: &mut App) -> Value {
        let items: Vec<SelectItem<V>> = self.state.read(cx).selected_items().to_vec();
        let text = match &self.format_value {
            Some(format) => Some(format(&items)),
            None => match items.as_slice() {
                [] => None,
                [item] => Some(item.label().clone()),
                many => Some(format!("{} selected", many.len()).into()),
            },
        };
        let leading = match (&self.icon, items.as_slice()) {
            (Some(icon), _) => Some(icon.clone().into_any_element()),
            (None, [item]) => item.render_leading(window, cx),
            _ => None,
        };
        match text {
            Some(text) => Value {
                text,
                leading,
                is_placeholder: false,
            },
            None => Value {
                text: self.placeholder.clone().unwrap_or_default(),
                leading,
                is_placeholder: true,
            },
        }
    }

    fn trigger(
        &self,
        look: &TriggerLook,
        value: Value,
        open: bool,
        pointer_cursors: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let state = &self.state;
        let interactive = !self.disabled;
        let hovered = window.use_keyed_state(
            ElementId::NamedChild(self.id.clone().into(), "hovered".into()),
            cx,
            |_, _| false,
        );
        let is_hovered = *hovered.read(cx) && interactive && !look.touch;
        let focus_visible =
            state.read(cx).trigger_focus().is_focused(window) && window.last_input_was_keyboard();
        let mut fill = match (self.ghost, open, is_hovered) {
            (true, true, _) => look.ghost_open,
            (true, false, true) => look.ghost_hovered,
            (true, false, false) => gpui_kit::transparent_black(),
            (false, false, false) => look.fill,
            (false, _, _) => look.fill_hovered,
        };
        let mut border = if self.ghost {
            gpui_kit::transparent_black()
        } else {
            look.border
        };
        let mut foreground = if value.is_placeholder {
            look.placeholder
        } else {
            look.foreground
        };
        if self.disabled {
            fill = fill.opacity(look.disabled_opacity);
            border = border.opacity(look.disabled_opacity);
            foreground = look.placeholder;
        }
        let trigger_bounds = state.read(cx).trigger_bounds().clone();
        h_flex()
            .id(ElementId::NamedChild(
                self.id.clone().into(),
                "trigger".into(),
            ))
            .test_support()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .h(look.geometry.height)
            .px(look.geometry.padding)
            .rounded(look.geometry.radius)
            .bg(fill)
            .border_1()
            .border_color(border)
            .text_size(look.geometry.text_size)
            .text_color(foreground)
            .whitespace_nowrap()
            .relative()
            .when(focus_visible, |this| {
                this.shadow(vec![gpui_kit::BoxShadow {
                    color: look.ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: look.ring_spread,
                    inset: false,
                }])
            })
            .map(|this| {
                if interactive && pointer_cursors {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .map(|this| match &self.render_value {
                Some(render) => {
                    let items: Vec<SelectItem<V>> = state.read(cx).selected_items().to_vec();
                    this.child(div().min_w_0().child(render(&items, window, cx)))
                }
                None => this
                    .when_some(value.leading, |this, leading| {
                        this.child(div().flex_shrink_0().child(leading))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(value.text),
                    ),
            })
            .when(self.chevron, |this| {
                // At the trailing edge when the trigger is wider than its
                // value, as shadcn's `justify-between` trigger puts it.
                this.child(
                    div().ml_auto().flex_none().child(
                        Icon::from(IconName::ChevronDown)
                            .size_4()
                            .text_color(look.indicator),
                    ),
                )
            })
            // The recorder spans the padding box, so the hairline around
            // the trigger is added back.
            .on_prepaint(move |bounds, _, _| trigger_bounds.set(bounds.dilate(px(1.))))
            .on_hover({
                let hovered = hovered.clone();
                move |is_hovered, _, cx| {
                    hovered.update(cx, |state, cx| {
                        if *state != *is_hovered {
                            *state = *is_hovered;
                            cx.notify();
                        }
                    });
                }
            })
            .when(interactive, |this| {
                let state = state.clone();
                this.on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| {
                    // The press is the trigger's: it must not move focus
                    // with the pointer, start the window text selection, or
                    // reach what is behind the trigger.
                    window.prevent_default();
                    cx.stop_propagation();
                    GlobalState::suppress_text_selection(cx);
                    state.update(cx, |state, cx| {
                        // A press on the trigger while the menu is open
                        // reaches the menu's outside-press first, which
                        // closes it; that press must not open it again.
                        if state.is_open() != open {
                            return;
                        }
                        if open {
                            state.close(window, cx);
                        } else {
                            state.open(window, cx);
                        }
                    });
                })
            })
            .into_any_element()
    }

    /// The menu, deferred over the page, while it is open or coming in:
    /// it fades up while it slides the last step down from the trigger,
    /// and goes out in one frame.
    fn menu(
        &self,
        look: &MenuLook,
        open: bool,
        pointer_cursors: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let motion = MenuMotion::sample(
            ElementId::NamedChild(self.id.clone().into(), "presence".into()),
            open,
            true,
            look,
            window,
            cx,
        )?;
        let state = &self.state;
        let (bounds, placement, content_focus) = {
            let state = state.read(cx);
            (
                state.trigger_bounds().get(),
                state.menu_placement().unwrap_or(Placement::Bottom),
                state.content_focus().clone(),
            )
        };
        let menu = Menu {
            rows: Rows {
                id: self.id.clone(),
                state: state.clone(),
                look: look.clone(),
                render_item: self.render_item.clone(),
                render_label: self.render_label.clone(),
                pointer_cursors,
            },
            content_focus,
            width: self.menu_width,
            trigger_width: bounds.size.width,
            empty_text: self.empty_text.clone(),
            render_empty: self.render_empty.clone(),
            render_loading: self.render_loading.clone(),
            search_menu_builder: self.search_context_menu.clone(),
            search_menu_enabled: self.search_context_menu_enabled,
            motion,
        }
        .render(window, cx);
        // A corner anchor clamps into the window but never flips, so the
        // side the state chose when the menu opened holds while a search
        // shortens the rows.
        let (anchor, position) = corner(placement, self.align, bounds, look.gap);
        Some(
            deferred(Positioner::corner(anchor, position).occlude().child(menu))
                .with_priority(base::POPUP_PRIORITY)
                .into_any_element(),
        )
    }
}

impl<V: SelectValue> RenderOnce for Select<V> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let (trigger_look, menu_look) = {
            let theme = cx.theme();
            (
                TriggerLook::of(self.size, theme),
                MenuLook::of(theme, window.rem_size()),
            )
        };
        let state = self.state.clone();
        let (open, trigger_focus, content_focus) = {
            let state = state.read(cx);
            (
                state.is_open(),
                state.trigger_focus().clone(),
                state.content_focus_handle(cx),
            )
        };
        let interactive = !self.disabled;

        let value = self.value(window, cx);
        let accessibility_value = (!value.is_placeholder).then(|| value.text.clone());
        let trigger = self.trigger(&trigger_look, value, open, pointer_cursors, window, cx);
        let menu = self.menu(&menu_look, open, pointer_cursors, window, cx);

        // The handle's tab index is window state, so setting it on a clone
        // sets it for the trigger.
        let trigger_focus = trigger_focus.tab_index(self.tab_index);
        let root = base::Select::new(self.id.clone())
            .open(open)
            .disabled(self.disabled)
            .focus_handle(&trigger_focus)
            .content_focus_handle(&content_focus)
            .when_some(self.accessibility_label, |this, label| {
                this.accessibility_label(label)
            })
            .when_some(accessibility_value, |this, value| {
                this.accessibility_value(value)
            })
            .on_open_change({
                let state = state.clone();
                move |next, window, cx| {
                    state.update(cx, |state, cx| {
                        if next {
                            state.open(window, cx);
                        } else {
                            state.close(window, cx);
                        }
                    });
                }
            })
            .on_confirm({
                let state = state.clone();
                move |window, cx| {
                    state.update(cx, |state, cx| state.choose_highlighted(window, cx));
                    // Base moves focus into the menu after a confirm, for a
                    // menu that stays open. One that closed on it wants the
                    // trigger back, once base is done.
                    let state = state.clone();
                    window.defer(cx, move |window, cx| {
                        let (closed, trigger_focus) = {
                            let state = state.read(cx);
                            (!state.is_open(), state.trigger_focus().clone())
                        };
                        if closed {
                            trigger_focus.focus(window, cx);
                        }
                    });
                }
            })
            .child(trigger)
            .children(menu);

        // Base opens the menu on the arrows and moves focus into it; once
        // it is open the arrows walk the rows. `open` is the state at
        // render, so the press that opens the menu does not also move.
        div()
            .id(ElementId::NamedChild(self.id.clone().into(), "root".into()))
            .key_context(CONTEXT)
            .flex_shrink_0()
            .refine_style(&self.style)
            .when(interactive && open, |this| {
                this.on_action({
                    let state = state.clone();
                    move |_: &SelectUp, _, cx| {
                        state.update(cx, |state, cx| state.move_highlight(-1, cx));
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &SelectDown, _, cx| {
                        state.update(cx, |state, cx| state.move_highlight(1, cx));
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &SelectFirst, _, cx| {
                        state.update(cx, |state, cx| state.highlight_first(cx));
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &SelectLast, _, cx| {
                        state.update(cx, |state, cx| state.highlight_last(cx));
                    }
                })
            })
            .child(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeConfig, to_hex};
    use gpui_kit::base::ThemeAppearance;

    #[test]
    fn the_trigger_reads_the_reference_values() {
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let trigger = TriggerLook::of(ButtonSize::Default, &theme);
        assert_eq!(to_hex(trigger.fill), "#292929");
        assert_eq!(to_hex(trigger.border), "#3a3a3a");
        assert_eq!(to_hex(trigger.foreground), "#ffffff");
        assert_eq!(trigger.geometry.height, px(28.));
        assert!(trigger.fill_hovered.l > trigger.fill.l);
    }
}
