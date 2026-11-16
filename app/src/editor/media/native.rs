//! Small UI-thread-only AppKit/PDFKit bridge. Uses GPUI Fast's managed native
//! surface, the same clipping/overlay composition as gpui-webview.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow, ensure};
use gpui_kit::*;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::{Allocated, Retained},
    runtime::{AnyClass, AnyObject},
};
use objc2_app_kit::NSView;
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::MediaFile;

#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

/// All objects and callbacks are main-thread local. The cache outlives PDFKit's
/// lazy page reads. Cleanup is deferred, just like gpui-webview surface cleanup.
pub(super) struct Preview {
    root: Retained<AnyObject>,
    kind: Kind,
    surface: WindowCompositionSurface,
    window: AnyWindowHandle,
    cx: AsyncApp,
    hidden: Cell<bool>,
    dark: Cell<Option<bool>>,
    viewport: Cell<NSSize>,
    zoom: Cell<Option<f64>>,
    scale: Cell<f64>,
    hand: Cell<bool>,
    hand_view: Retained<HandView>,
    _file: MediaFile,
}

enum Kind {
    Pdf {
        document: Retained<AnyObject>,
        pages: usize,
    },
    Image {
        image: Retained<AnyObject>,
        document: Retained<AnyObject>,
        size: NSSize,
    },
}

fn class(name: &'static std::ffi::CStr) -> Result<&'static AnyClass> {
    AnyClass::get(name).ok_or_else(|| {
        anyhow!(
            "system media decoder is unavailable: {}",
            name.to_string_lossy()
        )
    })
}

unsafe fn view(name: &'static std::ffi::CStr) -> Result<Retained<AnyObject>> {
    let allocated: Allocated<AnyObject> = unsafe { msg_send![class(name)?, alloc] };
    let initialized: Option<Retained<AnyObject>> =
        unsafe { msg_send![allocated, initWithFrame: NSRect::ZERO] };
    initialized.context("the system could not create the preview view")
}

#[derive(Default)]
struct HandIvars {
    /// Window coordinates remain stable while the clip view scrolls.
    last: Cell<Option<NSPoint>>,
}

define_class!(
    // SAFETY: This transparent NSView only handles pointing events and cursor
    // rects. Its ivars and AppKit calls stay on the main thread.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = HandIvars]
    struct HandView;

    unsafe impl NSObjectProtocol for HandView {}

    impl HandView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&AnyObject>) -> bool {
            true
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            unsafe {
                let bounds: NSRect = msg_send![self, bounds];
                let _: () = msg_send![self, addCursorRect: bounds, cursor: &*self.cursor()];
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &AnyObject) {
            let point: NSPoint = unsafe { msg_send![event, locationInWindow] };
            self.ivars().last.set(Some(point));
            self.update_cursor();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &AnyObject) {
            let point: NSPoint = unsafe { msg_send![event, locationInWindow] };
            let Some(previous) = self.ivars().last.replace(Some(point)) else {
                return;
            };
            self.update_cursor();
            unsafe {
                let root: Option<Retained<AnyObject>> = msg_send![self, superview];
                let Some(root) = root else { return };
                // Both PDFView and NSScrollView expose their document view.
                // Resolve this on each drag because PDFKit can replace its
                // document hierarchy when changing pages or display layout.
                let document: Option<Retained<AnyObject>> = msg_send![&*root, documentView];
                let Some(document) = document else { return };
                let scroll: Option<Retained<AnyObject>> =
                    msg_send![&*document, enclosingScrollView];
                let Some(scroll) = scroll else { return };
                let clip: Retained<AnyObject> = msg_send![&*scroll, contentView];
                let bounds: NSRect = msg_send![&*clip, bounds];
                // Convert both window positions using the current clip
                // transform, handling flipped PDF/image coordinates and zoom.
                let from: NSPoint = msg_send![&*clip, convertPoint: previous, fromView: None::<&NSView>];
                let to: NSPoint = msg_send![&*clip, convertPoint: point, fromView: None::<&NSView>];
                let proposed = NSRect::new(
                    NSPoint::new(
                        bounds.origin.x - (to.x - from.x),
                        bounds.origin.y - (to.y - from.y),
                    ),
                    bounds.size,
                );
                let constrained: NSRect = msg_send![&*clip, constrainBoundsRect: proposed];
                let _: () = msg_send![&*clip, scrollToPoint: constrained.origin];
                let _: () = msg_send![&*scroll, reflectScrolledClipView: &*clip];
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &AnyObject) {
            self.ivars().last.set(None);
            self.update_cursor();
            unsafe {
                let point: NSPoint = msg_send![event, locationInWindow];
                let point: NSPoint = msg_send![self, convertPoint: point, fromView: None::<&NSView>];
                let visible: NSRect = msg_send![self, visibleRect];
                let inside: bool = msg_send![self, mouse: point, inRect: visible];
                if !inside {
                    let cursor: Retained<AnyObject> =
                        msg_send![class(c"NSCursor").expect("AppKit provides NSCursor"), arrowCursor];
                    let _: () = msg_send![&*cursor, set];
                }
            }
        }
    }
);

impl HandView {
    fn new() -> Result<Retained<Self>> {
        let mtm = MainThreadMarker::new().context("previews require the AppKit main thread")?;
        let allocated = Self::alloc(mtm).set_ivars(HandIvars::default());
        let view: Retained<Self> =
            unsafe { msg_send![super(allocated), initWithFrame: NSRect::ZERO] };
        unsafe {
            // NSViewWidthSizable | NSViewHeightSizable.
            let _: () = msg_send![&*view, setAutoresizingMask: 18usize];
        }
        Ok(view)
    }

    fn cursor(&self) -> Retained<AnyObject> {
        let cursor = class(c"NSCursor").expect("AppKit provides NSCursor");
        unsafe {
            if self.ivars().last.get().is_some() {
                msg_send![cursor, closedHandCursor]
            } else {
                msg_send![cursor, openHandCursor]
            }
        }
    }

    fn update_cursor(&self) {
        unsafe {
            let _: () = msg_send![&*self.cursor(), set];
            let window: Option<Retained<AnyObject>> = msg_send![self, window];
            if let Some(window) = window {
                let _: () = msg_send![&*window, invalidateCursorRectsForView: self];
            }
        }
    }
}

impl Preview {
    pub(super) fn new(
        file: MediaFile,
        pdf: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self> {
        let path = NSString::from_str(file.0.to_str().context("invalid preview cache path")?);
        // SAFETY: GPUI creates and uses this bridge on AppKit's UI thread. All
        // selectors below are public AppKit/PDFKit APIs with their native types.
        let (root, kind) = unsafe {
            if pdf {
                let url: Option<Retained<AnyObject>> =
                    msg_send![class(c"NSURL")?, fileURLWithPath: &*path];
                let url = url.context("invalid PDF cache URL")?;
                let allocated: Allocated<AnyObject> = msg_send![class(c"PDFDocument")?, alloc];
                let document: Option<Retained<AnyObject>> =
                    msg_send![allocated, initWithURL: &*url];
                let document = document.context("PDFKit could not decode this PDF")?;
                let locked: bool = msg_send![&*document, isLocked];
                ensure!(
                    !locked,
                    "this PDF is password protected; unlock it before previewing"
                );
                let pages: usize = msg_send![&*document, pageCount];
                ensure!(pages > 0, "this PDF contains no readable pages");
                let root = view(c"PDFView")?;
                let _: () = msg_send![&*root, setDocument: &*document];
                // Single-page mode makes the explicit page controls authoritative.
                let _: () = msg_send![&*root, setDisplayMode: 0isize];
                let _: () = msg_send![&*root, setAutoScales: true];
                let _: () = msg_send![&*root, setMinScaleFactor: 0.05f64];
                let _: () = msg_send![&*root, setMaxScaleFactor: 16.0f64];
                (root, Kind::Pdf { document, pages })
            } else {
                let allocated: Allocated<AnyObject> = msg_send![class(c"NSImage")?, alloc];
                let decoded: Option<Retained<AnyObject>> =
                    msg_send![allocated, initWithContentsOfFile: &*path];
                let decoded = decoded.context(
                    "the system image decoder does not support this file or it is corrupt",
                )?;
                let valid: bool = msg_send![&*decoded, isValid];
                ensure!(
                    valid,
                    "the image is corrupt or unsupported by the system decoder"
                );
                let size: NSSize = msg_send![&*decoded, size];
                ensure!(
                    size.width.is_finite()
                        && size.height.is_finite()
                        && size.width > 0.
                        && size.height > 0.,
                    "the image has invalid dimensions"
                );
                ensure!(
                    size.width * size.height <= 64. * 1024. * 1024.,
                    "image dimensions exceed the preview limit"
                );
                let root = view(c"NSScrollView")?;
                let document = view(c"NSView")?;
                let image = view(c"NSImageView")?;
                let _: () = msg_send![&*image, setImage: &*decoded];
                let _: () = msg_send![&*image, setImageScaling: 1usize];
                let _: () = msg_send![&*image, setAnimates: true];
                let _: () = msg_send![&*document, addSubview: &*image];
                let _: () = msg_send![&*root, setDocumentView: &*document];
                let _: () = msg_send![&*root, setHasVerticalScroller: true];
                let _: () = msg_send![&*root, setHasHorizontalScroller: true];
                let _: () = msg_send![&*root, setAutohidesScrollers: true];
                let _: () = msg_send![&*root, setDrawsBackground: false];
                (
                    root,
                    Kind::Image {
                        image,
                        document,
                        size,
                    },
                )
            }
        };
        let hand_view = HandView::new()?;
        let composition = window.enable_window_composition()?;
        let surface = composition.create_native_surface()?;
        let attached = (|| -> Result<()> {
            let handle = surface.platform_surface()?.platform_handle()?;
            let parent = *handle
                .downcast::<usize>()
                .map_err(|_| anyhow!("media surface is not an AppKit view"))?;
            ensure!(parent != 0, "media surface has no AppKit view");
            unsafe {
                (&*(parent as *const NSView))
                    .addSubview(&*(Retained::as_ptr(&root) as *const NSView));
            }
            surface.platform_surface()?.set_visible(false)?;
            Ok(())
        })();
        if let Err(error) = attached {
            composition.remove_surface(surface.id())?;
            return Err(error);
        }
        Ok(Self {
            root,
            kind,
            surface,
            window: window.window_handle(),
            cx: cx.to_async(),
            hidden: Cell::new(false),
            dark: Cell::new(None),
            viewport: Cell::new(NSSize::ZERO),
            zoom: Cell::new(None),
            scale: Cell::new(1.),
            hand: Cell::new(false),
            hand_view,
            _file: file,
        })
    }

    pub(super) fn pages(&self) -> usize {
        match &self.kind {
            Kind::Pdf { pages, .. } => *pages,
            _ => 0,
        }
    }

    pub(super) fn current_page(&self) -> usize {
        let Kind::Pdf { document, pages } = &self.kind else {
            return 0;
        };
        unsafe {
            let page: Option<Retained<AnyObject>> = msg_send![&*self.root, currentPage];
            page.map_or(0, |page| {
                let index: usize = msg_send![&**document, indexForPage: &*page];
                index.min(pages.saturating_sub(1))
            })
        }
    }

    pub(super) fn explicit_zoom(&self) -> Option<f64> {
        if matches!(self.kind, Kind::Pdf { .. }) {
            let automatic: bool = unsafe { msg_send![&*self.root, autoScales] };
            let zoom = if automatic { None } else { Some(self.scale()) };
            self.zoom.set(zoom);
            zoom
        } else {
            self.zoom.get()
        }
    }

    pub(super) fn hide(&self, hidden: bool) {
        self.hidden.set(hidden);
        if hidden {
            self.show_hand(false);
            let _ = self
                .surface
                .platform_surface()
                .and_then(|s| s.set_visible(false));
        }
    }

    pub(super) fn set_hand(&self, hand: bool) {
        self.hand.set(hand);
        self.show_hand(hand && !self.hidden.get());
    }

    fn show_hand(&self, visible: bool) {
        unsafe {
            let parent: *mut AnyObject = msg_send![&*self.hand_view, superview];
            if visible && parent.is_null() {
                let bounds: NSRect = msg_send![&*self.root, bounds];
                let _: () = msg_send![&*self.hand_view, setFrame: bounds];
                let _: () = msg_send![&*self.root, addSubview: &*self.hand_view, positioned: 1isize, relativeTo: None::<&NSView>];
            } else if !visible && !parent.is_null() {
                self.hand_view.ivars().last.set(None);
                let _: () = msg_send![&*self.hand_view, removeFromSuperview];
                let cursor: Retained<AnyObject> = msg_send![
                    class(c"NSCursor").expect("AppKit provides NSCursor"),
                    arrowCursor
                ];
                let _: () = msg_send![&*cursor, set];
            } else {
                return;
            }
            let window: Option<Retained<AnyObject>> = msg_send![&*self.root, window];
            if let Some(window) = window {
                let _: () = msg_send![&*window, invalidateCursorRectsForView: &*self.root];
                if visible {
                    let _: () = msg_send![&*window, invalidateCursorRectsForView: &*self.hand_view];
                }
            }
        }
    }

    pub(super) fn scale(&self) -> f64 {
        if matches!(self.kind, Kind::Pdf { .. }) {
            unsafe { msg_send![&*self.root, scaleFactor] }
        } else {
            self.scale.get()
        }
    }

    pub(super) fn set_zoom(&self, zoom: Option<f64>) {
        self.zoom.set(zoom);
        match &self.kind {
            Kind::Pdf { .. } => unsafe {
                let _: () = msg_send![&*self.root, setAutoScales: zoom.is_none()];
                if let Some(scale) = zoom {
                    let _: () = msg_send![&*self.root, setScaleFactor: scale];
                }
            },
            Kind::Image { .. } => self.layout_image(self.viewport.get()),
        }
    }

    pub(super) fn set_page(&self, page: usize) {
        if let Kind::Pdf { document, pages } = &self.kind {
            if page >= *pages {
                return;
            }
            unsafe {
                let page: Option<Retained<AnyObject>> = msg_send![&**document, pageAtIndex: page];
                if let Some(page) = page {
                    let _: () = msg_send![&*self.root, goToPage: &*page];
                }
            }
        }
    }

    fn layout_image(&self, viewport: NSSize) {
        let Kind::Image {
            image,
            document,
            size,
        } = &self.kind
        else {
            return;
        };
        if viewport.width <= 0. || viewport.height <= 0. {
            return;
        }
        let fit = ((viewport.width - 24.).max(1.) / size.width)
            .min((viewport.height - 24.).max(1.) / size.height);
        let scale = self.zoom.get().unwrap_or(fit);
        self.scale.set(scale);
        let width = size.width * scale;
        let height = size.height * scale;
        let area = NSSize::new(width.max(viewport.width), height.max(viewport.height));
        unsafe {
            let _: () = msg_send![&**document, setFrame: NSRect::new(NSPoint::ZERO, area)];
            let _: () = msg_send![&**image, setFrame: NSRect::new(NSPoint::new((area.width - width) / 2., (area.height - height) / 2.), NSSize::new(width, height))];
        }
    }

    /// Places the native view over `bounds`; the hitbox it blocks GPUI's
    /// mouse with, when shown.
    fn geometry(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> Result<Option<Hitbox>> {
        let clip = bounds.intersect(&window.content_mask().bounds);
        if self.hidden.get() || clip.is_empty() {
            self.show_hand(false);
            self.surface.platform_surface()?.set_visible(false)?;
            return Ok(None);
        }
        self.surface
            .platform_surface()?
            .set_bounds(clip.to_device_pixels(window.scale_factor()))?;
        let viewport = NSSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height));
        let offset = bounds.origin - clip.origin;
        let frame = NSRect::new(
            NSPoint::new(
                f64::from(offset.x),
                f64::from(clip.size.height - offset.y - bounds.size.height),
            ),
            viewport,
        );
        unsafe {
            let _: () = msg_send![&*self.root, setFrame: frame];
        }
        if self.viewport.replace(viewport) != viewport {
            match self.kind {
                Kind::Pdf { .. } if self.zoom.get().is_none() => unsafe {
                    // PDFKit cannot fit a view created at a zero-sized frame.
                    // Reapply fit only after layout, preserving explicit zoom.
                    let _: () = msg_send![&*self.root, setAutoScales: true];
                },
                Kind::Image { .. } => self.layout_image(viewport),
                _ => {}
            }
        }
        let palette = crate::look::Palette::get(cx);
        if matches!(self.kind, Kind::Pdf { .. })
            && self.dark.replace(Some(palette.dark)) != Some(palette.dark)
        {
            let color = Rgba::from(palette.chrome);
            unsafe {
                let color: Retained<AnyObject> = msg_send![class(c"NSColor")?,
                    colorWithSRGBRed: f64::from(color.r),
                    green: f64::from(color.g),
                    blue: f64::from(color.b),
                    alpha: 1.0f64];
                let _: () = msg_send![&*self.root, setBackgroundColor: &*color];
            }
        }
        self.show_hand(self.hand.get());
        self.surface.platform_surface()?.set_visible(true)?;
        Ok(Some(window.insert_hitbox(clip, HitboxBehavior::BlockMouse)))
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.hide(true);
        let surface = self.surface.clone();
        let root = self.root.clone();
        let window = self.window;
        self.cx
            .clone()
            .spawn(async move |cx| {
                unsafe {
                    let _: () = msg_send![&*root, removeFromSuperview];
                }
                let _ = window.update(cx, |_, window, _| {
                    window
                        .enable_window_composition()?
                        .remove_surface(surface.id())
                });
            })
            .detach();
    }
}

pub(super) fn element(preview: Rc<Preview>) -> impl IntoElement {
    let painted = preview.clone();
    canvas(
        move |bounds, window, cx| match preview.geometry(bounds, window, cx) {
            Ok(hitbox) => hitbox,
            Err(err) => {
                preview.hide(true);
                tracing::error!("media preview geometry failed: {err:#}");
                None
            }
        },
        // GPUI sets the cursor on every mouse move, so the open hand must
        // be its own (the native view shows the closed hand while dragging).
        move |_, hitbox, window, _| {
            if let Some(hitbox) = hitbox
                && painted.hand.get()
            {
                window.set_cursor_style(CursorStyle::OpenHand, &hitbox);
            }
        },
    )
    .size_full()
}
