//! The machine a tab is connected to, as the Tauri app's left pane shows it
//! (`LeftPane.tsx`, `model/machine.ts`): the project's name, which switches
//! to another project on the machine; the project's running kernels, each
//! with its memory and a Shut down that asks first; and the machine's CPU,
//! memory, GPUs and disk. Polled every few seconds while connected.

use std::time::Duration;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, KernelspecInfo, ProjectInfo, SessionInfo, SessionStatus, SystemStats};

use crate::app_window::short_path;
use crate::look::{self, Palette, icon, picture};
use crate::start_screen::StartMode;
use crate::workbench::{Link, Workbench};

const POLL: Duration = Duration::from_secs(5);
/// What `asking` holds while "Shut down the unused sessions" asks.
const UNUSED: &str = "unused";

#[derive(Default)]
pub(crate) struct MachineUi {
    pub stats: Option<SystemStats>,
    /// Every session the daemon knows, as last listed.
    pub sessions: Vec<SessionInfo>,
    /// The project menu is open, with the machine's projects once read.
    pub project_menu: Option<Vec<ProjectInfo>>,
    /// The kernel row (a session id, or UNUSED) asking to shut down.
    pub asking: Option<String>,
    /// The resource whose details show above the status bar.
    pub resource: Option<&'static str>,
}

/// "2.1 GB", "412 MB", "56 KB".
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit >= 3 && value < 10. {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

fn percent_of(part: u64, whole: u64) -> u32 {
    if whole == 0 {
        0
    } else {
        (part as f64 / whole as f64 * 100.).round() as u32
    }
}

/// "R", "Python", or the language as the kernel names it.
fn language_label(language: &str) -> String {
    match language.to_lowercase().as_str() {
        "r" => "R".into(),
        "python" | "python3" => "Python".into(),
        _ => {
            let mut chars = language.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

/// "4.4.1", "3.12": only the number; Python's minor version, as people say it.
fn language_version(session: &SessionInfo) -> String {
    let said = session.language_version.clone().unwrap_or_default();
    let number = said
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|w| w.contains('.') && w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(|w| w.trim_end_matches('.').to_string())
        .unwrap_or(said);
    if language_label(&session.language) == "Python" {
        number.split('.').take(2).collect::<Vec<_>>().join(".")
    } else {
        number
    }
}

fn with_env(text: String, session: &SessionInfo, specs: &[KernelspecInfo]) -> String {
    match specs
        .iter()
        .find(|k| k.name == session.kernel)
        .and_then(|k| k.env.clone())
    {
        Some(env) if !env.is_empty() => format!("{text} ({env})"),
        _ => text,
    }
}

/// "R 4.4.1", "Python 3.12 (omicverse)".
pub(crate) fn session_label(session: &SessionInfo, specs: &[KernelspecInfo]) -> String {
    let name = language_label(&session.language);
    let version = language_version(session);
    let text = if version.is_empty() {
        name
    } else {
        format!("{name} {version}")
    };
    with_env(text, session, specs)
}

/// "3.12 (omicverse)": the row's name beside the language's icon.
fn version_label(session: &SessionInfo, specs: &[KernelspecInfo]) -> String {
    let version = language_version(session);
    let text = if version.is_empty() {
        language_label(&session.language)
    } else {
        version
    };
    with_env(text, session, specs)
}

/// One of the machine's resources, as a row with a bar.
pub(crate) struct Resource {
    pub key: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub name: String,
    pub percent: u32,
    pub main: String,
    pub detail: String,
}

/// What the machine's CPU, memory, GPUs and disk are doing.
pub(crate) fn resource_rows(
    stats: &SystemStats,
    sessions: &[SessionInfo],
    specs: &[KernelspecInfo],
) -> Vec<Resource> {
    let mut rows = Vec::new();
    let cpu = &stats.cpu;
    rows.push(Resource {
        key: "cpu",
        icon: "cpu",
        label: "CPU",
        name: "CPU".into(),
        percent: cpu.percent.round().clamp(0., 100.) as u32,
        main: if cpu.model.is_empty() {
            format!("{} cores", cpu.cores)
        } else {
            format!("{} · {} cores", cpu.model, cpu.cores)
        },
        detail: format!("Load {:.1}", cpu.load),
    });
    let kernels: Vec<String> = stats
        .sessions
        .iter()
        .filter_map(|m| {
            let s = sessions.iter().find(|s| s.id == m.session)?;
            Some(format!(
                "{} {}",
                session_label(s, specs),
                format_bytes(m.rss)
            ))
        })
        .collect();
    rows.push(Resource {
        key: "mem",
        icon: "memory",
        label: "MEM",
        name: "Memory".into(),
        percent: percent_of(stats.memory.used, stats.memory.total),
        main: format!(
            "{} of {} used",
            format_bytes(stats.memory.used),
            format_bytes(stats.memory.total)
        ),
        detail: if kernels.is_empty() {
            "No kernels running".into()
        } else {
            format!("Kernels: {}", kernels.join(" · "))
        },
    });
    if let Some(busiest) = stats
        .gpus
        .iter()
        .max_by(|a, b| a.percent.total_cmp(&b.percent))
    {
        let n = stats.gpus.len();
        rows.push(Resource {
            key: "gpu",
            icon: "gpu",
            label: "GPU",
            name: if n > 1 {
                format!("GPU ({n})")
            } else {
                "GPU".into()
            },
            percent: busiest.percent.round().clamp(0., 100.) as u32,
            main: if n > 1 {
                format!("{n} × {}", busiest.name)
            } else {
                busiest.name.clone()
            },
            detail: stats
                .gpus
                .iter()
                .map(|g| {
                    format!(
                        "{}% busy · {} of {}",
                        g.percent.round(),
                        format_bytes(g.memory_used),
                        format_bytes(g.memory_total)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    if let Some(disk) = &stats.disk {
        rows.push(Resource {
            key: "disk",
            icon: "disk",
            label: "Disk",
            name: "Disk".into(),
            percent: percent_of(disk.used, disk.used + disk.available),
            main: format!(
                "{} of {} used",
                format_bytes(disk.used),
                format_bytes(disk.total)
            ),
            detail: format!("{} free for {}", format_bytes(disk.available), disk.path),
        });
    }
    rows
}

/// "this Mac", or the server's name.
fn host_name(host: &str) -> String {
    if host == sandhill_link::LOCAL {
        if cfg!(target_os = "macos") {
            "this Mac".into()
        } else {
            "this computer".into()
        }
    } else {
        host.into()
    }
}

impl Workbench {
    /// Asks the daemon how its machine is doing, now and every few seconds
    /// for as long as this connection lasts.
    pub(crate) fn watch_machine(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let link = this
                    .update(cx, |this, _| {
                        if this.generation != generation {
                            return None;
                        }
                        Some((this.daemon()?, this.project.clone()))
                    })
                    .ok()
                    .flatten();
                let Some((daemon, path)) = link else {
                    break;
                };
                let stats = daemon.call::<SystemStats>(Call::SystemStats { path });
                let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
                drop(daemon);
                let (stats, sessions) = (stats.await, sessions.await);
                let alive = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    // An older daemon can't say; the section stays hidden.
                    if let Ok(stats) = stats {
                        this.machine.stats = Some(stats);
                    }
                    if let Ok(sessions) = sessions {
                        this.machine.sessions = sessions;
                    }
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(POLL).await;
            }
        })
        .detach();
    }

    /// The project's kernels that are still running.
    pub(crate) fn project_sessions(&self) -> Vec<&SessionInfo> {
        self.machine
            .sessions
            .iter()
            .filter(|s| s.status != SessionStatus::Ended && s.project == self.project)
            .collect()
    }

    /// The open documents using a session (and the Console's own kernel).
    pub(crate) fn used_by(&self, session: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .open
            .iter()
            .chain(&self.others)
            .filter(|o| o.session.id() == Some(session))
            .map(|o| crate::files::base_name(&o.notebook.path).to_string())
            .collect();
        if self.loose.id() == Some(session) {
            names.push("Console".into());
        }
        names
    }

    /// Stops sessions for good; documents using them see their kernel stop.
    fn shut_down(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.machine.asking = None;
        let Some(daemon) = self.daemon() else {
            return;
        };
        for id in ids {
            self.machine.sessions.retain(|s| s.id != id);
            daemon.fire(Call::Shutdown { session: id });
        }
        cx.notify();
    }

    fn toggle_project_menu(&mut self, cx: &mut Context<Self>) {
        if self.machine.project_menu.take().is_some() {
            cx.notify();
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        self.machine.project_menu = Some(Vec::new());
        let recent = daemon.call::<Vec<ProjectInfo>>(Call::ListRecentProjects);
        cx.spawn(async move |this, cx| {
            let recent = recent.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                if let Some(menu) = &mut this.machine.project_menu {
                    *menu = recent;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The left pane's head: the project's name, a button that lists the
    /// machine's projects.
    pub(crate) fn render_project_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let name = self
            .project
            .as_deref()
            .map(|p| crate::files::base_name(p).to_string())
            .unwrap_or_else(|| "No project".into());
        let hover = p.ink(7.);
        let open = self.machine.project_menu.is_some();
        h_flex()
            .id("projbtn")
            .min_w_0()
            .h(px(24.))
            .ml(px(-6.))
            .px(px(6.))
            .gap(px(6.))
            .rounded(px(look::RADIUS - 4.))
            .cursor_pointer()
            .when(open, |d| d.bg(hover))
            .hover(move |d| d.bg(hover))
            .child(icon("folder").size(px(15.)).text_color(p.muted))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .child(icon("chevron.down").size(px(11.)).text_color(p.faint))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.toggle_project_menu(cx)))
    }

    /// The project menu: the open project, then the machine's others most
    /// recent first; picking one switches this tab to it.
    pub(crate) fn render_project_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let recent = self.machine.project_menu.as_ref()?;
        let p = *Palette::get(cx);
        let host = self.host();
        let home = self.about.as_ref().map(|a| a.home.clone());
        let current = self.project.clone().unwrap_or_default();
        let mut projects: Vec<ProjectInfo> = recent
            .iter()
            .filter(|r| r.exists && r.path != current)
            .cloned()
            .collect();
        if !current.is_empty() {
            projects.insert(
                0,
                ProjectInfo {
                    name: crate::files::base_name(&current).to_string(),
                    path: current.clone(),
                    opened_ms: 0,
                    exists: true,
                    state: Default::default(),
                },
            );
        }
        let live = |path: &str| {
            self.machine
                .sessions
                .iter()
                .filter(|s| s.status != SessionStatus::Ended && s.project.as_deref() == Some(path))
                .count()
        };
        let hover = p.ink(8.);
        let mut rows: Vec<AnyElement> = vec![
            div()
                .px(px(8.))
                .py(px(4.))
                .text_color(p.faint)
                .child(format!("Projects on {}", host_name(&host)))
                .into_any_element(),
        ];
        for (ix, project) in projects.into_iter().enumerate() {
            let here = project.path == current;
            let n = live(&project.path);
            let right: AnyElement = if here {
                icon("check").size(px(13.)).into_any_element()
            } else if n > 0 {
                div()
                    .child(if n == 1 {
                        "1 kernel".to_string()
                    } else {
                        format!("{n} kernels")
                    })
                    .into_any_element()
            } else {
                div()
                    .child(when_opened(project.opened_ms))
                    .into_any_element()
            };
            let path = project.path.clone();
            rows.push(
                h_flex()
                    .id(("pj", ix))
                    .px(px(8.))
                    .py(px(4.))
                    .gap(px(8.))
                    .rounded(px(look::RADIUS - 4.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(icon("folder").size(px(13.)).text_color(p.muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_color(p.faint)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(short_path(&project.path, home.as_deref())),
                            ),
                    )
                    .child(div().flex_none().text_color(p.faint).child(right))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.machine.project_menu = None;
                        if !here {
                            this.open_project(path.clone(), cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        rows.push(crate::tree_actions::separator(&p));
        let item =
            |id: &'static str,
             name: &'static str,
             label: &'static str,
             keys: &'static str,
             cx: &mut Context<Self>,
             run: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
                h_flex()
                    .id(id)
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(look::RADIUS - 4.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(icon(name).size(px(13.)).text_color(p.muted))
                    .child(div().flex_1().child(label))
                    .child(div().text_color(p.faint).child(keys))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.machine.project_menu = None;
                        run(this, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            };
        rows.push(item(
            "pj-settings",
            "settings",
            "Project settings…",
            "",
            cx,
            |this, window, cx| this.show_project_settings(window, cx),
        ));
        rows.push(item(
            "pj-open",
            "folder",
            "Open project…",
            "⌘O",
            cx,
            |this, window, cx| {
                let host = this.host();
                this.with_shell(window, cx, move |shell, window, cx| {
                    shell.show_start(StartMode::Picking(host), window, cx)
                });
            },
        ));
        rows.push(item(
            "pj-new",
            "add",
            "New project…",
            "⇧⌘N",
            cx,
            |this, window, cx| {
                let host = this.host();
                this.with_shell(window, cx, move |shell, window, cx| {
                    shell.show_start(StartMode::Picking(host), window, cx)
                });
            },
        ));
        let body = crate::composer_view::popup(&p)
            .id("project-menu")
            .occlude()
            .w(px(280.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.machine.project_menu = None;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top(px(28.))
                .left(px(8.))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// The project's kernels, the Kernels tab: one row each, with its
    /// memory and a button that asks before shutting it down.
    pub(crate) fn render_kernels(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note = |text: &'static str| {
            h_flex()
                .h(px(28.))
                .px(px(14.))
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(text)
                .into_any_element()
        };
        if !matches!(self.link, Link::Ready(_)) {
            return note("Not connected");
        }
        if self.project.is_none() {
            return note("Open a project to see its kernels");
        }
        let sessions = self.project_sessions();
        let memory = |id: &str| {
            self.machine
                .stats
                .as_ref()
                .and_then(|s| s.sessions.iter().find(|m| m.session == id))
                .map(|m| format_bytes(m.rss))
                .unwrap_or_default()
        };
        let mut list = v_flex().pt(px(4.));
        if sessions.is_empty() {
            list = list.child(note("No kernels running"));
        }
        let hover = p.ink(7.);
        for (ix, s) in sessions.iter().enumerate() {
            let used = self.used_by(&s.id);
            let label = session_label(s, &self.kernels);
            let lang = look::language_icon(&s.language);
            let row = h_flex()
                .id(("krow", ix))
                .h(px(34.))
                .pl(px(14.))
                .pr(px(6.))
                .gap(px(8.))
                .text_size(px(look::SMALL))
                .rounded(px(look::RADIUS - 4.))
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .children(lang.map(|l| picture(l).size(px(14.)))),
                );
            if self.machine.asking.as_deref() == Some(s.id.as_str()) {
                let id = s.id.clone();
                list = list.child(ask_row(
                    format!("Shut down {label}?"),
                    lang,
                    "k",
                    &p,
                    cx.listener(|this, _, _, cx| {
                        this.machine.asking = None;
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| this.shut_down(vec![id.clone()], cx)),
                ));
                continue;
            }
            let id = s.id.clone();
            let busy = s.pending_runs > 0;
            list = list.child(
                row.hover(move |d| d.bg(hover))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(px(5.))
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .when(used.is_empty(), |d| d.text_color(p.muted))
                                            .child(version_label(s, &self.kernels)),
                                    )
                                    .when(busy, |d| d.child(look::dot(p.busy).size(px(6.)))),
                            )
                            .child(
                                div()
                                    .text_color(p.faint)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(if used.is_empty() {
                                        "not used by an open file".to_string()
                                    } else {
                                        used.join(", ")
                                    }),
                            ),
                    )
                    .child(div().flex_none().text_color(p.faint).child(memory(&s.id)))
                    .child(
                        crate::look::icon_button(("kx", ix), "delete", &p, false)
                            .tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("Shut down this session")
                                    .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.machine.asking = Some(id.clone());
                                cx.notify();
                            })),
                    ),
            );
        }
        // The idle sessions no open file uses, shut down together.
        let unused: Vec<String> = sessions
            .iter()
            .filter(|s| s.pending_runs == 0 && self.used_by(&s.id).is_empty())
            .map(|s| s.id.clone())
            .collect();
        if !unused.is_empty() {
            let n = unused.len();
            let row = h_flex()
                .id("kunused")
                .h(px(26.))
                .pl(px(14.))
                .pr(px(6.))
                .gap(px(8.))
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .rounded(px(look::RADIUS - 4.))
                .child(icon("delete").size(px(13.)));
            if self.machine.asking.as_deref() == Some(UNUSED) {
                list = list.child(ask_row(
                    if n == 1 {
                        "Shut down 1 unused session?".to_string()
                    } else {
                        format!("Shut down {n} unused sessions?")
                    },
                    None,
                    "ku",
                    &p,
                    cx.listener(|this, _, _, cx| {
                        this.machine.asking = None;
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| this.shut_down(unused.clone(), cx)),
                ));
            } else {
                list = list.child(
                    row.cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(if n == 1 {
                            "Shut down the unused session".to_string()
                        } else {
                            format!("Shut down {n} unused sessions")
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.machine.asking = Some(UNUSED.into());
                            cx.notify();
                        })),
                );
            }
        }
        div()
            .id("kernels")
            .size_full()
            .overflow_y_scroll()
            .child(list.px(px(6.)))
            .into_any_element()
    }

    /// The machine's CPU, memory, GPUs and disk at the right of the status
    /// bar: a short bar each; clicking one shows its details above it.
    pub(crate) fn render_resources(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stats = self.machine.stats.as_ref()?;
        if !matches!(self.link, Link::Ready(_)) {
            return None;
        }
        let p = *Palette::get(cx);
        let rows = resource_rows(stats, &self.machine.sessions, &self.kernels);
        let hover = p.ink(7.);
        let items: Vec<AnyElement> = rows
            .iter()
            .map(|r| {
                let key = r.key;
                let picked = self.machine.resource == Some(key);
                h_flex()
                    .id(SharedString::from(format!("res-{key}")))
                    .flex_none()
                    .h(px(20.))
                    .px(px(6.))
                    .gap(px(5.))
                    .rounded(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(if r.percent >= 80 { p.err } else { p.muted })
                    .cursor_pointer()
                    .when(picked, |d| d.bg(hover))
                    .hover(move |d| d.bg(hover))
                    .child(icon(r.icon).size(px(12.)))
                    .child(r.label)
                    .child(div().w(px(36.)).flex().child(bar(r.percent, &p)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.machine.resource = if this.machine.resource == Some(key) {
                            None
                        } else {
                            Some(key)
                        };
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        Some(
            h_flex()
                .flex_none()
                .gap(px(2.))
                .children(items)
                .into_any_element(),
        )
    }

    /// The details of the resource picked in the status bar, above it at
    /// the window's right.
    pub(crate) fn render_resource_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stats = self.machine.stats.as_ref()?;
        let picked = self.machine.resource?;
        let p = *Palette::get(cx);
        let rows = resource_rows(stats, &self.machine.sessions, &self.kernels);
        let r = rows.iter().find(|r| r.key == picked)?;
        let body = crate::composer_view::popup(&p)
            .id("res-pop")
            .occlude()
            .w(px(260.))
            .p(px(10.))
            .gap(px(6.))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(r.name.clone()),
                    )
                    .child(div().text_color(p.faint).child(host_name(&self.host()))),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::BOLD)
                            .child(format!("{}%", r.percent)),
                    )
                    .child(bar(r.percent, &p)),
            )
            .child(div().child(r.main.clone()))
            .child(div().text_color(p.muted).child(r.detail.clone()))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.machine.resource = None;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .bottom(px(look::STATUS_BAR + 2.))
                .right(px(look::EDGE))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }
}

/// "16:42" today, or "Oct 9".
fn when_opened(ms: i64) -> String {
    use chrono::TimeZone;
    let Some(at) = chrono::Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    if at.date_naive() == chrono::Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d").to_string()
    }
}

fn bar(percent: u32, p: &Palette) -> impl IntoElement {
    let hot = percent >= 80;
    div()
        .flex_1()
        .h(px(4.))
        .rounded_full()
        .bg(p.ink(12.))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(percent.min(100) as f32 / 100.))
                .bg(if hot { p.err } else { p.accent }),
        )
}

/// A kernel row asking before it shuts down: the question, then Cancel and
/// Shut down under it.
fn ask_row(
    question: String,
    lang: Option<&'static str>,
    id: &'static str,
    p: &Palette,
    cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (cancel_id, stop_id) = match id {
        "k" => ("k-cancel", "k-stop"),
        _ => ("ku-cancel", "ku-stop"),
    };
    v_flex()
        .pl(px(14.))
        .pr(px(6.))
        .py(px(4.))
        .gap(px(4.))
        .text_size(px(look::SMALL))
        .child(
            h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .children(lang.map(|l| picture(l).size(px(14.)))),
                )
                .child(div().flex_1().min_w_0().child(question)),
        )
        .child(
            h_flex()
                .justify_end()
                .gap(px(6.))
                .child(small_button(cancel_id, "Cancel", false, p).on_click(cancel))
                .child(small_button(stop_id, "Shut down", true, p).on_click(stop)),
        )
}

fn small_button(id: &'static str, label: &'static str, danger: bool, p: &Palette) -> Stateful<Div> {
    let hover = p.ink(8.);
    div()
        .id(id)
        .flex_none()
        .px(px(8.))
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .cursor_pointer()
        .map(|d| {
            if danger {
                d.bg(p.err).text_color(p.accent_text)
            } else {
                d.border_1()
                    .border_color(p.line)
                    .hover(move |d| d.bg(hover))
            }
        })
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, language_version};
    use sandhill_proto::{SessionInfo, SessionStatus};

    fn session(language: &str, version: &str) -> SessionInfo {
        SessionInfo {
            id: "s".into(),
            kernel: "k".into(),
            language: language.into(),
            cwd: "/".into(),
            pid: None,
            status: SessionStatus::Running,
            created_ms: 0,
            ended_ms: None,
            last_seq: 0,
            project: None,
            pending_runs: 0,
            language_version: Some(version.into()),
        }
    }

    #[test]
    fn names_versions_and_sizes() {
        assert_eq!(
            language_version(&session("R", "R version 4.6.1 (2026-06-24)")),
            "4.6.1"
        );
        assert_eq!(language_version(&session("python", "3.12.4")), "3.12");
        assert_eq!(format_bytes(56 * 1024), "56 KB");
        assert_eq!(format_bytes(412 * 1024 * 1024), "412 MB");
        assert_eq!(format_bytes(2_254_857_830), "2.1 GB");
    }
}
