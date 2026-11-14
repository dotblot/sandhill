//! A floating panel under a trigger, as shadcn's `Popover`.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    AnyElement, App, Bounds, ElementId, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, StyleRefinement, Styled, Window,
    base::{
        self, Align, ElementExt as _, Placement, Selectable, StyledExt as _, TestSupportExt as _,
    },
    div,
};

use crate::{
    ActiveTheme as _,
    menu::{MenuLook, MenuMotion, corner, placement},
};

type TriggerBuilder = Box<dyn FnOnce(bool) -> AnyElement>;
type ContentBuilder = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;
type OpenChange = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// A panel of any content that opens under a trigger, as shadcn's
/// `Popover`.
///
/// `gpui-base` owns the behavior: a press on the trigger, or Enter or
/// Space while it has focus, opens and closes it; Escape and a press
/// outside close it; focus moves into the panel and back to the trigger.
/// This component draws the panel as a menu's: the popover surface, its
/// hairline, the large radius, the shadow, and the menu's entrance. The
/// panel opens a menu gap under the trigger, lined up with its leading
/// edge unless [`align`](Self::align) says otherwise, and holds the
/// menu's inset around the content.
///
/// ```
/// use gpui_cn::{Button, Popover};
/// use gpui_kit::{IntoElement, ParentElement as _, div};
///
/// fn details() -> impl IntoElement {
///     Popover::new("details")
///         .trigger(Button::new("details-trigger").label("Details"))
///         .content(|_, _| div().child("Built on Tuesday"))
/// }
/// ```
///
/// Without [`open`](Self::open) the popover keeps its own open state,
/// keyed by the id, which must be stable across frames.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    trigger: Option<TriggerBuilder>,
    content: Option<ContentBuilder>,
    align: Align,
    open: Option<bool>,
    on_open_change: Option<OpenChange>,
    focus: Option<FocusHandle>,
    style: StyleRefinement,
}

impl Popover {
    /// A closed popover with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            trigger: None,
            content: None,
            align: Align::Start,
            open: None,
            on_open_change: None,
            focus: None,
            style: StyleRefinement::default(),
        }
    }

    /// The element that opens the popover, such as a [`Button`](crate::Button)
    /// without a click handler of its own. It shows as selected while the
    /// popover is open.
    pub fn trigger(mut self, trigger: impl Selectable + IntoElement + 'static) -> Self {
        self.trigger = Some(Box::new(move |open| trigger.open(open).into_any_element()));
        self
    }

    /// Builds the panel's content each frame it is open.
    pub fn content<E: IntoElement>(
        mut self,
        content: impl FnOnce(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.content = Some(Box::new(move |window, cx| {
            content(window, cx).into_any_element()
        }));
        self
    }

    /// Which edge of the trigger the panel lines up with. The default is
    /// `Start`.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Opens or closes the popover from the application's own state. The
    /// trigger, Escape, and an outside press report through
    /// [`on_open_change`](Self::on_open_change), which should update it.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Called when the trigger, Escape, or an outside press opens or
    /// closes the popover.
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Box::new(handler));
        self
    }

    /// The element that takes focus when the popover opens, such as a
    /// search field inside it. Without one the panel itself takes it.
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

/// Styles the panel.
impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = MenuLook::of(cx.theme(), window.rem_size());
        // Where the trigger was last laid out, which decides the side.
        let trigger_bounds = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "trigger-bounds".into()),
                cx,
                |_, _| Rc::new(Cell::new(Bounds::default())),
            )
            .read(cx)
            .clone();
        let side = placement(
            trigger_bounds.get(),
            window.viewport_size().height,
            look.max_height,
            look.gap,
        );
        let opens_above = side == Placement::Top;
        let (anchor, _) = corner(side, self.align, trigger_bounds.get(), look.gap);
        let panel_id = ElementId::NamedChild(self.id.clone().into(), "panel".into());
        let content = self.content;
        let style = self.style;
        let mut popover = base::Popover::new(self.id)
            .anchor(anchor)
            .offset(look.gap)
            .trigger_with(move |open, _, _| {
                div()
                    .relative()
                    .on_prepaint(move |bounds, _, _| trigger_bounds.set(bounds))
                    .children(self.trigger.map(|trigger| trigger(open)))
                    .into_any_element()
            })
            .content(move |_, window, cx| {
                let Some(motion) = MenuMotion::sample(
                    ElementId::NamedChild(panel_id.clone().into(), "presence".into()),
                    true,
                    true,
                    &look,
                    window,
                    cx,
                ) else {
                    return div().into_any_element();
                };
                div()
                    .id(panel_id)
                    .test_support()
                    .relative()
                    // A panel above its trigger slides up into place.
                    .top(if opens_above {
                        -motion.offset
                    } else {
                        motion.offset
                    })
                    .opacity(motion.opacity)
                    .p(look.padding)
                    .rounded(look.radius)
                    .bg(look.surface)
                    .border_1()
                    .border_color(look.border)
                    .shadow(look.shadow(motion.shadow_strength))
                    .text_size(look.text_size)
                    .line_height(look.line_height)
                    .text_color(look.foreground)
                    .refine_style(&style)
                    .children(content.map(|content| content(window, cx)))
                    .into_any_element()
            });
        if let Some(open) = self.open {
            popover = popover.open(open);
        }
        if let Some(handler) = self.on_open_change {
            popover = popover.on_open_change(move |open, window, cx| handler(*open, window, cx));
        }
        if let Some(focus) = &self.focus {
            popover = popover.track_focus(focus);
        }
        popover
    }
}
