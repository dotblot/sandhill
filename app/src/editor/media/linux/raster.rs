use std::cell::Cell;
use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use image::codecs::{gif::GifDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader};

use super::MediaFile;

const MAX_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;
const FIT_PADDING: Pixels = px(12.);

pub(super) fn decode(file: MediaFile) -> Result<Arc<RenderImage>> {
    // Read the cache directly, rather than taking a second copy of the source
    // in memory. SVG has already been rasterized by the shared inert renderer.
    let reader = ImageReader::new(BufReader::new(File::open(&file.0)?)).with_guessed_format()?;
    let image = match reader.format().context("unrecognized image format")? {
        ImageFormat::Gif => RenderImage::new(animation(GifDecoder::new(reader.into_inner())?)?),
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(reader.into_inner())?;
            if decoder.has_animation() {
                decoder.set_background_color(image::Rgba([0, 0, 0, 0]))?;
                RenderImage::new(animation(decoder)?)
            } else {
                RenderImage::new([still(decoder)?])
            }
        }
        _ => RenderImage::new([still(reader.into_decoder()?)?]),
    };
    Ok(Arc::new(image))
}

fn check_size(decoder: &impl ImageDecoder) -> Result<()> {
    let (width, height) = decoder.dimensions();
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "the image has no pixels or exceeds the 32-megapixel preview limit"
    );
    Ok(())
}

fn still(mut decoder: impl ImageDecoder) -> Result<Frame> {
    check_size(&decoder)?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    let mut frame = Frame::new(image.into_rgba8());
    to_bgra(&mut frame);
    Ok(frame)
}

fn animation<'a>(decoder: impl ImageDecoder + AnimationDecoder<'a>) -> Result<Vec<Frame>> {
    check_size(&decoder)?;
    let mut frames = Vec::new();
    let mut bytes = 0;
    for frame in decoder.into_frames() {
        let mut frame = frame?;
        bytes += frame.buffer().len();
        ensure!(
            bytes <= MAX_ANIMATION_BYTES,
            "the animation is too large to preview"
        );
        to_bgra(&mut frame);
        frames.push(frame);
    }
    ensure!(!frames.is_empty(), "the image contains no readable frames");
    Ok(frames)
}

fn to_bgra(frame: &mut Frame) {
    for pixel in frame.buffer_mut().as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

pub(super) struct Raster {
    image: Arc<RenderImage>,
    size: Size<Pixels>,
    viewport: Cell<Size<Pixels>>,
    zoom: Cell<Option<f64>>,
    hand: Cell<bool>,
    last: Cell<Option<Point<Pixels>>>,
    scroll: ScrollHandle,
}

impl Raster {
    pub(super) fn new(image: Arc<RenderImage>) -> Self {
        Self {
            size: image.size(0).map(|value| px(u32::from(value) as f32)),
            image,
            viewport: Cell::default(),
            zoom: Cell::default(),
            hand: Cell::default(),
            last: Cell::default(),
            scroll: ScrollHandle::new(),
        }
    }

    pub(super) fn set_hand(&self, hand: bool) {
        self.hand.set(hand);
        self.end_pan();
    }

    pub(super) fn end_pan(&self) {
        self.last.set(None);
    }

    pub(super) fn scale(&self) -> f64 {
        self.zoom.get().unwrap_or_else(|| {
            let view = self.viewport.get();
            f64::from(
                ((view.width - FIT_PADDING * 2.).max(px(1.)) / self.size.width)
                    .min((view.height - FIT_PADDING * 2.).max(px(1.)) / self.size.height),
            )
        })
    }

    pub(super) fn set_zoom(&self, zoom: Option<f64>) {
        if let Some(next) = zoom {
            // Keep the point under the middle of the pane when zoom changes.
            let old = self.scale() as f32;
            let view = self.viewport.get();
            let offset = self.scroll.offset();
            let next = next as f32;
            self.scroll.set_offset(point(
                zoom_offset(view.width, self.size.width, offset.x, old, next),
                zoom_offset(view.height, self.size.height, offset.y, old, next),
            ));
        } else {
            self.scroll.set_offset(Point::default());
        }
        self.zoom.set(zoom);
    }

    pub(super) fn element(self: &Rc<Self>) -> AnyElement {
        let pane = div()
            .id("media-raster-pane")
            .relative()
            .size_full()
            .overflow_hidden();
        let body = if let Some(zoom) = self.zoom.get() {
            let extent = self.size.map(|value| value * zoom as f32);
            let image = img(self.image.clone()).id("media-raster");
            div()
                .id("media-raster-scroll")
                .absolute()
                .size_full()
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(extent.width)
                        .h(extent.height)
                        .min_w_full()
                        .min_h_full()
                        .child(image.flex_none().w(extent.width).h(extent.height)),
                )
                .into_any_element()
        } else {
            // A percentage-sized img can grow to its intrinsic aspect ratio in
            // a flex pane. Fit the image to the pane's resolved bounds instead.
            canvas(
                {
                    let this = self.clone();
                    move |bounds, window, cx| {
                        this.viewport.set(bounds.size);
                        let scale = this.scale() as f32;
                        let extent = this.size.map(|length| length * scale);
                        let mut image = img(this.image.clone())
                            .id("media-raster")
                            .w(extent.width)
                            .h(extent.height)
                            .into_any_element();
                        image.prepaint_as_root(
                            bounds.center() - point(extent.width / 2., extent.height / 2.),
                            extent.map(AvailableSpace::Definite),
                            window,
                            cx,
                        );
                        image
                    }
                },
                |_, mut image, window, cx| image.paint(window, cx),
            )
            .absolute()
            .size_full()
            .into_any_element()
        };
        pane.child(body)
            .when(self.zoom.get().is_some(), |pane| {
                pane.child(
                    canvas(
                        {
                            let this = self.clone();
                            move |bounds, _, _| this.viewport.set(bounds.size)
                        },
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .when(self.hand.get(), |view| {
                view.cursor(if self.last.get().is_some() {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                })
                .on_mouse_down(MouseButton::Left, {
                    let this = self.clone();
                    move |event, window, _| {
                        this.last.set(Some(event.position));
                        window.refresh();
                    }
                })
                .on_mouse_move({
                    let this = self.clone();
                    move |event, window, _| {
                        if event.pressed_button == Some(MouseButton::Left)
                            && let Some(last) = this.last.get()
                        {
                            this.last.set(Some(event.position));
                            this.scroll
                                .set_offset(this.scroll.offset() + (event.position - last));
                            window.refresh();
                        }
                    }
                })
                .on_mouse_up(MouseButton::Left, {
                    let this = self.clone();
                    move |_, window, _| {
                        this.end_pan();
                        window.refresh();
                    }
                })
                .on_mouse_up_out(MouseButton::Left, {
                    let this = self.clone();
                    move |_, window, _| {
                        this.end_pan();
                        window.refresh();
                    }
                })
            })
            .into_any_element()
    }
}

fn zoom_offset(pane: Pixels, image: Pixels, offset: Pixels, from: f32, to: f32) -> Pixels {
    let center = if image * from <= pane {
        image / 2.
    } else {
        (pane / 2. - offset) / from
    };
    pane / 2. - center * to
}

#[cfg(test)]
mod tests {
    use super::{px, zoom_offset};

    #[test]
    fn zoom_keeps_the_centered_or_panned_pixel_in_view() {
        assert_eq!(zoom_offset(px(400.), px(600.), px(0.), 0.5, 1.), px(-100.));
        assert_eq!(
            zoom_offset(px(400.), px(600.), px(-150.), 1., 2.),
            px(-500.)
        );
    }
}
