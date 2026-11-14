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

use crate::agent_accounts::AccountManager;
use crate::app_window::AppWindow;
use crate::daemon::{Daemon, Streams};
use crate::folder_dialog::{FolderDialog, FolderDialogDone, Place as FolderPlace};
use crate::look::{self, Palette, icon, icon_button, side_pane, wall};

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
    /// Render-time connects respect per-server preferences; explicit actions do not.
    fn ensure_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        if auto_connect_host(host, &self.start.auto_connect) {
            self.connect_machine(host, cx);
        }
    }

    fn connect_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        if crate::app_window::quit::active(cx)
            || crate::resource_manager::host_blocked(host, cx)
            || self.resource_manager.is_some()
            || self.start.machines.contains_key(host)
        {
            return;
        }
        let target = match Target::parse(host) {
            Ok(target) => target,
            Err(err) => {
                self.start
                    .set_machine(host.into(), MachineLink::Failed(err.to_string()));
                return;
            }
        };
        let attempt = self.start.set_machine(host.into(), MachineLink::Connecting);
        let connect = Daemon::connect(Arc::clone(&self.rt), target);
        let host = host.to_string();
        cx.spawn(async move |this, cx| {
            let result = connect.await;
            this.update(cx, |this, cx| {
                if !this
                    .start
                    .machines
                    .get(&host)
                    .is_some_and(|m| m.attempt == attempt)
                {
                    return;
                }
                match result {
                    Ok((daemon, streams)) => this.welcome_connected(host, daemon, streams, cx),
                    Err(err) => {
                        this.start
                            .set_machine(host, MachineLink::Failed(format!("{err:#}")));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn welcome_connected(
        &mut self,
        host: String,
        daemon: Daemon,
        streams: Streams,
        cx: &mut Context<Self>,
    ) {
        let closed = daemon.closed();
        let attempt = self
            .start
            .set_machine(host.clone(), MachineLink::Ready(Rc::new(daemon)));
        fetch(&mut self.start.machines, host.clone(), cx);
        let mut terminals = streams.terminals;
        let terminal_host = host.clone();
        cx.spawn(async move |this, cx| {
            while let Some(event) = terminals.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some((host, attached, accounts)) = &this.start.accounts
                            && host == &terminal_host
                            && *attached == attempt
                        {
                            accounts.update(cx, |accounts, cx| accounts.terminal_event(&event, cx));
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            closed.await;
            this.update(cx, |this, cx| {
                if this
                    .start
                    .machines
                    .get(&host)
                    .is_some_and(|m| m.attempt == attempt)
                {
                    this.start
                        .set_machine(host, MachineLink::Failed("Connection closed".into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn reconnect_welcome(&mut self, host: &str, cx: &mut Context<Self>) {
        if self
            .start
            .machines
            .get(host)
            .is_some_and(|m| matches!(m.link, MachineLink::Connecting | MachineLink::Ready(_)))
        {
            return;
        }
        self.start.machines.remove(host);
        self.connect_machine(host, cx);
        cx.notify();
    }

    fn disconnect_welcome(&mut self, host: &str, cx: &mut Context<Self>) {
        if host == sandhill_link::LOCAL || crate::app_window::host_has_project(host, cx) {
            return;
        }
        // Dropping the attach process closes only this link, not the daemon.
        self.start
            .set_machine(host.into(), MachineLink::Disconnected);
        cx.notify();
    }

    fn machine_daemon(&self, host: &str) -> Option<Rc<Daemon>> {
        match &self.start.machines.get(host)?.link {
            MachineLink::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    fn add_server(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = host.trim().to_string();
        if host.is_empty() || self.start.connecting_server.is_some() || !self.start.server_dialog {
            return;
        }
        let target = match Target::parse(&host) {
            Ok(Target::Local) => {
                self.start.server_problem = Some("\"local\" is this Mac".into());
                cx.notify();
                return;
            }
            Ok(target) => target,
            Err(err) => {
                self.start.server_problem = Some(err.to_string());
                cx.notify();
                return;
            }
        };
        self.start.server_problem = None;
        self.start.connecting_server = Some(host.clone());
        self.start.server_attempt += 1;
        let attempt = self.start.server_attempt;
        let connect = Daemon::connect(Arc::clone(&self.rt), target);
        cx.spawn_in(window, async move |this, cx| {
            let result = connect.await;
            this.update_in(cx, |this, window, cx| {
                if !this.start.server_dialog || this.start.server_attempt != attempt {
                    return;
                }
                this.start.connecting_server = None;
                match result {
                    Ok((daemon, streams)) => {
                        let mut servers = crate::prefs::servers();
                        if !servers.contains(&host) {
                            servers.push(host.clone());
                        }
                        match crate::prefs::save_servers(&servers) {
                            Ok(()) => {
                                this.start.servers = servers;
                                this.start.selected_host = host.clone();
                                this.welcome_connected(host, daemon, streams, cx);
                                this.start.close_dialogs(window, cx);
                            }
                            Err(err) => this.start.server_problem = Some(err),
                        }
                    }
                    Err(err) => this.start.server_problem = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn remove_server(&mut self, host: &str, cx: &mut Context<Self>) {
        let mut servers = crate::prefs::servers();
        servers.retain(|h| h != host);
        match crate::prefs::save_servers(&servers) {
            Ok(()) => {
                self.start.servers = servers;
                self.start.machines.remove(host);
                self.start.auto_connect.remove(host);
                crate::prefs::save_server_auto_connect(&self.start.auto_connect);
                if self.start.selected_host == host {
                    self.start.selected_host = sandhill_link::LOCAL.into();
                }
            }
            Err(err) => self.start.problem = Some(err),
        }
        cx.notify();
    }

    fn forget_project(&mut self, host: String, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.machine_daemon(&host) else {
            return;
        };
        let forgotten =
            daemon.call::<serde_json::Value>(Call::ForgetProject { path: path.clone() });
        cx.spawn(async move |this, cx| {
            let forgotten = forgotten.await;
            this.update(cx, |this, cx| {
                if let Err(err) = &forgotten {
                    this.start.problem = Some(format!("{err:#}"));
                }
                if forgotten.is_ok() {
                    this.set_project_pin(&host, &path, false, cx);
                }
                fetch(&mut this.start.machines, host, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_project_pin(&mut self, host: &str, path: &str, pinned: bool, cx: &mut Context<Self>) {
        let pins = self.start.pinned.entry(host.to_string()).or_default();
        if pinned {
            pins.insert(path.to_string());
        } else {
            pins.remove(path);
        }
        if pins.is_empty() {
            self.start.pinned.remove(host);
        }
        crate::prefs::save_pinned_projects(&self.start.pinned);
        cx.notify();
    }

    /// Opens folders as projects, each in its own capsule.
    fn open_from_start(
        &mut self,
        host: String,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.machine_daemon(&host) else {
            self.start.problem = Some(format!("{} is not connected", host_label(&host)));
            cx.notify();
            return;
        };
        if self.start.opening || paths.is_empty() {
            return;
        }
        self.start.opening = true;
        let calls: Vec<_> = paths
            .iter()
            .map(|path| daemon.call::<ProjectInfo>(Call::OpenProject { path: path.clone() }))
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            let mut opened = Vec::new();
            for (path, call) in paths.into_iter().zip(calls) {
                opened.push((path, call.await));
            }
            this.update_in(cx, |this, window, cx| {
                this.start.opening = false;
                let mut problems = Vec::new();
                for (path, opened) in opened {
                    match opened {
                        Ok(project) if project.exists => {
                            this.start.selected_host = host.clone();
                            this.open_place(host.clone(), project.path, window, cx);
                        }
                        Ok(project) => problems.push(format!("{} doesn't exist", project.path)),
                        Err(err) => problems.push(format!("Could not open {path}: {err:#}")),
                    }
                }
                this.start.problem = (!problems.is_empty()).then(|| problems.join("\n"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A menu can request the chooser before its device finishes connecting.
    fn prepare_folder_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.start.pending_folder.clone() else {
            return;
        };
        if self
            .start
            .machines
            .get(&host)
            .is_none_or(|m| matches!(m.link, MachineLink::Disconnected))
        {
            self.start.machines.remove(&host);
            self.connect_machine(&host, cx);
        }
        let Some(daemon) = self.machine_daemon(&host) else {
            return;
        };
        self.start.pending_folder = None;
        if host == sandhill_link::LOCAL {
            // This Mac's folders: the system's own window.
            let chosen = crate::folder_dialog::choose_local_folder("Open", cx);
            cx.spawn_in(window, async move |this, cx| {
                let chosen = chosen.await;
                this.update_in(cx, |this, window, cx| {
                    this.start.close_dialogs(window, cx);
                    if let Some(path) = chosen {
                        this.open_from_start(host, vec![path], window, cx);
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }
        let machine = self.start.machines.get(&host);
        let home = machine.and_then(|machine| machine.home.clone());
        let mut places: Vec<FolderPlace> = Vec::new();
        if let Some(Some(Ok(recent))) = machine.map(|machine| &machine.recent) {
            for project in recent {
                let parent = crate::files::parent_of(&project.path);
                if parent != "/"
                    && Some(&parent) != home.as_ref()
                    && !places.iter().any(|place| place.path == parent)
                    && places.len() < 6
                {
                    let label = parent.rsplit('/').next().unwrap_or(&parent).to_string();
                    places.push(FolderPlace {
                        label: label.into(),
                        path: parent,
                    });
                }
            }
        }
        let dialog = cx.new(|cx| {
            FolderDialog::new(
                daemon,
                format!("Open Folder on {}", host_label(&host)).into(),
                "Open".into(),
                home,
                places,
                window,
                cx,
            )
        });
        self.start.folder_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            move |this, _, event: &FolderDialogDone, window, cx| {
                this.start.close_dialogs(window, cx);
                if let Some(path) = event.0.clone() {
                    this.open_from_start(host.clone(), vec![path], window, cx);
                }
                cx.notify();
            },
        ));
        self.start.folder_dialog = Some(dialog);
    }

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
                            .child(format!("build {build}"))
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

    fn welcome_connection_button(
        &self,
        host: String,
        id: &'static str,
        connect_label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let link = self.start.machines.get(&host).map(|m| &m.link);
        let connected = matches!(link, Some(MachineLink::Ready(_)));
        let connecting = matches!(link, Some(MachineLink::Connecting));
        let project_open = connected && crate::app_window::host_has_project(&host, cx);
        Button::new(id)
            .small()
            .label(if connected {
                "Disconnect"
            } else if connecting {
                "Connecting…"
            } else {
                connect_label
            })
            .disabled(connecting || project_open)
            .when(project_open, |b| {
                b.tooltip("Close projects on this server before disconnecting Welcome.")
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.start.device_menu = None;
                if connected {
                    this.disconnect_welcome(&host, cx);
                } else {
                    this.reconnect_welcome(&host, cx);
                }
            }))
            .into_any_element()
    }

    fn render_device_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (host, at) = self.start.device_menu.clone()?;
        let p = welcome_palette(cx);
        let connected = matches!(
            self.start.machines.get(&host).map(|m| &m.link),
            Some(MachineLink::Ready(_))
        );
        let connecting = matches!(
            self.start.machines.get(&host).map(|m| &m.link),
            Some(MachineLink::Connecting)
        );
        let blocked = connected && crate::app_window::host_has_project(&host, cx);
        let connect_host = host.clone();
        let rename_host = host.clone();
        let local = host == sandhill_link::LOCAL;
        let menu = crate::composer_view::popup(&p)
            .id("start-device-menu")
            .track_focus(&self.start.dialog_focus)
            .occlude()
            .min_w(px(210.))
            .text_size(px(look::SMALL))
            .child(
                sidebar_action("device-menu-rename", "edit.rename", "Rename…", &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.ask_rename(rename_host.clone(), window, cx);
                    }),
                ),
            )
            .when(!local, |menu| {
                menu.child(crate::tree_actions::separator(&p))
                    .child(
                        sidebar_action(
                            "device-menu-connect",
                            if connected { "close" } else { "device.server" },
                            if connected { "Disconnect" } else { "Connect" },
                            &p,
                        )
                        .when(blocked || connecting, |d| d.opacity(0.5).cursor_default())
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if blocked {
                                "Close projects on this server before disconnecting Welcome."
                            } else if connecting {
                                "Connecting…"
                            } else if connected {
                                "Disconnect Welcome only. Server work keeps running."
                            } else {
                                "Connect to this server"
                            })
                            .build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if blocked || connecting {
                                return;
                            }
                            this.start.device_menu = None;
                            if connected {
                                this.disconnect_welcome(&connect_host, cx);
                            } else {
                                this.reconnect_welcome(&connect_host, cx);
                            }
                        })),
                    )
                    .child(crate::tree_actions::separator(&p))
                    .child(
                        sidebar_action("device-menu-remove", "delete", "Remove from List", &p)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.start.device_menu = None;
                                this.remove_server(&host, cx);
                            })),
                    )
            })
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.start.device_menu = None;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.start.device_menu = None;
                    cx.notify();
                }
            }));
        Some(
            deferred(anchored().position(at).snap_to_window().child(menu))
                .with_priority(3)
                .into_any_element(),
        )
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

    fn render_start_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = welcome_palette(cx);
        let mut card = v_flex()
            .id("start-dialog-card")
            .occlude()
            .w(px(560.))
            .max_w(relative(0.9))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(14.))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .text_color(p.text)
            .text_size(px(look::SMALL))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.start.close_dialogs(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        if let Some(host) = &self.start.renaming {
            card = card
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Rename {}", host_label(host))),
                )
                .child(Input::new(&self.start.rename_input).small())
                .child(div().text_color(p.muted).child(if host == sandhill_link::LOCAL {
                    "The name shows only in Sandhill on this Mac. Leave it empty for the default.".to_string()
                } else {
                    format!("The name shows only in Sandhill on this Mac; Sandhill still connects to {host}. Leave it empty for the default.")
                }))
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(Button::new("rename-cancel").small().label("Cancel").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.start.close_dialogs(window, cx);
                                cx.notify();
                            }),
                        ))
                        .child(
                            Button::new("rename-save")
                                .small()
                                .primary()
                                .label("Rename")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save_rename(window, cx)),
                                ),
                        ),
                );
        } else if let Some((host, path)) = &self.start.choosing_icon {
            let chosen = crate::prefs::project_icon(host, path);
            let mut grid = h_flex().flex_wrap().gap(px(6.));
            for (i, icon_path) in look::project_icon_choices().into_iter().enumerate() {
                let on = chosen.as_deref() == Some(icon_path.as_str());
                let tip = icon_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&icon_path)
                    .replace('-', " ");
                let (set_host, set_path, set_icon) =
                    (host.clone(), path.clone(), icon_path.clone());
                grid = grid.child(
                    div()
                        .id(("project-icon", i))
                        .size(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if on { p.accent } else { p.line })
                        .when(on, |d| d.bg(p.accent.opacity(0.12)))
                        .cursor_pointer()
                        .hover(move |d| d.bg(p.ink(6.)))
                        .child(look::picture(&icon_path).size(px(24.)))
                        .tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            crate::prefs::save_project_icon(&set_host, &set_path, Some(&set_icon));
                            this.start.close_dialogs(window, cx);
                            cx.notify();
                        })),
                );
            }
            let (reset_host, reset_path) = (host.clone(), path.clone());
            card = card
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Icon for {}", crate::files::base_name(path))),
                )
                .child(div().text_color(p.muted).child(
                    "Shown for this project in Sandhill on this Mac. The project folder is not changed.",
                ))
                .child(grid)
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(Button::new("icon-default").small().label("Use Default").on_click(
                            cx.listener(move |this, _, window, cx| {
                                crate::prefs::save_project_icon(&reset_host, &reset_path, None);
                                this.start.close_dialogs(window, cx);
                                cx.notify();
                            }),
                        ))
                        .child(Button::new("icon-cancel").small().label("Cancel").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.start.close_dialogs(window, cx);
                                cx.notify();
                            }),
                        )),
                );
        } else if self.start.server_dialog {
            let connecting = self.start.connecting_server.is_some();
            let empty = self.start.server_input.read(cx).value().trim().is_empty();
            card = card
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Connect to Server"))
                .child(Input::new(&self.start.server_input).small().disabled(connecting))
                .child(div().text_color(p.muted).child(
                    "Sandhill connects with ssh, using your ~/.ssh/config and keys, so a name from there works. The first time, it installs sandhilld in ~/.local/bin on the server.",
                ))
                .when_some(self.start.server_problem.clone(), |d, error| {
                    d.child(div().text_color(p.err).child(error))
                })
                .child(
                    h_flex().justify_end().gap(px(8.))
                        .child(Button::new("server-cancel").small().label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start.close_dialogs(window, cx);
                                cx.notify();
                            })))
                        .child(Button::new("server-connect").small().primary()
                            .label(if connecting { "Connecting…" } else { "Connect" })
                            .disabled(empty || connecting)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let host = this.start.server_input.read(cx).value().to_string();
                                this.add_server(host, window, cx);
                            }))),
                );
        } else if let Some(host) = &self.start.pending_folder {
            let error = match self.start.machines.get(host).map(|machine| &machine.link) {
                Some(MachineLink::Failed(error)) => Some(error.clone()),
                _ => None,
            };
            card = card
                .track_focus(&self.start.dialog_focus)
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Open Folder on {}", host_label(host))),
                )
                .child(
                    div()
                        .text_color(if error.is_some() { p.err } else { p.muted })
                        .child(error.unwrap_or_else(|| "Connecting…".into())),
                )
                .child(
                    h_flex().justify_end().child(
                        Button::new("folder-pending-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start.close_dialogs(window, cx);
                                cx.notify();
                            })),
                    ),
                );
        } else {
            return None;
        }
        Some(
            deferred(
                div()
                    .id("start-dialog-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.start.close_dialogs(window, cx);
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }

    /// Opens the rename dialog for a device, with its name now.
    fn ask_rename(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let current = crate::prefs::device_name(&host).unwrap_or_default();
        let placeholder = match host.as_str() {
            sandhill_link::LOCAL if cfg!(target_os = "macos") => "This Mac".to_string(),
            sandhill_link::LOCAL => "This computer".to_string(),
            _ => host.clone(),
        };
        self.start.close_dialogs(window, cx);
        self.start.rename_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value(current, window, cx);
            input.focus(window, cx);
        });
        self.start.renaming = Some(host);
        cx.notify();
    }

    fn save_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.start.renaming.clone() else {
            return;
        };
        let name = self.start.rename_input.read(cx).value().to_string();
        crate::prefs::save_device_name(&host, &name);
        self.start.close_dialogs(window, cx);
        // Other windows name it in their title bar and status bar too.
        cx.refresh_windows();
    }

    /// A project row's menu (its ⋯ button, or a right-click): save its
    /// files, stop its kernels, close it, change its icon. Actions that
    /// don't apply are dimmed.
    fn render_project_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.start.project_menu.clone()?;
        let p = welcome_palette(cx);
        let (icon_host, icon_path) = (menu.host.clone(), menu.path.clone());
        let unsaved = menu.open.map_or(0, |id| {
            if self.welcome {
                crate::app_window::main_unsaved(id, cx)
            } else {
                self.tabs
                    .iter()
                    .find(|t| t.workbench.entity_id() == id)
                    .map_or(0, |t| t.workbench.read(cx).unsaved_count())
            }
        });
        let kernels = self.project_kernel_ids(&menu.host, &menu.path);
        let item = |id: &'static str, name: &str, label: &'static str, enabled: bool| {
            sidebar_action(id, name, label, &p).when(!enabled, |d| d.opacity(0.4).cursor_default())
        };
        let save_open = menu.open;
        let close_open = menu.open;
        let (stop_host, stop_path) = (menu.host.clone(), menu.path.clone());
        let body = crate::composer_view::popup(&p)
            .id("start-project-menu")
            .track_focus(&self.start.dialog_focus)
            .occlude()
            .min_w(px(210.))
            .text_size(px(look::SMALL))
            .child(
                item(
                    "project-menu-save",
                    "doc.save-all",
                    "Save All Files",
                    unsaved > 0,
                )
                .when(unsaved > 0, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| {
                        this.start.project_menu = None;
                        if let Some(id) = save_open {
                            if this.welcome {
                                crate::app_window::with_main(cx, move |main, _, cx| {
                                    main.save_tab(id, cx)
                                });
                            } else {
                                this.save_tab(id, cx);
                            }
                        }
                        cx.notify();
                    }))
                }),
            )
            .child(
                item(
                    "project-menu-stop",
                    "stop",
                    "Stop All Kernels",
                    !kernels.is_empty(),
                )
                .when(!kernels.is_empty(), |d| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.start.project_menu = None;
                        this.ask_stop_kernels(stop_host.clone(), stop_path.clone(), window, cx);
                    }))
                }),
            )
            .child(
                item(
                    "project-menu-close",
                    "close",
                    "Close Project",
                    close_open.is_some(),
                )
                .when_some(close_open, |d, id| {
                    d.on_click(cx.listener(move |this, _, window, cx| {
                        this.start.project_menu = None;
                        this.ask_close_tab(id, window, cx);
                    }))
                }),
            )
            .child(div().h(px(1.)).my(px(4.)).mx(px(8.)).bg(p.line))
            .child(
                sidebar_action("project-menu-icon", "edit.compose", "Change Icon…", &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.start.close_dialogs(window, cx);
                        this.start.choosing_icon = Some((icon_host.clone(), icon_path.clone()));
                        cx.notify();
                    }),
                ),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.start.project_menu = None;
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.start.project_menu = None;
                    cx.notify();
                }
            }));
        Some(
            deferred(anchored().position(menu.at).snap_to_window().child(body))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// The running kernels a device lists for a project.
    fn project_kernel_ids(&self, host: &str, path: &str) -> Vec<String> {
        self.start
            .machines
            .get(host)
            .map(|m| {
                m.sessions
                    .iter()
                    .filter(|s| {
                        s.status != SessionStatus::Ended && s.project.as_deref() == Some(path)
                    })
                    .map(|s| s.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Asks before stopping a project's kernels: their variables are lost.
    fn ask_stop_kernels(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.project_kernel_ids(&host, &path);
        if ids.is_empty() {
            return;
        }
        let name = crate::files::base_name(&path).to_string();
        let what = if ids.len() == 1 {
            "1 kernel".to_string()
        } else {
            format!("{} kernels", ids.len())
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Stop {what} in {name}?"),
            Some("Variables in them are lost. Agents keep running."),
            &["Stop Kernels", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = answer.await {
                this.update(cx, |this, cx| {
                    let Some(daemon) = this.machine_daemon(&host) else {
                        return;
                    };
                    let stops: Vec<_> = ids
                        .into_iter()
                        .map(|session| daemon.call::<serde_json::Value>(Call::Shutdown { session }))
                        .collect();
                    cx.spawn(async move |this, cx| {
                        for stop in stops {
                            let _ = stop.await;
                        }
                        this.update(cx, |this, cx| {
                            fetch(&mut this.start.machines, host, cx);
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Removes a project from the list; an open one is closed first, after
    /// asking.
    fn ask_forget_project(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self
            .projects_in_main(cx)
            .into_iter()
            .find(|(_, h, p)| *h == host && *p == path)
            .map(|(id, ..)| id);
        let Some(id) = open else {
            self.forget_project(host, path, cx);
            return;
        };
        let name = crate::files::base_name(&path).to_string();
        let answer = window.prompt(
            PromptLevel::Info,
            &format!("Close {name} and remove it from the list?"),
            Some("It is open. Closing saves its files; you choose what happens to its kernels."),
            &["Close and Remove", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = answer.await {
                this.update_in(cx, |this, window, cx| {
                    let forget = Box::new(move |cx: &mut App| {
                        crate::app_window::with_welcome(cx, move |welcome, _, cx| {
                            welcome.forget_project(host, path, cx)
                        });
                    });
                    this.ask_close_tab_then(id, Some(forget), window, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// A row's ⋯ button, shown while the pointer is over the row: the
    /// project's menu.
    fn more_button(
        &self,
        id: SharedString,
        host: String,
        path: String,
        group: SharedString,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(24.))
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.muted)
            .cursor_pointer()
            .opacity(0.)
            .group_hover(group, |s| s.opacity(1.))
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("More").build(w, cx))
            .child(icon("more").size(px(14.)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                let open = this
                    .projects_in_main(cx)
                    .into_iter()
                    .find(|(_, h, p)| *h == host && *p == path)
                    .map(|(id, ..)| id);
                this.show_project_menu(
                    host.clone(),
                    path.clone(),
                    open,
                    event.position(),
                    window,
                    cx,
                );
            }))
    }

    fn show_project_menu(
        &mut self,
        host: String,
        path: String,
        open: Option<EntityId>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start.project_menu = Some(ProjectMenu {
            host,
            path,
            open,
            at,
        });
        self.start.dialog_focus.focus(window, cx);
        cx.notify();
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

    /// Projects: pinned ones from every connected device, then those open
    /// here, then those with live kernels. Each appears once, in the first
    /// group that has it; its status says the rest.
    fn projects_page(&self, list_height: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        let open = self.projects_in_main(cx);
        let mut pinned: Vec<ProjectEntry> = Vec::new();
        let mut running: Vec<ProjectEntry> = Vec::new();
        let mut kernels_total = 0;
        let mut unconnected = 0;
        for host in self.start.hosts() {
            let Some(machine) = self
                .start
                .machines
                .get(&host)
                .filter(|m| matches!(m.link, MachineLink::Ready(_)))
            else {
                unconnected += 1;
                continue;
            };
            let counts = kernel_counts(&machine.sessions);
            let agents = agent_counts(&machine.terminals);
            kernels_total += counts
                .keys()
                .chain(agents.keys())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            let recent: &[ProjectInfo] = match &machine.recent {
                Some(Ok(recent)) => recent,
                _ => &[],
            };
            let pins = self.start.pinned.get(&host);
            let entry = |path: &str| {
                let info = recent.iter().find(|r| r.path == path);
                ProjectEntry {
                    host: host.clone(),
                    path: path.to_string(),
                    name: info.map_or_else(
                        || crate::files::base_name(path).to_string(),
                        |i| i.name.clone(),
                    ),
                    opened_ms: info.map_or(0, |i| i.opened_ms),
                    exists: info.is_none_or(|i| i.exists),
                    open: open
                        .iter()
                        .find(|(_, h, op)| *h == host && op == path)
                        .map(|(id, ..)| *id),
                    kernels: counts.get(path).copied().unwrap_or(0),
                    agents: agents.get(path).copied().unwrap_or(0),
                    pinned: pins.is_some_and(|pins| pins.contains(path)),
                }
            };
            pinned.extend(
                recent
                    .iter()
                    .filter(|r| pins.is_some_and(|pins| pins.contains(&r.path)))
                    .map(|r| entry(&r.path)),
            );
            // Live kernels or conversations, in a project not open here.
            let busy: std::collections::BTreeSet<&String> =
                counts.keys().chain(agents.keys()).collect();
            running.extend(
                busy.into_iter()
                    .map(|path| entry(path))
                    .filter(|e| !e.pinned && e.open.is_none()),
            );
        }
        pinned.sort_by_key(|entry| std::cmp::Reverse(entry.opened_ms));
        let open_rows: Vec<ProjectEntry> = open
            .iter()
            .filter(|(_, h, path)| !pinned.iter().any(|e| e.host == *h && e.path == *path))
            .map(|(id, h, path)| {
                let machine = self.start.machines.get(h);
                let info = machine
                    .and_then(|m| m.recent.as_ref())
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|r| r.iter().find(|i| i.path == *path));
                ProjectEntry {
                    host: h.clone(),
                    path: path.clone(),
                    name: info.map_or_else(
                        || match crate::files::base_name(path) {
                            "" => host_label(h),
                            name => name.to_string(),
                        },
                        |i| i.name.clone(),
                    ),
                    opened_ms: info.map_or(0, |i| i.opened_ms),
                    exists: true,
                    open: Some(*id),
                    kernels: machine.map_or(0, |m| {
                        kernel_counts(&m.sessions).get(path).copied().unwrap_or(0)
                    }),
                    agents: machine.map_or(0, |m| {
                        agent_counts(&m.terminals).get(path).copied().unwrap_or(0)
                    }),
                    pinned: false,
                }
            })
            .collect();
        let mut summary = Vec::new();
        if !open.is_empty() {
            summary.push(format!("{} open", open.len()));
        }
        if kernels_total > 0 {
            summary.push(format!("{kernels_total} running"));
        }
        if !pinned.is_empty() {
            summary.push(format!("{} pinned", pinned.len()));
        }
        let mut column =
            v_flex().w_full().flex_none().gap(px(16.)).child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .child("Projects"),
                    )
                    .child(div().text_size(px(14.)).text_color(p.muted).child(
                        if summary.is_empty() {
                            "Nothing open, running or pinned".to_string()
                        } else {
                            summary.join(" · ")
                        },
                    )),
            );
        let groups = [
            ("Pinned", pinned),
            ("Open", open_rows),
            ("Running", running),
        ];
        let empty = groups.iter().all(|(_, rows)| rows.is_empty());
        let mut row = 0;
        for (title, rows) in groups {
            if rows.is_empty() {
                continue;
            }
            let count = rows.len();
            let list = v_flex().children(rows.into_iter().enumerate().map(|(ix, entry)| {
                row += 1;
                self.project_entry_row(row, entry, ix + 1 == count, cx)
            }));
            column = column.child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.muted)
                            .child(title),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("projects-{title}")))
                            .w_full()
                            .max_h(list_height)
                            .rounded(px(look::RADIUS))
                            .border_1()
                            .border_color(p.line)
                            .bg(p.panel)
                            .overflow_y_scroll()
                            .child(list),
                    ),
            );
        }
        if empty {
            column = column.child(note(
                "Pin a project with its star on a device's page, or open one, to see it here.",
                &p,
            ));
        }
        if unconnected > 0 {
            column = column.child(div().text_size(px(12.5)).text_color(p.muted).child(
                if unconnected == 1 {
                    "Projects on 1 device that isn't connected are not shown.".to_string()
                } else {
                    format!(
                        "Projects on {unconnected} devices that aren't connected are not shown."
                    )
                },
            ));
        }
        column.into_any_element()
    }

    /// A row of the Projects page: folder, name over path, device, status
    /// and the pin star.
    fn project_entry_row(
        &self,
        row: usize,
        entry: ProjectEntry,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = welcome_palette(cx);
        let group: SharedString = format!("project-entry-{row}").into();
        let mut status = Vec::new();
        if entry.open.is_some() {
            status.push("Open".to_string());
        }
        status.extend(work_status(entry.kernels, entry.agents));
        let status: AnyElement = if !entry.exists {
            div()
                .text_color(p.warn)
                .child("Folder not found")
                .into_any_element()
        } else if status.is_empty() {
            div()
                .child(opened_ago(entry.opened_ms, chrono::Local::now()))
                .into_any_element()
        } else {
            h_flex()
                .gap(px(5.))
                .when(entry.kernels + entry.agents > 0, |d| {
                    d.child(look::dot(p.ok))
                })
                .child(status.join(" · "))
                .into_any_element()
        };
        let (pin_host, pin_path, pinned) = (entry.host.clone(), entry.path.clone(), entry.pinned);
        let (open_host, open_path, open_id) = (entry.host.clone(), entry.path.clone(), entry.open);
        h_flex()
            .id(("project-entry", row))
            .group(group.clone())
            .w_full()
            .min_w_0()
            .px(px(12.))
            .py(px(9.))
            .gap(px(10.))
            .cursor_pointer()
            .hover(move |d| d.bg(p.ink(6.)))
            .active(|d| d.opacity(0.8))
            .when(!last, |d| d.border_b_1().border_color(p.line))
            .child(
                look::picture(&look::project_icon(&entry.host, &entry.path, &entry.name))
                    .size(px(22.)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(entry.name.clone()),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .child(
                                icon(if entry.host == sandhill_link::LOCAL {
                                    "device.laptop"
                                } else {
                                    "device.server"
                                })
                                .size(px(12.)),
                            )
                            .child(div().flex_none().child(host_label(&entry.host)))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(look::MONO)
                                    .child(entry.path.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.5))
                    .text_color(p.muted)
                    .child(status),
            )
            .child(self.more_button(
                SharedString::from(format!("project-entry-more-{row}")),
                entry.host.clone(),
                entry.path.clone(),
                group.clone(),
                &p,
                cx,
            ))
            .child(
                div()
                    .id(("project-entry-pin", row))
                    .w(px(24.))
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(if pinned { p.accent } else { p.muted })
                    .when(!pinned, |d| {
                        d.opacity(0.).group_hover(group, |s| s.opacity(1.))
                    })
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if pinned {
                            "Unpin project"
                        } else {
                            "Pin to Projects"
                        })
                        .build(w, cx)
                    })
                    .child(icon(if pinned { "star.filled" } else { "star" }).size(px(14.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_project_pin(&pin_host, &pin_path, !pinned, cx);
                    })),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let (host, path) = (entry.host.clone(), entry.path.clone());
                    move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.show_project_menu(
                            host.clone(),
                            path.clone(),
                            open_id,
                            event.position,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| match open_id {
                Some(id) => this.show_tab(Some(id), window, cx),
                None => {
                    this.open_from_start(open_host.clone(), vec![open_path.clone()], window, cx)
                }
            }))
            .into_any_element()
    }

    fn agent_accounts_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        let host = self.start.selected_host.clone();
        let ready = self
            .start
            .machines
            .get(&host)
            .and_then(|machine| match &machine.link {
                MachineLink::Ready(daemon) => Some((machine.attempt, daemon.clone())),
                _ => None,
            });
        if let Some((attempt, daemon)) = ready {
            if !self
                .start
                .accounts
                .as_ref()
                .is_some_and(|(device, attached, _)| device == &host && *attached == attempt)
            {
                self.start.accounts = Some((
                    host.clone(),
                    attempt,
                    cx.new(|cx| AccountManager::new(daemon, host.clone(), cx)),
                ));
            }
        } else {
            self.start.accounts = None;
        }
        let mut devices = h_flex().gap(px(6.)).flex_wrap();
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let selected = device == host;
            devices = devices.child(
                Button::new(("accounts-device", ix))
                    .small()
                    .label(host_label(&device))
                    .when(selected, |button| button.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if selected {
                            return;
                        }
                        this.start.selected_host = device.clone();
                        this.start.accounts = None;
                        this.ensure_machine(&device, cx);
                        cx.notify();
                    })),
            );
        }
        let mut page = v_flex().w_full().gap(px(16.))
            .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child("Agent Accounts"))
            .child(div().text_size(px(14.)).text_color(p.muted)
                .child("Keep your own accounts on each device. Switching accounts in a conversation is always your choice."))
            .child(devices);
        if let Some((_, _, accounts)) = &self.start.accounts {
            page = page.child(accounts.clone());
        } else {
            let message = match self.start.machines.get(&host).map(|machine| &machine.link) {
                Some(MachineLink::Failed(error)) => error.clone(),
                Some(MachineLink::Connecting) => "Connecting…".into(),
                _ => "Connect to this device to manage its agent accounts.".into(),
            };
            page = page
                .child(div().text_size(px(13.)).text_color(p.muted).child(message))
                .child(self.welcome_connection_button(host, "accounts-connect", "Connect", cx));
        }
        page.into_any_element()
    }

    /// Manage → Trusted Folders: reads every connected device's list.
    fn show_trusted_folders(&mut self, cx: &mut Context<Self>) {
        self.start.trusted.clear();
        self.show_page(Page::Trusted, cx);
        self.start.problem = None;
        for (host, daemon) in self.start.connected_daemons() {
            let listed = daemon.call::<Vec<TrustedFolder>>(Call::ListTrustedFolders);
            cx.spawn(async move |this, cx| {
                let listed = listed.await.map_err(|err| format!("{err:#}"));
                this.update(cx, |this, cx| {
                    if this.start.page == Page::Trusted {
                        this.start.trusted.insert(host, listed);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Stops trusting a folder on its device; it is asked about when next
    /// opened. What runs there keeps running.
    fn untrust_folder(&mut self, host: String, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.machine_daemon(&host) else {
            return;
        };
        let removed = daemon.call::<()>(Call::UntrustFolder { path: path.clone() });
        cx.spawn(async move |this, cx| {
            let removed = removed.await;
            this.update(cx, |this, cx| {
                match removed {
                    Ok(()) => {
                        if let Some(Ok(list)) = this.start.trusted.get_mut(&host) {
                            list.retain(|folder| folder.path != path);
                        }
                    }
                    Err(err) => {
                        this.start.problem = Some(format!("Could not remove {path}: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The folders trusted on each connected device, with a remove button.
    fn trusted_folders(&self, list_height: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        let trusted = &self.start.trusted;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for host in self.start.hosts() {
            let connected = matches!(
                self.start.machines.get(&host).map(|m| &m.link),
                Some(MachineLink::Ready(_))
            );
            if !connected {
                notes.push(format!("{} is not connected.", host_label(&host)));
                continue;
            }
            match trusted.get(&host) {
                None => notes.push(format!("Reading {}…", host_label(&host))),
                // A daemon from before folder trust has no such list.
                Some(Err(_)) => notes.push(format!(
                    "{} runs an older Sandhill, which trusts every folder until it is updated.",
                    host_label(&host)
                )),
                Some(Ok(folders)) => {
                    for folder in folders {
                        let row = rows.len();
                        let (remove_host, remove_path) = (host.clone(), folder.path.clone());
                        rows.push(
                            h_flex()
                                .id(("trusted-row", row))
                                .w_full()
                                .min_w_0()
                                .px(px(12.))
                                .py(px(8.))
                                .gap(px(10.))
                                .when(row > 0, |d| d.border_t_1().border_color(p.line))
                                .child(
                                    icon(if host == sandhill_link::LOCAL {
                                        "device.laptop"
                                    } else {
                                        "device.server"
                                    })
                                    .size(px(15.))
                                    .text_color(p.muted),
                                )
                                .child(
                                    div()
                                        .w(px(110.))
                                        .flex_none()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(host_label(&host)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .font_family(look::MONO)
                                        .text_size(px(12.5))
                                        .when(!folder.exists, |d| d.text_color(p.muted))
                                        .child(folder.path.clone()),
                                )
                                .when(!folder.exists, |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(12.5))
                                            .text_color(p.warn)
                                            .child("Folder not found"),
                                    )
                                })
                                .child(
                                    icon_button(("trusted-remove", row), "close", &p, false)
                                        .flex_none()
                                        .tooltip(|w, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                "Stop trusting this folder",
                                            )
                                            .build(w, cx)
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.untrust_folder(
                                                remove_host.clone(),
                                                remove_path.clone(),
                                                cx,
                                            );
                                        })),
                                )
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        let empty = rows.is_empty();
        v_flex()
            .w_full()
            .flex_none()
            .gap(px(16.))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .child("Trusted Folders"),
                    )
                    .child(div().text_size(px(14.)).text_color(p.muted).child(
                        "R, Python, the Terminal, jobs, Git and AI agents start only in these folders and the folders inside them. A folder you remove is asked about the next time you open it.",
                    )),
            )
            .child(
                div()
                    .id("trusted-folder-list")
                    .w_full()
                    .max_h(list_height)
                    .flex_none()
                    .rounded(px(look::RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.panel)
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |d| d.child(note("No trusted folders yet.", &p))),
            )
            .children(
                notes
                    .into_iter()
                    .map(|text| div().text_size(px(12.5)).text_color(p.muted).child(text)),
            )
            .into_any_element()
    }

    /// The selected device's status, preferences and grouped recent projects.
    fn host_projects(
        &self,
        host: String,
        list_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = welcome_palette(cx);
        let machine = self.start.machines.get(&host);
        let link = machine.map(|m| &m.link);
        let local = host == sandhill_link::LOCAL;
        let connected = matches!(link, Some(MachineLink::Ready(_)));
        let subtitle = if connected {
            let projects = machine
                .and_then(|m| m.recent.as_ref())
                .and_then(|r| r.as_ref().ok())
                .map(Vec::len);
            let kernels = machine.map_or(0, |m| {
                m.sessions
                    .iter()
                    .filter(|s| s.status != SessionStatus::Ended)
                    .count()
            });
            connected_summary(projects, kernels)
        } else {
            connection_text(link)
        };
        let title = h_flex()
            .w_full()
            .gap(px(12.))
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(24.))
                    .font_weight(FontWeight::BOLD)
                    .child(host_label(&host)),
            )
            .when(!local, |d| {
                d.child(self.welcome_connection_button(
                    host.clone(),
                    "host-connection",
                    if matches!(link, Some(MachineLink::Failed(_))) {
                        "Reconnect"
                    } else {
                        "Connect"
                    },
                    cx,
                ))
            });
        let mut column = v_flex().w_full().flex_none().gap(px(16.)).child(
            v_flex().gap(px(6.)).child(title).child(
                div()
                    .text_size(px(14.))
                    .text_color(if matches!(link, Some(MachineLink::Failed(_))) {
                        p.err
                    } else {
                        p.muted
                    })
                    .child(subtitle),
            ),
        );
        if !local {
            let auto_host = host.clone();
            column = column.child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .justify_between()
                    .child("Connect automatically")
                    .child(
                        Switch::new("host-auto-connect")
                            .small()
                            .color(p.accent)
                            .checked(auto_connect_host(&host, &self.start.auto_connect))
                            .accessibility_label("Connect automatically")
                            .on_change(cx.listener(move |this, on: &bool, _, cx| {
                                this.start.auto_connect.insert(auto_host.clone(), *on);
                                crate::prefs::save_server_auto_connect(&this.start.auto_connect);
                                cx.notify();
                            })),
                    ),
            );
        }
        if !connected {
            let connecting = matches!(link, Some(MachineLink::Connecting));
            // The title's Connect button is the one action; the body only explains.
            return column
                .child(
                    div().py(px(24.)).text_color(p.muted).child(if connecting {
                        "Connecting to this device…"
                    } else {
                        "Connect to see this device’s projects. Kernels and other work on the server keep running while Welcome is disconnected."
                    }),
                )
                .into_any_element();
        }
        let body: AnyElement = match machine.and_then(|m| m.recent.as_ref()) {
            None => note("Loading projects…", &p).into_any_element(),
            Some(Err(err)) => note(err, &p).into_any_element(),
            Some(Ok(recent)) if recent.is_empty() => note(
                "No projects yet. Open a folder with your analysis to start one.",
                &p,
            )
            .into_any_element(),
            Some(Ok(recent)) => {
                let now = chrono::Local::now();
                let ordered = pinned_order(recent, self.start.pinned.get(&host));
                v_flex()
                    .flex_none()
                    .children(ordered.into_iter().enumerate().map(|(row, project)| {
                        let kernels = machine.map_or(0, |m| {
                            m.sessions
                                .iter()
                                .filter(|s| {
                                    s.project.as_deref() == Some(&project.path)
                                        && s.status != SessionStatus::Ended
                                })
                                .count()
                        });
                        let agents = machine.map_or(0, |m| {
                            agent_counts(&m.terminals)
                                .get(&project.path)
                                .copied()
                                .unwrap_or(0)
                        });
                        self.recent_row(
                            &host,
                            row,
                            project,
                            (kernels, agents),
                            row + 1 == recent.len(),
                            now,
                            cx,
                        )
                    }))
                    .into_any_element()
            }
        };
        column
            .child(
                div()
                    .id("start-project-list")
                    .w_full()
                    .max_h(list_height)
                    .flex_none()
                    .rounded(px(look::RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.panel)
                    .overflow_y_scroll()
                    .child(body),
            )
            // Its icon lines up with the project icons above.
            .child(
                h_flex()
                    .w_full()
                    .pl(px(2.))
                    .gap(px(10.))
                    .child(
                        welcome_button("start-open", "folder", "Open Folder…", &p).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.show_start(StartMode::Picking(host.clone()), window, cx);
                            }),
                        ),
                    )
                    .when(local, |d| {
                        d.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(p.muted)
                                .child("or drag folders here from Finder"),
                        )
                    }),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn recent_row(
        &self,
        host: &str,
        row: usize,
        project: &ProjectInfo,
        (kernels, agents): (usize, usize),
        last: bool,
        now: chrono::DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = welcome_palette(cx);
        let group: SharedString = format!("recent-{row}").into();
        let exists = project.exists;
        let pinned = self
            .start
            .pinned
            .get(host)
            .is_some_and(|pins| pins.contains(&project.path));
        let open_host = host.to_string();
        let open_path = project.path.clone();
        let pin_host = host.to_string();
        let pin_path = project.path.clone();
        let forget_host = host.to_string();
        let forget_path = project.path.clone();
        let status: AnyElement = if !exists {
            div()
                .text_color(p.warn)
                .child("Folder not found")
                .into_any_element()
        } else if kernels + agents > 0 {
            h_flex()
                .gap(px(5.))
                .child(look::dot(p.ok))
                .child(work_status(kernels, agents).join(" · "))
                .into_any_element()
        } else {
            div()
                .child(opened_ago(project.opened_ms, now))
                .into_any_element()
        };
        h_flex()
            .id(SharedString::from(format!("recent-{row}")))
            .group(group.clone())
            .relative()
            .flex_none()
            .w_full()
            .min_w_0()
            .px(px(12.))
            .py(px(10.))
            .gap(px(10.))
            .when(exists, |d| d.cursor_pointer())
            .hover(move |d| d.bg(p.ink(6.)))
            .active(|d| d.opacity(0.8))
            .child(
                div()
                    .size(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(!exists, |d| {
                        d.border_1()
                            .border_dashed()
                            .border_color(p.muted)
                            .rounded(px(4.))
                    })
                    .child(if exists {
                        look::picture(&look::project_icon(host, &project.path, &project.name))
                            .size(px(22.))
                            .into_any_element()
                    } else {
                        icon("folder")
                            .size(px(14.))
                            .text_color(p.muted)
                            .into_any_element()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .when(!exists, |d| d.text_color(p.muted))
                            .child(project.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(p.muted)
                            .font_family(look::MONO)
                            .text_size(px(12.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(project.path.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.5))
                    .text_color(p.muted)
                    .child(status),
            )
            .child(
                div()
                    .id(SharedString::from(format!("recent-pin-{row}")))
                    .w(px(24.))
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(if pinned { p.accent } else { p.muted })
                    .cursor_pointer()
                    .when(!pinned, |d| {
                        d.opacity(0.).group_hover(group.clone(), |s| s.opacity(1.))
                    })
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if pinned {
                            "Unpin project"
                        } else {
                            "Pin to Projects"
                        })
                        .build(w, cx)
                    })
                    .child(icon(if pinned { "star.filled" } else { "star" }).size(px(14.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.set_project_pin(&pin_host, &pin_path, !pinned, cx);
                    })),
            )
            .child(self.more_button(
                SharedString::from(format!("recent-more-{row}")),
                host.to_string(),
                project.path.clone(),
                group.clone(),
                &p,
                cx,
            ))
            .child(
                div()
                    .id(SharedString::from(format!("recent-forget-{row}")))
                    .w(px(20.))
                    .h(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.muted)
                    .cursor_pointer()
                    .when(exists, |d| {
                        d.opacity(0.).group_hover(group, |s| s.opacity(1.))
                    })
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Remove from this list")
                            .build(w, cx)
                    })
                    .child(icon("close").size(px(13.)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.ask_forget_project(
                            forget_host.clone(),
                            forget_path.clone(),
                            window,
                            cx,
                        );
                    })),
            )
            .when(!last, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left(px(44.))
                        .right(px(12.))
                        .h(px(1.))
                        .bg(p.line),
                )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let (host, path) = (host.to_string(), project.path.clone());
                    move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        let open = this
                            .projects_in_main(cx)
                            .into_iter()
                            .find(|(_, h, p)| *h == host && *p == path)
                            .map(|(id, ..)| id);
                        this.show_project_menu(
                            host.clone(),
                            path.clone(),
                            open,
                            event.position,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if exists {
                    this.open_from_start(open_host.clone(), vec![open_path.clone()], window, cx);
                }
            }))
            .into_any_element()
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

/// A project as the Projects page lists it.
struct ProjectEntry {
    host: String,
    path: String,
    name: String,
    opened_ms: i64,
    exists: bool,
    /// Its tab in this window, when open.
    open: Option<EntityId>,
    kernels: usize,
    /// Running conversations.
    agents: usize,
    pinned: bool,
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

fn pinned_order<'a>(
    recent: &'a [ProjectInfo],
    pins: Option<&std::collections::BTreeSet<String>>,
) -> Vec<&'a ProjectInfo> {
    let mut projects: Vec<_> = recent.iter().collect();
    projects.sort_by_key(|project| {
        (
            !pins.is_some_and(|pins| pins.contains(&project.path)),
            std::cmp::Reverse(project.opened_ms),
        )
    });
    projects
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
            include_bytes!("../assets/brand/icon.png").to_vec(),
        ))
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::{
        MachineLink, auto_connect_host, connected_summary, connection_text, opened_ago,
        ordered_hosts, pinned_order,
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
    fn project_recency_is_sorted_within_pin_groups() {
        let projects: Vec<_> = [("old", 1), ("new", 3), ("pinned", 2)]
            .into_iter()
            .map(|(name, opened_ms)| sandhill_proto::ProjectInfo {
                path: format!("/{name}"),
                name: name.into(),
                opened_ms,
                exists: true,
                state: serde_json::Value::Null,
                trusted: true,
            })
            .collect();
        let pins = ["/pinned".into()].into();
        let names: Vec<_> = pinned_order(&projects, Some(&pins))
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["pinned", "new", "old"]);
    }

    #[test]
    fn pins_come_first_and_preserve_recency_within_each_group() {
        use sandhill_proto::ProjectInfo;
        let projects: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|name| ProjectInfo {
                path: format!("/{name}"),
                name: name.into(),
                opened_ms: 0,
                exists: true,
                state: serde_json::Value::Null,
                trusted: true,
            })
            .collect();
        let pins = ["/b".into(), "/d".into()].into();
        let names = |pins| {
            pinned_order(&projects, pins)
                .into_iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(Some(&pins)), ["b", "d", "a", "c"]);
        assert_eq!(names(None), ["a", "b", "c", "d"]);
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
