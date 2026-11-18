//! Web pages (htmlwidgets, plotly figures, reports) drawn in a web view:
//! `gpui-webview` puts the system's web engine (WebKit on the Mac, WebKitGTK
//! on Linux) into the window as a native view over GPUI's drawing.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use gpui_webview::WebView;
use sandhill_proto::{Blob, Call, PageUrl};

use tokio::sync::mpsc::UnboundedSender;

use crate::daemon::Daemon;
use crate::editor::shown::{Page, plotly_page};
use crate::look;

/// What a web view is given: a page's HTML, or an address to load.
enum Content {
    Html(String),
    Url(String),
}

enum State {
    Loading,
    Ready(Entity<WebView>),
    Failed(String),
}

/// One page in its own web view. Dropping it removes the native view, so
/// the window keeps only the pages it is drawing.
pub struct PageView {
    state: State,
    /// Parked off the screen (see `hide`).
    hidden: bool,
}

impl PageView {
    /// `remote` says the daemon is on another machine, whose own addresses
    /// (`127.0.0.1:port`) the web view can't reach without a tunnel.
    /// `wheel` gets the mouse wheel's turns the page doesn't use itself, so
    /// the notebook around it scrolls.
    pub fn new(
        page: Page,
        daemon: Option<Rc<Daemon>>,
        remote: bool,
        near: Option<String>,
        wheel: Option<UnboundedSender<f32>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = content(page, daemon, remote, near);
        cx.spawn_in(window, async move |this, cx| {
            let content = content.await;
            this.update_in(cx, |this, window, cx| {
                this.state = match content.and_then(|c| build(c, wheel, window, cx)) {
                    Ok(view) => {
                        if this.hidden {
                            view.update(cx, |view, _| view.hide());
                        }
                        State::Ready(view)
                    }
                    Err(err) => State::Failed(format!("{err:#}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            state: State::Loading,
            hidden: false,
        }
    }
}

impl PageView {
    /// Takes the native view off the screen, keeping the page; drawing it
    /// again doesn't bring it back, `show` does.
    pub fn hide(&mut self, cx: &mut App) {
        self.hidden = true;
        if let State::Ready(view) = &self.state {
            view.update(cx, |view, _| view.hide());
        }
    }

    pub fn show(&mut self, cx: &mut App) {
        self.hidden = false;
        if let State::Ready(view) = &self.state {
            view.update(cx, |view, _| view.show());
        }
    }

    /// Loads the page again, as a browser's Reload does.
    pub fn reload(&self, cx: &App) -> Result<()> {
        match &self.state {
            State::Ready(view) => view
                .read(cx)
                .raw()
                .reload()
                .map_err(|e| anyhow!("could not reload: {e}")),
            _ => Ok(()),
        }
    }
}

/// Where a page can be opened in a browser on this Mac: its address, or
/// one the daemon serves its HTML at (through the daemon's connection when
/// the daemon is on another machine).
pub fn address(
    page: Page,
    daemon: Rc<Daemon>,
    remote: bool,
    near: Option<String>,
) -> Pin<Box<dyn Future<Output = Result<String>>>> {
    let serve = move |daemon: Rc<Daemon>, html: Option<String>, blob: Option<String>| async move {
        let url: PageUrl = daemon.call(Call::ServeHtml { html, blob }).await?;
        let port = reach(&daemon, remote, url.port).await?;
        Ok(format!("http://127.0.0.1:{port}{}", url.path))
    };
    match page {
        Page::Html(html) => Box::pin(serve(daemon, Some(html), None)),
        Page::Blob(hash) => Box::pin(serve(daemon, None, Some(hash))),
        Page::PlotlyBlob(hash) => Box::pin(async move {
            let blob: Blob = daemon.call(Call::GetBlob { hash, near }).await?;
            let figure = serde_json::from_str(&blob.data)?;
            serve(daemon, Some(plotly_page(&figure)), None).await
        }),
        page @ (Page::File(_) | Page::Url(_)) => Box::pin(async move {
            match content(page, Some(daemon), remote, near).await? {
                Content::Url(url) => Ok(url),
                Content::Html(_) => Err(anyhow!("the page has no address")),
            }
        }),
    }
}

type Pending = Pin<Box<dyn Future<Output = Result<Content>>>>;

/// Gets the page's HTML or address from the daemon when it isn't inline.
fn content(page: Page, daemon: Option<Rc<Daemon>>, remote: bool, near: Option<String>) -> Pending {
    let ready = |content: Result<Content>| Box::pin(async move { content }) as Pending;
    let Some(daemon) = daemon else {
        return match page {
            Page::Html(html) => ready(Ok(Content::Html(html))),
            _ => ready(Err(anyhow!("not connected"))),
        };
    };
    match page {
        Page::Html(html) => ready(Ok(Content::Html(html))),
        Page::Blob(hash) => {
            let fetch = daemon.call::<Blob>(Call::GetBlob { hash, near });
            Box::pin(async move { Ok(Content::Html(fetch.await?.data)) })
        }
        Page::PlotlyBlob(hash) => {
            let fetch = daemon.call::<Blob>(Call::GetBlob { hash, near });
            Box::pin(async move {
                let figure = serde_json::from_str(&fetch.await?.data)?;
                Ok(Content::Html(plotly_page(&figure)))
            })
        }
        Page::File(path) => {
            let served = daemon.call::<PageUrl>(Call::ServePage { path });
            Box::pin(async move {
                let url = served.await?;
                let port = reach(&daemon, remote, url.port).await?;
                Ok(Content::Url(format!("http://127.0.0.1:{port}{}", url.path)))
            })
        }
        Page::Url(url) => match local_port(&url) {
            Some(port) if remote => Box::pin(async move {
                let here = daemon.forward_port(port).await?;
                Ok(Content::Url(at_port(&url, here)))
            }),
            _ => ready(Ok(Content::Url(url))),
        },
    }
}

/// The port here that reaches `port` on the daemon's machine: itself when
/// the daemon is on this one, else one carried over its connection.
async fn reach(daemon: &Daemon, remote: bool, port: u16) -> Result<u16> {
    if remote {
        daemon.forward_port(port).await
    } else {
        Ok(port)
    }
}

/// The port of an address on the machine it was made on (`localhost`,
/// `127.0.0.1`), which another machine reaches only through a tunnel.
pub fn local_port(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
        _ => (authority, 80),
    };
    ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"]
        .contains(&host)
        .then_some(port)
}

/// The same address at `port` on 127.0.0.1.
pub fn at_port(url: &str, port: u16) -> String {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let path = rest.find(['/', '?', '#']).map_or("/", |i| &rest[i..]);
    format!("http://127.0.0.1:{port}{path}")
}

/// Sends the wheel's turns up to the window when nothing in the page
/// scrolls or uses them (a map's zoom does), as an iframe in a web page
/// would.
const PASS_WHEEL: &str = r#"
addEventListener("wheel", (e) => {
  if (e.defaultPrevented || e.ctrlKey) return;
  for (let el = e.target; el && el.nodeType === 1; el = el.parentElement) {
    const style = getComputedStyle(el);
    const scrolls = /(auto|scroll)/.test(style.overflowY) || el === document.scrollingElement;
    if (scrolls && el.scrollHeight > el.clientHeight + 1) {
      const atTop = el.scrollTop <= 0, atEnd = el.scrollTop + el.clientHeight >= el.scrollHeight - 1;
      if ((e.deltaY < 0 && !atTop) || (e.deltaY > 0 && !atEnd)) return;
    }
  }
  const px = e.deltaY * (e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? innerHeight : 1);
  window.ipc.postMessage(JSON.stringify({ wheel: px }));
});
"#;

fn build(
    content: Content,
    wheel: Option<UnboundedSender<f32>>,
    window: &mut Window,
    cx: &mut Context<PageView>,
) -> Result<Entity<WebView>> {
    let mut builder = wry::WebViewBuilder::new();
    if let Some(wheel) = wheel {
        builder = builder
            .with_initialization_script(PASS_WHEEL)
            .with_ipc_handler(move |request| {
                let message: serde_json::Value =
                    serde_json::from_str(request.body()).unwrap_or_default();
                if let Some(px) = message["wheel"].as_f64() {
                    let _ = wheel.send(px as f32);
                }
            });
    }
    let builder = match content {
        Content::Html(html) => builder.with_html(html),
        Content::Url(url) => builder.with_url(url),
    };
    let view =
        WebView::build(builder, window, cx).map_err(|e| anyhow!("the web view failed: {e}"))?;
    Ok(cx.new(|_| view))
}

impl Render for PageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let note = |s: String| {
            div()
                .p_2()
                .text_size(px(look::small()))
                .italic()
                .text_color(theme.muted_foreground)
                .child(s)
                .into_any_element()
        };
        let body = match &self.state {
            State::Loading => note("Loading the page…".into()),
            State::Failed(err) => note(format!("The page could not be shown: {err}")),
            State::Ready(view) => view.clone().into_any_element(),
        };
        div().size_full().child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{at_port, local_port};

    #[test]
    fn finds_and_moves_local_ports() {
        assert_eq!(local_port("http://127.0.0.1:4321/"), Some(4321));
        assert_eq!(local_port("http://localhost:8080/app?x=1"), Some(8080));
        assert_eq!(local_port("http://localhost/"), Some(80));
        assert_eq!(local_port("https://example.org/"), None);
        assert_eq!(local_port("http://example.org:8080/"), None);
        assert_eq!(
            at_port("http://localhost:8080/app?x=1", 5000),
            "http://127.0.0.1:5000/app?x=1"
        );
        assert_eq!(
            at_port("http://0.0.0.0:3838", 5000),
            "http://127.0.0.1:5000/"
        );
    }
}
