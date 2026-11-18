//! The machine a tab is connected to, as the Tauri app's left pane shows it
//! (`LeftPane.tsx`, `model/machine.ts`): the project's running kernels, each
//! with its memory and a Shut down that asks first; and the machine's CPU,
//! memory, GPUs and disk. Polled every few seconds while connected.

use std::time::Duration;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, KernelspecInfo, SessionInfo, SessionStatus, SystemStats};

use crate::look::{self, Palette, icon, picture};
use crate::workbench::{Link, Workbench};

const POLL: Duration = Duration::from_secs(5);
/// What `asking` holds while "Shut down the unused sessions" asks.
const UNUSED: &str = "unused";
const MEMORY_WARNING: &str = "Kernels are using more than 80% of this device's memory. Shut down kernels you no longer need.";

#[derive(Default)]
struct MemoryWarnings(std::collections::HashMap<String, bool>);

impl MemoryWarnings {
    fn update(&mut self, host: &str, high: bool) -> bool {
        let previous = self.0.insert(host.to_string(), high).unwrap_or(false);
        high && !previous
    }
}

fn kernel_memory_high(stats: &SystemStats, sessions: &[SessionInfo]) -> Option<bool> {
    let total = stats.memory.as_ref()?.total;
    if total == 0 {
        return None;
    }
    let used: u128 = stats
        .sessions
        .iter()
        .filter(|memory| {
            sessions.iter().any(|session| {
                session.id == memory.session && session.status == SessionStatus::Running
            })
        })
        .map(|memory| u128::from(memory.rss))
        .sum();
    Some(used * 5 > u128::from(total) * 4)
}

static MEMORY_WARNINGS: std::sync::LazyLock<std::sync::Mutex<MemoryWarnings>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(MemoryWarnings::default()));

#[derive(Default)]
pub(crate) struct MachineUi {
    pub stats: Option<SystemStats>,
    /// Every session the daemon knows, as last listed.
    pub sessions: Vec<SessionInfo>,
    /// The kernel row (a session id, or UNUSED) asking to shut down.
    pub asking: Option<String>,
    /// Kernels that didn't stop when asked, with why: their rows offer Try
    /// again and Force stop (Huan, 2026-11-18).
    pub failed: std::collections::HashMap<String, String>,
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

fn percent_of(part: u64, whole: u64) -> Option<u32> {
    (whole > 0).then(|| (part as f64 / whole as f64 * 100.).round().clamp(0., 100.) as u32)
}

fn known_percent(value: Option<f64>) -> Option<u32> {
    value
        .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
        .map(|p| p.round() as u32)
}

fn temperature_label(value: Option<f64>) -> String {
    value.filter(|v| v.is_finite()).map_or_else(
        || "Temperature unavailable".into(),
        |v| format!("Temperature {v:.0} °C"),
    )
}

/// Which R kernel runs a session, when it is R: Ark or IRkernel (Huan,
/// 2026-11-15: their sessions otherwise read the same), as the daemon
/// says, else from the kernel's name for daemons that don't say. Conda's
/// R kernels (`conda-env-…-r`) are IRkernel.
fn r_engine(session: &SessionInfo, spec: Option<&KernelspecInfo>) -> Option<String> {
    if sandhill_proto::language_label(&session.language) != "R" {
        return None;
    }
    if let Some(engine) = spec.and_then(|k| k.engine.clone()) {
        return Some(engine);
    }
    let name = session.kernel.to_ascii_lowercase();
    let display = spec.map_or(String::new(), |k| k.display_name.to_ascii_lowercase());
    if name.contains("ark") || display.contains("ark") {
        Some("Ark".into())
    } else if display.contains("irkernel")
        || name == "ir"
        || name.starts_with("ir-")
        || name.starts_with("ir_")
        || (name.starts_with("conda-") && name.ends_with("-r"))
    {
        Some("IRkernel".into())
    } else {
        None
    }
}

/// A session's name in the shared format: "R 4.6.2 · Ark", "Python 3.12
/// (omicverse)". The version is the one the running kernel reported.
pub(crate) fn session_label(session: &SessionInfo, specs: &[KernelspecInfo]) -> String {
    let spec = specs.iter().find(|k| k.name == session.kernel);
    sandhill_proto::kernel_label(
        &session.language,
        session
            .language_version
            .as_deref()
            .or_else(|| spec.and_then(|k| k.version.as_deref())),
        spec.and_then(|k| k.env.as_deref()),
        r_engine(session, spec).as_deref(),
    )
}

/// One host resource, including explicitly unavailable measurements.
pub(crate) struct Resource {
    pub key: &'static str,
    pub icon: &'static str,
    pub name: String,
    pub percent: Option<u32>,
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
        name: "CPU".into(),
        percent: known_percent(cpu.percent),
        main: if cpu.model.is_empty() {
            format!("{} cores", cpu.cores)
        } else {
            format!("{} · {} cores", cpu.model, cpu.cores)
        },
        detail: format!(
            "{} · {}",
            cpu.load.map_or_else(
                || "Load unavailable".into(),
                |load| format!("Load {load:.1}")
            ),
            temperature_label(cpu.temperature_celsius)
        ),
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
        name: "Memory".into(),
        percent: stats
            .memory
            .as_ref()
            .and_then(|m| percent_of(m.used, m.total)),
        main: stats.memory.as_ref().filter(|m| m.total > 0).map_or_else(
            || "Memory unavailable".into(),
            |m| format!("{} of {} used", format_bytes(m.used), format_bytes(m.total)),
        ),
        detail: if kernels.is_empty() {
            "No kernels running".into()
        } else {
            format!("Kernels: {}", kernels.join(" · "))
        },
    });
    if !stats.gpus.is_empty() || stats.gpu_status != sandhill_proto::GpuStatus::Absent {
        // Only available measurements participate. An unknown device never
        // wins over a measured device, nor becomes a fabricated zero.
        let busiest = stats
            .gpus
            .iter()
            .filter_map(|g| known_percent(g.percent))
            .max();
        let n = stats.gpus.len();
        let mut details: Vec<String> = stats
            .gpus
            .iter()
            .map(|g| {
                let busy = known_percent(g.percent).map_or_else(
                    || "Utilization unavailable".into(),
                    |p| format!("{p}% busy"),
                );
                let memory = match (g.memory_used, g.memory_total) {
                    (Some(used), Some(total)) => {
                        format!("VRAM {} of {}", format_bytes(used), format_bytes(total))
                    }
                    (Some(used), None) => {
                        format!("GPU memory {} · capacity unavailable", format_bytes(used))
                    }
                    _ => "VRAM unavailable".into(),
                };
                format!(
                    "{}: {busy}\n{memory} · {}",
                    g.name,
                    temperature_label(g.temperature_celsius)
                )
            })
            .collect();
        if let Some(error) = &stats.gpu_error {
            details.push(error.clone());
        }
        if details.is_empty() {
            details.push("GPU discovery unavailable".into());
        }
        rows.push(Resource {
            key: "gpu",
            icon: "gpu",
            name: if n > 1 {
                format!("GPU ({n})")
            } else {
                "GPU".into()
            },
            percent: busiest,
            main: if n > 1 {
                format!("{n} devices · busiest measured utilization")
            } else if let Some(device) = stats.gpus.first() {
                device.name.clone()
            } else {
                "GPU availability unknown".into()
            },
            detail: details.join("\n\n"),
        });
    }
    if let Some(disk) = &stats.disk {
        rows.push(Resource {
            key: "disk",
            icon: "disk",
            name: "Disk".into(),
            percent: percent_of(disk.used, disk.used + disk.available),
            main: format!(
                "{} of {} used",
                format_bytes(disk.used),
                format_bytes(disk.total)
            ),
            detail: format!("{} free for {}", format_bytes(disk.available), disk.path),
        });
    } else {
        rows.push(Resource {
            key: "disk",
            icon: "disk",
            name: "Disk".into(),
            percent: None,
            main: "Disk unavailable".into(),
            detail: "The daemon could not read the project's filesystem capacity.".into(),
        });
    }
    rows
}

/// A resource's name in the status bar.
fn resource_label(key: &str) -> &'static str {
    match key {
        "cpu" => "CPU",
        "mem" => "Mem",
        "gpu" => "GPU",
        _ => "Disk",
    }
}

impl Workbench {
    /// Whether the device's usage is measured now: shown in the status bar
    /// (its switch on and the chevron open), or the device popup is open.
    pub(crate) fn measuring(&self) -> bool {
        (crate::prefs::usage_in_status_bar() && crate::prefs::resources_shown())
            || self.connection_menu
    }

    /// Asks the daemon how its machine is doing, now and every few seconds
    /// for as long as this connection lasts. The resources are measured
    /// only while [`Self::measuring`]; the sessions always.
    pub(crate) fn watch_machine(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let link = this
                    .update(cx, |this, _| {
                        if this.generation != generation {
                            return None;
                        }
                        Some((this.daemon()?, this.project.clone(), this.measuring()))
                    })
                    .ok()
                    .flatten();
                let Some((daemon, path, measuring)) = link else {
                    break;
                };
                let stats =
                    measuring.then(|| daemon.call::<SystemStats>(Call::SystemStats { path }));
                let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
                drop(daemon);
                let stats = match stats {
                    Some(stats) => Some(stats.await),
                    None => None,
                };
                let sessions = sessions.await;
                let alive = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    // An older daemon can't say; the section stays hidden.
                    // Hidden since asked, the answer is dropped.
                    if let Some(Ok(stats)) = stats
                        && this.measuring()
                    {
                        this.machine.stats = Some(stats);
                    }
                    if let Ok(sessions) = sessions {
                        // A kernel that didn't stop is no longer offered
                        // again once it has gone.
                        this.machine.failed.retain(|id, _| {
                            sessions
                                .iter()
                                .any(|s| &s.id == id && s.status != SessionStatus::Ended)
                        });
                        this.machine.sessions = sessions;
                    }
                    if let Some(high) = this
                        .machine
                        .stats
                        .as_ref()
                        .and_then(|stats| kernel_memory_high(stats, &this.machine.sessions))
                        && MEMORY_WARNINGS
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .update(&this.host(), high)
                    {
                        this.notice = Some(MEMORY_WARNING.into());
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

    /// Measures the resources once, now: they were just shown again.
    pub(crate) fn read_stats(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let stats = daemon.call::<SystemStats>(Call::SystemStats {
            path: self.project.clone(),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(stats) = stats.await {
                this.update(cx, |this, cx| {
                    if this.generation == generation && this.measuring() {
                        this.machine.stats = Some(stats);
                        cx.notify();
                    }
                })
                .ok();
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
            .map(|o| crate::panes::files::base_name(&o.notebook.path).to_string())
            .collect();
        if self.loose.id() == Some(session) || self.console_sessions.retained.contains(session) {
            names.push("Console".into());
        }
        names
    }

    /// Stops sessions for good; documents using them see their kernel stop.
    pub(crate) fn shut_down(&mut self, ids: Vec<String>, cx: &mut Context<Self>) {
        self.machine.asking = None;
        for id in ids {
            self.stop_kernel(id, cx);
        }
        cx.notify();
    }

    /// Asks the daemon to stop a kernel; it leaves the lists at once. One
    /// that doesn't stop comes back in Kernels with Try again and Force
    /// stop, and the problem line says so, rather than running on unseen.
    pub(crate) fn stop_kernel(&mut self, id: String, cx: &mut Context<Self>) {
        self.end_kernel(id, false, cx);
    }

    /// The user's Force stop of a kernel that didn't stop when asked.
    pub(crate) fn force_stop_kernel(&mut self, id: String, cx: &mut Context<Self>) {
        self.end_kernel(id, true, cx);
    }

    fn end_kernel(&mut self, id: String, force: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        self.machine.sessions.retain(|s| s.id != id);
        self.machine.failed.remove(&id);
        let session = id.clone();
        let ended = daemon.call::<serde_json::Value>(if force {
            Call::ForceStop { session }
        } else {
            Call::Shutdown { session }
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = ended.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(if force {
                        format!("Force stop failed: {err:#}")
                    } else {
                        "A kernel didn't stop when asked. It is still in Kernels: Try again, or Force stop.".into()
                    });
                    this.machine.failed.insert(id, format!("{err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// The project's kernels, the Kernels tab: one row each, with its
    /// memory and a button that asks before shutting it down.
    pub(crate) fn render_kernels(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note = |text: &'static str| {
            h_flex()
                .h(px(look::row()))
                .px(px(look::GAP_8))
                .text_size(px(look::small()))
                .text_color(p.faint)
                .child(text)
                .into_any_element()
        };
        if !matches!(self.link, Link::Ready(_)) {
            return div()
                .pt(px(look::strip() + look::PANE_PAD_Y))
                .px(px(look::GAP_4))
                .child(note("Not connected"))
                .into_any_element();
        }
        if self.project.is_none() {
            return div()
                .pt(px(look::strip() + look::PANE_PAD_Y))
                .px(px(look::GAP_4))
                .child(note("Open a project to see its kernels"))
                .into_any_element();
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
        let mut list = v_flex().py(px(look::PANE_PAD_Y));
        if self
            .machine
            .stats
            .as_ref()
            .and_then(|stats| kernel_memory_high(stats, &self.machine.sessions))
            == Some(true)
        {
            list = list.child(
                div()
                    .px(px(look::GAP_8))
                    .py(px(look::GAP_4))
                    .text_size(px(look::small()))
                    .text_color(p.warn)
                    .child(MEMORY_WARNING),
            );
        }
        if sessions.is_empty() {
            list = list.child(note("No kernels running"));
        }
        let hover = p.ink(look::ROW_HOVER);
        for (ix, s) in sessions.iter().enumerate() {
            let used = self.used_by(&s.id);
            let label = session_label(s, &self.kernels);
            let lang = look::language_icon(&s.language);
            let row = h_flex()
                .id(("krow", ix))
                .h(px(look::row()))
                .px(px(look::GAP_8))
                .gap(px(look::GAP_4))
                .text_size(px(look::small()))
                .rounded(px(look::CONTROL_RADIUS))
                .child(
                    div()
                        .w(px(look::ICON))
                        .flex_none()
                        .children(lang.map(|l| picture(l).size(px(look::ICON)))),
                );
            if let Some(why) = self.machine.failed.get(&s.id) {
                let (again, force) = (s.id.clone(), s.id.clone());
                list = list.child(
                    div()
                        .id(("kfailed", ix))
                        .tooltip({
                            let why = why.clone();
                            move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(why.clone()).build(w, cx)
                            }
                        })
                        .child(look::question_row(
                            format!("{label} didn't stop"),
                            lang.map(|l| picture(l).into_any_element()),
                            (("kf-again", ix).into(), ("kf-force", ix).into()),
                            ("Try again", "Force stop"),
                            cx.listener(move |this, _, _, cx| this.stop_kernel(again.clone(), cx)),
                            cx.listener(move |this, _, _, cx| {
                                this.force_stop_kernel(force.clone(), cx)
                            }),
                        )),
                );
                continue;
            }
            if self.machine.asking.as_deref() == Some(s.id.as_str()) {
                let id = s.id.clone();
                list = list.child(look::question_row(
                    format!("Shut down {label}?"),
                    lang.map(|l| picture(l).into_any_element()),
                    ("k-cancel".into(), "k-stop".into()),
                    ("Cancel", "Shut down"),
                    cx.listener(|this, _, _, cx| {
                        this.machine.asking = None;
                        cx.notify();
                    }),
                    cx.listener(move |this, _, _, cx| this.shut_down(vec![id.clone()], cx)),
                ));
                continue;
            }
            let id = s.id.clone();
            let selected = id.clone();
            let busy = s.pending_runs > 0;
            list = list.child(
                row.cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_console_session(&selected, cx);
                        this.show_tab(crate::workbench::ConsoleTab::Console, window, cx);
                    }))
                    // "Python 3.12 (omicverse) · qc.ipynb": the kernel, then
                    // the open files using it; the files give way first.
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(look::GAP_4))
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .flex_none()
                                    .when(used.is_empty(), |d| d.text_color(p.muted))
                                    .child(label.clone()),
                            )
                            .when(busy, |d| d.child(look::dot(p.busy).size(px(6.))))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(p.faint)
                                    .child(if used.is_empty() {
                                        "· unused".to_string()
                                    } else {
                                        format!("· {}", used.join(", "))
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
                                cx.stop_propagation();
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
                .h(px(look::row()))
                .px(px(look::GAP_8))
                .gap(px(look::GAP_4))
                .text_size(px(look::small()))
                .text_color(p.muted)
                .rounded(px(look::CONTROL_RADIUS))
                .child(icon("delete").size(px(look::ICON)));
            if self.machine.asking.as_deref() == Some(UNUSED) {
                list = list.child(look::question_row(
                    if n == 1 {
                        "Shut down 1 unused session?".to_string()
                    } else {
                        format!("Shut down {n} unused sessions?")
                    },
                    None,
                    ("ku-cancel".into(), "ku-stop".into()),
                    ("Cancel", "Shut down"),
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
        look::PaneScroll::new(
            "kernels-scroll",
            look::strip(),
            0.,
            div().id("kernels").child(list.px(px(look::GAP_4))),
        )
        .into_any_element()
    }

    /// The status bar's left capsule: the device, then (while shown and
    /// `wide`) its CPU, memory, GPUs and disk as numbers, each opening its
    /// details above it, and a chevron that hides or shows them. Hidden,
    /// they aren't measured at all (Huan, 2026-11-15). The device popup's
    /// switch takes numbers and chevron away (Huan, 2026-11-16).
    pub(crate) fn render_device_capsule(&self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let usage = crate::prefs::usage_in_status_bar();
        let shown = usage && crate::prefs::resources_shown();
        let ready = matches!(self.link, Link::Ready(_));
        let hover = p.ink(8.);
        let mut items: Vec<AnyElement> = Vec::new();
        if let Some(stats) = self
            .machine
            .stats
            .as_ref()
            .filter(|_| shown && ready && wide)
        {
            for r in resource_rows(stats, &self.machine.sessions, &self.kernels) {
                let key = r.key;
                let picked = self.machine.resource == Some(key);
                let high = r.percent.is_some_and(|v| v >= 80);
                items.push(
                    h_flex()
                        .id(SharedString::from(format!("res-{key}")))
                        .on_prepaint(self.popup_triggers.track(key))
                        .flex_none()
                        .h(px(16.))
                        .px(px(5.))
                        .gap(px(4.))
                        .rounded_full()
                        .cursor_pointer()
                        .when(picked, |d| d.bg(hover))
                        .hover(move |d| d.bg(hover))
                        .child(div().text_color(p.muted).child(resource_label(key)))
                        .child(
                            div()
                                .text_color(if high { p.err } else { p.text })
                                .child(r.percent.map_or("–".to_string(), |v| format!("{v}%"))),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.machine.resource = if this.machine.resource == Some(key) {
                                None
                            } else {
                                Some(key)
                            };
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let toggle = (ready && wide && usage).then(|| {
            look::close_circle("status-resources", &p)
                .child(
                    icon(if shown {
                        "chevron.left"
                    } else {
                        "chevron.right"
                    })
                    .size(px(12.)),
                )
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if shown {
                        "Hide CPU, memory, GPU and disk (stops measuring them)"
                    } else {
                        "Show CPU, memory, GPU and disk"
                    })
                    .build(w, cx)
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    crate::shell::choices::choose(
                        crate::shell::choices::Choice::Resources(!shown),
                        cx,
                    )
                }))
        });
        look::capsule(&p, 20.)
            .gap(px(1.))
            .min_w_0()
            .flex_shrink(1.)
            .child(self.render_connection_status(cx))
            .when(!items.is_empty(), |d| {
                d.child(look::capsule_divider(&p)).children(items)
            })
            .children(toggle)
            .into_any_element()
    }

    /// The resource details open above their status-bar trigger.
    pub(crate) fn render_resource_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stats = self.machine.stats.as_ref()?;
        let picked = self.machine.resource?;
        let p = *Palette::get(cx);
        let rows = resource_rows(stats, &self.machine.sessions, &self.kernels);
        let r = rows.iter().find(|r| r.key == picked)?;
        let body = crate::agents::composer_view::popup(&p)
            .id("res-pop")
            .occlude()
            .max_h(px(420.))
            .overflow_y_scroll()
            .w(px(260.))
            .p(px(10.))
            .gap(px(6.))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon(r.icon).size(px(14.)))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(p.faint)
                            .child(crate::welcome::start_screen::host_label(&self.host())),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(look::title()))
                            .font_weight(FontWeight::BOLD)
                            .child(
                                r.percent
                                    .map_or_else(|| "Unavailable".into(), |v| format!("{v}%")),
                            ),
                    )
                    .child(crate::shell::meter::ring(
                        r.percent.map(|v| v as f32 / 100.),
                        if r.percent.is_some_and(|v| v >= 80) {
                            p.err
                        } else {
                            p.accent
                        },
                        p.ink(24.),
                    )),
            )
            .child(div().child(r.main.clone()))
            .child(div().text_color(p.muted).child(r.detail.clone()))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this
                    .machine
                    .resource
                    .is_some_and(|key| this.popup_triggers.contains(key, event.position))
                {
                    return;
                }
                this.machine.resource = None;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .bottom(px(look::STATUS_BAR + 2.))
                .left(
                    self.popup_triggers
                        .bounds(picked)
                        .map_or(px(look::EDGE), |bounds| bounds.origin.x),
                )
                .child(
                    deferred(
                        anchored()
                            .anchor(Anchor::BottomLeft)
                            .snap_to_window_with_margin(px(8.))
                            .child(body),
                    )
                    .with_priority(3),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryWarnings, format_bytes, kernel_memory_high, resource_rows, session_label};
    use sandhill_proto::{KernelspecInfo, SessionInfo, SessionStatus};

    #[test]
    fn memory_threshold_is_strict_device_scoped_and_rearms() {
        let mut stats: sandhill_proto::SystemStats = serde_json::from_value(serde_json::json!({
            "cpu": {"cores": 8}, "memory": {"total": 100, "used": 99},
            "sessions": [{"session": "s", "rss": 80}, {"session": "ended", "rss": 999}]
        }))
        .unwrap();
        let live = vec![session("r", "4.6")];
        assert_eq!(kernel_memory_high(&stats, &live), Some(false));
        stats.sessions[0].rss = 81;
        assert_eq!(kernel_memory_high(&stats, &live), Some(true));
        let mut warnings = MemoryWarnings::default();
        assert!(warnings.update("local", true));
        assert!(!warnings.update("local", true));
        assert!(warnings.update("remote", true));
        assert!(!warnings.update("local", false));
        assert!(warnings.update("local", true));
        stats.memory = None;
        assert_eq!(kernel_memory_high(&stats, &live), None);
    }

    #[test]
    fn unavailable_resources_and_mixed_gpu_capabilities_remain_truthful() {
        let mut stats: sandhill_proto::SystemStats = serde_json::from_value(serde_json::json!({
            "cpu": {"cores":8}, "memory":null,
            "gpu_status":"unavailable", "gpu_error":"NVIDIA driver mismatch",
            "gpus":[
                {"name":"Unknown NVIDIA", "percent":null},
                {"name":"AMD", "percent":32.0, "memory_used":100, "memory_total":200, "temperature_celsius":51.0},
                {"name":"Idle Intel", "percent":0.0}
            ]
        })).unwrap();
        let rows = resource_rows(&stats, &[], &[]);
        assert_eq!(rows.iter().find(|r| r.key == "cpu").unwrap().percent, None);
        assert_eq!(rows.iter().find(|r| r.key == "mem").unwrap().percent, None);
        assert_eq!(rows.iter().find(|r| r.key == "disk").unwrap().percent, None);
        let gpu = rows.iter().find(|r| r.key == "gpu").unwrap();
        assert_eq!(gpu.percent, Some(32));
        assert!(
            gpu.detail
                .contains("Unknown NVIDIA: Utilization unavailable")
        );
        assert!(gpu.detail.contains("Idle Intel: 0% busy"));
        assert!(gpu.detail.contains("Temperature 51 °C"));
        assert!(gpu.detail.contains("NVIDIA driver mismatch"));
        stats.gpus.clear();
        stats.gpu_status = sandhill_proto::GpuStatus::Absent;
        stats.gpu_error = None;
        assert!(
            !resource_rows(&stats, &[], &[])
                .iter()
                .any(|r| r.key == "gpu")
        );
        stats.gpu_status = sandhill_proto::GpuStatus::Unavailable;
        assert_eq!(
            resource_rows(&stats, &[], &[])
                .iter()
                .find(|r| r.key == "gpu")
                .unwrap()
                .percent,
            None
        );
    }

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
            session_label(&session("R", "R version 4.6.1 (2026-06-24)"), &[]),
            "R 4.6.1"
        );
        assert_eq!(
            session_label(&session("python", "3.12.4"), &[]),
            "Python 3.12"
        );
        assert_eq!(format_bytes(56 * 1024), "56 KB");
        assert_eq!(format_bytes(412 * 1024 * 1024), "412 MB");
        assert_eq!(format_bytes(2_254_857_830), "2.1 GB");
    }

    #[test]
    fn sessions_are_named_in_the_shared_format() {
        let spec =
            |name: &str, display: &str, env: Option<&str>, engine: Option<&str>| KernelspecInfo {
                name: name.into(),
                display_name: display.into(),
                language: "R".into(),
                env: env.map(str::to_string),
                version: None,
                engine: engine.map(str::to_string),
            };
        let specs = [
            spec("ark", "R 4.6.2 · Ark", None, Some("Ark")),
            // From an older daemon, which names neither kernel.
            spec("ir", "R 4.6.2 (IRkernel)", None, None),
            spec(
                "conda-env-omicverse-r",
                "R [conda env:omicverse]",
                Some("omicverse"),
                None,
            ),
        ];
        let on = |kernel: &str| SessionInfo {
            kernel: kernel.into(),
            ..session("R", "4.6.2")
        };
        assert_eq!(session_label(&on("ark"), &specs), "R 4.6.2 · Ark");
        assert_eq!(session_label(&on("ir"), &specs), "R 4.6.2 · IRkernel");
        assert_eq!(
            session_label(&on("conda-env-omicverse-r"), &specs),
            "R 4.6.2 (omicverse) · IRkernel"
        );
        assert_eq!(
            session_label(&session("python", "3.12.4"), &specs),
            "Python 3.12"
        );
    }
}
