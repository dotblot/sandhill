use std::rc::Rc;

use gpui_kit::{
    AccessibleAction, AnyElement, App, Edges, ElementId, Entity, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Pixels, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, TextAlign, Window,
    accesskit::ActionData,
    assets::IconName,
    base::{
        Disableable, StyledExt as _, TextStyleToken,
        input::{
            InputBase, InputBaseState, InputContextMenuCapabilities, InputEditorStyle, InputMode,
            InputModeKind, TextareaMode,
        },
    },
    div,
    prelude::FluentBuilder as _,
};

use crate::{
    ActiveTheme as _, Button, ButtonSize, MenuEntry, MenuState, Theme,
    menu::{MenuPanels, TextMenuBuilder, open_text_menu},
    touch_selection,
};

/// Everything the render needs from the theme, read in one borrow.
struct Look {
    fill: Hsla,
    border: Hsla,
    focus_border: Hsla,
    foreground: Hsla,
    placeholder: Hsla,
    selection: Hsla,
    radius: Pixels,
    height: Pixels,
    padding_x: Pixels,
    padding_y: Pixels,
    text: TextStyleToken,
}

impl Look {
    /// A multi-line field takes the looser textarea text and the settings
    /// textarea's side padding; a single line takes the search field's.
    fn read<M: InputModeKind>(disabled: bool, cx: &App) -> Self {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let strength = |color: Hsla| {
            if disabled {
                color.opacity(theme.disabled_opacity)
            } else {
                color
            }
        };
        Self {
            fill: strength(theme.field),
            border: strength(theme.field_border),
            focus_border: theme.field_focus_border,
            foreground: theme.foreground(),
            placeholder: theme.muted_foreground(),
            selection: theme.selection(),
            radius: theme.radius_md(),
            height: metrics.field_height,
            padding_x: if M::MULTI_LINE {
                metrics.control_padding_md
            } else {
                metrics.field_padding_x
            },
            padding_y: metrics.field_padding_y,
            text: if M::MULTI_LINE {
                theme.text_textarea
            } else {
                theme.text_control
            },
        }
    }
}

/// A shadcn-style text field on `gpui_base`'s editing engine, in the
/// kind `M` names: [`Input`] for one line, [`Textarea`] for many.
///
/// The state owns the text, the caret, the selection, focus, the
/// placeholder, the disabled and read-only flags, and the events. This
/// type owns the look: the fill, the hairline that takes the focus color
/// while the caret is inside, the height, and where a prefix and a
/// suffix sit. Each render projects the theme's text colors onto the
/// engine.
///
/// `Styled` refinements apply after the tokens, so they win.
#[derive(IntoElement)]
#[non_exhaustive]
pub struct Field<M: InputModeKind> {
    state: Entity<InputBaseState<M>>,
    id: ElementId,
    style: StyleRefinement,
    disabled: Option<bool>,
    readonly: Option<bool>,
    accessibility_label: Option<SharedString>,
    prefix: Option<AnyElement>,
    suffix: Option<AnyElement>,
    cleanable: bool,
    mask_toggle: bool,
    context_menu: Option<TextMenuBuilder>,
    context_menu_enabled: bool,
}

/// A single-line text field on an [`InputState`](crate::InputState).
///
/// ```no_run
/// use gpui_cn::{Input, InputState};
/// use gpui_kit::{AppContext as _, Context, Window};
///
/// fn field(window: &mut Window, cx: &mut Context<()>) -> Input {
///     let state = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
///     Input::new(&state)
/// }
/// ```
pub type Input = Field<InputMode>;

/// A multi-line text field on a [`TextareaState`](crate::TextareaState).
///
/// The state's `auto_grow(min, max)` sets the height in rows, and
/// `auto_grow(n, n)` fixes it at `n`. `Styled` on this type (`.h(..)`,
/// `.min_h(..)`) refines the frame after the tokens.
///
/// ```no_run
/// use gpui_cn::{Textarea, TextareaState};
/// use gpui_kit::{AppContext as _, Context, Window};
///
/// fn field(window: &mut Window, cx: &mut Context<()>) -> Textarea {
///     let state = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8));
///     Textarea::new(&state)
/// }
/// ```
pub type Textarea = Field<TextareaMode>;

impl<M: InputModeKind> Field<M> {
    /// A field on `state`. The id is derived from the state.
    pub fn new(state: &Entity<InputBaseState<M>>) -> Self {
        Self {
            state: state.clone(),
            id: ("input", state.entity_id()).into(),
            style: StyleRefinement::default(),
            disabled: None,
            readonly: None,
            accessibility_label: None,
            prefix: None,
            suffix: None,
            cleanable: false,
            mask_toggle: false,
            context_menu: None,
            context_menu_enabled: true,
        }
    }

    /// The frame's id, for tests and focus.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Whether the text can be selected and copied but not changed.
    /// Pushes onto the state; leave it unset to control the state directly.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = Some(readonly);
        self
    }

    /// The name a screen reader announces. The placeholder is the fallback.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Shapes the menu a right click opens: `build` gets the default rows
    /// (Cut, Copy, Paste, a separator, Select All, enabled from what the
    /// field can do at that moment) and returns the rows to show, the
    /// defaults extended or replaced. No rows, no menu.
    ///
    /// ```no_run
    /// use gpui_cn::{Input, InputState, MenuEntry, MenuItem};
    /// use gpui_kit::Entity;
    ///
    /// fn field(state: &Entity<InputState>) -> Input {
    ///     Input::new(state).context_menu(|mut entries, _, _, _| {
    ///         entries.push(MenuEntry::Separator);
    ///         entries.push(MenuItem::new("insert-date", "Insert Date").into());
    ///         entries
    ///     })
    /// }
    /// ```
    pub fn context_menu(
        mut self,
        build: impl Fn(
            Vec<MenuEntry>,
            InputContextMenuCapabilities,
            &mut Window,
            &mut App,
        ) -> Vec<MenuEntry>
        + 'static,
    ) -> Self {
        self.context_menu = Some(Rc::new(build));
        self
    }

    /// Whether a right click opens the field's menu. On by default.
    pub fn context_menu_enabled(mut self, enabled: bool) -> Self {
        self.context_menu_enabled = enabled;
        self
    }
}

impl Field<InputMode> {
    /// An element before the text, inside the frame, such as an icon.
    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    /// An element after the text, inside the frame, such as a button.
    pub fn suffix(mut self, suffix: impl IntoElement) -> Self {
        self.suffix = Some(suffix.into_any_element());
        self
    }

    /// Whether a button after the text clears it, shown while the field
    /// is editable and holds a value, as a search field does.
    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    /// Whether a button after the text shows and hides a masked value.
    pub fn mask_toggle(mut self, mask_toggle: bool) -> Self {
        self.mask_toggle = mask_toggle;
        self
    }
}

impl<M: InputModeKind> Disableable for Field<M> {
    /// Whether the field ignores input. Pushes onto the state; leave it
    /// unset to control the state directly.
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = Some(disabled);
        self
    }
}

impl<M: InputModeKind> Styled for Field<M> {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl<M: InputModeKind> RenderOnce for Field<M> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let (disabled, readonly) = (self.disabled, self.readonly);
        let text_align = self.style.text.text_align.unwrap_or(TextAlign::Left);
        let context_menu_enabled = self.context_menu_enabled;
        let presentation = self.state.update(cx, |state, cx| {
            state.set_context_menu_enabled(context_menu_enabled);
            if let Some(disabled) = disabled {
                state.set_disabled(disabled, cx);
            }
            if let Some(readonly) = readonly {
                state.set_readonly(readonly, cx);
            }
            state.set_text_align(text_align, cx);
            state.presentation()
        });
        let disabled = presentation.is_disabled();
        let look = Look::read::<M>(disabled, cx);
        let menu_id = ElementId::NamedChild(self.id.clone().into(), "context-menu".into());
        let menu = window.use_keyed_state(menu_id.clone(), cx, |_, cx| MenuState::new(cx));
        let menu_open = menu.read(cx).is_open();
        // Base fills an unset selection from its `accent` token, which
        // gpui-cn projects as the hover surface, so a selection would
        // vanish into the fill.
        self.state.update(cx, |state, _| {
            state.set_editor_style(InputEditorStyle {
                foreground: look.foreground,
                muted_foreground: look.placeholder,
                background: look.fill,
                border: look.border,
                selection: look.selection,
                caret: look.foreground,
                ..InputEditorStyle::default()
            });
            if M::MULTI_LINE {
                // The text pads itself so its scrollbar and caret stay
                // inside the hairline.
                state.set_editor_paddings(Edges {
                    top: look.padding_y,
                    right: look.padding_x,
                    bottom: look.padding_y,
                    left: look.padding_x,
                });
            }
        });

        // The frame keeps its own focus handle so a control inside it,
        // such as a suffix button, keeps the focused look.
        let frame_handle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focused = !disabled && frame_handle.contains_focused(window, cx);
        let placeholder = Some(presentation.placeholder().clone()).filter(|p| !p.is_empty());
        let label = self.accessibility_label.or_else(|| placeholder.clone());
        // Assistive technology reads the value from the frame; a masked
        // value stays private. Outside an a11y session the rope is not
        // copied, except in tests, which read the same value.
        let value = ((window.is_a11y_active() || cfg!(feature = "test-support"))
            && !presentation.is_masked())
        .then(|| state_value(&self.state, cx));
        let state = self.state;
        let set_value = state.clone();
        let touch_selection = touch_selection::for_state(&state, window, cx);
        let clear = (self.cleanable
            && presentation.is_editable()
            && state.read(cx).text().len() > 0)
            .then(|| {
                let state = state.clone();
                Button::new("clear")
                    .ghost()
                    .size(ButtonSize::Xs)
                    .icon(IconName::CircleX)
                    .accessibility_label("Clear")
                    .tab_stop(false)
                    .text_color(look.placeholder)
                    .on_click(move |_, window, cx| {
                        state.update(cx, |state, cx| {
                            state.clean(window, cx);
                            state.focus(window, cx);
                        })
                    })
            });
        let mask_toggle = self.mask_toggle.then(|| {
            let state = state.clone();
            let masked = presentation.is_masked();
            Button::new("mask-toggle")
                .ghost()
                .size(ButtonSize::Xs)
                .icon(if masked {
                    IconName::Eye
                } else {
                    IconName::EyeOff
                })
                .accessibility_label(if masked { "Show value" } else { "Hide value" })
                .tab_stop(false)
                .text_color(look.placeholder)
                .on_click(move |_, window, cx| {
                    state.update(cx, |state, cx| state.toggle_masked(window, cx))
                })
        });
        let suffix =
            (self.suffix.is_some() || clear.is_some() || mask_toggle.is_some()).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(self.suffix)
                    .children(mask_toggle)
                    .children(clear)
                    .into_any_element()
            });

        InputBase::new(self.id)
            .focused(focused)
            .disabled(disabled)
            .track_focus(&frame_handle)
            .role(if M::MULTI_LINE {
                Role::MultilineTextInput
            } else {
                Role::TextInput
            })
            .when_some(label, |this, label| this.accessibility_label(label))
            .when_some(placeholder, |this, placeholder| {
                this.aria_placeholder(placeholder)
            })
            .when_some(value, |this, value| this.aria_value(value))
            .when(presentation.is_editable(), |this| {
                this.on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                    if let Some(ActionData::Value(value)) = data {
                        let value = value.to_string();
                        set_value.update(cx, |state, cx| state.replace_all(value, window, cx));
                    }
                })
            })
            // Base keeps a disabled field focusable; the frame refuses the
            // click before it can move focus into the text.
            .when(disabled, |this| {
                this.capture_any_mouse_down(|_, window, _| window.prevent_default())
            })
            // The field opens its own menu: base's hook stays off while any
            // popup is open. The press is the field's, not a context menu's
            // around it.
            .when(!disabled && context_menu_enabled, |this| {
                let (menu, input, builder) = (menu.clone(), state.clone(), self.context_menu);
                this.on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_up(MouseButton::Right, move |event, window, cx| {
                        open_text_menu(&menu, &input, event.position, builder.as_ref(), window, cx);
                    })
            })
            // A click on the padding or an affix puts the caret in the
            // text instead of focusing the frame. A control inside the
            // frame that prevents default keeps the click.
            .when(!disabled, |this| {
                let state = state.clone();
                this.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if !window.default_prevented() {
                        state.update(cx, |state, cx| state.focus(window, cx));
                        window.prevent_default();
                    }
                })
            })
            .relative()
            .flex()
            .w_full()
            .map(|this| {
                if M::MULTI_LINE {
                    this.flex_col().h_auto().min_h(look.height)
                } else {
                    this.items_center().h(look.height).px(look.padding_x)
                }
            })
            .text_size(look.text.size)
            .line_height(look.text.line_height)
            .font_weight(look.text.weight)
            .text_color(look.foreground)
            .rounded(look.radius)
            .bg(look.fill)
            .border_1()
            .border_color(look.border)
            .styles(|styles| styles.focused(|style| style.border_color(look.focus_border)))
            .map(|this| {
                if presentation.is_editable() && pointer_cursors {
                    this.cursor_text()
                } else {
                    this.cursor_default()
                }
            })
            .refine_style(&self.style)
            // The reference app's search field leaves 12px between the
            // icon and the text, measured at 2x.
            .gap_3()
            .children(self.prefix.map(|prefix| affix(prefix, disabled, cx)))
            .child(div().relative().flex().flex_1().min_w_0().child(state))
            .children(suffix.map(|suffix| affix(suffix, disabled, cx)))
            .children(touch_selection)
            .when(menu_open, |this| {
                this.child(MenuPanels::new(menu_id, &menu))
            })
    }
}

/// A prefix or suffix, at the disabled strength with the field.
fn affix(element: AnyElement, disabled: bool, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .when(disabled, |this| this.opacity(cx.theme().disabled_opacity))
        .child(element)
}

/// The text as a string, for assistive technology.
fn state_value<M: InputModeKind>(state: &Entity<InputBaseState<M>>, cx: &App) -> String {
    state.read(cx).text().to_string()
}
