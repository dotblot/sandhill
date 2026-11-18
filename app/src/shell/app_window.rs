//! A window with one capsule per project, each keeping its own Workbench.
//! Projects on the same machine are independent. Only the active capsule
//! expands its document labels; hidden projects keep their work running.
//! The Start screen is shown when no project is picked.
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

use crate::look::{self, Palette, capsule, capsule_divider, dot, icon_button};
use crate::welcome::start_screen::{Start, StartMode};
use crate::workbench::{Link, Workbench};
use crate::{
    About, CloseWindow, ConnectServer, Minimize, NewProject, OpenProject, ToggleFullScreen, Zoom,
};

pub(crate) mod quit;

/// Something to do once a project has closed.
pub(crate) type AfterClose = Box<dyn FnOnce(&mut App)>;

/// A project's identity, including the machine it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub host: String,
    pub path: String,
}

/// A window's projects and its active project; none is the Start screen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Places {
    pub tabs: Vec<Place>,
    pub current: Option<Place>,
}

impl<'de> Deserialize<'de> for Places {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Current {
            Project(Place),
            Machine(String),
        }
        #[derive(Deserialize)]
        struct Saved {
            tabs: Vec<Place>,
            #[serde(default)]
            current: Option<Current>,
        }
        let saved = Saved::deserialize(deserializer)?;
        let current = match saved.current {
            Some(Current::Project(project)) => Some(project),
            Some(Current::Machine(host)) => saved.tabs.iter().find(|p| p.host == host).cloned(),
            None => None,
        };
        Ok(Self {
            tabs: saved.tabs,
            current,
        })
    }
}

/// The open windows: the one main window, whose projects are saved as they
/// change, and Welcome (Huan, 2026-11-12: Welcome is its own window).
struct Windows {
    rt: Arc<Runtime>,
    open: Vec<(WindowId, Places)>,
    main: Option<Shell>,
    welcome: Option<Shell>,
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

/// Includes inactive project capsules and projects in other windows.
pub(crate) fn host_has_project(host: &str, cx: &App) -> bool {
    cx.global::<Windows>()
        .open
        .iter()
        .any(|(_, places)| places.tabs.iter().any(|place| place.host == host))
}

/// Starts keeping track of windows. `places` are what was open: the saved
/// windows, or the project named on the command line. They open together
/// in the main window; with none, Sandhill starts with Welcome.
pub fn start(rt: Arc<Runtime>, places: Vec<Places>, cx: &mut App) {
    cx.set_global(Windows {
        rt,
        open: Vec::new(),
        main: None,
        welcome: None,
        quitting: false,
    });
    quit::install(cx);
    cx.on_window_closed(|cx, id| {
        let windows = cx.global_mut::<Windows>();
        if windows
            .welcome
            .as_ref()
            .is_some_and(|w| w.window.window_id() == id)
        {
            windows.welcome = None;
            return;
        }
        if windows
            .main
            .as_ref()
            .is_some_and(|w| w.window.window_id() == id)
        {
            windows.main = None;
            windows.open.retain(|(w, _)| *w != id);
            windows.save();
            // The projects closed: Welcome comes back to open another.
            if !windows.quitting {
                show_welcome(None, cx);
            }
        }
    })
    .detach();
    let mut merged = Places::default();
    for saved in places {
        if merged.current.is_none() {
            merged.current = saved.current.clone();
        }
        for place in saved.tabs {
            if !merged.tabs.contains(&place) {
                merged.tabs.push(place);
            }
        }
    }
    if merged.tabs.is_empty() {
        show_welcome(None, cx);
    } else {
        open_main(merged, cx);
    }
}

/// Asks how to leave connected background services, preserving window restore state.
pub fn quit(cx: &mut App) {
    quit::request(cx);
}

/// Called as the project window closes. Linux has no Dock to bring back an
/// app with no window, so there, with Welcome not open either, closing it
/// quits, asking about background work as Quit does.
fn quit_if_last(cx: &mut App) {
    if cfg!(target_os = "linux") && cx.global::<Windows>().welcome.is_none() {
        cx.defer(quit);
    }
}

/// The window options shared by Sandhill's windows; Welcome's can't be
/// made smaller than 800×600 (Huan, 2026-11-13).
fn window_options(bounds: WindowBounds, welcome: bool) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(bounds),
        // Welcome keeps its device sidebar fixed while the content can scroll.
        window_min_size: Some(if welcome {
            size(px(800.), px(600.))
        } else {
            size(px(720.), px(480.))
        }),
        // On a Mac the title bar is Sandhill's own row of capsules, with
        // the window buttons at its left, as in the Tauri app.
        titlebar: Some(TitlebarOptions {
            title: Some("Sandhill".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(10.))),
        }),
        // On a Mac the window is frosted glass unless turned off: the
        // system blurs what is behind it, and `look::wall` tints it.
        window_background: look::window_background(),
        // On Linux Sandhill draws its own title bar and window buttons
        // (`look::window_buttons`); Kit's root draws the frame, its shadow
        // and the resize edges.
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        // The desktop matches windows to the launcher's entry by this.
        app_id: Some("sandhill".into()),
        ..Default::default()
    }
}

/// A Sandhill window and its view.
#[derive(Clone)]
struct Shell {
    window: AnyWindowHandle,
    view: WeakEntity<AppWindow>,
}

impl Shell {
    fn update(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut AppWindow, &mut Window, &mut Context<AppWindow>),
    ) {
        let view = self.view.clone();
        cx.update_window(self.window, |_, window, cx| {
            view.update(cx, |this, cx| f(this, window, cx)).ok();
        })
        .ok();
    }
}

fn open_app_window(
    places: Places,
    welcome: bool,
    bounds: WindowBounds,
    cx: &mut App,
) -> Option<Shell> {
    let rt = cx.global::<Windows>().rt.clone();
    let opened = gpui_kit::open_window(window_options(bounds, welcome), cx, |window, cx| {
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
        cx.new(|cx| AppWindow::new(rt, places, welcome, window, cx))
    });
    match opened {
        Ok((window, view)) => Some(Shell {
            window,
            view: view.downgrade(),
        }),
        Err(err) => {
            tracing::error!("could not open a window: {err:#}");
            None
        }
    }
}

/// Opens the main window with `places`, filling the screen.
fn open_main(places: Places, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
    if let Some(shell) = open_app_window(places, false, WindowBounds::Maximized(bounds), cx) {
        cx.global_mut::<Windows>().main = Some(shell);
    }
}

/// Shows Welcome in its own window, as wide as its sidebar and widest page
/// and half the height of the current display (the main window's, else the
/// primary one; menu bar and Dock included), at least 800×600 and never
/// beyond the display, opening it if
/// needed, on the page `mode` asks for (else as it was, This Mac's page when
/// new).
pub(crate) fn show_welcome(mode: Option<StartMode>, cx: &mut App) {
    let shell = match cx.global::<Windows>().welcome.clone() {
        Some(shell) => shell,
        None => {
            let main_display = cx.global::<Windows>().main.clone().and_then(|main| {
                main.window
                    .update(cx, |_, window, cx| window.display(cx).map(|d| d.id()))
                    .ok()
                    .flatten()
            });
            let screen = main_display
                .and_then(|id| cx.find_display(id))
                .or_else(|| cx.primary_display())
                .map(|display| display.bounds().size)
                .unwrap_or_default();
            let width = px(crate::welcome::start_screen::WELCOME_WIDTH)
                .max(px(800.))
                .min(screen.width.max(px(800.)));
            let height = (screen.height / 2.).max(px(600.));
            let bounds = Bounds::centered(main_display, size(width, height), cx);
            let Some(shell) =
                open_app_window(Places::default(), true, WindowBounds::Windowed(bounds), cx)
            else {
                return;
            };
            cx.global_mut::<Windows>().welcome = Some(shell.clone());
            shell
        }
    };
    shell.update(cx, |this, window, cx| {
        window.activate_window();
        this.start.refresh(cx);
        if let Some(mode) = mode {
            this.show_start(mode, window, cx);
        }
    });
}

/// Does something to every project open in the main window, with its window.
pub(crate) fn update_projects(
    cx: &mut App,
    f: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) {
    let Some(main) = cx.global::<Windows>().main.clone() else {
        return;
    };
    main.update(cx, |shell, window, cx| {
        for tab in &shell.tabs {
            tab.workbench
                .update(cx, |workbench, cx| f(workbench, window, cx));
        }
    });
}

/// Opens a project in the main window, opening that window if needed, and
/// closes Welcome.
pub(crate) fn open_in_main(host: String, path: String, cx: &mut App) {
    match cx.global::<Windows>().main.clone() {
        Some(main) => main.update(cx, |this, window, cx| {
            window.activate_window();
            this.open_place(host, path, window, cx);
        }),
        None => {
            let place = Place { host, path };
            open_main(
                Places {
                    tabs: vec![place.clone()],
                    current: Some(place),
                },
                cx,
            );
        }
    }
    close_welcome(cx);
}

/// Does something to the main window, bringing it to the front.
pub(crate) fn in_main(
    cx: &mut App,
    f: impl FnOnce(&mut AppWindow, &mut Window, &mut Context<AppWindow>) + 'static,
) {
    if let Some(main) = cx.global::<Windows>().main.clone() {
        main.update(cx, |this, window, cx| {
            window.activate_window();
            f(this, window, cx);
        });
    }
}

fn close_welcome(cx: &mut App) {
    if let Some(welcome) = cx.global::<Windows>().welcome.clone() {
        welcome.update(cx, |_, window, _| window.remove_window());
    }
}

/// The main window's projects, for Welcome: tab, machine and folder.
pub(crate) fn main_projects(cx: &App) -> Vec<(EntityId, String, String)> {
    cx.global::<Windows>()
        .main
        .as_ref()
        .and_then(|main| main.view.upgrade())
        .map(|view| view.read(cx).open_projects())
        .unwrap_or_default()
}

/// How many unsaved files the main window's project tab `id` has.
pub(crate) fn main_unsaved(id: EntityId, cx: &App) -> usize {
    cx.global::<Windows>()
        .main
        .as_ref()
        .and_then(|main| main.view.upgrade())
        .and_then(|view| {
            view.read(cx)
                .tabs
                .iter()
                .find(|t| t.workbench.entity_id() == id)
                .map(|t| t.workbench.read(cx).unsaved_count())
        })
        .unwrap_or(0)
}

/// Does something to the main window without bringing it to the front.
pub(crate) fn with_main(
    cx: &mut App,
    f: impl FnOnce(&mut AppWindow, &mut Window, &mut Context<AppWindow>) + 'static,
) {
    if let Some(main) = cx.global::<Windows>().main.clone() {
        main.update(cx, |this, window, cx| f(this, window, cx));
    }
}

/// Does something to Welcome's window, when it is open.
pub(crate) fn with_welcome(
    cx: &mut App,
    f: impl FnOnce(&mut AppWindow, &mut Window, &mut Context<AppWindow>) + 'static,
) {
    if let Some(welcome) = cx.global::<Windows>().welcome.clone() {
        welcome.update(cx, |this, window, cx| f(this, window, cx));
    }
}

/// Opens a Data Explorer in a window of its own, with its toolbar along the
/// top beside the window buttons.
pub(crate) fn open_explorer_window(
    explorer: Entity<crate::panes::explorer::Explorer>,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let title = format!("{} — Data Explorer", explorer.read(cx).name());
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    let mut options = window_options(WindowBounds::Windowed(bounds), false);
    if let Some(titlebar) = options.titlebar.as_mut() {
        titlebar.title = Some(title.into());
    }
    match gpui_kit::open_window(options, cx, |window, cx| {
        look::set_appearance(window.appearance(), cx);
        cx.new(|_| crate::panes::explorer::ExplorerWindow::new(explorer))
    }) {
        Ok((window, _)) => Some(window),
        Err(err) => {
            tracing::error!("could not open the Data Explorer's window: {err:#}");
            None
        }
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

/// Compare borrowed state first, so notebook scroll notifications do not
/// allocate header snapshots or redraw the title bar.
#[derive(Default)]
struct Documents {
    files: Vec<(String, bool)>,
    active: Option<String>,
    explorer: Option<EntityId>,
    explorer_shown: bool,
    diff: Option<(String, Option<String>, bool)>,
    diff_shown: bool,
    history: Option<String>,
    history_shown: bool,
}

impl Documents {
    fn update(&mut self, wb: &Workbench) -> bool {
        let active = wb.active_document_path();
        let explorer = wb.explorer.as_ref().map(Entity::entity_id);
        let history = wb.history.as_ref().map(|tab| tab.path.as_str());
        let diff_matches = match (&self.diff, &wb.diff) {
            (None, None) => true,
            (Some((file, sha, staged)), Some(diff)) => {
                file == &diff.file && sha == &diff.sha && *staged == diff.staged
            }
            _ => false,
        };
        let same = self.files.len() == wb.tabs.len()
            && self
                .files
                .iter()
                .zip(&wb.tabs)
                .all(|((path, dirty), next)| path == next && *dirty == wb.tab_dirty(next))
            && self.active.as_deref() == active
            && self.explorer == explorer
            && self.explorer_shown == wb.explorer_shown
            && diff_matches
            && self.diff_shown == wb.diff_shown
            && self.history.as_deref() == history
            && self.history_shown == wb.history_shown;
        if same {
            return false;
        }
        self.files = wb
            .tabs
            .iter()
            .map(|p| (p.clone(), wb.tab_dirty(p)))
            .collect();
        self.active = active.map(str::to_string);
        self.explorer = explorer;
        self.explorer_shown = wb.explorer_shown;
        self.diff = wb
            .diff
            .as_ref()
            .map(|d| (d.file.clone(), d.sha.clone(), d.staged));
        self.diff_shown = wb.diff_shown;
        self.history = history.map(str::to_owned);
        self.history_shown = wb.history_shown;
        true
    }
}

pub(crate) struct Tab {
    pub host: String,
    pub workbench: Entity<Workbench>,
    /// The folder asked for, kept while it isn't open (the machine can't
    /// be reached), so the tab is remembered anyway.
    wanted: String,
    info: TabInfo,
    documents: Documents,
    _observe: Subscription,
}

pub struct AppWindow {
    pub(crate) rt: Arc<Runtime>,
    id: WindowId,
    /// Welcome's window: no projects, only the Start screen.
    pub(crate) welcome: bool,
    pub(crate) tabs: Vec<Tab>,
    /// The project shown.
    pub(crate) current: Option<EntityId>,
    pub(crate) start: Start,
    pub(crate) resource_manager: Option<crate::welcome::resource_manager::Manager>,
    focus: FocusHandle,
    project_scroll: ScrollHandle,
}

impl AppWindow {
    fn new(
        rt: Arc<Runtime>,
        places: Places,
        welcome: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = window.window_handle().window_id();
        if !welcome {
            let windows = cx.global_mut::<Windows>();
            windows.open.push((id, places.clone()));
            // It may open with its projects already (from Welcome): saved
            // now, as `changed` saves only what differs.
            windows.save();
        }
        quit::register(cx.weak_entity(), cx);
        // Closing the window saves what was edited first, then closes it.
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(this) = this.upgrade() else {
                return true;
            };
            if cx.global::<Windows>().quitting {
                return true;
            }
            if quit::active(cx) {
                return false;
            }
            // Welcome alone: closing it quits, asking about background work.
            if this.read(cx).welcome {
                if cx.global::<Windows>().main.is_none() {
                    cx.defer(quit);
                    return false;
                }
                return true;
            }
            if !this.read(cx).any_dirty(cx) {
                this.update(cx, |this, cx| quit::retain(this, cx));
                quit_if_last(cx);
                return true;
            }
            this.update(cx, |this, cx| this.save_and_close(window, cx));
            false
        });
        // Focus changes update the window's active/inactive chrome.
        cx.observe_window_activation(window, |_, _, cx| {
            cx.notify();
        })
        .detach();
        let mut this = AppWindow {
            start: Start::new(window, cx),
            resource_manager: None,
            rt,
            id,
            welcome,
            tabs: Vec::new(),
            current: None,
            focus: cx.focus_handle(),
            project_scroll: ScrollHandle::new(),
        };
        for place in &places.tabs {
            this.add_tab(place.host.clone(), place.path.clone(), window, cx);
        }
        this.current = places
            .current
            .and_then(|place| {
                this.tabs
                    .iter()
                    .find(|t| t.host == place.host && t.wanted == place.path)
                    .map(|t| t.workbench.entity_id())
            })
            .or_else(|| this.tabs.first().map(|t| t.workbench.entity_id()));
        this.changed(window, cx);
        this
    }

    fn add_tab(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<EntityId> {
        let target = match Target::parse(&host) {
            Ok(target) => target,
            Err(err) => {
                tracing::warn!("cannot open a tab for {host}: {err}");
                return None;
            }
        };
        let shell = cx.weak_entity();
        let rt = self.rt.clone();
        let project = (!path.is_empty()).then(|| path.clone());
        let workbench = cx.new(|cx| Workbench::new(rt, target, project, shell, window, cx));
        let id = workbench.entity_id();
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
            documents: Default::default(),
        });
        Some(id)
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
        if tab.documents.update(wb) {
            cx.notify();
        }
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
            current: self.shown().map(|tab| Place {
                host: tab.host.clone(),
                path: tab
                    .info
                    .project
                    .clone()
                    .unwrap_or_else(|| tab.wanted.clone()),
            }),
        }
    }

    /// Saves the places and names the window after the project shown.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.welcome {
            window.set_window_title("Welcome to Sandhill");
            cx.notify();
            return;
        }
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
        let id = self.current?;
        self.tabs.iter().find(|t| t.workbench.entity_id() == id)
    }

    /// The projects open in this window, in capsule order: id, machine and
    /// folder (the one asked for while it isn't open yet).
    pub(crate) fn open_projects(&self) -> Vec<(EntityId, String, String)> {
        self.tabs
            .iter()
            .map(|t| {
                let path = t.info.project.as_deref().unwrap_or(&t.wanted).to_string();
                (t.workbench.entity_id(), t.host.clone(), path)
            })
            .collect()
    }

    /// The main window's projects, also when asked from Welcome.
    pub(crate) fn projects_in_main(&self, cx: &App) -> Vec<(EntityId, String, String)> {
        if self.welcome {
            main_projects(cx)
        } else {
            self.open_projects()
        }
    }

    /// Shows a project's view. From Welcome, shows it in the main window
    /// and closes Welcome.
    pub(crate) fn show_tab(
        &mut self,
        project: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.welcome {
            if let Some(id) = project {
                cx.defer(move |cx| {
                    in_main(cx, move |this, window, cx| {
                        this.show_tab(Some(id), window, cx)
                    });
                    close_welcome(cx);
                });
            }
            return;
        }
        if project != self.current {
            // A hidden tab's web pages would stay over the window.
            if let Some(tab) = self.shown() {
                tab.workbench.update(cx, |wb, cx| wb.park_pages(cx));
            }
            self.current = project;
            if let Some(ix) = self
                .tabs
                .iter()
                .position(|t| Some(t.workbench.entity_id()) == project)
            {
                self.project_scroll.scroll_to_item(ix);
            }
        }
        // The new tab's view takes the keyboard.
        window.blur(cx);
        self.changed(window, cx);
    }

    /// Shows Welcome on a page, with a form open for some modes. In the main
    /// window this brings up Welcome's own window.
    pub(crate) fn show_start(
        &mut self,
        mode: StartMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.welcome {
            cx.defer(move |cx| show_welcome(Some(mode), cx));
            return;
        }
        let resources = match &mode {
            StartMode::Resources(host) => Some(host.clone()),
            _ => None,
        };
        self.start.set_mode(mode, window, cx);
        match resources {
            Some(host) if self.resource_manager.is_none() => self.open_resource_manager(host, cx),
            Some(_) => {}
            None => {
                let page = self.start.page.clone();
                self.show_page(page, cx);
            }
        }
        cx.notify();
    }

    /// Opens a project without replacing another project on the same machine.
    pub(crate) fn open_place(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Welcome opens projects in the main window, then closes.
        if self.welcome {
            cx.defer(move |cx| open_in_main(host, path, cx));
            return;
        }
        let path = if host == sandhill_link::LOCAL {
            std::fs::canonicalize(&path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(path)
        } else {
            path
        };
        let id = match self.tabs.iter().find(|t| {
            t.host == host && (t.info.project.as_deref() == Some(&path) || t.wanted == path)
        }) {
            Some(tab) => tab.workbench.entity_id(),
            None => {
                let Some(id) = self.add_tab(host, path, window, cx) else {
                    return;
                };
                id
            }
        };
        self.show_tab(Some(id), window, cx);
    }

    /// Closes a project, first asking when it has unsaved edits or running
    /// kernels: save and close (kernels keep running), save, stop its
    /// kernels and close, or cancel. With neither it closes at once.
    pub(crate) fn ask_close_tab(
        &mut self,
        id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ask_close_tab_then(id, None, window, cx);
    }

    /// As [`Self::ask_close_tab`], then `after` once the project is closed.
    pub(crate) fn ask_close_tab_then(
        &mut self,
        id: EntityId,
        after: Option<AfterClose>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.welcome {
            cx.defer(move |cx| {
                in_main(cx, move |this, window, cx| {
                    this.ask_close_tab_then(id, after, window, cx)
                })
            });
            return;
        }
        let Some(tab) = self.tabs.iter().find(|t| t.workbench.entity_id() == id) else {
            return;
        };
        let name = tab
            .info
            .project
            .as_deref()
            .map(|p| crate::panes::files::base_name(p).to_string())
            .unwrap_or_else(|| "project".into());
        let unsaved = tab.workbench.read(cx).unsaved_count();
        let kernels = tab.workbench.read(cx).project_kernels();
        if unsaved == 0 && kernels.is_empty() {
            self.close_tab_then(id, after, window, cx);
            return;
        }
        let files = match unsaved {
            0 => None,
            1 => Some("1 unsaved file will be saved.".to_string()),
            n => Some(format!("{n} unsaved files will be saved.")),
        };
        let running = match kernels.len() {
            0 => None,
            1 => Some("1 kernel is running.".to_string()),
            n => Some(format!("{n} kernels are running.")),
        };
        let detail = [files, running]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let (keep, stop) = if unsaved == 0 {
            ("Close", "Stop Kernels and Close")
        } else {
            ("Save and Close", "Save, Stop Kernels and Close")
        };
        let mut answers = vec![keep];
        if !kernels.is_empty() {
            answers.push(stop);
        }
        answers.push("Cancel");
        let answer = window.prompt(
            PromptLevel::Info,
            &format!("Close {name}?"),
            Some(&detail),
            &answers,
            cx,
        );
        let stops = !kernels.is_empty();
        cx.spawn_in(window, async move |this, cx| {
            let choice = answer.await;
            this.update_in(cx, |this, window, cx| match choice {
                Ok(0) => this.close_tab_then(id, after, window, cx),
                Ok(1) if stops => {
                    this.stop_project_kernels(id, cx);
                    this.close_tab_then(id, after, window, cx);
                }
                _ => {}
            })
            .ok();
        })
        .detach();
    }

    /// Shuts down the kernels of a project's tab.
    pub(crate) fn stop_project_kernels(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|t| t.workbench.entity_id() == id) {
            tab.workbench
                .update(cx, |wb, cx| wb.shut_down_project_kernels(cx));
        }
    }

    /// Saves a project tab's edited files.
    pub(crate) fn save_tab(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.iter().find(|t| t.workbench.entity_id() == id) {
            for task in tab.workbench.update(cx, |wb, cx| wb.save_all_tasks(cx)) {
                task.detach();
            }
        }
    }

    /// Closes a project after saving its edits, then selects a neighbour.
    pub(crate) fn close_tab(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_then(id, None, window, cx);
    }

    fn close_tab_then(
        &mut self,
        id: EntityId,
        after: Option<AfterClose>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|t| t.workbench.entity_id() == id) else {
            return;
        };
        let workbench = tab.workbench.clone();
        let tasks = workbench.update(cx, |wb, cx| wb.save_all_tasks(cx));
        cx.spawn_in(window, async move |this, cx| {
            for task in tasks {
                task.await;
            }
            this.update_in(cx, move |this, window, cx| {
                if quit::active(cx) || workbench.read(cx).any_dirty() {
                    return;
                }
                quit::retain(this, cx);
                workbench.update(cx, |wb, cx| {
                    wb.park_pages(cx);
                    wb.close_project(cx);
                });
                if let Some(ix) = this.tabs.iter().position(|t| t.workbench.entity_id() == id) {
                    this.tabs.remove(ix);
                    if this.tabs.is_empty() {
                        // The last project closed: the main window goes,
                        // and Welcome comes back.
                        window.remove_window();
                    } else if this.current == Some(id) {
                        let next = this
                            .tabs
                            .get(ix.saturating_sub(1))
                            .map(|t| t.workbench.entity_id());
                        this.current = None;
                        this.show_tab(next, window, cx);
                    } else {
                        this.changed(window, cx);
                    }
                }
                if let Some(after) = after {
                    after(cx);
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
        if quit::active(cx) {
            return;
        }
        if self.welcome {
            if cx.global::<Windows>().main.is_none() {
                cx.defer(quit);
            } else {
                window.remove_window();
            }
            return;
        }
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
            if saved && !cx.update(|_, cx| quit::active(cx)).unwrap_or(true) {
                this.update(cx, |this, cx| quit::retain(this, cx)).ok();
                cx.update(|window, cx| {
                    window.remove_window();
                    quit_if_last(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// Each project gets its own capsule. Only the active project's document
    /// tabs have labels and close buttons; inactive projects show icons.
    fn render_project_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let mut row = h_flex()
            .id("project-capsules-scroll")
            .gap(px(look::GAP_4))
            .min_w_0()
            .max_w_full()
            .overflow_x_scroll()
            .track_scroll(&self.project_scroll);
        for (ix, tab) in self.tabs.iter().enumerate() {
            let id = tab.workbench.entity_id();
            let picked = self.current == Some(id);
            let path = tab.info.project.as_deref().unwrap_or(&tab.wanted);
            let project = if path.is_empty() {
                crate::welcome::start_screen::host_label(&tab.host)
            } else {
                crate::panes::files::base_name(path).to_string()
            };
            let tip = format!(
                "{}: {path}",
                crate::welcome::start_screen::host_label(&tab.host)
            );
            let documents = tab
                .workbench
                .update(cx, |wb, cx| wb.render_document_tabs(picked, cx));
            // The active project's close button follows its name; others'
            // take the device icon's place under the pointer.
            let (device, close) = look::place_close(
                look::icon(if tab.host == sandhill_link::LOCAL {
                    "device.laptop"
                } else {
                    "device.server"
                })
                .size(px(look::ICON_LARGE))
                .into_any_element(),
                look::tab_close(("close-project", ix), picked, &p)
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Close project").build(w, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.ask_close_tab(id, window, cx);
                    })),
                picked,
            );
            // Not raised itself: the name's colour marks the active project,
            // and only its picked document is raised.
            let pill = capsule(&p, 26.)
                .id(("project-capsule", ix))
                .group(look::TAB_GROUP)
                .flex_none()
                .occlude()
                .gap(px(look::GAP_2))
                .pl(px(7.))
                .pr(px(3.))
                .text_size(px(look::small()))
                .text_color(if picked { p.text } else { p.muted })
                .child(device)
                // A project's chosen icon (Welcome → right-click → Change Icon…).
                .children(crate::prefs::project_icon(&tab.host, path).map(|icon| {
                    look::picture(&icon)
                        .size(px(look::ICON_LARGE))
                        .ml(px(look::GAP_2))
                }))
                .child(
                    div()
                        .id(("project-name", ix))
                        .flex_none()
                        .max_w(px(180.))
                        .pl(px(look::GAP_4))
                        .overflow_hidden()
                        .text_ellipsis()
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .child(project)
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        }),
                )
                .children(close)
                .when(!documents.is_empty(), |d| d.child(capsule_divider(&p)))
                .children(documents)
                .when(tab.info.waiting && !picked, |d| {
                    d.child(dot(p.accent).size(px(6.)))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_tab(Some(id), window, cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.ask_close_tab(id, window, cx);
                    }),
                );
            row = row.child(pill);
        }
        // Drawn only over a project: Welcome lists open projects in its sidebar.
        row = row.child(
            icon_button("new-project", "add", &p, false)
                .flex_none()
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Open a project").build(w, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    let host = this.start.last_host(&this.shown().map(|t| t.host.clone()));
                    this.show_start(StartMode::Showing(host), window, cx);
                })),
        );
        div()
            .absolute()
            .top(px(3.))
            .left(px(238.))
            .right(px(42.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .child(row)
    }
}

impl Render for AppWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.welcome {
            if window.focused(cx).is_none() {
                window.focus(&self.focus, cx);
            }
            self.render_start(window, cx)
        } else {
            match self.shown().or_else(|| self.tabs.first()) {
                Some(tab) => tab.workbench.clone().into_any_element(),
                None => look::wall(Palette::get(cx)).size_full().into_any_element(),
            }
        };
        div()
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if quit::active(cx) {
                    cx.stop_propagation();
                    if let Some(answer) = quit::key_answer(&event.keystroke.key) {
                        cx.defer(move |cx| quit::answer(answer, cx));
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &OpenProject, window, cx| {
                let host = this.start.last_host(&this.shown().map(|t| t.host.clone()));
                this.show_start(StartMode::Picking(host), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| {
                let host = this.start.last_host(&this.shown().map(|t| t.host.clone()));
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
            .when(!self.welcome, |d| d.child(self.render_project_tabs(cx)))
            .children(quit::render(self, cx))
    }
}

/// The window's name in the Window menu and Mission Control: the project
/// shown and its machine.
fn title(places: &Places) -> String {
    let shown = places
        .tabs
        .iter()
        .find(|t| Some(*t) == places.current.as_ref());
    let Some(Place { host, path }) = shown else {
        return "Sandhill".to_string();
    };
    let name = crate::panes::files::base_name(path);
    if host == sandhill_link::LOCAL {
        name.to_string()
    } else {
        format!("{name} — {host}")
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` macro would replace `#[test]`.
    use super::{Place, Places};

    fn place(host: &str, path: &str) -> Place {
        Place {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn reads_saved_places() {
        let saved: Vec<Places> =
            serde_json::from_str(r#"[{"tabs":[{"host":"local","path":"/a"}],"current":"local"}]"#)
                .unwrap();
        assert_eq!(saved[0].tabs[0], place("local", "/a"));
        assert_eq!(saved[0].current, Some(place("local", "/a")));
        let projects = Places {
            tabs: vec![
                place("local", "/a"),
                place("local", "/b"),
                place("gpu", "/b"),
            ],
            current: Some(place("local", "/b")),
        };
        let roundtrip: Places =
            serde_json::from_str(&serde_json::to_string(&projects).unwrap()).unwrap();
        assert_eq!(roundtrip, projects);
        assert_eq!(roundtrip.current, Some(roundtrip.tabs[1].clone()));
    }
}
