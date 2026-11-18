//! Read-only document previews. The daemon serves the original binary and the
//! existing port tunnel carries it; no binary is copied through JSON-RPC.

use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{Call, PageUrl};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::runtime::Runtime;

use crate::daemon::Daemon;
use crate::look::{self, Palette, bar_button, icon_button};
use crate::workbench::Workbench;

pub(crate) mod cache;
use cache::MediaFile;

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "linux")]
#[path = "media/linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "media/macos.rs"]
mod platform;

const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SVG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SVG_PIXELS: u64 = 32 * 1024 * 1024;

pub(crate) fn is_media(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "pdf" | "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico"
    )
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

async fn download(port: u16, path: String, svg: bool) -> Result<MediaFile> {
    ensure!(
        path.starts_with('/') && !path.contains(['\r', '\n']),
        "invalid preview address"
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    ensure!(
        line.split_whitespace().nth(1) == Some("200"),
        "the daemon could not serve this file: {}",
        line.trim()
    );
    let mut total = line.len();
    let mut length = None;
    loop {
        line.clear();
        let count = reader.read_line(&mut line).await?;
        total += count;
        ensure!(
            count > 0 && total <= 64 * 1024,
            "invalid preview response headers"
        );
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<u64>()?);
        }
    }
    let length = length.context("the daemon did not provide the preview file size")?;
    let limit = if svg { MAX_SVG_BYTES } else { MAX_MEDIA_BYTES };
    ensure!(
        length <= limit,
        "this file is too large to preview (limit {} MiB)",
        limit / 1024 / 1024
    );
    let (cached, file) = MediaFile::create()?;
    let mut file = tokio::fs::File::from_std(file);
    let copied = tokio::io::copy(&mut reader.take(length + 1), &mut file).await?;
    ensure!(
        copied == length,
        "the preview transfer was incomplete or the file changed; reopen it"
    );
    file.flush().await?;
    drop(file);
    if svg {
        return tokio::task::spawn_blocking(move || rasterize_svg(cached)).await?;
    }
    Ok(cached)
}

/// usvg does not execute scripts. Explicitly deny filesystem/URL image resolvers,
/// including relative hrefs; embedded data images remain part of the document.
fn rasterize_svg(file: MediaFile) -> Result<MediaFile> {
    let data = std::fs::read(&file.0)?;
    std::fs::write(&file.0, rasterize_svg_bytes(&data)?)?;
    Ok(file)
}

pub(crate) fn rasterize_svg_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut options = resvg::usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    static FONTS: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        Arc::new(fonts)
    });
    options.fontdb = FONTS.clone();
    let tree = resvg::usvg::Tree::from_data(data, &options).context("invalid SVG")?;
    let size = tree.size().to_int_size();
    ensure!(
        u64::from(size.width()) * u64::from(size.height()) <= MAX_SVG_PIXELS,
        "SVG dimensions exceed the preview limit"
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .context("SVG has no drawable size")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap.encode_png()?)
}

enum State {
    Loading,
    Ready(platform::Preview),
    Failed(String),
}

pub(crate) struct MediaView {
    state: State,
    hidden: bool,
    page: usize,
    pages: usize,
    /// None means fit, otherwise an absolute scale (1 = actual size).
    zoom: Option<f64>,
    hand: bool,
    owner: WeakEntity<Workbench>,
}

impl MediaView {
    pub(crate) fn new(
        path: String,
        daemon: Rc<Daemon>,
        remote: bool,
        rt: Arc<Runtime>,
        handle: AnyWindowHandle,
        owner: WeakEntity<Workbench>,
        cx: &mut Context<Self>,
    ) -> Self {
        let pdf = extension(&path) == "pdf";
        let svg = extension(&path) == "svg";
        cx.spawn(async move |this, cx| {
            let loaded = async {
                let url: PageUrl = daemon.call(Call::ServePage { path }).await?;
                let port = if remote {
                    daemon.forward_port(url.port).await?
                } else {
                    url.port
                };
                rt.spawn(async move {
                    let file = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        download(port, url.path, svg),
                    )
                    .await
                    .context("preview transfer timed out")??;
                    platform::load(file, pdf).await
                })
                .await?
            }
            .await;
            let _ = handle.update(cx, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let loaded = loaded.and_then(|file| platform::build(file, window, cx));
                    this.state = match loaded {
                        Ok(preview) => {
                            this.pages = platform::pages(&preview);
                            platform::hide(&preview, this.hidden, cx);
                            preview.set_hand(this.hand);
                            State::Ready(preview)
                        }
                        Err(err) => State::Failed(format!("{err:#}")),
                    };
                    this.changed(cx);
                })
            });
        })
        .detach();
        Self {
            state: State::Loading,
            hidden: false,
            page: 0,
            pages: 0,
            zoom: None,
            hand: false,
            owner,
        }
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.notify();
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            owner.update(cx, |_, cx| cx.notify()).ok();
        });
    }

    fn sync_pdf_toolbar(
        &mut self,
        pages: usize,
        page: usize,
        zoom: Option<f64>,
        cx: &mut Context<Self>,
    ) {
        if (pages, page, zoom) != (self.pages, self.page, self.zoom) {
            self.pages = pages;
            self.page = page;
            self.zoom = zoom;
            self.changed(cx);
        }
    }

    pub(crate) fn hide(&mut self, hidden: bool, cx: &mut App) {
        self.hidden = hidden;
        if let State::Ready(view) = &self.state {
            platform::hide(view, hidden, cx);
        }
    }

    fn set_zoom(&mut self, zoom: Option<f64>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        if let State::Ready(view) = &self.state {
            view.set_zoom(zoom);
        }
        self.changed(cx);
    }

    fn step_zoom(&mut self, factor: f64, cx: &mut Context<Self>) {
        if let State::Ready(view) = &self.state {
            self.set_zoom(Some((view.scale() * factor).clamp(0.05, 16.)), cx);
        }
    }

    fn toggle_hand(&mut self, cx: &mut Context<Self>) {
        self.hand = !self.hand;
        if let State::Ready(view) = &self.state {
            view.set_hand(self.hand);
        }
        self.changed(cx);
    }

    fn turn_page(&mut self, delta: isize, cx: &mut Context<Self>) {
        let page = self
            .page
            .saturating_add_signed(delta)
            .min(self.pages.saturating_sub(1));
        if page == self.page {
            return;
        }
        if let State::Ready(view) = &self.state {
            view.set_page(page);
        }
        self.page = page;
        self.changed(cx);
    }
}

impl MediaView {
    pub(crate) fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let ready = matches!(&self.state, State::Ready(view) if platform::ready(view));
        let mut bar = h_flex()
            .h_full()
            .flex_none()
            .px(px(6.))
            .gap(px(6.))
            .text_size(px(look::small()))
            .text_color(p.muted);
        if self.pages > 0 {
            bar = bar
                .child(
                    bar_button("media-previous", "", Some("previous"), &p)
                        .opacity(if self.page == 0 { 0.35 } else { 1. })
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Previous page").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.turn_page(-1, cx))),
                )
                .child(format!("{} / {}", self.page + 1, self.pages))
                .child(
                    bar_button("media-next", "", Some("next"), &p)
                        .opacity(if self.page + 1 >= self.pages {
                            0.35
                        } else {
                            1.
                        })
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Next page").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.turn_page(1, cx))),
                );
        } else {
            bar = bar.child("Preview");
        }
        if ready {
            bar = bar
                .child(
                    bar_button("media-zoom-out", "−", None, &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Zoom out").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.step_zoom(1. / 1.25, cx))),
                )
                .child(
                    bar_button("media-fit", "Fit", None, &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Fit to pane").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.set_zoom(None, cx))),
                )
                .children(self.zoom.map(|z| div().child(format!("{:.0}%", z * 100.))))
                .child(
                    bar_button("media-actual-size", "Actual Size", None, &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Actual size").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Some(1.), cx))),
                )
                .child(
                    bar_button("media-zoom-in", "+", None, &p)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Zoom in").build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.step_zoom(1.25, cx))),
                )
                .child(
                    icon_button("media-hand", "preview.hand", &p, false)
                        .when(self.hand, |button| {
                            button.bg(p.accent_soft).text_color(p.accent)
                        })
                        .tooltip({
                            let hand = self.hand;
                            move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if hand {
                                    "Back to the pointer"
                                } else {
                                    "Move the picture"
                                })
                                .build(w, cx)
                            }
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_hand(cx))),
                );
        }
        bar.into_any_element()
    }
}

impl Render for MediaView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let body = match &self.state {
            State::Loading => div()
                .p_4()
                .text_color(p.muted)
                .child("Loading preview…")
                .into_any_element(),
            State::Failed(err) => div()
                .p_4()
                .text_color(p.err)
                .child(format!("This file could not be previewed: {err}"))
                .into_any_element(),
            State::Ready(view) => platform::element(view),
        };
        div().size_full().min_h_0().child(body)
    }
}

impl Workbench {
    pub(crate) fn active_document_path(&self) -> Option<&str> {
        self.opening_shown
            .as_deref()
            .or(self.media_shown.as_deref())
            .or_else(|| self.open.as_ref().map(|o| o.notebook.path.as_str()))
    }

    pub(crate) fn open_media(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        self.reveal_middle();
        self.put_away();
        self.explorer_shown = false;
        self.diff_shown = false;
        self.media_shown = Some(path.clone());
        if !self.media.contains_key(&path) {
            let remote = self.target != Target::Local;
            let rt = self.rt.clone();
            let path_for_view = path.clone();
            let handle = self.window_handle;
            let owner = cx.weak_entity();
            let view =
                cx.new(|cx| MediaView::new(path_for_view, daemon, remote, rt, handle, owner, cx));
            self.tabs.push(path.clone());
            self.media.insert(path, view);
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn sync_media(&self, cx: &mut App) {
        let visible = self.panel_visible(crate::shell::pane_sizes::Panel::Editor)
            && !self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor)
            && !self.explorer_shown
            && !self.diff_shown;
        for (path, view) in &self.media {
            let hidden = !visible || self.media_shown.as_ref() != Some(path);
            view.update(cx, |view, cx| view.hide(hidden, cx));
        }
    }

    pub(crate) fn close_media(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        if self.media.remove(path).is_none() {
            return false;
        }
        let index = self.tabs.iter().position(|p| p == path).unwrap_or(0);
        self.tabs.retain(|p| p != path);
        if self.media_shown.as_deref() == Some(path) {
            self.media_shown = None;
            if let Some(next) = self
                .tabs
                .get(index.min(self.tabs.len().saturating_sub(1)))
                .cloned()
            {
                self.show_file_tab(&next, cx);
            }
        }
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaFile, download, is_media, rasterize_svg, rasterize_svg_bytes};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn extensions_are_case_insensitive_and_not_suffix_guesses() {
        for ext in [
            "PDF", "sVg", "PNG", "jpg", "JPEG", "GIF", "WebP", "BMP", "TIF", "TIFF", "ICO",
        ] {
            assert!(is_media(&format!("/实验 #1/report.{ext}")));
        }
        for path in ["plot.png.txt", "pdf", "/png/file", "report.html"] {
            assert!(!is_media(path));
        }
    }

    #[test]
    fn svg_external_images_are_not_resolved() {
        let (file, _) = MediaFile::create().unwrap();
        std::fs::write(&file.0, br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><script>throw 1</script><image href="file:///etc/passwd" width="2" height="2"/><image href="https://example.com/a.png" width="2" height="2"/><rect width="2" height="2" fill="red"/></svg>"#).unwrap();
        let raster = rasterize_svg(file).unwrap();
        let (baseline, _) = MediaFile::create().unwrap();
        std::fs::write(&baseline.0, br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#).unwrap();
        let baseline = rasterize_svg(baseline).unwrap();
        assert_eq!(
            std::fs::read(&raster.0).unwrap(),
            std::fs::read(&baseline.0).unwrap()
        );
    }

    /// Plotting libraries (matplotlib, scanpy's spatial plots) put each
    /// picture in an SVG as an embedded PNG; only the axes are vectors.
    #[test]
    fn svg_embedded_pictures_are_drawn() {
        use base64::Engine;
        let mut red = resvg::tiny_skia::Pixmap::new(1, 1).unwrap();
        red.fill(resvg::tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let png = base64::engine::general_purpose::STANDARD.encode(red.encode_png().unwrap());
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="data:image/png;base64,{png}"/></svg>"#
        );
        let drawn =
            resvg::tiny_skia::Pixmap::decode_png(&rasterize_svg_bytes(svg.as_bytes()).unwrap())
                .unwrap();
        let pixel = drawn.pixel(2, 2).unwrap();
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
            (255, 0, 0, 255)
        );
    }

    #[tokio::test]
    async fn binary_transport_preserves_bytes_and_private_cache_lifetime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("实验 #1.PDF");
        let bytes = b"%PDF-1.7\n\x00\xff\x80\r\n";
        std::fs::write(&path, bytes).unwrap();
        let pages = sandhill_daemon::pages::Pages::open(tmp.path()).unwrap();
        let url = pages.serve(&path).await.unwrap();
        let cached = download(url.port, url.path, false).await.unwrap();
        assert_eq!(std::fs::read(&cached.0).unwrap(), bytes);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&cached.0).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let cache_path = cached.0.clone();
        drop(cached);
        assert!(!cache_path.exists());
    }

    #[tokio::test]
    async fn truncated_binary_transfers_are_rejected() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
        });
        let error = download(port, "/preview".into(), false)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("incomplete"), "{error:#}");
        server.await.unwrap();
    }
}
