//! gpui-cn's part of every window: a plugin on gpui-base's [`Root`].

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Stateful, Styled as _, Subscription, Window,
    base::{Root, RootPlugin, TooltipOverlay},
    div,
};

use crate::{ActiveTheme as _, Theme, TooltipHost};

/// Registers the plugin, so every [`Root`] opened after [`crate::init`]
/// carries it.
pub(crate) fn init(cx: &mut App) {
    Root::register_plugin(cx, RootLayer::new);
}

/// What gpui-cn adds to a window's [`Root`]: the window surface in the
/// theme's font and colors, the rem size the scale resolves against, the
/// system appearance, the tooltip overlay, and a focus that holds the
/// keyboard while nothing else does.
///
/// gpui-base's root moves focus on Tab and Shift-Tab and copies the text
/// selection. GPUI dispatches keys only along the focused element's
/// ancestors, so a window where nothing is focused would never see Tab;
/// the layer takes focus then, and again when the focused element is
/// removed. An owner that keeps a focused [`FocusHandle`] alive while
/// hiding its element should blur the window itself.
pub(crate) struct RootLayer {
    focus: FocusHandle,
    tooltip_overlay: Entity<TooltipOverlay>,
    _appearance: Subscription,
}

impl RootLayer {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tooltip_overlay =
            cx.new(|_| TooltipOverlay::new().render_with(crate::tooltip::render_surface));
        TooltipHost::register(window, &tooltip_overlay, cx);
        Theme::sync_system_appearance(Some(window), cx);
        let appearance = window.observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        Self {
            focus: cx.focus_handle(),
            tooltip_overlay,
            _appearance: appearance,
        }
    }
}

impl RootPlugin for RootLayer {
    fn prepare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_rem_size(Theme::global(cx).ui_font_size);
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
    }

    /// The focus goes on the root surface itself, where gpui-base's Tab
    /// and copy handlers are, so a key pressed while nothing else is
    /// focused reaches them.
    fn style(&self, surface: &mut Stateful<Div>, _: &mut Window, _: &mut App) {
        // The hook lends the surface, and `track_focus` takes it by value:
        // swap in an empty element for the moment it takes.
        let taken = std::mem::replace(surface, div().id("placeholder"));
        *surface = taken.track_focus(&self.focus);
    }

    fn decorate(
        &self,
        surface: AnyElement,
        _: &Root,
        _: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let tokens = cx.theme();
        div()
            .size_full()
            .font_family(tokens.font_family().clone())
            .bg(tokens.background())
            .text_color(tokens.foreground())
            .child(surface)
    }
}

/// The overlay layer: the tooltip overlay, above the application.
impl Render for RootLayer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.tooltip_overlay.clone()
    }
}
