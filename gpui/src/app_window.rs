//! A window, as the Tauri app's `windows.rs` and `Workbench.tsx` have it:
//! a tab for each machine in the title bar, each a [`Workbench`] connected
//! to that machine's sandhilld with one project open, and the Start screen
//! when no tab is picked. A project stays open, its kernels and agents
//! running, while another tab is shown.
//!
//! The windows open, and the tabs in each, are saved as they change, and
//! open again at launch. Closing the last window that had projects shows a
//! fresh Start window instead of quitting; quitting keeps the list as it was.

use std::sync::Arc;

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;

use crate::look::{self, Palette, capsule, capsule_divider, dot, icon, icon_button, raised};
use crate::start_screen::{Start, StartMode};
use crate::workbench::{Link, Workbench};
use crate::{
    About, CloseWindow, ConnectServer, Minimize, NewProject, OpenProject, ToggleFullScreen, Zoom,
};

/// A project on a machine: a machine tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub host: String,
    pub path: String,
}

/// A window's tabs, and the one shown (by host); none is the Start screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Places {
    pub tabs: Vec<Place>,
    #[serde(default)]
    pub current: Option<String>,
}

/// Every open window's places, saved as they change.
struct Windows {
    rt: Arc<Runtime>,
    open: Vec<(WindowId, Places)>,
    quitting: bool,
}

impl Global for Windows {}

impl Windows {
    fn save(&self) {
        if self.quitting {
            return;
        }
        let windows: Vec<Places> = self
            .open
            .iter()
            .filter(|(_, p)| !p.tabs.is_empty())
            .map(|(_, p)| p.clone())
            .collect();
        crate::prefs::save_windows(&windows);
    }
}

/// Starts keeping track of windows. `places` are the windows to open: the
/// ones saved, or one for the project named on the command line.
pub fn start(rt: Arc<Runtime>, places: Vec<Places>, cx: &mut App) {
    cx.set_global(Windows {
        rt,
        open: Vec::new(),
        quitting: false,
    });
    cx.on_window_closed(|cx, id| {
        let windows = cx.global_mut::<Windows>();
        let index = windows.open.iter().position(|(w, _)| *w == id);
        let had_tabs = index.is_some_and(|i| !windows.open.remove(i).1.tabs.is_empty());
        windows.save();
        if windows.open.is_empty() {
            if !windows.quitting && had_tabs {
                open_window(Places::default(), cx);
            } else {
                cx.quit();
            }
        }
    })
    .detach();
    if places.is_empty() {
        open_window(Places::default(), cx);
    }
    for places in places {
        open_window(places, cx);
    }
}

/// Quits, keeping the windows as they are for next time.
pub fn quit(cx: &mut App) {
    cx.global_mut::<Windows>().quitting = true;
    cx.quit();
}

/// Opens a window showing `places`.
pub fn open_window(places: Places, cx: &mut App) {
    let rt = cx.global::<Windows>().rt.clone();
    // Each window after the first a little lower and to the right.
    let step = 24. * cx.windows().len() as f32;
    let mut bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
    bounds.origin += point(px(step), px(step));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // On a Mac the title bar is Sandhill's own row of capsules, with
        // the window buttons at its left, as in the Tauri app.
        titlebar: Some(TitlebarOptions {
            title: Some("Sandhill".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(10.))),
        }),
        ..Default::default()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        look::set_appearance(window.appearance(), cx);
        // After the window's root is built, so this runs after gpui-cn's
        // own appearance observer and Sandhill's theme is the one left.
        window.defer(cx, |window, _| {
            window
                .observe_window_appearance(|window, cx| {
                    look::set_appearance(window.appearance(), cx)
                })
                .detach();
        });
        cx.new(|cx| AppWindow::new(rt, places, window, cx))
    });
    if let Err(err) = opened {
        tracing::error!("could not open a window: {err:#}");
    }
}

/// What a tab shows of its workbench, kept so the title bar and the saved
/// places change only when it does.
#[derive(Clone, PartialEq)]
struct TabInfo {
    project: Option<String>,
    home: Option<String>,
    link: u8,
    waiting: bool,
}

pub(crate) struct Tab {
    pub host: String,
    pub workbench: Entity<Workbench>,
    /// The folder asked for, kept while it isn't open (the machine can't
    /// be reached), so the tab is remembered anyway.
    wanted: String,
    info: TabInfo,
    _observe: Subscription,
}

pub struct AppWindow {
    pub(crate) rt: Arc<Runtime>,
    id: WindowId,
    pub(crate) tabs: Vec<Tab>,
    /// The tab shown, by host; none is the Start screen.
    pub(crate) current: Option<String>,
    pub(crate) start: Start,
    focus: FocusHandle,
}

impl AppWindow {
    fn new(rt: Arc<Runtime>, places: Places, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = window.window_handle().window_id();
        cx.global_mut::<Windows>().open.push((id, places.clone()));
        // Closing the window saves what was edited first, then closes it.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(this) = this.upgrade() else {
                return true;
            };
            if !this.read(cx).any_dirty(cx) {
                return true;
            }
            this.update(cx, |this, cx| this.save_and_close(window, cx));
            false
        });
        let mut this = AppWindow {
            start: Start::new(window, cx),
            rt,
            id,
            tabs: Vec::new(),
            current: None,
            focus: cx.focus_handle(),
        };
        for place in places.tabs {
            this.add_tab(place.host, place.path, window, cx);
        }
        this.current = places
            .current
            .filter(|host| this.tabs.iter().any(|t| t.host == *host));
        this.changed(window, cx);
        this
    }

    fn add_tab(&mut self, host: String, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let target = match Target::parse(&host) {
            Ok(target) => target,
            Err(err) => {
                tracing::warn!("cannot open a tab for {host}: {err}");
                return;
            }
        };
        let shell = cx.weak_entity();
        let rt = self.rt.clone();
        let project = (!path.is_empty()).then(|| path.clone());
        let workbench = cx.new(|cx| Workbench::new(rt, target, project, shell, window, cx));
        let observe = cx.observe_in(&workbench, window, |this, workbench, window, cx| {
            this.refresh_tab(&workbench, window, cx)
        });
        self.tabs.push(Tab {
            host,
            workbench,
            wanted: path,
            info: TabInfo {
                project: None,
                home: None,
                link: 0,
                waiting: false,
            },
            _observe: observe,
        });
    }

    /// Notes what changed in a tab's workbench.
    fn refresh_tab(
        &mut self,
        workbench: &Entity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let wb = workbench.read(cx);
        let info = TabInfo {
            project: wb.project.clone(),
            home: wb.about.as_ref().map(|a| a.home.clone()),
            link: match wb.link {
                Link::Connecting => 0,
                Link::Failed(_) => 1,
                Link::Ready(_) => 2,
            },
            waiting: wb.agents.as_ref().is_some_and(|a| a.read(cx).any_waiting()),
        };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.workbench == *workbench) else {
            return;
        };
        if tab.info == info {
            return;
        }
        if let Some(project) = &info.project {
            tab.wanted = project.clone();
        }
        tab.info = info;
        self.changed(window, cx);
    }

    /// The window's tabs and the one shown, as saved.
    fn places(&self) -> Places {
        Places {
            tabs: self
                .tabs
                .iter()
                .map(|t| Place {
                    host: t.host.clone(),
                    path: t.info.project.clone().unwrap_or_else(|| t.wanted.clone()),
                })
                .collect(),
            current: self.current.clone(),
        }
    }

    /// Saves the places and names the window after the project shown.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let places = self.places();
        window.set_window_title(&title(&places));
        let windows = cx.global_mut::<Windows>();
        if let Some((_, saved)) = windows.open.iter_mut().find(|(w, _)| *w == self.id)
            && *saved != places
        {
            *saved = places;
            windows.save();
        }
        cx.notify();
    }

    pub(crate) fn shown(&self) -> Option<&Tab> {
        let host = self.current.as_ref()?;
        self.tabs.iter().find(|t| t.host == *host)
    }

    /// Shows a machine's tab, or the Start screen.
    pub(crate) fn show_tab(
        &mut self,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if host != self.current {
            // A hidden tab's web pages would stay over the window.
            if let Some(tab) = self.shown() {
                tab.workbench.update(cx, |wb, cx| wb.park_pages(cx));
            }
            self.current = host;
        }
        if self.current.is_none() {
            self.start.refresh(cx);
        }
        // The new tab's view takes the keyboard.
        window.blur(cx);
        self.changed(window, cx);
    }

    /// Shows the Start screen, with a form open.
    pub(crate) fn show_start(
        &mut self,
        mode: StartMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_tab(None, window, cx);
        self.start.set_mode(mode, window, cx);
        cx.notify();
    }

    /// Opens a project in its machine's tab: replacing the project there,
    /// or in a new tab.
    pub(crate) fn open_place(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.tabs.iter_mut().find(|t| t.host == host) {
            Some(tab) => {
                tab.wanted = path.clone();
                tab.workbench.update(cx, |wb, cx| wb.open_project(path, cx));
            }
            None => self.add_tab(host.clone(), path, window, cx),
        }
        self.show_tab(Some(host), window, cx);
    }

    /// Asks, then closes a machine's tab. Its kernels and agents keep
    /// running.
    pub(crate) fn ask_close_tab(
        &mut self,
        host: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .tabs
            .iter()
            .find(|t| t.host == host)
            .and_then(|t| t.info.project.as_deref())
            .map(|p| crate::files::base_name(p).to_string())
            .unwrap_or_else(|| machine_name(&host));
        let answer = window.prompt(
            PromptLevel::Info,
            &format!("Close the {name} tab?"),
            Some("Kernels and agents there keep running."),
            &["Close", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = answer.await {
                this.update_in(cx, |this, window, cx| this.close_tab(host, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Closes a machine's tab once what was edited there is saved, and
    /// shows the tab before it, or the Start screen.
    pub(crate) fn close_tab(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter().find(|t| t.host == host) else {
            return;
        };
        let workbench = tab.workbench.clone();
        let tasks = workbench.update(cx, |wb, cx| wb.save_all_tasks(cx));
        cx.spawn_in(window, async move |this, cx| {
            for task in tasks {
                task.await;
            }
            this.update_in(cx, |this, window, cx| {
                if workbench.read(cx).any_dirty() {
                    return;
                }
                workbench.update(cx, |wb, cx| {
                    wb.park_pages(cx);
                    wb.close_project(cx);
                });
                let Some(ix) = this.tabs.iter().position(|t| t.host == host) else {
                    return;
                };
                this.tabs.remove(ix);
                if this.current.as_ref() == Some(&host) {
                    let next = this.tabs.get(ix.saturating_sub(1)).map(|t| t.host.clone());
                    this.current = None;
                    this.show_tab(next, window, cx);
                } else {
                    this.changed(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn any_dirty(&self, cx: &App) -> bool {
        self.tabs.iter().any(|t| t.workbench.read(cx).any_dirty())
    }

    /// Saves every edited document in every tab, then closes the window
    /// (⇧⌘W).
    pub(crate) fn save_and_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tasks: Vec<Task<()>> = self
            .tabs
            .iter()
            .flat_map(|t| t.workbench.update(cx, |wb, cx| wb.save_all_tasks(cx)))
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            for task in tasks {
                task.await;
            }
            // Closes even if a save failed only when nothing is left unsaved.
            let saved = this
                .update(cx, |this, cx| !this.any_dirty(cx))
                .unwrap_or(true);
            if saved {
                cx.update(|window, _| window.remove_window()).ok();
            }
        })
        .detach();
    }

    /// The machine tabs, centred in the title bar: a pill for each, the
    /// shown one raised, then + for the Start screen's Connect.
    fn render_machine_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let current = self.current.clone();
        // Over the title bar's drag area: clicks here are the tabs'.
        let mut row = capsule(&p, 25.).max_w(px(620.)).min_w_0().occlude();
        for (ix, tab) in self.tabs.iter().enumerate() {
            let picked = current.as_ref() == Some(&tab.host);
            let before_picked = ix > 0 && current.as_ref() == Some(&self.tabs[ix - 1].host);
            if ix > 0 && !picked && !before_picked {
                row = row.child(div().w(px(1.)).h(px(11.)).flex_none().bg(p.ink(16.)));
            }
            let device = if tab.host == sandhill_link::LOCAL {
                "device.laptop"
            } else {
                "device.server"
            };
            let link = match tab.info.link {
                0 => p.busy,
                1 => p.err,
                _ => p.ok,
            };
            let path = tab
                .info
                .project
                .as_deref()
                .map(|path| short_path(path, tab.info.home.as_deref()))
                .unwrap_or_else(|| short_path(&tab.wanted, tab.info.home.as_deref()));
            let hover = p.ink(6.);
            let pill = h_flex()
                .id(("mtab", ix))
                .flex_shrink(1.)
                .min_w_0()
                .max_w(px(300.))
                .gap(px(6.))
                .h(px(21.))
                .px(px(12.))
                .rounded_full()
                .whitespace_nowrap()
                .cursor_pointer()
                .text_color(p.muted)
                .when(!picked, |d| d.hover(move |d| d.bg(hover)))
                .child(icon(device).size(px(14.)).text_color(p.muted))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .child(machine_name(&tab.host)),
                )
                .child(
                    div()
                        .text_color(p.faint)
                        .overflow_hidden()
                        .text_ellipsis()
                        .min_w_0()
                        .child(path),
                )
                .child(dot(link).size(px(6.)))
                .when(tab.info.waiting && !picked, |d| {
                    d.child(dot(p.accent).size(px(6.)))
                })
                .on_click({
                    let host = tab.host.clone();
                    cx.listener(move |this, _, window, cx| {
                        this.show_tab(Some(host.clone()), window, cx)
                    })
                })
                .on_mouse_down(MouseButton::Right, {
                    let host = tab.host.clone();
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.ask_close_tab(host.clone(), window, cx)
                    })
                });
            row = row.child(if picked { raised(pill, &p) } else { pill });
        }
        if !self.tabs.is_empty() {
            row = row.child(capsule_divider(&p));
        }
        row = row.child(
            icon_button("new-machine", "add", &p, true)
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Connect to server ⇧⌘O").build(w, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_start(StartMode::Adding, window, cx)
                })),
        );
        h_flex()
            .absolute()
            .top(px(6.))
            .left(px(240.))
            .right(px(240.))
            .h(px(25.))
            .justify_center()
            .child(row)
    }
}

impl Render for AppWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.shown() {
            Some(tab) => tab.workbench.clone().into_any_element(),
            None => {
                if window.focused(cx).is_none() {
                    window.focus(&self.focus, cx);
                }
                self.render_start(window, cx)
            }
        };
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenProject, window, cx| {
                let host = this.start.last_host(&this.current);
                this.show_start(StartMode::Picking(host), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| {
                let host = this.start.last_host(&this.current);
                this.show_start(StartMode::Picking(host), window, cx)
            }))
            .on_action(cx.listener(|this, _: &ConnectServer, window, cx| {
                this.show_start(StartMode::Adding, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CloseWindow, window, cx| this.save_and_close(window, cx)),
            )
            .on_action(cx.listener(|_, _: &Minimize, w, _| w.minimize_window()))
            .on_action(cx.listener(|_, _: &Zoom, w, _| w.zoom_window()))
            .on_action(cx.listener(|_, _: &ToggleFullScreen, w, _| w.toggle_fullscreen()))
            .on_action(cx.listener(|_, _: &About, w, cx| {
                let detail = format!(
                    "Version {}{}",
                    env!("CARGO_PKG_VERSION"),
                    option_env!("SANDHILL_COMMIT")
                        .map(|c| format!(", build {c}"))
                        .unwrap_or_default()
                );
                drop(w.prompt(PromptLevel::Info, "Sandhill", Some(&detail), &["OK"], cx));
            }))
            .child(content)
            .child(self.render_machine_tabs(cx))
    }
}

/// The window's name in the Window menu and Mission Control: the project
/// shown and its machine.
fn title(places: &Places) -> String {
    let shown = places
        .tabs
        .iter()
        .find(|t| Some(&t.host) == places.current.as_ref());
    let Some(Place { host, path }) = shown else {
        return "Sandhill".to_string();
    };
    let name = crate::files::base_name(path);
    if host == sandhill_link::LOCAL {
        name.to_string()
    } else {
        format!("{name} — {host}")
    }
}

/// "Local", or the server's name.
pub(crate) fn machine_name(host: &str) -> String {
    if host == sandhill_link::LOCAL {
        "Local".into()
    } else {
        host.into()
    }
}

/// A path with the home folder as ~.
pub(crate) fn short_path(path: &str, home: Option<&str>) -> String {
    if let Some(home) = home.filter(|h| *h != "/" && !h.is_empty())
        && let Some(rest) = path.strip_prefix(home)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    for root in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(root) {
            let rest = rest.find('/').map_or("", |i| &rest[i..]);
            return format!("~{rest}");
        }
    }
    match path.strip_prefix("/root") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` macro would replace `#[test]`.
    use super::{Place, Places, short_path, title};

    fn place(host: &str, path: &str) -> Place {
        Place {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn names_windows() {
        let mut places = Places {
            tabs: vec![place("local", "/u/a"), place("gpu01", "/s/b")],
            current: None,
        };
        assert_eq!(title(&places), "Sandhill");
        places.current = Some("gpu01".into());
        assert_eq!(title(&places), "b — gpu01");
        places.current = Some("local".into());
        assert_eq!(title(&places), "a");
    }

    #[test]
    fn shortens_paths() {
        assert_eq!(short_path("/home/huan/proj", None), "~/proj");
        assert_eq!(short_path("/Users/h", None), "~");
        assert_eq!(short_path("/data/x", Some("/data")), "~/x");
        assert_eq!(short_path("/database", Some("/data")), "/database");
        assert_eq!(short_path("/srv/p", None), "/srv/p");
    }

    #[test]
    fn reads_saved_places() {
        let saved: Vec<Places> =
            serde_json::from_str(r#"[{"tabs":[{"host":"local","path":"/a"}],"current":"local"}]"#)
                .unwrap();
        assert_eq!(saved[0].tabs[0], place("local", "/a"));
        assert_eq!(saved[0].current.as_deref(), Some("local"));
    }
}
