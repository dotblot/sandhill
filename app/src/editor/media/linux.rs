//! Linux previews share the editor toolbar but use GPUI for images and
//! WebKitGTK's native surface for PDFs.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui_kit::*;
use tokio::runtime::Handle;

use super::{MediaFile, MediaView};

#[path = "linux/pdf.rs"]
mod pdf;
#[path = "linux/raster.rs"]
mod raster;

pub(super) enum Loaded {
    Pdf(MediaFile, Handle),
    Raster(Arc<RenderImage>),
}

pub(super) async fn load(file: MediaFile, pdf: bool) -> Result<Loaded> {
    if pdf {
        Ok(Loaded::Pdf(file, Handle::current()))
    } else {
        Ok(Loaded::Raster(
            tokio::task::spawn_blocking(move || raster::decode(file)).await??,
        ))
    }
}

pub(super) struct Preview(Kind);

enum Kind {
    Pdf(pdf::Pdf),
    Raster(Rc<raster::Raster>),
}

pub(super) fn build(
    loaded: Loaded,
    window: &mut Window,
    cx: &mut Context<MediaView>,
) -> Result<Preview> {
    Ok(Preview(match loaded {
        Loaded::Pdf(file, rt) => Kind::Pdf(pdf::Pdf::new(file, rt, window, cx)?),
        Loaded::Raster(image) => Kind::Raster(Rc::new(raster::Raster::new(image))),
    }))
}

pub(super) fn pages(view: &Preview) -> usize {
    match &view.0 {
        Kind::Pdf(pdf) => pdf.pages(),
        Kind::Raster(_) => 0,
    }
}

pub(super) fn ready(view: &Preview) -> bool {
    match &view.0 {
        Kind::Pdf(pdf) => pdf.pages() != 0,
        Kind::Raster(_) => true,
    }
}

pub(super) fn hide(view: &Preview, hidden: bool, cx: &mut App) {
    match &view.0 {
        Kind::Pdf(pdf) => pdf.hide(hidden, cx),
        Kind::Raster(image) if hidden => image.end_pan(),
        _ => {}
    }
}

pub(super) fn element(view: &Preview) -> AnyElement {
    match &view.0 {
        Kind::Pdf(pdf) => pdf.element(),
        Kind::Raster(image) => image.element(),
    }
}

impl Preview {
    pub(super) fn set_hand(&self, hand: bool) {
        match &self.0 {
            Kind::Pdf(pdf) => pdf.set_hand(hand),
            Kind::Raster(image) => image.set_hand(hand),
        }
    }

    pub(super) fn set_zoom(&self, zoom: Option<f64>) {
        match &self.0 {
            Kind::Pdf(pdf) => pdf.set_zoom(zoom),
            Kind::Raster(image) => image.set_zoom(zoom),
        }
    }

    pub(super) fn scale(&self) -> f64 {
        match &self.0 {
            Kind::Pdf(pdf) => pdf.scale(),
            Kind::Raster(image) => image.scale(),
        }
    }

    pub(super) fn set_page(&self, page: usize) {
        if let Kind::Pdf(pdf) = &self.0 {
            pdf.set_page(page);
        }
    }
}
