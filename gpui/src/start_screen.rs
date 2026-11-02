//! The Start screen, as the Tauri app's `StartScreen.tsx` draws it: a card
//! for each server and this Mac with its recent projects, Open Folder… (a
//! folder picker that browses any of them through its sandhilld) and
//! Connect to Server…. A project opens in its machine's tab.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{
    Call, DirEntry, EntryKind, PingReply, ProjectInfo, SessionInfo, SessionStatus,
};

use crate::app_window::AppWindow;
use crate::daemon::Daemon;
use crate::files::{base_name, join_path, parent_of};
use crate::look::{self, Palette, capsule, icon, icon_button, pane, picture, raised, wall};

/// What the Start screen opens with.
pub(crate) enum StartMode {
    /// Connect to Server…'s form.
    Adding,
    /// The folder picker, on a host.
    Picking(String),
}

pub(crate) enum MachineLink {
    Connecting,
    Failed(String),
    Ready(Rc<Daemon>),
}

/// A machine's card: its connection (the Start screen's own, kept while
/// the window is open), recent projects and running kernels.
pub(crate) struct Machine {
    link: MachineLink,
    home: Option<String>,
    recent: Option<Result<Vec<ProjectInfo>, String>>,
    sessions: Vec<SessionInfo>,
}

/// The folder picker: a folder on a host and the folders in it.
struct Picker {
    host: String,
    path: Option<String>,
    folders: Option<Result<Vec<String>, String>>,
}

pub(crate) struct Start {
    servers: Vec<String>,
    adding: bool,
    picker: Option<Picker>,
    machines: HashMap<String, Machine>,
    server_input: Entity<InputState>,
    folder_input: Entity<InputState>,
    problem: Option<String>,
    opening: bool,
    _subscriptions: Vec<Subscription>,
}

impl Start {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppWindow>) -> Self {
        let server_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("lab, or you@server.example.org"));
        let folder_input = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &server_input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let host = input.read(cx).value().trim().to_string();
                        this.add_server(host, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &folder_input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let typed = input.read(cx).value().to_string();
                        this.go_to_folder(typed, window, cx);
                    }
                },
            ),
        ];
        Start {
            servers: crate::prefs::servers(),
            adding: false,
            picker: None,
            machines: HashMap::new(),
            server_input,
            folder_input,
            problem: None,
            opening: false,
            _subscriptions: subscriptions,
        }
    }

    /// The servers, then this Mac.
    fn hosts(&self) -> Vec<String> {
        let mut hosts = self.servers.clone();
        hosts.push(sandhill_link::LOCAL.into());
        hosts
    }

    /// Where Open Folder… starts: the tab shown, the host used last, or the
    /// first server.
    pub(crate) fn last_host(&self, current: &Option<String>) -> String {
        let hosts = self.hosts();
        current
            .clone()
            .or_else(|| crate::prefs::start_host().filter(|h| hosts.contains(h)))
            .unwrap_or_else(|| hosts[0].clone())
    }

    pub(crate) fn set_mode(&mut self, mode: StartMode, window: &mut Window, cx: &mut App) {
        self.problem = None;
        match mode {
            StartMode::Adding => {
                self.picker = None;
                self.adding = true;
                self.server_input.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus(window, cx);
                });
            }
            StartMode::Picking(host) => {
                self.adding = false;
                self.picker = Some(Picker {
                    host,
                    path: None,
                    folders: None,
                });
            }
        }
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
    cx.spawn(async move |this, cx| {
        let (ping, recent, sessions) = (ping.await, recent.await, sessions.await);
        this.update(cx, |this, cx| {
            if let Some(machine) = this.start.machines.get_mut(&host) {
                if let Ok(ping) = ping {
                    machine.home = Some(ping.home);
                }
                machine.recent = Some(recent.map_err(|err| format!("{err:#}")));
                machine.sessions = sessions.unwrap_or_default();
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

fn host_label(host: &str) -> String {
    if host != sandhill_link::LOCAL {
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
    /// Connects to a machine for its card, once.
    fn ensure_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        if self.start.machines.contains_key(host) {
            return;
        }
        let target = match Target::parse(host) {
            Ok(target) => target,
            Err(err) => {
                self.start.machines.insert(
                    host.into(),
                    Machine {
                        link: MachineLink::Failed(err.to_string()),
                        home: None,
                        recent: None,
                        sessions: Vec::new(),
                    },
                );
                return;
            }
        };
        self.start.machines.insert(
            host.into(),
            Machine {
                link: MachineLink::Connecting,
                home: None,
                recent: None,
                sessions: Vec::new(),
            },
        );
        let connect = Daemon::connect(Arc::clone(&self.rt), target);
        let host = host.to_string();
        cx.spawn(async move |this, cx| {
            let result = connect.await;
            this.update(cx, |this, cx| {
                let Some(machine) = this.start.machines.get_mut(&host) else {
                    return;
                };
                // Only replies are used; the connection's events aren't.
                machine.link = match result {
                    Ok((daemon, _streams)) => MachineLink::Ready(Rc::new(daemon)),
                    Err(err) => MachineLink::Failed(format!("{err:#}")),
                };
                fetch(&mut this.start.machines, host, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn machine_daemon(&self, host: &str) -> Option<Rc<Daemon>> {
        match &self.start.machines.get(host)?.link {
            MachineLink::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    fn add_server(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = host.trim().to_string();
        if host.is_empty() {
            return;
        }
        let checked = match Target::parse(&host) {
            Ok(Target::Local) => Err("\"local\" is this Mac".to_string()),
            Ok(_) => Ok(()),
            Err(err) => Err(err.to_string()),
        };
        if let Err(err) = checked {
            self.start.problem = Some(err);
            cx.notify();
            return;
        }
        let mut servers = crate::prefs::servers();
        if !servers.contains(&host) {
            servers.push(host);
        }
        match crate::prefs::save_servers(&servers) {
            Ok(()) => {
                self.start.servers = servers;
                self.start.adding = false;
                self.start.problem = None;
                window.blur(cx);
            }
            Err(err) => self.start.problem = Some(err),
        }
        cx.notify();
    }

    fn remove_server(&mut self, host: &str, cx: &mut Context<Self>) {
        let mut servers = crate::prefs::servers();
        servers.retain(|h| h != host);
        match crate::prefs::save_servers(&servers) {
            Ok(()) => {
                self.start.servers = servers;
                self.start.machines.remove(host);
                if self.start.picker.as_ref().is_some_and(|p| p.host == host) {
                    self.start.picker = None;
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
        let forgotten = daemon.call::<serde_json::Value>(Call::ForgetProject { path });
        cx.spawn(async move |this, cx| {
            let forgotten = forgotten.await;
            this.update(cx, |this, cx| {
                if let Err(err) = forgotten {
                    this.start.problem = Some(format!("{err:#}"));
                }
                fetch(&mut this.start.machines, host, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a folder as a project in its machine's tab.
    fn open_from_start(
        &mut self,
        host: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.machine_daemon(&host) else {
            return;
        };
        if self.start.opening {
            return;
        }
        self.start.opening = true;
        let opened = daemon.call::<ProjectInfo>(Call::OpenProject { path: path.clone() });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opened.await;
            this.update_in(cx, |this, window, cx| {
                this.start.opening = false;
                match opened {
                    Ok(project) if project.exists => {
                        crate::prefs::save_start_host(&host);
                        this.start.picker = None;
                        this.start.problem = None;
                        this.open_place(host, project.path, window, cx);
                    }
                    Ok(project) => {
                        this.start.problem = Some(format!("{} doesn't exist", project.path))
                    }
                    Err(err) => {
                        this.start.problem = Some(format!("Could not open {path}: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a folder in the picker.
    fn go_to_folder(&mut self, typed: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = &self.start.picker else {
            return;
        };
        let host = picker.host.clone();
        let home = self
            .start
            .machines
            .get(&host)
            .and_then(|m| m.home.clone())
            .unwrap_or_default();
        let mut path = typed.trim().to_string();
        if let Some(rest) = path.strip_prefix('~')
            && (rest.is_empty() || rest.starts_with('/'))
        {
            path = format!("{home}{rest}");
        }
        let trimmed = path.trim_end_matches('/');
        let path = if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        };
        let Some(daemon) = self.machine_daemon(&host) else {
            return;
        };
        if let Some(picker) = &mut self.start.picker {
            picker.path = Some(path.clone());
            picker.folders = None;
        }
        self.start
            .folder_input
            .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: path.clone(),
            sizes: false,
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                let Some(picker) = &mut this.start.picker else {
                    return;
                };
                if picker.host != host || picker.path.as_deref() != Some(&path) {
                    return;
                }
                picker.folders = Some(
                    listed
                        .map(|entries| {
                            let mut names: Vec<String> = entries
                                .into_iter()
                                .filter(|e| e.kind == EntryKind::Dir && !e.name.starts_with('.'))
                                .map(|e| e.name)
                                .collect();
                            names.sort_by_key(|n| n.to_lowercase());
                            names
                        })
                        .map_err(|err| format!("{err:#}")),
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_start(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        for host in self.start.hosts() {
            self.ensure_machine(&host, cx);
        }
        // The picker starts in the home folder, once it is known.
        if let Some(picker) = &self.start.picker
            && picker.path.is_none()
            && let Some(home) = self
                .start
                .machines
                .get(&picker.host)
                .filter(|m| matches!(m.link, MachineLink::Ready(_)))
                .and_then(|m| m.home.clone())
        {
            self.go_to_folder(home, window, cx);
        }
        let primary = h_flex()
            .id("start-open")
            .gap(px(6.))
            .px(px(12.))
            .py(px(5.))
            .rounded(px(look::RADIUS))
            .bg(p.accent)
            .text_color(p.accent_text)
            .cursor_pointer()
            .child(icon("folder").size(px(14.)))
            .child("Open Folder…")
            .on_click(cx.listener(|this, _, window, cx| {
                if this.start.picker.is_some() {
                    this.start.picker = None;
                    cx.notify();
                } else {
                    let host = this.start.last_host(&None);
                    this.show_start(StartMode::Picking(host), window, cx);
                }
            }));
        let hover = p.bg;
        let connect = h_flex()
            .id("start-connect")
            .gap(px(6.))
            .px(px(12.))
            .py(px(5.))
            .rounded(px(look::RADIUS))
            .bg(if self.start.adding { p.bg } else { p.glass })
            .border_1()
            .border_color(p.glass_edge)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(icon("device.server").size(px(14.)))
            .child("Connect to Server…")
            .on_click(cx.listener(|this, _, window, cx| {
                if this.start.adding {
                    this.start.adding = false;
                    cx.notify();
                } else {
                    this.show_start(StartMode::Adding, window, cx);
                }
            }));
        let cards: Vec<AnyElement> = self
            .start
            .hosts()
            .into_iter()
            .enumerate()
            .map(|(ix, host)| self.host_card(ix, host, cx))
            .collect();
        let build = format!(
            "Sandhill {}{}",
            env!("CARGO_PKG_VERSION"),
            option_env!("SANDHILL_COMMIT")
                .map(|c| format!(", build {c}"))
                .unwrap_or_default()
        );
        let column = v_flex()
            .w_full()
            .max_w(px(760.))
            .gap(px(22.))
            .pt(px(12.))
            .pb(px(40.))
            .child(
                h_flex()
                    .gap(px(14.))
                    .child(
                        img(brand_icon())
                            .size(px(78.))
                            .m(px(-7.))
                            .flex_none(),
                    )
                    .child(
                        v_flex()
                            .child(
                                div()
                                    .text_size(px(28.))
                                    .font_weight(FontWeight::BOLD)
                                    .line_height(rems(1.5))
                                    .child("Sandhill"),
                            )
                            .child(div().text_color(p.muted).child(
                                "R and Python notebooks that keep running when you close the lid.",
                            )),
                    ),
            )
            .child(h_flex().flex_wrap().gap(px(8.)).child(primary).child(connect))
            .when_some(self.start.problem.clone(), |d, problem| {
                d.child(div().text_color(p.err).child(problem))
            })
            .when(self.start.adding, |d| d.child(self.add_server_card(cx)))
            .children(self.picker_card(cx))
            .child(h_flex().flex_wrap().items_start().gap(px(16.)).children(cards))
            .child(
                div()
                    .text_color(p.faint)
                    .child("Kernels keep running after you close a project. Reopening it reattaches to them and restores your tabs.")
                    .child(div().child(build)),
            );
        wall(&p)
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text)
            .text_size(px(look::CHROME))
            .line_height(rems(1.4))
            .child(self.start_title_bar(window, cx))
            .child(
                div()
                    .id("start")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(16.))
                    .child(
                        v_flex()
                            .min_h_full()
                            .items_center()
                            .justify_center()
                            .child(column),
                    ),
            )
            .into_any_element()
    }

    /// The Start screen's title bar: the window can be dragged by it; the
    /// machine tabs are drawn over it by the window.
    fn start_title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let dim = |id: &'static str, name: &str| icon_button(id, name, &p, true).opacity(0.4);
        div()
            .relative()
            .flex_none()
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
                    .top(px(6.))
                    .left(px(if window.is_fullscreen() {
                        6.
                    } else {
                        crate::view::TITLE_BAR_LEFT
                    }))
                    .child(
                        capsule(&p, 25.)
                            .child(dim("s-files", "sidebar.left"))
                            .child(dim("s-agents", "chat"))
                            .child(dim("s-right", "sidebar.right")),
                    ),
            )
    }

    fn add_server_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let empty = self.start.server_input.read(cx).value().trim().is_empty();
        pane(&p)
            .p(px(10.))
            .gap(px(8.))
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(div().flex_1().child(Input::new(&self.start.server_input).small()))
                    .child(
                        Button::new("server-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.adding = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("server-add")
                            .small()
                            .primary()
                            .label("Add")
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let host = this.start.server_input.read(cx).value().to_string();
                                this.add_server(host, window, cx)
                            })),
                    ),
            )
            .child(div().text_color(p.muted).child(
                "Sandhill connects with ssh, using your ~/.ssh/config and keys, so a name from there works. The first time, it installs sandhilld in ~/.local/bin on the server.",
            ))
    }

    fn picker_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.start.picker.as_ref()?;
        let p = *Palette::get(cx);
        // The hosts, as a row of pills in place of the Tauri app's select.
        let mut hosts = capsule(&p, 25.);
        for (ix, host) in self.start.hosts().into_iter().enumerate() {
            let picked = host == picker.host;
            let hover = p.ink(6.);
            let pill = h_flex()
                .id(("pick-host", ix))
                .h(px(21.))
                .px(px(10.))
                .rounded_full()
                .cursor_pointer()
                .font_weight(FontWeight::NORMAL)
                .text_color(p.muted)
                .when(!picked, |d| d.hover(move |d| d.bg(hover)))
                .child(host_label(&host))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.show_start(StartMode::Picking(host.clone()), window, cx)
                }));
            hosts = hosts.child(if picked { raised(pill, &p) } else { pill });
        }
        let link = self.start.machines.get(&picker.host).map(|m| &m.link);
        let body: AnyElement = match link {
            None | Some(MachineLink::Connecting) => note("Connecting…", &p).into_any_element(),
            Some(MachineLink::Failed(err)) => {
                note(&format!("Not connected: {err}"), &p).into_any_element()
            }
            Some(MachineLink::Ready(_)) => {
                let path = picker.path.clone().unwrap_or_default();
                let folders = picker.folders.clone();
                let hover = p.ink(7.);
                let rows: Vec<AnyElement> = match &folders {
                    None => vec![note("Loading…", &p).into_any_element()],
                    Some(Err(err)) => vec![note(err, &p).into_any_element()],
                    Some(Ok(names)) if names.is_empty() => {
                        vec![note("No folders here", &p).into_any_element()]
                    }
                    Some(Ok(names)) => names
                        .iter()
                        .enumerate()
                        .map(|(ix, name)| {
                            let child = join_path(&path, name);
                            h_flex()
                                .id(("pick-folder", ix))
                                .h(px(26.))
                                .px(px(8.))
                                .gap(px(6.))
                                .flex_none()
                                .rounded(px(look::RADIUS))
                                .cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .child(picture(&look::folder_icon(name)))
                                .child(name.clone())
                                .on_click(cx.listener(
                                    move |this, event: &ClickEvent, window, cx| {
                                        if event.click_count() == 1 {
                                            this.go_to_folder(child.clone(), window, cx);
                                        }
                                    },
                                ))
                                .into_any_element()
                        })
                        .collect(),
                };
                let can_open = matches!(folders, Some(Ok(_))) && !path.is_empty();
                let name = base_name(&path).to_string();
                v_flex()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .gap(px(4.))
                            .child(
                                icon_button("pick-parent", "arrow.up", &p, false)
                                    .tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new("Parent folder")
                                            .build(w, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let path = this
                                            .start
                                            .picker
                                            .as_ref()
                                            .and_then(|p| p.path.clone())
                                            .unwrap_or_default();
                                        this.go_to_folder(parent_of(&path), window, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(look::MONO)
                                    .child(Input::new(&self.start.folder_input).small()),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("pick-list")
                            .h(px(220.))
                            .p(px(4.))
                            .overflow_y_scroll()
                            .bg(p.bg)
                            .border_1()
                            .border_color(p.line)
                            .rounded(px(look::RADIUS))
                            .children(rows)
                            // A double click: the first click went into a
                            // folder (the list may be loading it), so the
                            // folder shown opens.
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                if event.click_count() < 2 {
                                    return;
                                }
                                let Some(picker) = &this.start.picker else {
                                    return;
                                };
                                let host = picker.host.clone();
                                let path = picker.path.clone().unwrap_or_default();
                                this.open_from_start(host, path, window, cx);
                            })),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(Button::new("pick-cancel").small().label("Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.start.picker = None;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                Button::new("pick-open")
                                    .small()
                                    .primary()
                                    .label(format!("Open {name}"))
                                    .disabled(!can_open || self.start.opening)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let Some(picker) = &this.start.picker else {
                                            return;
                                        };
                                        let host = picker.host.clone();
                                        let path = picker.path.clone().unwrap_or_default();
                                        this.open_from_start(host, path, window, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        };
        Some(
            pane(&p)
                .p(px(10.))
                .gap(px(8.))
                .child(
                    h_flex()
                        .gap(px(8.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Open a folder on")
                        .child(hosts),
                )
                .child(body)
                .into_any_element(),
        )
    }

    /// One machine: whether it's reachable, and its recent projects.
    fn host_card(&self, ix: usize, host: String, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let machine = self.start.machines.get(&host);
        let (state, color) = match machine.map(|m| &m.link) {
            Some(MachineLink::Ready(_)) => ("connected", p.ok),
            Some(MachineLink::Failed(_)) => ("not connected", p.err),
            _ => ("connecting", p.busy),
        };
        let local = host == sandhill_link::LOCAL;
        let group: SharedString = format!("host-card-{ix}").into();
        let hover = p.ink(8.);
        let text = p.text;
        let header = h_flex()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(8.))
            .py(px(9.))
            .border_b_1()
            .border_color(p.line)
            .font_weight(FontWeight::SEMIBOLD)
            .child(
                icon(if local {
                    "device.laptop"
                } else {
                    "device.server"
                })
                .size(px(14.)),
            )
            .child(
                div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .min_w_0()
                    .child(host_label(&host)),
            )
            .child(
                h_flex()
                    .ml_auto()
                    .gap(px(5.))
                    .flex_none()
                    .text_color(p.faint)
                    .font_weight(FontWeight::NORMAL)
                    .child(look::dot(color))
                    .child(state),
            )
            .when(!local, |d| {
                let host = host.clone();
                d.child(
                    div()
                        .id(("host-remove", ix))
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_color(p.faint)
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .hover(move |d| d.bg(hover).text_color(text))
                        .cursor_pointer()
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Remove from this list")
                                .build(w, cx)
                        })
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_server(&host, cx))),
                )
            });
        let body: AnyElement = match machine {
            None => note("Connecting…", &p).into_any_element(),
            Some(Machine {
                link: MachineLink::Connecting,
                ..
            }) => note("Connecting…", &p).into_any_element(),
            Some(Machine {
                link: MachineLink::Failed(err),
                ..
            }) => note(&format!("Not connected: {err}"), &p).into_any_element(),
            Some(Machine { recent: None, .. }) => note("Loading…", &p).into_any_element(),
            Some(Machine {
                recent: Some(Err(err)),
                ..
            }) => note(err, &p).into_any_element(),
            Some(Machine {
                recent: Some(Ok(recent)),
                ..
            }) if recent.is_empty() => {
                let host = host.clone();
                h_flex()
                    .flex_wrap()
                    .px(px(12.))
                    .py(px(10.))
                    .gap(px(4.))
                    .text_color(p.muted)
                    .child("No projects yet.")
                    .child(
                        div()
                            .id(("host-pick", ix))
                            .text_color(p.accent)
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .child("Open a folder")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.show_start(StartMode::Picking(host.clone()), window, cx)
                            })),
                    )
                    .child("with your analysis to start one.")
                    .into_any_element()
            }
            Some(Machine {
                recent: Some(Ok(recent)),
                sessions,
                ..
            }) => {
                let now = chrono::Local::now();
                let count = recent.len();
                v_flex()
                    .children(recent.iter().enumerate().map(|(row, project)| {
                        let kernels = sessions
                            .iter()
                            .filter(|s| {
                                s.project.as_deref() == Some(&project.path)
                                    && s.status != SessionStatus::Ended
                            })
                            .count();
                        self.recent_row(
                            &host,
                            (ix, row),
                            project,
                            kernels,
                            row + 1 == count,
                            now,
                            cx,
                        )
                    }))
                    .into_any_element()
            }
        };
        pane(&p)
            .group(group)
            // Two to a row in the 760px column, as the Tauri app's grid.
            .w(px(372.))
            .child(header)
            .child(body)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn recent_row(
        &self,
        host: &str,
        (card, row): (usize, usize),
        project: &ProjectInfo,
        kernels: usize,
        last: bool,
        now: chrono::DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let hover = p.ink(6.);
        let group: SharedString = format!("recent-{card}-{row}").into();
        let when: AnyElement = if !project.exists {
            div().child("folder not found").into_any_element()
        } else if kernels > 0 {
            h_flex()
                .gap(px(5.))
                .child(look::dot(p.ok))
                .child(if kernels == 1 {
                    "1 kernel".to_string()
                } else {
                    format!("{kernels} kernels")
                })
                .into_any_element()
        } else {
            div()
                .child(opened_ago(project.opened_ms, now))
                .into_any_element()
        };
        let open_host = host.to_string();
        let open_path = project.path.clone();
        let forget_host = host.to_string();
        let forget_path = project.path.clone();
        let exists = project.exists;
        h_flex()
            .id(SharedString::from(format!("recent-{card}-{row}")))
            .group(group.clone())
            .items_stretch()
            .when(!last, |d| d.border_b_1().border_color(p.line.opacity(0.6)))
            .hover(move |d| d.bg(hover))
            .child(
                h_flex()
                    .id(SharedString::from(format!("recent-open-{card}-{row}")))
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.))
                    .pl(px(12.))
                    .pr(px(4.))
                    .py(px(8.))
                    .when(exists, |d| d.cursor_pointer())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .when(!exists, |d| d.text_color(p.faint).line_through())
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_color(p.faint)
                                    .font_family(look::MONO)
                                    .text_size(px(look::SMALL))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(project.path.clone()),
                            ),
                    )
                    .child(div().flex_none().text_color(p.muted).child(when))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if exists {
                            this.open_from_start(open_host.clone(), open_path.clone(), window, cx)
                        }
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("recent-forget-{card}-{row}")))
                    .w(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.faint)
                    .cursor_pointer()
                    .when(exists, |d| {
                        d.opacity(0.).group_hover(group, |s| s.opacity(1.))
                    })
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Remove from this list")
                            .build(w, cx)
                    })
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.forget_project(forget_host.clone(), forget_path.clone(), cx)
                    })),
            )
            .into_any_element()
    }
}

/// A muted line in a card.
fn note(text: &str, p: &Palette) -> Div {
    div()
        .px(px(12.))
        .py(px(10.))
        .text_color(p.muted)
        .child(text.to_string())
}

/// The app's icon, for the Start screen's header.
fn brand_icon() -> Arc<Image> {
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
    use super::opened_ago;
    use chrono::TimeZone;

    #[test]
    fn says_when_projects_were_opened() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 2, 15, 0, 0)
            .unwrap();
        let ms = |d: u32, h: u32| {
            chrono::Local
                .with_ymd_and_hms(2026, 11, d, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(opened_ago(ms(2, 1), now), "opened today");
        assert_eq!(opened_ago(ms(1, 23), now), "opened yesterday");
        assert_eq!(
            opened_ago(ms(1, 23) - 2 * 86_400_000, now),
            "opened 3 days ago"
        );
        assert!(opened_ago(ms(1, 0) - 20 * 86_400_000, now).starts_with("opened Oct"));
    }
}
