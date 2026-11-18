use std::borrow::Cow;
use std::cell::Cell;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use gpui_kit::*;
use gpui_webview::{WebView, WebViewHandle};
use rust_embed::RustEmbed;
use serde::Deserialize;
use tokio::runtime::Handle;
use wry::http::{Response, StatusCode};

use super::{Kind, Preview};
use crate::editor::media::{MediaFile, MediaView, State};

const SCHEME: &str = "sandhill-pdf";
const VIEWER: &str = "sandhill-pdf://viewer/web/viewer.html";
const DOCUMENT: &str = "/web/document.pdf";
const CSS: &str = "/web/sandhill.css";
const BRIDGE: &str = include_str!("pdf.js");

#[derive(RustEmbed)]
#[folder = "vendor/pdfjs/"]
#[include = "build/**"]
#[include = "web/**"]
#[exclude = "**/LICENSE*"]
struct Assets;

pub(super) struct Pdf {
    view: Entity<WebView>,
    handle: WebViewHandle,
    pages: Cell<usize>,
    scale: Cell<f64>,
}

impl Pdf {
    pub(super) fn new(
        file: MediaFile,
        rt: Handle,
        window: &mut Window,
        cx: &mut Context<MediaView>,
    ) -> Result<Self> {
        let file = Arc::new(file);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let url = format!("{VIEWER}?file={DOCUMENT}");
        let builder = wry::WebViewBuilder::new()
            .with_url(&url)
            .with_incognito(true)
            .with_navigation_handler(move |next| next.split('#').next() == Some(&url))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_initialization_script(BRIDGE)
            .with_ipc_handler(move |req| {
                if let Ok(message) = serde_json::from_str::<Message>(req.body()) {
                    let _ = tx.send(message);
                }
            })
            // A private origin per webview. Keep using the already-downloaded
            // file so even an older remote daemon works without serving PDF.js.
            .with_asynchronous_custom_protocol(SCHEME.into(), move |_, req, reply| {
                let path = req.uri().path();
                if req.uri().host() != Some("viewer") || req.method() != wry::http::Method::GET {
                    reply.respond(response(StatusCode::NOT_FOUND, "text/plain", &[][..]));
                } else if path == DOCUMENT {
                    // Wry's protocol response takes the complete buffer; don't
                    // read the cached file on the GTK thread.
                    let file = file.clone();
                    rt.spawn(async move {
                        let response = match tokio::fs::read(&file.0).await {
                            Ok(bytes) => response(StatusCode::OK, "application/pdf", bytes),
                            Err(err) => {
                                tracing::warn!(%err, "reading cached PDF failed");
                                response(StatusCode::NOT_FOUND, "text/plain", &[][..])
                            }
                        };
                        reply.respond(response);
                    });
                } else {
                    reply.respond(asset(path));
                }
            });
        let webview = WebView::build(builder, window, cx)
            .map_err(|err| anyhow!("the PDF web view failed: {err}"))?;
        let handle = webview.handle();
        let view = cx.new(|_| webview);
        cx.spawn(async move |this, cx| {
            while let Some(message) = rx.recv().await {
                if this
                    .update(cx, |this, cx| receive(message, this, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Ok(Self {
            view,
            handle,
            pages: Cell::new(0),
            scale: Cell::new(1.),
        })
    }

    pub(super) fn pages(&self) -> usize {
        self.pages.get()
    }

    pub(super) fn scale(&self) -> f64 {
        self.scale.get()
    }

    pub(super) fn set_zoom(&self, zoom: Option<f64>) {
        let zoom = match zoom {
            Some(scale) => {
                self.scale.set(scale);
                scale.to_string()
            }
            None => "'page-fit'".into(),
        };
        self.eval(&format!(
            "PDFViewerApplication.pdfViewer.currentScaleValue = {zoom}"
        ));
    }

    pub(super) fn set_page(&self, page: usize) {
        self.eval(&format!("PDFViewerApplication.page = {}", page + 1));
    }

    pub(super) fn set_hand(&self, hand: bool) {
        // PDF.js CursorTool: SELECT = 0, HAND = 1.
        self.eval(&format!(
            "globalThis.PDFViewerApplication?.pdfCursorTools?.switchTool({})",
            usize::from(hand)
        ));
    }

    fn eval(&self, script: &str) {
        if let Err(err) = self.handle.raw().evaluate_script(script) {
            tracing::warn!(%err, "controlling the PDF preview failed");
        }
    }

    pub(super) fn hide(&self, hidden: bool, cx: &App) {
        let view = self.view.read(cx);
        if view.visible() == hidden {
            let _ = view.set_visible(!hidden);
        }
    }

    pub(super) fn element(&self) -> AnyElement {
        self.view.clone().into_any_element()
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Message {
    Position(Position),
    Error { message: String },
}

#[derive(Deserialize)]
struct Position {
    pages: usize,
    page: usize,
    scale: f64,
    fit: bool,
}

impl Position {
    fn toolbar(&self) -> Option<(usize, Option<f64>)> {
        if !(1..=self.pages).contains(&self.page) || !self.scale.is_finite() || self.scale <= 0. {
            return None;
        }
        Some((self.page - 1, (!self.fit).then_some(self.scale)))
    }
}

fn receive(message: Message, this: &mut MediaView, cx: &mut Context<MediaView>) {
    let State::Ready(Preview(Kind::Pdf(pdf))) = &this.state else {
        return;
    };
    match message {
        Message::Position(position) => {
            let Some((page, zoom)) = position.toolbar() else {
                return;
            };
            pdf.pages.set(position.pages);
            pdf.scale.set(position.scale);
            this.sync_pdf_toolbar(position.pages, page, zoom, cx);
        }
        Message::Error { message } => {
            this.state = State::Failed(message);
            this.changed(cx);
        }
    }
}

fn response(
    status: StatusCode,
    content_type: &str,
    data: impl Into<Cow<'static, [u8]>>,
) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-store")
        // The protocol is only registered in this preview's ephemeral context.
        .header("Access-Control-Allow-Origin", "*")
        .body(data.into())
        .expect("static PDF response headers are valid")
}

fn asset(path: &str) -> Response<Cow<'static, [u8]>> {
    if path == CSS {
        return response(
            StatusCode::OK,
            "text/css",
            b":root { --toolbar-height: 0px !important; } .toolbar { display: none; }".as_slice(),
        );
    }
    let Some(asset) = path.strip_prefix('/').and_then(Assets::get) else {
        return response(StatusCode::NOT_FOUND, "text/plain", &[][..]);
    };
    let mime = match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("mjs" | "js") => "text/javascript",
        Some("json") => "application/json",
        Some("ftl") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("gif") => "image/gif",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    };
    let mut result = response(StatusCode::OK, mime, asset.data);
    if path == "/web/viewer.html" {
        result.headers_mut().insert(
            "Content-Security-Policy",
            wry::http::HeaderValue::from_static("connect-src 'self' blob: data:; frame-src 'none'"),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::Message;

    #[test]
    fn pdf_position_converts_page_numbers_and_distinguishes_fit_from_zoom() {
        let Message::Position(mut position) = serde_json::from_str(
            r#"{"type":"position","pages":3,"page":2,"scale":0.75,"fit":true}"#,
        )
        .unwrap() else {
            panic!("expected a position message");
        };
        assert_eq!(position.toolbar(), Some((1, None)));
        position.fit = false;
        assert_eq!(position.toolbar(), Some((1, Some(0.75))));
        position.page = 0;
        assert_eq!(position.toolbar(), None);
        position.page = 4;
        assert_eq!(position.toolbar(), None);
    }
}
