use gpui_kit::{
    App, ElementId, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    TestSupportExt as _, Window,
    base::{
        Disableable, StyledExt as _,
        input::{InputBaseState, InputModeKind},
    },
    div,
    prelude::FluentBuilder as _,
};

use crate::ActiveTheme as _;

type FocusField = Box<dyn Fn(&mut Window, &mut App)>;

/// A shadcn-style form label: shadcn's `text-sm font-medium leading-none`
/// moved to the theme's 13px control text, in the foreground color, with
/// no margin of its own.
///
/// [`for_field`](Self::for_field) ties the label to a text field, so a
/// click on the label moves focus into the field, as a `for` attribute
/// does. A disabled label paints its text at the disabled strength,
/// shadcn's `peer-disabled:opacity-50`, and ignores clicks.
///
/// ```
/// use gpui_cn::Label;
///
/// let _ = Label::new("email-label", "Email");
/// ```
#[derive(IntoElement)]
#[non_exhaustive]
pub struct Label {
    id: ElementId,
    text: SharedString,
    style: StyleRefinement,
    disabled: bool,
    focus_field: Option<FocusField>,
}

impl Label {
    /// A label with a stable id and its text.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            style: StyleRefinement::default(),
            disabled: false,
            focus_field: None,
        }
    }

    /// The field a click on the label focuses.
    pub fn for_field<M: InputModeKind>(mut self, state: &Entity<InputBaseState<M>>) -> Self {
        let state = state.clone();
        self.focus_field = Some(Box::new(move |window, cx| {
            state.update(cx, |state, cx| state.focus(window, cx));
        }));
        self
    }
}

impl Disableable for Label {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for Label {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Label {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (text, color) = {
            let theme = cx.theme();
            let color = theme.foreground();
            (
                theme.text_control,
                if self.disabled {
                    color.opacity(theme.disabled_opacity)
                } else {
                    color
                },
            )
        };
        div()
            .id(self.id)
            .test_support()
            .text_size(text.size)
            .line_height(text.line_height)
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .cursor_default()
            .when_some(
                self.focus_field.filter(|_| !self.disabled),
                |this, focus_field| this.on_click(move |_, window, cx| focus_field(window, cx)),
            )
            .refine_style(&self.style)
            .child(self.text)
    }
}
