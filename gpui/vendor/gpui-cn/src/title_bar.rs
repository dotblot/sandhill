//! A title bar the application draws itself: a drag region with room for
//! the platform's window controls, and whatever the application puts in
//! it.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ClickEvent, Context, Decorations, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement, Render, RenderOnce, StatefulInteractiveElement as _,
    StyleRefinement, Styled, TitlebarOptions, Window, WindowControlArea, WindowOptions,
    base::{InteractiveElementExt as _, StyledExt as _, TestSupportExt as _},
    div, point,
    prelude::FluentBuilder as _,
};

use crate::{ActiveTheme as _, Icon};
use gpui_kit::assets::IconName;

type CloseHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type DoubleClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A title bar the application draws itself.
///
/// The bar paints nothing of its own, so it takes the surface it sits on:
/// the sidebar's when it heads a sidebar, the window's when it heads the
/// content. Dragging it moves the window and double-clicking it does what
/// the platform does with a title bar. On macOS it leaves room for the
/// window controls at its left edge (see [`inset`](Self::inset)); on
/// Windows and Linux it draws minimize, maximize, and close at its right
/// edge.
///
/// Open the window with [`TitleBar::window_options`] so the platform hides
/// its own title bar and hands dragging to this one.
///
/// ```
/// use gpui_cn::TitleBar;
/// use gpui_kit::{IntoElement, ParentElement as _, div};
///
/// fn header() -> impl IntoElement {
///     TitleBar::new().child(div().child("Untitled"))
/// }
/// ```
#[derive(IntoElement)]
pub struct TitleBar {
    children: Vec<AnyElement>,
    style: StyleRefinement,
    inset: bool,
    on_close_window: Option<CloseHandler>,
    on_double_click: Option<DoubleClickHandler>,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new()
    }
}

impl TitleBar {
    /// An empty title bar with the platform inset.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            style: StyleRefinement::default(),
            inset: true,
            on_close_window: None,
            on_double_click: None,
        }
    }

    /// What a double-click on the bar does instead of the platform's title
    /// bar behavior (zooming the window on macOS and Linux). A double-click
    /// that starts on a control in the bar, such as a button pressed twice,
    /// is the control's and never reaches this.
    pub fn on_double_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_double_click = Some(Rc::new(handler));
        self
    }

    /// The title bar options a window needs for this title bar: no system
    /// title bar, and the macOS window controls where the bar expects them
    /// (the theme's `window_controls_position`).
    pub fn title_bar_options(cx: &App) -> TitlebarOptions {
        let position = cx.theme().metrics.window_controls_position;
        TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(position, position)),
        }
    }

    /// The window options a window needs for this title bar. Override the
    /// rest with struct update syntax.
    pub fn window_options(cx: &App) -> WindowOptions {
        WindowOptions {
            titlebar: Some(Self::title_bar_options(cx)),
            // The bar moves the window itself, so the platform must not
            // treat it as a system drag region; on macOS that would delay
            // clicks on it while disambiguating double clicks.
            app_owns_titlebar_drag: true,
            ..Default::default()
        }
    }

    /// Whether the bar leaves room for the macOS window controls at its
    /// left edge. On by default. Turn it off for a bar that does not reach
    /// the window's left edge: the content's title bar while a sidebar is
    /// open. No-op on other platforms.
    pub fn inset(mut self, inset: bool) -> Self {
        self.inset = inset;
        self
    }

    /// What the close control does on Linux instead of closing the window.
    /// Other platforms handle their controls themselves.
    pub fn on_close_window(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        if cfg!(target_os = "linux") {
            self.on_close_window = Some(Rc::new(handler));
        }
        self
    }
}

impl Styled for TitleBar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for TitleBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// What the last press on the bar was.
struct DragState {
    /// A press on the bar itself, waiting for the pointer to move.
    pending: bool,
    /// The press started on a control in the bar, which took it.
    on_control: bool,
}

impl Render for DragState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl RenderOnce for TitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_macos = cfg!(target_os = "macos");
        let is_linux = cfg!(target_os = "linux");
        let is_web = cfg!(target_family = "wasm");
        let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
        let macos_inset = is_macos && self.inset && !window.is_fullscreen();
        // The bar's own padding sits on the outer element so a caller can
        // clear it and let a child, such as a tab strip, paint the whole
        // bar. Where the bar draws window controls at its right edge the
        // trailing padding and the content offset move inside, between the
        // content and them, so the controls keep the window's top edge.
        let draws_controls = draws_window_controls(window);
        let (height, offset, inset, gap) = {
            let theme = cx.theme();
            (
                theme.metrics.title_bar,
                theme.metrics.title_bar_content_offset,
                theme.metrics.window_controls_inset,
                theme.base.spacing.xs,
            )
        };

        let drag = window.use_state(cx, |_, _| DragState {
            pending: false,
            on_control: false,
        });
        let on_double_click = self.on_double_click.clone();

        div()
            .id("title-bar")
            .on_double_click(window.listener_for(&drag, move |drag, event, window, cx| {
                // Two fast clicks on a button in the bar are a double-click
                // on the bar too; only a press on the bar itself counts.
                if drag.on_control {
                    return;
                }
                match &on_double_click {
                    Some(handler) => handler(event, window, cx),
                    None if is_macos => window.titlebar_double_click(),
                    None if is_linux => window.zoom_window(),
                    None => {}
                }
            }))
            .test_support()
            .h_flex()
            .flex_shrink_0()
            .w_full()
            .h(height)
            .when(!draws_controls, |this| this.pt(offset))
            .map(|this| {
                if macos_inset {
                    this.pl(inset)
                } else {
                    this.pl_3()
                }
            })
            .when(!draws_controls, |this| this.pr_3())
            .refine_style(&self.style)
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&drag, |drag, _, window, _| {
                    // A control in the bar takes the press for itself and
                    // prevents the default; only a press on the bar moves
                    // the window.
                    drag.on_control = window.default_prevented();
                    drag.pending = !drag.on_control;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&drag, |drag, _, _, _| drag.pending = false),
            )
            .on_mouse_down_out(window.listener_for(&drag, |drag, _, _, _| drag.pending = false))
            .on_mouse_move(window.listener_for(&drag, |drag, _, window, _| {
                if drag.pending {
                    drag.pending = false;
                    window.start_window_move();
                }
            }))
            .child(
                div()
                    .id("title-bar-content")
                    .h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .gap(gap)
                    .when(draws_controls, |this| this.pt(offset).pr_3())
                    .when(!is_web, |this| {
                        this.window_control_area(WindowControlArea::Drag).when(
                            is_linux && client_decorated,
                            |this| {
                                this.on_mouse_down(MouseButton::Right, |event, window, _| {
                                    window.show_window_menu(event.position)
                                })
                            },
                        )
                    })
                    .children(self.children),
            )
            .child(WindowControls {
                on_close_window: self.on_close_window,
            })
    }
}

/// The minimize, maximize, and close controls on Windows and Linux.
#[derive(IntoElement)]
struct WindowControls {
    on_close_window: Option<CloseHandler>,
}

/// Whether the bar draws the window controls itself. Only Windows and
/// Linux let the application draw them; macOS draws its own, and a phone
/// or the web has none. Under server-side decorations on Linux the window
/// manager draws them, and a second set here would sit on top of its.
fn draws_window_controls(window: &Window) -> bool {
    cfg!(target_os = "windows")
        || (cfg!(target_os = "linux")
            && matches!(window.window_decorations(), Decorations::Client { .. }))
}

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        if !draws_window_controls(window) {
            return div().id("window-controls");
        }
        let supported = window.window_controls();
        div()
            .id("window-controls")
            .h_flex()
            .flex_shrink_0()
            .h_full()
            .when(supported.minimize, |this| this.child(Control::Minimize))
            .when(supported.maximize, |this| {
                this.child(if window.is_maximized() {
                    Control::Restore
                } else {
                    Control::Maximize
                })
            })
            .child(Control::Close {
                on_close_window: self.on_close_window,
            })
    }
}

/// One window control. On Windows the platform handles the press through
/// its control area. On Linux the control acts itself.
#[derive(IntoElement, Clone)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close {
        on_close_window: Option<CloseHandler>,
    },
}

impl Control {
    fn id(&self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Restore => "restore",
            Self::Close { .. } => "close",
        }
    }

    fn icon(&self) -> Icon {
        Icon::from(match self {
            Self::Minimize => IconName::WindowMinimize,
            Self::Maximize => IconName::WindowMaximize,
            Self::Restore => IconName::WindowRestore,
            Self::Close { .. } => IconName::WindowClose,
        })
    }

    fn area(&self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close { .. } => WindowControlArea::Close,
        }
    }

    fn is_close(&self) -> bool {
        matches!(self, Self::Close { .. })
    }
}

impl RenderOnce for Control {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (hover_bg, hover_fg): (Hsla, Hsla) = if self.is_close() {
            (theme.destructive(), theme.destructive_foreground())
        } else {
            (theme.secondary(), theme.foreground())
        };
        let active_bg = if self.is_close() {
            theme.destructive().opacity(0.8)
        } else {
            theme.selected
        };
        let control = self.clone();
        let on_close_window = match &self {
            Self::Close { on_close_window } => on_close_window.clone(),
            _ => None,
        };
        div()
            .id(self.id())
            .h_flex()
            .justify_center()
            .flex_shrink_0()
            .w(theme.metrics.window_control_width)
            .h_full()
            .text_color(theme.muted_foreground())
            .hover(|style| style.bg(hover_bg).text_color(hover_fg))
            .active(|style| style.bg(active_bg).text_color(hover_fg))
            .when(cfg!(target_os = "windows"), |this| {
                this.window_control_area(self.area())
            })
            .when(cfg!(target_os = "linux"), |this| {
                this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match &control {
                        Control::Minimize => window.minimize_window(),
                        Control::Maximize | Control::Restore => window.zoom_window(),
                        Control::Close { .. } => match &on_close_window {
                            Some(handler) => handler(&ClickEvent::default(), window, cx),
                            None => window.remove_window(),
                        },
                    }
                })
            })
            .child(self.icon().size_3p5())
    }
}
