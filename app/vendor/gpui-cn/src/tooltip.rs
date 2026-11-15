use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Div, ElementId,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Pixels, Render,
    SharedString, StatefulInteractiveElement, Styled as _, Window,
    base::{
        ElementExt as _, Keyframe, Keyframes, Placement, TestSupportExt as _, Timing,
        Tooltip as BaseTooltip, TooltipRequest, TooltipTransition, animate_keyframes,
    },
    div,
};

use crate::{ActiveTheme as _, Theme, TooltipHost};
use gpui_kit::{StyledText, TextLayout};

/// A label rendered as a `StyledText` so its layout can be read back after
/// the frame that painted it: a label the layout cut short shows its full
/// text as a tooltip. Split it with [`into_parts`](Self::into_parts) and
/// put the text in the element tree.
pub(crate) struct TruncatedLabel {
    label: SharedString,
    text: StyledText,
}

impl TruncatedLabel {
    pub(crate) fn new(label: SharedString) -> Self {
        Self {
            text: StyledText::new(label.clone()),
            label,
        }
    }

    /// The text to render and the check that reads its layout back.
    pub(crate) fn into_parts(self) -> (StyledText, TruncationCheck) {
        let layout = self.text.layout().clone();
        (
            self.text,
            TruncationCheck {
                label: self.label,
                layout,
            },
        )
    }
}

/// Whether a [`TruncatedLabel`] was cut short in the last frame.
#[derive(Clone)]
pub(crate) struct TruncationCheck {
    label: SharedString,
    layout: TextLayout,
}

impl TruncationCheck {
    pub(crate) fn is_truncated(&self) -> bool {
        self.layout.text() != self.label.as_ref()
    }

    /// The hover rule of a truncation tooltip: ask to show only while the
    /// label is truncated, and always pass a leave on so a tooltip that
    /// showed hides.
    pub(crate) fn hovered(
        &self,
        trigger: &TooltipTrigger,
        hovered: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.is_truncated() || !hovered {
            trigger.hovered(hovered, window, cx);
        }
    }
}

thread_local! {
    /// One ramp, `0.` to `1.`, built once per thread (keyframes hold an
    /// `Rc` easing, so they cannot be a global static).
    static RAMP: Keyframes<f32> =
        Keyframes::try_new([Keyframe::new(0., 0f32), Keyframe::new(1., 1f32)])
            .expect("two ordered stops");
}

/// A short text tooltip, the content a trigger builds for the overlay.
///
/// It draws the gpui-cn tooltip surface itself, so it looks the same
/// through the overlay gpui-cn adds to each `Root` and as GPUI's native tooltip
/// in a window without one.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    /// A tooltip with `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }

    /// Builds the tooltip as the view the overlay expects.
    pub fn build(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
        let text = text.into();
        cx.new(|_| Self::new(text)).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Observed so UI tests can find the shown tooltip by id.
        div()
            .id("gpui-cn-tooltip")
            .test_support()
            .child(surface(cx).child(self.text.clone()))
    }
}

/// The tooltip surface as the reference app draws it: a 30px box
/// on the `tooltip` token with the large radius, a hairline, 13px text,
/// and 13px side padding. Any custom tooltip view can wrap itself in it.
pub fn surface(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex()
        .items_center()
        .h(theme.metrics.row)
        .px(theme.text_control.size)
        .rounded(theme.radius_lg())
        .bg(theme.tooltip)
        .text_color(theme.tooltip_foreground)
        .border_1()
        .border_color(theme.border())
        .text_size(theme.text_control.size)
        .whitespace_nowrap()
}

/// Positions and fades any tooltip view. Installed on the base overlay by
/// the overlay gpui-cn adds to each `Root`.
///
/// A new tooltip fades in over `motion.normal`. Switching between
/// neighboring triggers is immediate. The view draws its own surface.
pub(crate) fn render_surface(
    view: AnyView,
    transition: TooltipTransition,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let normal = Theme::global(cx).motion.normal;
    let opacity = match transition {
        TooltipTransition::Enter { epoch } if !normal.is_zero() => RAMP.with(|ramp| {
            animate_keyframes(
                ElementId::from(("gpui-cn-tooltip-enter", epoch)),
                ramp,
                Timing::new(normal),
                window,
                cx,
            )
            .value
        }),
        _ => 1.,
    };
    BaseTooltip::new("gpui-cn-tooltip-positioner")
        .opacity(opacity)
        .child(view)
        .into_any_element()
}

/// The trigger side of a managed tooltip, for a component that owns its
/// own hover listener (GPUI allows one per element).
///
/// Record the trigger's bounds with [`track_bounds`](Self::track_bounds),
/// forward the hover listener to [`hovered`](Self::hovered), and call
/// [`pressed`](Self::pressed) on mouse down. In a window with no registered
/// overlay use [`native`](Self::native) as GPUI's tooltip builder instead.
/// [`Button`](crate::Button) shows the intended use.
pub struct TooltipTrigger {
    owner: ElementId,
    build: Rc<dyn Fn(&mut App) -> AnyView>,
    placement: Option<Placement>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl TooltipTrigger {
    /// A trigger owned by the element `owner`, building `build` when shown.
    /// The owner id is stable across frames, and only the trigger that
    /// asked for the tooltip hides it on leave.
    pub fn new(
        owner: ElementId,
        placement: Option<Placement>,
        build: impl Fn(&mut App) -> AnyView + 'static,
    ) -> Self {
        Self {
            owner,
            build: Rc::new(build),
            placement,
            bounds: Rc::new(Cell::new(Bounds::default())),
        }
    }

    /// A trigger for a [`Tooltip`] with `text`.
    pub fn text(owner: ElementId, placement: Option<Placement>, text: SharedString) -> Self {
        Self::new(owner, placement, move |cx| Tooltip::build(text.clone(), cx))
    }

    /// The prepaint callback that records the trigger's bounds.
    pub fn track_bounds(&self) -> impl Fn(Bounds<Pixels>, &mut Window, &mut App) + 'static {
        let bounds = self.bounds.clone();
        move |resolved, _, _| bounds.set(resolved)
    }

    /// Call from the trigger's hover listener.
    pub fn hovered(&self, hovered: bool, window: &mut Window, cx: &mut App) {
        let Some(overlay) = TooltipHost::overlay(window, cx) else {
            return;
        };
        if hovered {
            let build = self.build.clone();
            let request = TooltipRequest::new(self.bounds.get(), move |_, cx| build(cx));
            let request = match self.placement {
                Some(placement) => request.with_placement(placement),
                None => request,
            };
            TooltipHost::note_requester(window, &self.owner, cx);
            overlay.update(cx, |overlay, cx| overlay.request_show(request, window, cx));
        } else if TooltipHost::is_requester(window, &self.owner, cx) {
            overlay.update(cx, |overlay, cx| overlay.request_hide(window, cx));
        }
    }

    /// Call from the trigger's mouse-down listener.
    pub fn pressed(window: &mut Window, cx: &mut App) {
        if let Some(overlay) = TooltipHost::overlay(window, cx) {
            overlay.update(cx, |overlay, cx| overlay.hide(cx));
        }
    }

    /// The native tooltip builder for a window with no overlay.
    pub fn native(&self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let build = self.build.clone();
        move |_, cx| build(cx)
    }
}

/// Gives a stateful element a managed tooltip.
///
/// The tooltip shows through the window's registered base overlay (see
/// [`TooltipHost`]) after the overlay's delay, follows the trigger's bounds,
/// hides on press, and is suppressed on iOS and Android by base. In a window
/// with no registered overlay the element gets GPUI's native tooltip
/// instead, so the text is never lost. The window decides which, so the
/// methods take it.
///
/// The managed tooltip installs the element's hover listener. An element
/// that sets `on_hover` itself uses [`TooltipTrigger`] directly instead, as
/// gpui-cn components do.
pub trait TooltipExt: StatefulInteractiveElement + Sized {
    /// A text tooltip.
    fn managed_tooltip(self, text: impl Into<SharedString>, window: &Window, cx: &App) -> Self {
        let text = text.into();
        self.managed_tooltip_with(None, move |cx| Tooltip::build(text.clone(), cx), window, cx)
    }

    /// A text tooltip that prefers `placement`, falling back when it does
    /// not fit.
    fn managed_tooltip_at(
        self,
        placement: Placement,
        text: impl Into<SharedString>,
        window: &Window,
        cx: &App,
    ) -> Self {
        let text = text.into();
        self.managed_tooltip_with(
            Some(placement),
            move |cx| Tooltip::build(text.clone(), cx),
            window,
            cx,
        )
    }

    /// A tooltip with an arbitrary view.
    fn managed_tooltip_with(
        self,
        placement: Option<Placement>,
        build: impl Fn(&mut App) -> AnyView + 'static,
        window: &Window,
        cx: &App,
    ) -> Self;
}

impl<E: StatefulInteractiveElement + gpui_kit::ParentElement> TooltipExt for E {
    fn managed_tooltip_with(
        mut self,
        placement: Option<Placement>,
        build: impl Fn(&mut App) -> AnyView + 'static,
        window: &Window,
        cx: &App,
    ) -> Self {
        let owner = self
            .interactivity()
            .element_id
            .clone()
            .expect("a stateful element has an id");
        let trigger = TooltipTrigger::new(owner, placement, build);
        if TooltipHost::overlay(window, cx).is_none() {
            return self.tooltip(trigger.native());
        }
        self.on_prepaint(trigger.track_bounds())
            .on_hover(move |hovered, window, cx| trigger.hovered(*hovered, window, cx))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                TooltipTrigger::pressed(window, cx);
            })
    }
}
