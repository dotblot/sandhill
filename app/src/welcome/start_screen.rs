//! Welcome: a glass device sidebar and a solid, settings-style project view.
//! Disconnecting only releases Welcome's attach, never server-side work.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{
    Call, PingReply, ProjectInfo, SessionInfo, SessionStatus, TerminalInfo, TrustedFolder,
};

use crate::agents::accounts::AccountManager;
use crate::daemon::{Daemon, Streams};
use crate::look::{self, Palette, icon, icon_button, side_pane, wall};
use crate::shell::app_window::AppWindow;
use crate::welcome::folder_dialog::{FolderDialog, FolderDialogDone, Place as FolderPlace};

mod devices;
mod projects;
mod trusted;

const SIDEBAR_WIDTH: f32 = 260.;
/// The page column's widest, and the room around it.
const CONTENT_WIDTH: f32 = 720.;
const CONTENT_PAD: f32 = 28.;
/// Welcome's opening width: the sidebar and the widest page, no wider, so
/// lists don't end in empty space (Huan, 2026-11-13).
pub(crate) const WELCOME_WIDTH: f32 = SIDEBAR_WIDTH + CONTENT_WIDTH + 2. * CONTENT_PAD;

// Secondary text sits on both translucent panes and the glass wall. The
// scrollbar-thumb token is not a text color on either translucent surface.
fn welcome_palette(cx: &App) -> Palette {
    let mut p = *Palette::get(cx);
    p.faint = p.muted;
    if p.dark {
        p.muted = p.text.opacity(0.85);
        p.faint = p.text.opacity(0.8);
    }
    p
}

/// A Welcome view requested by a menu command or device action.
pub(crate) enum StartMode {
    Adding,
    /// Shows the device's projects; the user opens its folder chooser.
    Showing(String),
    Picking(String),
    /// Resources and Storage, for this device first.
    Resources(String),
    /// Agent accounts on this device.
    Accounts(String),
}

/// What Welcome's right side shows; the sidebar selects it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Page {
    /// Pinned, open and running projects on every connected device.
    Projects,
    /// The selected device's projects.
    Device,
    Resources,
    Trusted,
    Accounts,
}

pub(crate) enum MachineLink {
    Disconnected,
    Connecting,
    Failed(String),
    Ready(Rc<Daemon>),
}

/// A machine's connection (kept while the window is open), recent projects,
/// running kernels and conversations.
pub(crate) struct Machine {
    link: MachineLink,
    attempt: u64,
    home: Option<String>,
    recent: Option<Result<Vec<ProjectInfo>, String>>,
    sessions: Vec<SessionInfo>,
    terminals: Vec<TerminalInfo>,
}

/// A project row's right-click menu.
#[derive(Clone)]
struct ProjectMenu {
    host: String,
    path: String,
    /// Its tab, when open in the main window.
    open: Option<EntityId>,
    at: Point<Pixels>,
}

pub(crate) struct Start {
    servers: Vec<String>,
    auto_connect: std::collections::BTreeMap<String, bool>,
    machine_attempt: u64,
    device_menu: Option<(String, Point<Pixels>)>,
    pinned: crate::prefs::ProjectPins,
    selected_host: String,
    connecting_server: Option<String>,
    server_dialog: bool,
    server_problem: Option<String>,
    server_attempt: u64,
    pending_folder: Option<String>,
    folder_dialog: Option<Entity<FolderDialog>>,
    folder_subscription: Option<Subscription>,
    dialog_focus: FocusHandle,
    machines: HashMap<String, Machine>,
    server_input: Entity<InputState>,
    /// The device being renamed, and the name typed.
    renaming: Option<String>,
    rename_input: Entity<InputState>,
    /// A project's icon being chosen: host and folder.
    choosing_icon: Option<(String, String)>,
    project_menu: Option<ProjectMenu>,
    problem: Option<String>,
    opening: bool,
    /// Manage → Trusted Folders is shown: each connected device's list, or
    /// why it could not be read.
    trusted: HashMap<String, Result<Vec<TrustedFolder>, String>>,
    /// Bound to both the selected device and that device's attach generation.
    accounts: Option<(String, u64, Entity<AccountManager>)>,
    /// The page on the right (Huan, 2026-11-12: Projects first).
    pub(crate) page: Page,
    _subscriptions: Vec<Subscription>,
}

impl Start {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppWindow>) -> Self {
        let server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("lab, or you@server.example.org"));
        let rename_input = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &server_input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if this.start.server_dialog
                        && let InputEvent::PressEnter { .. } = event
                    {
                        let host = input.read(cx).value().trim().to_string();
                        this.add_server(host, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &rename_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_rename(window, cx);
                    }
                },
            ),
        ];
        Start {
            servers: crate::prefs::servers(),
            auto_connect: crate::prefs::server_auto_connect(),
            machine_attempt: 0,
            device_menu: None,
            pinned: crate::prefs::pinned_projects(),
            selected_host: sandhill_link::LOCAL.into(),
            connecting_server: None,
            server_dialog: false,
            server_problem: None,
            server_attempt: 0,
            pending_folder: None,
            folder_dialog: None,
            folder_subscription: None,
            dialog_focus: cx.focus_handle(),
            machines: HashMap::new(),
            server_input,
            renaming: None,
            rename_input,
            choosing_icon: None,
            project_menu: None,
            problem: None,
            opening: false,
            trusted: HashMap::new(),
            accounts: None,
            page: Page::Device,
            _subscriptions: subscriptions,
        }
    }

    /// Only live device connections, never saved server names.
    pub(crate) fn connected_daemons(&self) -> Vec<(String, Rc<Daemon>)> {
        self.machines
            .iter()
            .filter_map(|(host, machine)| match &machine.link {
                MachineLink::Ready(daemon) => Some((host.clone(), daemon.clone())),
                _ => None,
            })
            .collect()
    }

    fn set_machine(&mut self, host: String, link: MachineLink) -> u64 {
        if self
            .accounts
            .as_ref()
            .is_some_and(|(device, _, _)| device == &host)
        {
            self.accounts = None;
        }
        self.machine_attempt += 1;
        let attempt = self.machine_attempt;
        self.machines.insert(
            host,
            Machine {
                link,
                attempt,
                home: None,
                recent: None,
                sessions: Vec::new(),
                terminals: Vec::new(),
            },
        );
        attempt
    }

    pub(crate) fn forget_host(&mut self, host: &str) {
        self.set_machine(
            host.to_string(),
            MachineLink::Failed(
                "Stopped or uninstalled. Connect explicitly to start again.".into(),
            ),
        );
    }

    /// This computer first, then saved servers alphabetically.
    fn hosts(&self) -> Vec<String> {
        ordered_hosts(&self.servers)
    }

    /// Open Folder… uses the current project or the selected Welcome device.
    pub(crate) fn last_host(&self, current: &Option<String>) -> String {
        current
            .clone()
            .unwrap_or_else(|| self.selected_host.clone())
    }

    pub(crate) fn set_mode(&mut self, mode: StartMode, window: &mut Window, cx: &mut App) {
        self.problem = None;
        self.close_dialogs(window, cx);
        if !matches!(&mode, StartMode::Accounts(host) if host == &self.selected_host && self.page == Page::Accounts)
        {
            self.accounts = None;
        }
        match mode {
            StartMode::Adding => {
                self.server_dialog = true;
                self.server_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            StartMode::Showing(host) => {
                self.selected_host = host;
                self.page = Page::Device;
            }
            StartMode::Resources(host) => {
                self.selected_host = host;
                self.page = Page::Resources;
            }
            StartMode::Accounts(host) => {
                self.selected_host = host;
                self.page = Page::Accounts;
            }
            StartMode::Picking(host) => {
                self.selected_host = host.clone();
                self.page = Page::Device;
                self.pending_folder = Some(host);
                self.dialog_focus.focus(window, cx);
            }
        }
    }

    fn close_dialogs(&mut self, window: &mut Window, cx: &mut App) {
        self.device_menu = None;
        self.renaming = None;
        self.choosing_icon = None;
        self.project_menu = None;
        self.server_dialog = false;
        self.connecting_server = None;
        self.server_problem = None;
        self.server_attempt += 1;
        self.pending_folder = None;
        self.folder_dialog = None;
        self.folder_subscription = None;
        window.blur(cx);
    }

    /// Reads each connected machine's projects and kernels again.
    pub(crate) fn refresh(&mut self, cx: &mut Context<AppWindow>) {
        let hosts: Vec<String> = self.machines.keys().cloned().collect();
        for host in hosts {
            fetch(&mut self.machines, host, cx);
        }
    }
}

/// Reads a connected machine's home, recent projects and kernels.
fn fetch(machines: &mut HashMap<String, Machine>, host: String, cx: &mut Context<AppWindow>) {
    let Some(MachineLink::Ready(daemon)) = machines.get(&host).map(|m| &m.link) else {
        return;
    };
    let ping = daemon.call::<PingReply>(Call::Ping);
    let recent = daemon.call::<Vec<ProjectInfo>>(Call::ListRecentProjects);
    let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
    let terminals = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
    let attempt = machines[&host].attempt;
    cx.spawn(async move |this, cx| {
        let (ping, recent, sessions, terminals) =
            (ping.await, recent.await, sessions.await, terminals.await);
        this.update(cx, |this, cx| {
            if let Some(machine) = this.start.machines.get_mut(&host)
                && machine.attempt == attempt
                && matches!(machine.link, MachineLink::Ready(_))
            {
                if let Ok(ping) = ping {
                    machine.home = Some(ping.home);
                }
                machine.recent = Some(recent.map_err(|err| format!("{err:#}")));
                machine.sessions = sessions.unwrap_or_default();
                machine.terminals = terminals.unwrap_or_default();
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

/// The device's name here: the one the user gave it, else This Mac or the
/// server's ssh name.
pub(crate) fn host_label(host: &str) -> String {
    if let Some(name) = crate::prefs::device_name(host) {
        name
    } else if host != sandhill_link::LOCAL {
        host.into()
    } else if cfg!(target_os = "macos") {
        "This Mac".into()
    } else {
        "This computer".into()
    }
}

/// "opened today", "opened yesterday", "opened 3 days ago", or the date.
pub(crate) fn opened_ago(ms: i64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::TimeZone;
    let Some(at) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        ..=0 => "opened today".into(),
        1 => "opened yesterday".into(),
        2..7 => format!("opened {days} days ago"),
        _ => format!("opened {}", at.format("%b %-d, %Y")),
    }
}

impl AppWindow {
    pub(crate) fn render_start(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = welcome_palette(cx);
        for host in self.start.hosts() {
            self.ensure_machine(&host, cx);
        }
        self.prepare_folder_dialog(window, cx);
        let host = self.start.selected_host.clone();
        let page = self.start.page.clone();
        let mut devices = v_flex().gap(px(2.)).child(sidebar_heading("Devices", &p));
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let active = device == host && page == Page::Device;
            let local = device == sandhill_link::LOCAL;
            let link = self.start.machines.get(&device).map(|m| &m.link);
            let state = connection_text(link);
            let color = match link {
                Some(MachineLink::Ready(_)) => p.ok,
                Some(MachineLink::Connecting) => p.busy,
                Some(MachineLink::Failed(_)) => p.err,
                _ => {
                    if active {
                        p.primary_text
                    } else {
                        p.muted
                    }
                }
            };
            let hollow = matches!(link, None | Some(MachineLink::Disconnected));
            let menu_host = device.clone();
            devices = devices.child(
                h_flex()
                    .id(("start-device", ix))
                    .h(px(34.))
                    .flex_none()
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(look::RADIUS - 4.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(p.accent).text_color(p.primary_text))
                    .when(!active, |d| d.hover(move |d| d.bg(p.ink(6.))))
                    .active(|d| d.opacity(0.8))
                    .child(
                        icon(if local {
                            "device.laptop"
                        } else {
                            "device.server"
                        })
                        .size(px(16.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(host_label(&device)),
                    )
                    .child(look::dot(color).when(hollow, |d| {
                        d.bg(color.opacity(0.)).border_1().border_color(color)
                    }))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(format!(
                            "{}: {state}",
                            host_label(&menu_host)
                        ))
                        .build(w, cx)
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let device = device.clone();
                            move |this, event: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                this.start.device_menu = Some((device.clone(), event.position));
                                this.start.dialog_focus.focus(window, cx);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let device = device.clone();
                            move |this, event: &MouseDownEvent, window, cx| {
                                if event.click_count == 2 {
                                    cx.stop_propagation();
                                    this.ask_rename(device.clone(), window, cx);
                                }
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start.selected_host = device.clone();
                        this.start.problem = None;
                        this.start.device_menu = None;
                        this.show_page(Page::Device, cx);
                    })),
            );
        }
        let resources_host = host.clone();
        devices = devices
            .child(
                sidebar_action("start-connect", "add", "Connect to Server…", &p).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.show_start(StartMode::Adding, window, cx);
                    }),
                ),
            )
            .child(
                sidebar_row(
                    "start-projects",
                    "folder",
                    "Projects",
                    page == Page::Projects,
                    &p,
                )
                .mt(px(18.))
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Projects, cx))),
            )
            .child(sidebar_heading("Manage", &p).mt(px(18.)))
            .child(
                sidebar_row(
                    "start-resources",
                    "cpu",
                    "Resources and Storage",
                    page == Page::Resources,
                    &p,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_start(StartMode::Resources(resources_host.clone()), window, cx);
                })),
            )
            .child(
                sidebar_row(
                    "start-trusted",
                    "lock",
                    "Trusted Folders",
                    page == Page::Trusted,
                    &p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.show_trusted_folders(cx))),
            )
            .child(
                sidebar_row(
                    "start-accounts",
                    "agent",
                    "Agent Accounts",
                    page == Page::Accounts,
                    &p,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let host = this.start.selected_host.clone();
                    this.show_start(StartMode::Accounts(host), window, cx);
                })),
            );
        let sidebar = side_pane(&p)
            .rounded_none()
            .border_0()
            .border_r_1()
            .border_color(p.line)
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .child(
                div()
                    .id("start-devices")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(12.))
                    // Clear of the traffic lights, which sit on the sidebar.
                    .pt(px(look::TITLE_BAR + 12.))
                    .child(devices),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .p(px(20.))
                    .child(img(brand_icon()).size(px(62.)))
                    .child(
                        h_flex()
                            .gap(px(5.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child("Sandhill"))
                            .child(div().text_color(p.muted).child(env!("CARGO_PKG_VERSION"))),
                    )
                    .children(option_env!("SANDHILL_COMMIT").map(|build| {
                        div()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .child(format!("build {}", build.get(..7).unwrap_or(build)))
                    }))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(p.muted)
                            .text_center()
                            .child("Elegance in discovery."),
                    ),
            );
        let list_height = (window.viewport_size().height - px(340.)).max(px(140.));
        let content = match page {
            Page::Projects => self.projects_page(list_height, cx),
            Page::Device => self.host_projects(host, list_height, cx),
            Page::Resources => self.resource_page(window, cx),
            Page::Trusted => self.trusted_folders(list_height, cx),
            Page::Accounts => self.agent_accounts_page(cx),
        };
        let footnote = matches!(page, Page::Projects | Page::Device);
        wall(&p)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text)
            .text_size(px(look::CHROME))
            .line_height(rems(1.4))
            .child(
                h_flex().items_stretch().flex_1().min_h_0().w_full()
                    .child(sidebar)
                    .child(div().id("start-content").flex_1().min_w_0().h_full().overflow_y_scroll().bg(p.bg).p(px(CONTENT_PAD)).pt(px(look::TITLE_BAR + CONTENT_PAD))
                        // Folders dragged from Finder open as projects on this Mac.
                        .drag_over::<ExternalPaths>(move |d, _, _, _| d.bg(p.accent_soft))
                        .on_drop(cx.listener(|this, dropped: &ExternalPaths, window, cx| {
                            let paths = dropped
                                .paths()
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect();
                            this.open_from_start(sandhill_link::LOCAL.to_string(), paths, window, cx);
                        }))
                        .child(v_flex().w_full().max_w(px(CONTENT_WIDTH)).min_h_full()
                            .child(content)
                            .when_some(self.start.problem.clone(), |d, problem| {
                                d.child(div().mt(px(12.)).text_color(p.err).child(problem))
                            })
                            .child(div().flex_1().min_h(px(28.)))
                            .when(footnote, |d| d.child(div().flex_none().text_size(px(12.5)).text_color(p.muted)
                                .child("Kernels keep running after you close a project. Reopening it reattaches to them and restores your tabs."))))),
            )
            .child(self.start_title_bar(cx))
            .children(self.render_device_menu(cx))
            .children(self.render_project_menu(cx))
            .children(self.render_start_dialog(cx))
            .children(self.start.folder_dialog.clone())
            .into_any_element()
    }

    /// The Start screen's title strip, floating over the top of both columns:
    /// the window can be dragged by it, and the theme switch sits at its
    /// right. Welcome draws no project capsules; its sidebar lists projects.
    fn start_title_bar(&self, cx: &App) -> impl IntoElement {
        let p = *Palette::get(cx);
        div()
            .absolute()
            .top_0()
            .left_0()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                div()
                    .id("start-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            if cfg!(target_os = "macos") {
                                window.titlebar_double_click();
                            } else {
                                window.zoom_window();
                            }
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                h_flex()
                    .absolute()
                    .right(px(look::EDGE))
                    .top(px(3.))
                    .h(px(25.))
                    .px(px(2.))
                    .child(
                        icon_button(
                            "start-theme",
                            if p.dark { "theme.light" } else { "theme.dark" },
                            &p,
                            true,
                        )
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if p.dark {
                                "Switch to the light theme"
                            } else {
                                "Switch to the dark theme"
                            })
                            .build(w, cx)
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, cx| look::toggle_appearance(window, cx)),
                    ),
            )
    }

    /// Shows a page on Welcome's right, with each connected device's
    /// projects, kernels and conversations read again. Resources and Storage
    /// keeps its state only while shown or while an action runs.
    pub(crate) fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if page != Page::Resources && self.resource_manager.as_ref().is_some_and(|m| !m.busy()) {
            self.resource_manager = None;
        }
        if page != Page::Accounts {
            self.start.accounts = None;
        }
        if matches!(page, Page::Projects | Page::Device) {
            self.start.refresh(cx);
        }
        self.start.page = page;
        self.start.problem = None;
        cx.notify();
    }
}

/// The folder action retains Welcome's quiet outline style.
fn welcome_button(id: &'static str, name: &str, label: &'static str, p: &Palette) -> Stateful<Div> {
    let hover = p.bg;
    h_flex()
        .id(id)
        .h(px(28.))
        .flex_none()
        .gap(px(6.))
        .px(px(10.))
        .rounded(px(look::RADIUS))
        .border_1()
        .border_color(p.glass_edge)
        .bg(p.glass)
        .text_color(p.text)
        .text_size(px(look::SMALL))
        .font_weight(FontWeight::NORMAL)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .active(|d| d.opacity(0.8))
        .child(icon(name).size(px(14.)))
        .child(label)
}

fn sidebar_heading(label: &'static str, p: &Palette) -> Div {
    div()
        .px(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.muted)
        .child(label)
}

/// "2 kernels", "3 agents": what runs in a project, when anything does.
fn work_status(kernels: usize, agents: usize) -> Vec<String> {
    let mut parts = Vec::new();
    if kernels > 0 {
        parts.push(format!(
            "{kernels} kernel{}",
            if kernels == 1 { "" } else { "s" }
        ));
    }
    if agents > 0 {
        parts.push(format!(
            "{agents} agent{}",
            if agents == 1 { "" } else { "s" }
        ));
    }
    parts
}

/// Running conversations by project folder (not shells or jobs).
fn agent_counts(terminals: &[TerminalInfo]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for terminal in terminals {
        if terminal.running
            && terminal.kind != sandhill_proto::TERMINAL_SHELL
            && terminal.kind != sandhill_proto::TERMINAL_JOB
            && let Some(project) = &terminal.project
        {
            *counts.entry(project.clone()).or_default() += 1;
        }
    }
    counts
}

/// Live kernels by project folder.
fn kernel_counts(sessions: &[SessionInfo]) -> std::collections::BTreeMap<String, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for session in sessions {
        if session.status != SessionStatus::Ended
            && let Some(project) = &session.project
        {
            *counts.entry(project.clone()).or_default() += 1;
        }
    }
    counts
}

fn sidebar_action(id: &'static str, name: &str, label: &'static str, p: &Palette) -> Stateful<Div> {
    sidebar_row(id, name, label, false, p)
}

/// A sidebar row; the selected one is filled with the accent, as the
/// selected device is. GPUI takes one hover style per element.
fn sidebar_row(
    id: &'static str,
    name: &str,
    label: &'static str,
    selected: bool,
    p: &Palette,
) -> Stateful<Div> {
    let hover = p.ink(6.);
    h_flex()
        .id(id)
        .min_h(px(34.))
        .px(px(10.))
        .gap(px(8.))
        .rounded(px(look::RADIUS - 4.))
        .cursor_pointer()
        .map(|d| {
            if selected {
                d.bg(p.accent).text_color(p.primary_text)
            } else {
                d.text_color(p.muted).hover(move |d| d.bg(hover))
            }
        })
        .active(|d| d.opacity(0.8))
        .child(icon(name).size(px(16.)))
        .child(label)
}

fn auto_connect_host(host: &str, preferences: &std::collections::BTreeMap<String, bool>) -> bool {
    host == sandhill_link::LOCAL || preferences.get(host).copied().unwrap_or(true)
}

fn ordered_hosts(servers: &[String]) -> Vec<String> {
    let mut hosts = servers.to_vec();
    hosts.retain(|host| host != sandhill_link::LOCAL);
    hosts.sort();
    hosts.dedup();
    hosts.insert(0, sandhill_link::LOCAL.into());
    hosts
}

fn connection_text(link: Option<&MachineLink>) -> String {
    match link {
        Some(MachineLink::Ready(_)) => "Connected".into(),
        Some(MachineLink::Connecting) => "Connecting…".into(),
        Some(MachineLink::Failed(error)) => format!("Connection failed: {error}"),
        None | Some(MachineLink::Disconnected) => "Not connected".into(),
    }
}

fn connected_summary(projects: Option<usize>, kernels: usize) -> String {
    let mut text = "Connected".to_string();
    if let Some(projects) = projects {
        text.push_str(&format!(
            " · {projects} project{}",
            if projects == 1 { "" } else { "s" }
        ));
    }
    text.push_str(&format!(
        " · {kernels} kernel{} running",
        if kernels == 1 { "" } else { "s" }
    ));
    text
}

/// A muted line in a device pane.
fn note(text: &str, p: &Palette) -> Div {
    div()
        .px(px(12.))
        .py(px(10.))
        .text_color(p.muted)
        .child(text.to_string())
}

/// The app's crane icon, shared by Welcome's sidebar brand block.
pub(crate) fn brand_icon() -> Arc<Image> {
    static ICON: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../../assets/brand/icon.png").to_vec(),
        ))
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::{
        MachineLink, auto_connect_host, connected_summary, connection_text, opened_ago,
        ordered_hosts,
    };
    use chrono::TimeZone;

    #[test]
    fn automatic_connections_default_on_but_respect_each_server() {
        let preferences = [
            ("lab".into(), false),
            ("cluster".into(), true),
            (sandhill_link::LOCAL.into(), false),
        ]
        .into();
        assert!(!auto_connect_host("lab", &preferences));
        assert!(auto_connect_host("cluster", &preferences));
        assert!(auto_connect_host("new-server", &preferences));
        assert!(auto_connect_host(sandhill_link::LOCAL, &preferences));
    }

    #[test]
    fn devices_are_local_first_then_unique_sorted_servers() {
        let servers = ["zebra", "lab", "local", "lab"].map(str::to_string);
        assert_eq!(ordered_hosts(&servers), ["local", "lab", "zebra"]);
    }

    #[test]
    fn connection_subtitles_distinguish_idle_connecting_and_failure() {
        assert_eq!(connection_text(None), "Not connected");
        assert_eq!(
            connection_text(Some(&MachineLink::Disconnected)),
            "Not connected"
        );
        assert_eq!(
            connection_text(Some(&MachineLink::Connecting)),
            "Connecting…"
        );
        assert_eq!(
            connection_text(Some(&MachineLink::Failed("SSH unavailable".into()))),
            "Connection failed: SSH unavailable"
        );
        assert_eq!(
            connected_summary(Some(3), 2),
            "Connected · 3 projects · 2 kernels running"
        );
        assert_eq!(
            connected_summary(Some(1), 1),
            "Connected · 1 project · 1 kernel running"
        );
        assert_eq!(connected_summary(None, 0), "Connected · 0 kernels running");
    }

    #[test]
    fn says_when_projects_were_opened() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 2, 15, 0, 0)
            .unwrap();
        // Calendar dates, not days of 24 hours: 1 November 2026 is 25
        // hours long where summer time ends that night.
        let ms = |m: u32, d: u32, h: u32| {
            chrono::Local
                .with_ymd_and_hms(2026, m, d, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(opened_ago(ms(11, 2, 1), now), "opened today");
        assert_eq!(opened_ago(ms(11, 1, 23), now), "opened yesterday");
        assert_eq!(opened_ago(ms(10, 30, 23), now), "opened 3 days ago");
        assert!(opened_ago(ms(10, 12, 0), now).starts_with("opened Oct"));
    }
}
