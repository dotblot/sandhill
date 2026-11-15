//! Navigation between pages: a stack of views with back and forward.
//!
//! `gpui-base` owns the stack ([`NavStackState`]) and the lifecycle of a
//! change. gpui-cn adds how a change looks ([`NavStack`]) and the pair of
//! arrows that drive it ([`NavButtons`]).
//!
//! ```
//! use gpui_cn::{NavButtons, NavStack, NavStackState};
//! use gpui_kit::{Entity, IntoElement, ParentElement as _, Styled as _, div};
//!
//! fn window(stack: &Entity<NavStackState>) -> impl IntoElement {
//!     div()
//!         .child(NavButtons::new("nav", stack))
//!         .child(NavStack::new(stack).size_full())
//! }
//! ```

use std::time::Duration;

use gpui_kit::{
    AnyView, App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, StyleRefinement, Styled, Window,
    base::{self, Disableable as _, PresencePhase, StyledExt as _},
    div,
    prelude::FluentBuilder as _,
};

pub use gpui_kit::base::{NavMotion, NavOperation, NavPage, NavStackEvent, NavStackState};

use crate::{Button, Icon, Theme};
use gpui_kit::assets::IconName;

/// Navigation that also frees pages.
///
/// A popped page waits in the stack's forward history until the next
/// push, so its view stays alive. These methods drop that history, so a
/// page the application will not bring back releases its state.
pub trait NavStackExt {
    /// Drops the forward history at once, keeping the views from the root
    /// to the current page. The stack reports `Cleared` and then one
    /// `Pushed` per kept page. A running transition is abandoned.
    fn discard_forward(&self, cx: &mut App);

    /// Pops the top view like [`NavStackState::pop`], then drops it from
    /// the forward history once its exit transition has run, so `forward`
    /// cannot bring it back and nothing keeps it alive. Returns the popped
    /// view, or `None` at the root.
    fn pop_and_discard(&self, motion: NavMotion, cx: &mut App) -> Option<AnyView>;
}

impl NavStackExt for Entity<NavStackState> {
    fn discard_forward(&self, cx: &mut App) {
        self.update(cx, |stack, cx| {
            if stack.forward_views().len() == 0 {
                return;
            }
            let views: Vec<AnyView> = stack.views().cloned().collect();
            stack.clear(cx);
            for view in views {
                stack.push(view, NavMotion::Immediate, cx);
            }
        });
    }

    fn pop_and_discard(&self, motion: NavMotion, cx: &mut App) -> Option<AnyView> {
        let popped = self.update(cx, |stack, cx| stack.pop(motion, cx))?;
        // The transition samples the outgoing view until it is done, so
        // the history can only go once it has run. Discarding it earlier
        // would abandon the motion.
        let animated = motion == NavMotion::Animated && !cx.reduce_motion();
        if !animated {
            self.discard_forward(cx);
            return Some(popped);
        }
        let delay = Theme::global(cx).motion.fast + Duration::from_millis(16);
        let stack = self.clone();
        let waiting = popped.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            cx.update(|cx| {
                // Only the page that was popped waits in front; a push in
                // the meantime already dropped it, and a `forward` brought
                // it back on purpose.
                let still_waiting = stack.read(cx).forward_views().next() == Some(&waiting);
                if still_waiting {
                    stack.discard_forward(cx);
                }
            });
        })
        .detach();
        Some(popped)
    }
}

/// The pages of a [`NavStackState`], one visible at a time.
///
/// A change crossfades over the theme's fast transition. Under reduced
/// motion it switches at once. The stack fills its parent, so give it a
/// sized one.
#[derive(IntoElement)]
pub struct NavStack {
    state: Entity<NavStackState>,
    style: StyleRefinement,
}

impl NavStack {
    /// The pages of `state`.
    pub fn new(state: &Entity<NavStackState>) -> Self {
        Self {
            state: state.clone(),
            style: StyleRefinement::default(),
        }
    }
}

impl Styled for NavStack {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for NavStack {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let transition = Theme::global(cx).motion.fast_transition();
        base::NavStack::new(&self.state)
            .size_full()
            .refine_style(&self.style)
            .transition(transition)
            .item(|page, _, _| {
                let opacity = match page.phase() {
                    PresencePhase::Entering => page.progress(),
                    PresencePhase::Exiting => 1. - page.progress(),
                    PresencePhase::Present => 1.,
                    PresencePhase::Absent => 0.,
                };
                page.opacity(opacity).into_any_element()
            })
    }
}

/// Back and forward arrows for a [`NavStackState`].
///
/// Two icon-only ghost [`Button`]s. Back is enabled while there is a page
/// under the current one, forward while a popped page waits to come
/// back. Each moves the stack with the animated motion.
#[derive(IntoElement)]
pub struct NavButtons {
    id: ElementId,
    state: Entity<NavStackState>,
    tooltips: bool,
    back_icon: Icon,
    forward_icon: Icon,
}

impl NavButtons {
    /// Arrows for `state`. The id keys the two buttons.
    pub fn new(id: impl Into<ElementId>, state: &Entity<NavStackState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            tooltips: true,
            back_icon: IconName::ArrowLeft.into(),
            forward_icon: IconName::ArrowRight.into(),
        }
    }

    /// The back arrow, in place of Lucide's `arrow-left`, such as an
    /// application's own SVG through [`Icon::from_bytes`].
    pub fn back_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.back_icon = icon.into();
        self
    }

    /// The forward arrow, in place of Lucide's `arrow-right`.
    pub fn forward_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.forward_icon = icon.into();
        self
    }

    /// Whether the arrows show their names as tooltips. On by default.
    pub fn tooltips(mut self, tooltips: bool) -> Self {
        self.tooltips = tooltips;
        self
    }
}

impl RenderOnce for NavButtons {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (can_go_back, can_go_forward) = {
            let state = self.state.read(cx);
            (state.depth() > 1, state.forward_views().len() > 0)
        };
        let back_state = self.state.clone();
        let forward_state = self.state;
        div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new(ElementId::NamedChild(self.id.clone().into(), "back".into()))
                    .ghost()
                    .icon(self.back_icon)
                    .accessibility_label("Back")
                    .when(self.tooltips, |this| this.tooltip("Back"))
                    .disabled(!can_go_back)
                    .on_click(move |_, _, cx| {
                        back_state.update(cx, |state, cx| {
                            state.pop(NavMotion::Animated, cx);
                        });
                    }),
            )
            .child(
                Button::new(ElementId::NamedChild(self.id.into(), "forward".into()))
                    .ghost()
                    .icon(self.forward_icon)
                    .accessibility_label("Forward")
                    .when(self.tooltips, |this| this.tooltip("Forward"))
                    .disabled(!can_go_forward)
                    .on_click(move |_, _, cx| {
                        forward_state.update(cx, |state, cx| {
                            state.forward(NavMotion::Animated, cx);
                        });
                    }),
            )
    }
}
