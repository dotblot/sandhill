//! Adapts the AppKit/PDFKit preview to the editor's shared toolbar.

use std::rc::Rc;
use std::time::Duration;

use anyhow::Result;
use gpui_kit::*;

use super::{MediaFile, MediaView, State, native};

pub(super) type Preview = Rc<native::Preview>;

pub(super) async fn load(file: MediaFile, pdf: bool) -> Result<(MediaFile, bool)> {
    Ok((file, pdf))
}

pub(super) fn build(
    (file, pdf): (MediaFile, bool),
    window: &mut Window,
    cx: &mut Context<MediaView>,
) -> Result<Preview> {
    let preview = native::Preview::new(file, pdf, window, cx)?;
    if pdf {
        // PDFKit also changes pages and zoom in response to native gestures.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if !this.hidden
                            && let State::Ready(view) = &this.state
                        {
                            this.sync_pdf_toolbar(
                                view.pages(),
                                view.current_page(),
                                view.explicit_zoom(),
                                cx,
                            );
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    Ok(Rc::new(preview))
}

pub(super) fn pages(view: &Preview) -> usize {
    view.pages()
}

pub(super) fn ready(_: &Preview) -> bool {
    true
}

pub(super) fn hide(view: &Preview, hidden: bool, _: &mut App) {
    view.hide(hidden);
}

pub(super) fn element(view: &Preview) -> AnyElement {
    native::element(view.clone()).into_any_element()
}
