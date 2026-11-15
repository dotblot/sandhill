//! The Viewer's own pages, beside those the Console's runs show: Quarto
//! previews (`quarto preview` on the daemon's machine, rendered again each
//! time the document is saved) and HTML files opened from the file tree,
//! as the Tauri app's Viewer has them. Pages on a server's ports come
//! through the daemon's connection (`Daemon::forward_port`).

use gpui_kit::*;
use sandhill_proto::{Call, PreviewInfo};

use crate::editor::shown::Page;
use crate::workbench::{BottomTab, Workbench};

/// A page the Viewer was asked to show.
pub(crate) struct Extra {
    pub key: String,
    pub title: String,
    pub state: ExtraState,
    /// The terminal a preview runs in; closing it ends the preview.
    pub terminal: Option<String>,
}

pub(crate) enum ExtraState {
    Rendering,
    Failed(String),
    Ready(Page),
}

/// One of the Viewer's pages, as it shows.
pub(crate) struct ViewerItem {
    pub page: Option<Page>,
    pub title: String,
    /// Said instead of a page: rendering, or why it failed.
    pub note: Option<String>,
    /// The Viewer's own pages can be closed (a preview is stopped).
    pub extra: Option<String>,
    pub stoppable: bool,
}

/// Whether a file is a document Quarto previews, as
/// `crates/daemon/src/preview.rs` says.
pub(crate) fn is_previewable(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".qmd") || lower.ends_with(".rmd")
}

pub(crate) fn is_html(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".html") || lower.ends_with(".htm")
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

impl Workbench {
    /// Puts a page at the end of the Viewer's own and shows it.
    fn add_extra(&mut self, extra: Extra) {
        self.problem = None;
        self.viewer_extra.retain(|e| e.key != extra.key);
        self.viewer_extra.push(extra);
        self.viewer_index = None;
        self.bottom_tab = BottomTab::Viewer;
        self.show_files = true;
    }

    /// Previews a Quarto document in the Viewer. Asking again starts a
    /// preview that stopped.
    pub(crate) fn preview_document(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        if self.held_back("render documents", cx) {
            return;
        }
        let key = format!("preview:{path}");
        let name = file_name(&path);
        self.add_extra(Extra {
            key: key.clone(),
            title: format!("Quarto Preview · {name}"),
            state: ExtraState::Rendering,
            terminal: None,
        });
        let preview = daemon.call::<PreviewInfo>(Call::PreviewDocument {
            path,
            project: self.project.clone(),
        });
        cx.spawn(async move |this, cx| {
            let preview = preview.await;
            this.update(cx, |this, cx| {
                let Some(extra) = this.viewer_extra.iter_mut().find(|e| e.key == key) else {
                    return;
                };
                match preview {
                    Ok(info) => {
                        extra.state = ExtraState::Ready(Page::Url(info.url));
                        extra.terminal = Some(info.terminal.id);
                    }
                    Err(err) => {
                        extra.state =
                            ExtraState::Failed(format!("Could not preview {name}.\n{err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Shows an HTML file from the project in the Viewer.
    pub(crate) fn open_html(&mut self, path: String, title: String, cx: &mut Context<Self>) {
        self.add_extra(Extra {
            key: format!("file:{path}"),
            title,
            state: ExtraState::Ready(Page::File(path)),
            terminal: None,
        });
        cx.notify();
    }

    /// Shows text (a vignette's R code) in the Viewer, as written.
    pub(crate) fn show_text(
        &mut self,
        key: String,
        title: String,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let escaped = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let html = format!(
            "<!doctype html><meta charset=\"utf-8\"><style>body{{margin:0;padding:12px 16px;\
             font:13px/1.5 ui-monospace,Menlo,monospace;white-space:pre-wrap}}\
             @media (prefers-color-scheme: dark){{body{{background:#1f1f24;color:#dddddf}}}}</style>{escaped}"
        );
        self.add_extra(Extra {
            key,
            title,
            state: ExtraState::Ready(Page::Html(html)),
            terminal: None,
        });
        cx.notify();
    }

    /// Closes one of the Viewer's own pages, ending its preview.
    pub(crate) fn close_extra(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.viewer_extra.iter().position(|e| e.key == key) else {
            return;
        };
        let extra = self.viewer_extra.remove(ix);
        if let (Some(terminal), Some(daemon)) = (extra.terminal, self.daemon()) {
            daemon.fire(Call::CloseTerminal { terminal });
        }
        self.viewer_index = None;
        cx.notify();
    }

    /// The Viewer's pages: those the Console's runs (and scripts') showed,
    /// oldest first, then its own. A notebook's pages stay under its cells.
    pub(crate) fn viewer_pages(&self) -> Vec<ViewerItem> {
        let mut items = self.run_pages();
        items.extend(self.viewer_extra.iter().map(|e| {
            let (page, note) = match &e.state {
                ExtraState::Rendering => (
                    None,
                    Some(format!(
                        "Rendering {} with Quarto…",
                        e.title.trim_start_matches("Quarto Preview · ")
                    )),
                ),
                ExtraState::Failed(err) => (None, Some(err.clone())),
                ExtraState::Ready(page) => (Some(page.clone()), None),
            };
            ViewerItem {
                page,
                title: e.title.clone(),
                note,
                extra: Some(e.key.clone()),
                stoppable: e.terminal.is_some(),
            }
        }));
        items
    }

    /// The Viewer's page now, with its place and how many there are.
    pub(crate) fn viewer_page(&self) -> Option<(ViewerItem, usize, usize)> {
        let pages = self.viewer_pages();
        let count = pages.len();
        let ix = self
            .viewer_index
            .unwrap_or(count.checked_sub(1)?)
            .min(count - 1);
        let item = pages.into_iter().nth(ix)?;
        Some((item, ix, count))
    }

    /// Loads the Viewer's page again. A Quarto preview that stopped or
    /// failed is started again.
    pub(crate) fn reload_viewer(&mut self, cx: &mut Context<Self>) {
        let Some((item, ..)) = self.viewer_page() else {
            return;
        };
        if let Some(path) = item
            .extra
            .as_deref()
            .and_then(|k| k.strip_prefix("preview:"))
            && item.page.is_none()
        {
            self.preview_document(path.to_string(), cx);
            return;
        }
        let Some(page) = item.page else { return };
        if let Some(view) = self.pages.get(&page.key())
            && let Err(err) = view.read(cx).reload(cx)
        {
            self.problem = Some(format!("{err:#}"));
            cx.notify();
        }
    }

    /// Opens the Viewer's page in the Mac's web browser.
    pub(crate) fn open_viewer_in_browser(&mut self, cx: &mut Context<Self>) {
        let (Some((item, ..)), Some(daemon)) = (self.viewer_page(), self.daemon()) else {
            return;
        };
        let Some(page) = item.page else { return };
        let remote = self.target != sandhill_link::Target::Local;
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        let address = crate::editor::pages::address(page, daemon, remote, near);
        cx.spawn(async move |this, cx| {
            let address = address.await;
            this.update(cx, |this, cx| match address {
                Ok(url) => cx.open_url(&url),
                Err(err) => {
                    this.problem = Some(format!("Could not open the page: {err:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}
