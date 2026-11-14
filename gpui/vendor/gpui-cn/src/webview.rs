//! An embedded web page, on `gpui-webview`.

use gpui_kit::{
    AnyElement, App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, StyleRefinement, Styled, TestSupportExt as _, Window, base::StyledExt as _, div,
};

mod browser;

pub use browser::{BrowserEvent, BrowserState};
pub use gpui_webview::{WebView, WebViewHandle};

use crate::ActiveTheme as _;

/// The frame of an embedded web page: a rounded box with a hairline
/// border on the theme's window surface.
///
/// The page itself is a native view that the operating system owns, built
/// with `WebView::build` from a `wry::WebViewBuilder` and kept in an
/// `Entity`. The frame lays that view out, clips it to the rounded
/// corners, and tells it whether the theme is dark or light, so the
/// page's `prefers-color-scheme` follows the app on every render.
///
/// ```ignore
/// let page = cx.new(|cx| {
///     WebView::build(wry::WebViewBuilder::new().with_url("https://example.com"), window, cx)
///         .expect("a native web view")
/// });
/// let _ = WebFrame::new("docs").webview(page).h_96();
/// ```
///
/// Without the `gpui-fast` feature the native view covers GPUI content in
/// its bounds, so a popover or a dialog cannot open above the page and the
/// view is not clipped to the corners. The feature is on by default; see
/// `gpui_webview::WebView` for the platform setup, such as Windows'
/// `GPUI_DISABLE_DIRECT_COMPOSITION` and Linux's X11 window.
#[derive(IntoElement)]
pub struct WebFrame {
    id: ElementId,
    style: StyleRefinement,
    webview: Option<Entity<WebView>>,
    children: Vec<AnyElement>,
}

impl WebFrame {
    /// A frame with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            webview: None,
            children: Vec::new(),
        }
    }

    /// The page the frame shows. It fills the frame.
    pub fn webview(mut self, webview: Entity<WebView>) -> Self {
        self.webview = Some(webview);
        self
    }

    /// The page the frame shows, if any.
    pub fn page(&self) -> Option<&Entity<WebView>> {
        self.webview.as_ref()
    }
}

impl Styled for WebFrame {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for WebFrame {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for WebFrame {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (fill, border, radius, dark) = {
            let theme = cx.theme();
            (
                theme.background(),
                theme.border(),
                theme.radius_lg(),
                theme.is_dark(),
            )
        };
        if let Some(webview) = &self.webview {
            let scheme = if dark {
                wry::Theme::Dark
            } else {
                wry::Theme::Light
            };
            let _ = webview.read(cx).set_theme(scheme);
        }
        div()
            .id(self.id)
            .test_support()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .rounded(radius)
            .bg(fill)
            .refine_style(&self.style)
            .children(self.webview)
            .children(self.children)
    }
}
