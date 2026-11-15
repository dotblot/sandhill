//! A browser: a tab strip, back, forward and reload buttons, and an
//! address field over native web views.

use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, TestSupportExt as _, Window,
    base::{h_flex, v_flex},
    div,
    prelude::FluentBuilder as _,
};
use std::sync::Arc;

use smol::channel::{Sender, unbounded};

use super::{WebFrame, WebView};
use crate::{
    ActiveTheme as _, Button, Input, InputEvent, InputState, Tab, Tabs, TabsEvent, TabsState,
    gpui_kit::assets::IconName,
};

/// The element id the browser's parts derive theirs from.
const ID: &str = "browser";

/// The most tabs a browser holds. A page that opens windows in a loop
/// stops here instead of exhausting native views.
const MAX_TABS: usize = 32;

/// The search the address field falls back to for text that is not an
/// address.
const SEARCH: &str = "https://duckduckgo.com/?q=";

/// What a page tells the browser from inside its native callbacks, which
/// have no context to update the state with.
enum Notice {
    Url(SharedString, String),
    Title(SharedString, String),
    OpenTab(String),
}

struct Page {
    id: SharedString,
    view: Entity<WebView>,
    url: SharedString,
}

/// Something the browser did that its host may want to know.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrowserEvent {
    /// A tab opened. The payload is its id.
    Opened(SharedString),
    /// A tab closed. The payload is its id.
    Closed(SharedString),
    /// A tab started to load an address.
    Navigated {
        /// The id of the tab.
        tab: SharedString,
        /// The address it navigates to.
        url: SharedString,
    },
    /// A tab could not open, such as at the tab limit or when the platform
    /// refused a native view. The payload is the reason.
    Failed(SharedString),
}

/// A browser: a tab strip, back, forward and reload buttons, an address
/// field, and the selected tab's page in a [`WebFrame`].
///
/// It is a view that fills its parent: open it with
/// [`BrowserState::open`] and place the entity as a child. It repaints
/// itself when a tab opens, closes or changes. It always has at least one
/// tab, since closing the last one opens the home page again. The pages
/// are native views, so the same limits as [`WebView`] apply.
pub struct BrowserState {
    tabs: Entity<TabsState>,
    address: Entity<InputState>,
    pages: Vec<Page>,
    home: SharedString,
    next: u64,
    notices: Sender<Notice>,
    _pump: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BrowserEvent> for BrowserState {}

impl BrowserState {
    /// A browser with one tab on `home`, which a new tab opens as well.
    ///
    /// # Errors
    ///
    /// If the platform cannot create the first native web view. See
    /// [`WebView::build`] for the setup each platform needs.
    pub fn open(
        home: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> wry::Result<Entity<Self>> {
        let home = home.into();
        let state = cx.new(|cx| Self::create(home.clone(), window, cx));
        state.update(cx, |this, cx| this.open_tab(home.to_string(), window, cx))?;
        Ok(state)
    }

    fn create(home: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tabs = cx.new(|_| TabsState::new([]));
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter address"));
        let (notices, inbox) = unbounded();
        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Ok(notice) = inbox.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.apply(notice, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let subscriptions = vec![
            cx.subscribe_in(&tabs, window, |this, _, event, window, cx| match event {
                TabsEvent::Selected(_) => this.show_selected(window, cx),
                TabsEvent::Closed(id) => this.drop_page(id, window, cx),
                TabsEvent::AddRequested => {
                    this.open_home(window, cx);
                }
                _ => {}
            }),
            cx.subscribe_in(&address, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let text = input.read(cx).value().to_string();
                    this.navigate(text, cx);
                }
            }),
        ];
        Self {
            tabs,
            address,
            pages: Vec::new(),
            home,
            next: 0,
            notices,
            _pump: pump,
            _subscriptions: subscriptions,
        }
    }

    /// Opens a tab on the home page, or reports why it could not.
    fn open_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = self.home.to_string();
        self.open_reporting(home, window, cx);
    }

    /// Opens a tab on `url`, or emits [`BrowserEvent::Failed`].
    fn open_reporting(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.open_tab(url, window, cx) {
            cx.emit(BrowserEvent::Failed(error.to_string().into()));
        }
    }

    /// Opens a tab on `url` and selects it. Returns its id.
    ///
    /// # Errors
    ///
    /// If the browser holds 32 tabs, or the platform cannot
    /// create the native web view.
    pub fn open_tab(
        &mut self,
        url: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> wry::Result<SharedString> {
        let url = url.into();
        if self.pages.len() >= MAX_TABS {
            return Err(wry::Error::Io(std::io::Error::other("too many tabs")));
        }
        let id = SharedString::from(format!("tab-{}", self.next));
        self.next += 1;
        let navigation = self.notices.clone();
        let titles = self.notices.clone();
        let windows = self.notices.clone();
        let tab = id.clone();
        let named = id.clone();
        let builder = wry::WebViewBuilder::new()
            .with_url(url.clone())
            .with_navigation_handler(move |url| {
                if !is_browsable(&url) {
                    return false;
                }
                navigation.try_send(Notice::Url(tab.clone(), url)).ok();
                true
            })
            .with_document_title_changed_handler(move |title| {
                titles.try_send(Notice::Title(named.clone(), title)).ok();
            })
            .with_new_window_req_handler(move |url, _| {
                windows.try_send(Notice::OpenTab(url)).ok();
                wry::NewWindowResponse::Deny
            });
        let view = WebView::build(builder, window, cx)?;
        let view = cx.new(|_| view);
        self.pages.push(Page {
            id: id.clone(),
            view,
            url: url.into(),
        });
        self.tabs.update(cx, |tabs, cx| {
            tabs.push(
                Tab::new(id.clone(), "New tab").with_icon(IconName::Globe),
                cx,
            );
        });
        self.show_selected(window, cx);
        cx.emit(BrowserEvent::Opened(id.clone()));
        Ok(id)
    }

    /// Navigates the selected tab. Text with a scheme loads as it is, text
    /// that looks like a host gets `https://`, and anything else is a web
    /// search.
    pub fn navigate(&mut self, text: impl AsRef<str>, cx: &mut Context<Self>) {
        let url = address_to_url(text.as_ref());
        let Some(page) = self.selected_page(cx) else {
            return;
        };
        page.view.update(cx, |view, _| view.load_url(&url));
        let tab = page.id.clone();
        cx.emit(BrowserEvent::Navigated {
            tab,
            url: url.into(),
        });
    }

    /// Goes back in the selected tab's history.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.selected_page(cx) {
            page.view.update(cx, |view, _| view.back().ok());
        }
    }

    /// Goes forward in the selected tab's history.
    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.selected_page(cx) {
            page.view.update(cx, |view, _| view.forward().ok());
        }
    }

    /// Reloads the selected tab.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.selected_page(cx) {
            page.view.read(cx).raw().reload().ok();
        }
    }

    /// Closes the tab with `id`.
    pub fn close_tab(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.tabs.update(cx, |tabs, cx| tabs.remove(id, cx));
    }

    /// The tab strip's state, to observe or to select a tab.
    pub fn tabs(&self) -> &Entity<TabsState> {
        &self.tabs
    }

    /// The address of the selected tab, if there is one.
    pub fn url(&self, cx: &App) -> Option<SharedString> {
        let selected = self.tabs.read(cx).selected()?;
        self.pages
            .iter()
            .find(|page| &page.id == selected)
            .map(|page| page.url.clone())
    }

    fn selected_page(&self, cx: &App) -> Option<&Page> {
        let selected = self.tabs.read(cx).selected()?;
        self.pages.iter().find(|page| &page.id == selected)
    }

    /// Shows the selected tab's native view, hides the others, and shows
    /// the selected tab's address.
    fn show_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.tabs.read(cx).selected().cloned();
        for page in &self.pages {
            let shown = selected.as_ref() == Some(&page.id);
            page.view.read(cx).set_visible(shown).ok();
        }
        let url = self.url(cx).unwrap_or_default();
        self.address
            .update(cx, |input, cx| input.set_value(url.to_string(), window, cx));
        cx.notify();
    }

    fn drop_page(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.pages.iter().position(|page| &page.id == id) else {
            return;
        };
        self.pages.remove(index);
        cx.emit(BrowserEvent::Closed(id.clone()));
        if self.pages.is_empty() {
            self.open_home(window, cx);
        }
    }

    fn apply(&mut self, notice: Notice, window: &mut Window, cx: &mut Context<Self>) {
        match notice {
            Notice::Url(id, url) => {
                let selected = self.tabs.read(cx).selected() == Some(&id);
                let Some(page) = self.pages.iter_mut().find(|page| page.id == id) else {
                    return;
                };
                page.url = url.clone().into();
                let editing = self.address.read(cx).focus_handle(cx).is_focused(window);
                if selected && !editing {
                    self.address
                        .update(cx, |input, cx| input.set_value(url, window, cx));
                }
            }
            Notice::Title(id, title) => {
                let label = if title.trim().is_empty() {
                    SharedString::from("New tab")
                } else {
                    title.into()
                };
                self.tabs.update(cx, |tabs, cx| tabs.rename(id, label, cx));
            }
            Notice::OpenTab(url) => {
                if is_browsable(&url) {
                    self.open_reporting(url, window, cx);
                }
            }
        }
        cx.notify();
    }
}

/// Whether the browser loads `url`: web pages and `about:blank`. Files,
/// scripts, data and custom schemes stay out of a page's reach and out of
/// the address field's.
fn is_browsable(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower == "about:blank"
}

/// Whether `text` names a host on this machine or a network address, which
/// is served over plain HTTP in development.
fn is_local_host(text: &str) -> bool {
    let host = text.split(['/', '?', '#']).next().unwrap_or_default();
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    name == "localhost" || name.parse::<std::net::Ipv4Addr>().is_ok()
}

/// The address a line typed in the address field stands for.
fn address_to_url(text: &str) -> String {
    let text = text.trim();
    if is_browsable(text) {
        text.to_owned()
    } else if !text.contains(char::is_whitespace) && is_local_host(text) {
        format!("http://{text}")
    } else if !text.contains(char::is_whitespace) && !text.contains(':') && text.contains('.') {
        format!("https://{text}")
    } else {
        let query: String = text
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    char::from(byte).to_string()
                }
                b' ' => "+".to_owned(),
                _ => format!("%{byte:02X}"),
            })
            .collect();
        format!("{SEARCH}{query}")
    }
}

impl Render for BrowserState {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.selected_page(cx).map(|page| page.view.clone());
        let (fill, bar, hairline) = {
            let theme = cx.theme();
            (theme.background(), theme.metrics.title_bar, theme.border())
        };
        let button = |name: &'static str, icon: IconName, label: &'static str| {
            Button::new(ElementId::NamedChild(
                Arc::new(ElementId::Name(ID.into())),
                name.into(),
            ))
            .ghost()
            .icon(icon)
            .accessibility_label(label)
            .tooltip(label)
        };
        v_flex()
            .size_full()
            .id(ID)
            .test_support()
            .bg(fill)
            .child(div().h(bar).w_full().flex_shrink_0().child(Tabs::new(
                ElementId::NamedChild(Arc::new(ElementId::Name(ID.into())), "tabs".into()),
                &self.tabs,
            )))
            .child(
                h_flex()
                    .gap_1()
                    .p_2()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(hairline)
                    .items_center()
                    .child(
                        button("back", IconName::ArrowLeft, "Back")
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .child(
                        button("forward", IconName::ArrowRight, "Forward")
                            .on_click(cx.listener(|this, _, _, cx| this.forward(cx))),
                    )
                    .child(
                        button("reload", IconName::RotateCw, "Reload")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        Input::new(&self.address)
                            .id(ElementId::NamedChild(
                                Arc::new(ElementId::Name(ID.into())),
                                "address".into(),
                            ))
                            .accessibility_label("Address")
                            .flex_1(),
                    ),
            )
            .when_some(page, |column, page| {
                column.child(
                    div().flex_1().min_h_0().child(
                        WebFrame::new(ElementId::NamedChild(
                            Arc::new(ElementId::Name(ID.into())),
                            "page".into(),
                        ))
                        .webview(page)
                        .size_full()
                        .rounded_none()
                        .border_0(),
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{address_to_url, is_browsable};

    #[test]
    fn a_web_address_keeps_its_scheme() {
        assert_eq!(address_to_url(" https://a.dev/x "), "https://a.dev/x");
        assert_eq!(address_to_url("about:blank"), "about:blank");
    }

    #[test]
    fn a_host_gets_https() {
        assert_eq!(address_to_url("example.com/a"), "https://example.com/a");
    }

    #[test]
    fn a_local_host_gets_http() {
        assert_eq!(address_to_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(
            address_to_url("127.0.0.1:8080/x"),
            "http://127.0.0.1:8080/x"
        );
    }

    #[test]
    fn other_text_is_a_search() {
        assert_eq!(
            address_to_url("rust gpui?"),
            "https://duckduckgo.com/?q=rust+gpui%3F"
        );
    }

    #[test]
    fn files_scripts_and_data_are_never_loaded() {
        for text in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,x",
        ] {
            assert!(!is_browsable(text), "{text}");
            assert!(
                address_to_url(text).starts_with("https://duckduckgo.com/?q="),
                "{text}"
            );
        }
    }
}
