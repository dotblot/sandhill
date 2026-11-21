//! A cancellable quit gate, before GPUI's irreversible, time-limited shutdown.
//! The confirmation uses the same panel and buttons as the import dialog.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::Call;

use super::{AppWindow, Windows};
use crate::daemon::Daemon;
use crate::look::{self, Palette};
use crate::workbench::Workbench;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Checking,
    Asking,
    Saving,
    Stopping,
    StopFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Keep,
    Stop,
    Cancel,
    Force,
}

impl Phase {
    fn accepts(self, answer: Answer, has_targets: bool) -> bool {
        match answer {
            Answer::Keep => matches!(self, Self::Asking | Self::StopFailed),
            Answer::Stop => self == Self::Asking && has_targets,
            Answer::Force => self == Self::StopFailed && has_targets,
            Answer::Cancel => matches!(
                self,
                Self::Checking | Self::Asking | Self::Saving | Self::StopFailed
            ),
        }
    }
}

#[derive(Default)]
struct QuitState {
    shells: Vec<WeakEntity<AppWindow>>,
    phase: Phase,
    attempt: u64,
    owner: Option<WindowId>,
    targets: BTreeMap<String, Rc<Daemon>>,
    retired: BTreeMap<String, Rc<Daemon>>,
    /// One short sentence, shown in red.
    error: Option<String>,
    /// What failed on each device, behind "Show details".
    details: Option<String>,
    show_details: bool,
}
impl Global for QuitState {}

pub(super) fn install(cx: &mut App) {
    // Sandhill, not GPUI's platform-dependent last-window rule, owns termination.
    cx.set_quit_mode(QuitMode::Explicit);
    cx.set_global(QuitState::default());
    native::install(cx);
}

pub(super) fn register(shell: WeakEntity<AppWindow>, cx: &mut App) {
    cx.global_mut::<QuitState>().shells.push(shell);
}

/// Keep only the RPC connection, not a project, event receiver or subscription.
/// This lets quitting the replacement Start window offer services from closed projects.
pub(super) fn retain(shell: &AppWindow, cx: &mut App) {
    let mut targets = shell.start.connected_daemons();
    targets.extend(shell.tabs.iter().filter_map(|t| {
        t.workbench
            .read(cx)
            .daemon()
            .map(|daemon| (t.host.clone(), daemon))
    }));
    targets.retain(|(host, _)| !crate::welcome::resource_manager::host_blocked(host, cx));
    cx.global_mut::<QuitState>().retired.extend(targets);
}

pub(crate) fn retain_daemon(host: String, daemon: Rc<Daemon>, cx: &mut App) {
    if !crate::welcome::resource_manager::host_blocked(&host, cx) {
        cx.global_mut::<QuitState>().retired.insert(host, daemon);
    }
}

pub(crate) fn connections(
    current: &AppWindow,
    current_entity: EntityId,
    cx: &App,
) -> BTreeMap<String, Rc<Daemon>> {
    let mut targets = cx.global::<QuitState>().retired.clone();
    targets.extend(current.start.connected_daemons());
    for tab in &current.tabs {
        if let Some(daemon) = tab.workbench.read(cx).daemon() {
            targets.insert(tab.host.clone(), daemon);
        }
    }
    for shell in shells(cx) {
        if shell.entity_id() == current_entity {
            continue;
        }
        let shell = shell.read(cx);
        targets.extend(shell.start.connected_daemons());
        for tab in &shell.tabs {
            if let Some(daemon) = tab.workbench.read(cx).daemon() {
                targets.insert(tab.host.clone(), daemon);
            }
        }
    }
    targets
}

/// Detach every view and retained quit connection without reconnecting.
pub(crate) fn forget_host(host: &str, cx: &mut App) {
    crate::welcome::resource_manager::block_host(host, cx);
    cx.global_mut::<QuitState>().retired.remove(host);
    cx.global_mut::<QuitState>().targets.remove(host);
    for shell in shells(cx) {
        let workbenches: Vec<_> = shell
            .read(cx)
            .tabs
            .iter()
            .filter(|tab| tab.host == host)
            .map(|tab| tab.workbench.clone())
            .collect();
        for wb in workbenches {
            wb.update(cx, |wb, cx| {
                wb.forget_connection(cx);
                wb.link = crate::workbench::Link::Failed(
                    "Stopped or uninstalled. Connect explicitly to start again.".into(),
                );
                cx.notify();
            });
        }
        shell.update(cx, |shell, cx| {
            shell.start.forget_host(host);
            cx.notify();
        });
    }
}

pub(super) fn key_answer(key: &str) -> Option<Answer> {
    match key {
        "enter" | "space" => Some(Answer::Keep),
        "escape" => Some(Answer::Cancel),
        _ => None,
    }
}

pub(crate) fn active(cx: &App) -> bool {
    cx.try_global::<QuitState>()
        .is_some_and(|s| s.phase != Phase::Idle)
}

fn shells(cx: &App) -> Vec<Entity<AppWindow>> {
    cx.global::<QuitState>()
        .shells
        .iter()
        .filter_map(WeakEntity::upgrade)
        .collect()
}

pub(crate) fn workbenches(cx: &App) -> Vec<Entity<Workbench>> {
    shells(cx)
        .iter()
        .flat_map(|s| {
            s.read(cx)
                .tabs
                .iter()
                .map(|t| t.workbench.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(crate) fn workbenches_from(
    current: &AppWindow,
    current_entity: EntityId,
    cx: &App,
) -> Vec<Entity<Workbench>> {
    let mut workbenches: Vec<_> = current
        .tabs
        .iter()
        .map(|tab| tab.workbench.clone())
        .collect();
    for shell in shells(cx) {
        if shell.entity_id() != current_entity {
            workbenches.extend(shell.read(cx).tabs.iter().map(|tab| tab.workbench.clone()));
        }
    }
    workbenches
}

fn connecting_target(cx: &App) -> Option<String> {
    cx.global::<QuitState>()
        .targets
        .keys()
        .find(|host| Daemon::connecting(host))
        .map(|host| crate::welcome::start_screen::host_label(host))
}

fn notify(cx: &mut App) {
    for shell in shells(cx) {
        shell.update(cx, |_, cx| cx.notify());
    }
}

pub(super) fn request(cx: &mut App) {
    if active(cx)
        || cx.global::<Windows>().quitting
        || crate::welcome::start_screen::uninstall_running(cx)
    {
        return;
    }
    if shells(cx).is_empty() && !cx.global::<QuitState>().retired.is_empty() {
        super::show_welcome(None, cx);
    }
    let shells = shells(cx);
    let owner = cx
        .active_window()
        .map(|w| w.window_id())
        .or_else(|| shells.first().map(|s| s.read(cx).id));
    if owner.is_none() {
        // No window and no retained connection: nothing can be stopped.
        finish(cx);
        return;
    }
    let mut targets = BTreeMap::new();
    for shell in &shells {
        let shell = shell.read(cx);
        for (host, daemon) in shell.start.connected_daemons() {
            targets.entry(host).or_insert(daemon);
        }
        for tab in &shell.tabs {
            if let Some(daemon) = tab.workbench.read(cx).daemon() {
                targets.entry(tab.host.clone()).or_insert(daemon);
            }
        }
    }
    let state = cx.global_mut::<QuitState>();
    state.attempt += 1;
    let attempt = state.attempt;
    state.phase = Phase::Checking;
    state.owner = owner;
    state.targets = targets;
    state.error = None;
    state.details = None;
    state.show_details = false;
    for shell in shells {
        let shell_read = shell.read(cx);
        if Some(shell_read.id) == owner {
            let focus = shell_read.focus.clone();
            if let Some(handle) = cx
                .windows()
                .into_iter()
                .find(|w| Some(w.window_id()) == owner)
            {
                handle
                    .update(cx, |_, window, cx| {
                        window.activate_window();
                        window.focus(&focus, cx);
                    })
                    .ok();
            }
        }
    }
    notify(cx);
    let retired = cx.global::<QuitState>().retired.clone();
    cx.spawn(async move |cx| {
        let mut connected = BTreeMap::new();
        for (host, daemon) in retired {
            // A bounded ping checks an existing connection; it never starts or
            // reattaches a saved/disconnected server.
            if daemon
                .call_within::<sandhill_proto::PingReply>(Call::Ping, Duration::from_secs(2))
                .await
                .is_ok()
            {
                connected.insert(host, daemon);
            }
        }
        cx.update(|cx| {
            let state = cx.global_mut::<QuitState>();
            if state.phase != Phase::Checking || state.attempt != attempt {
                return;
            }
            state.retired = connected.clone();
            for (host, daemon) in connected {
                state.targets.entry(host).or_insert(daemon);
            }
            state.phase = Phase::Asking;
            notify(cx);
        });
    })
    .detach();
}

pub(crate) fn answer(answer: Answer, cx: &mut App) {
    let phase = cx.global::<QuitState>().phase;
    if !phase.accepts(answer, !cx.global::<QuitState>().targets.is_empty()) {
        return;
    }
    if answer == Answer::Cancel {
        let state = cx.global_mut::<QuitState>();
        state.phase = Phase::Idle;
        state.owner = None;
        state.targets.clear();
        state.error = None;
        state.details = None;
        notify(cx);
        return;
    }
    if matches!(answer, Answer::Stop | Answer::Force)
        && let Some(host) = connecting_target(cx)
    {
        cx.global_mut::<QuitState>().error = Some(format!(
            "{host} is still connecting. Wait for it, or keep it running."
        ));
        notify(cx);
        return;
    }
    cx.global_mut::<QuitState>().phase = Phase::Saving;
    cx.global_mut::<QuitState>().error = None;
    cx.global_mut::<QuitState>().details = None;
    let attempt = cx.global::<QuitState>().attempt;
    let workbenches = workbenches(cx);
    let tasks: Vec<Task<()>> = workbenches
        .iter()
        .flat_map(|wb| wb.update(cx, |wb, cx| wb.save_all_tasks(cx)))
        .collect();
    notify(cx);
    cx.spawn(async move |cx| {
        for task in tasks {
            task.await;
        }
        let saved = cx.update(|cx| {
            let state = cx.global::<QuitState>();
            if state.phase != Phase::Saving || state.attempt != attempt {
                return false;
            }
            if workbenches.iter().any(|wb| wb.read(cx).any_dirty()) {
                let state = cx.global_mut::<QuitState>();
                state.phase = Phase::Asking;
                state.error = Some(
                    "Some edited documents could not be saved, so nothing was stopped.".into(),
                );
                notify(cx);
                false
            } else {
                true
            }
        });
        if !saved {
            return;
        }
        if answer == Answer::Keep {
            cx.update(finish);
            return;
        }
        let targets = cx.update(|cx| {
            cx.global_mut::<QuitState>().phase = Phase::Stopping;
            notify(cx);
            cx.global::<QuitState>().targets.clone()
        });
        let mut failures = Vec::new();
        for (host, daemon) in targets {
            let result = daemon
                .call_within::<serde_json::Value>(
                    Call::StopResources {
                        force: answer == Answer::Force,
                    },
                    Duration::from_secs(60),
                )
                .await;
            match result {
                Ok(value) if crate::welcome::resource_manager::resources_stopped(&value) => {
                    cx.update(|cx| crate::welcome::resource_manager::block_host(&host, cx));
                    match daemon.stop_confirmed().await {
                        Ok(_) => {
                            cx.update(|cx| forget_host(&host, cx));
                        }
                        Err(error) => failures.push(format!(
                            "{}: resources stopped, but daemon stop was not confirmed: {error:#}",
                            target_label(&host)
                        )),
                    }
                }
                Ok(value) => failures.push(format!(
                    "{}: {}",
                    target_label(&host),
                    crate::welcome::resource_manager::stop_failure(&value)
                )),
                Err(error) => failures.push(format!(
                    "{}: resource stop was not confirmed: {error:#}",
                    target_label(&host)
                )),
            }
        }
        cx.update(|cx| {
            if failures.is_empty() {
                finish(cx);
            } else {
                let state = cx.global_mut::<QuitState>();
                state.phase = Phase::StopFailed;
                state.error = Some("Some background work could not be stopped.".into());
                state.details = Some(failures.join("\n"));
                notify(cx);
            }
        });
    })
    .detach();
}

pub(crate) fn finish_after_cleanup(cx: &mut App) {
    finish(cx);
}

fn finish(cx: &mut App) {
    cx.global_mut::<Windows>().quitting = true;
    native::approve();
    cx.quit();
}

fn target_label(host: &str) -> String {
    let name = crate::welcome::start_screen::host_label(host);
    if host == sandhill_link::LOCAL {
        format!("Local: {name}")
    } else if name == host {
        format!("Remote: {host}")
    } else {
        format!("Remote: {name} ({host})")
    }
}

pub(super) fn render(shell: &AppWindow, cx: &mut Context<AppWindow>) -> Option<AnyElement> {
    let state = cx.global::<QuitState>();
    if state.phase == Phase::Idle {
        return None;
    }
    let p = *Palette::get(cx);
    let owner = state.owner == Some(shell.id);
    let asking = matches!(state.phase, Phase::Asking | Phase::StopFailed);
    let can_stop = state.phase == Phase::Asking && !state.targets.is_empty();
    let failed = state.phase == Phase::StopFailed && !state.targets.is_empty();
    let devices = device_list(
        state
            .targets
            .keys()
            .map(|h| crate::welcome::start_screen::host_label(h))
            .collect(),
    );
    let error = state.error.clone();
    let details = state.details.clone().filter(|_| state.show_details);
    let has_details = state.details.is_some();
    let status = match state.phase {
        Phase::Checking => "Checking connected devices…",
        Phase::Saving => "Saving edited documents…",
        Phase::Stopping => "Stopping background work…",
        _ => "Quit Sandhill?",
    };
    let message = match &devices {
        Some(devices) => {
            format!("Kernels, agents and jobs on {devices} can keep running after Sandhill quits.")
        }
        None => "Nothing is running in the background.".into(),
    };
    let wide = |button: Button| button.w_full().h(px(look::control_card()));
    let card = v_flex()
        .id("quit-card")
        .occlude()
        .w(px(380.))
        .max_w(relative(0.95))
        .max_h(relative(0.9))
        .overflow_y_scroll()
        .p(px(20.))
        .gap(px(look::GAP_12))
        .rounded(px(look::RADIUS))
        .bg(p.panel)
        .border_1()
        .border_color(p.glass_edge)
        .shadow_lg()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // The crane beside the question, the sentence under both.
        .child(
            h_flex()
                .gap(px(look::GAP_8))
                .child(
                    img(crate::welcome::start_screen::brand_icon())
                        .size(px(40.))
                        .flex_none(),
                )
                .child(
                    div()
                        .text_size(px(look::title()))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(status),
                ),
        )
        .child(div().text_color(p.muted).child(message))
        .when_some(error, |d, error| {
            d.child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.err).child(error))
                    .when(has_details, |d| {
                        d.child(
                            div()
                                .id("quit-details")
                                .cursor_pointer()
                                .text_size(px(look::small()))
                                .text_color(p.accent)
                                .child(if details.is_some() {
                                    "Hide details"
                                } else {
                                    "Show details"
                                })
                                .on_click(cx.listener(|_, _, _, cx| {
                                    let state = cx.global_mut::<QuitState>();
                                    state.show_details = !state.show_details;
                                    cx.notify();
                                })),
                        )
                    })
                    .when_some(details, |d, details| {
                        d.child(
                            div()
                                .text_size(px(look::small()))
                                .text_color(p.muted)
                                .child(details),
                        )
                    }),
            )
        })
        .child(
            v_flex()
                .gap(px(look::GAP_8))
                .child(wide(
                    Button::new("quit-keep")
                        .primary()
                        .label(if devices.is_some() {
                            "Keep Running and Quit"
                        } else {
                            "Quit"
                        })
                        .disabled(!asking)
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.defer(|cx| answer(Answer::Keep, cx))),
                        ),
                ))
                .when(failed, |d| {
                    d.child(wide(
                        Button::new("quit-force")
                            .danger()
                            .label("Force Stop and Quit")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.defer(|cx| answer(Answer::Force, cx))),
                            ),
                    ))
                })
                .when(devices.is_some() && !failed, |d| {
                    d.child(wide(
                        Button::new("quit-stop")
                            .label("Stop All and Quit")
                            .disabled(!can_stop)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.defer(|cx| answer(Answer::Stop, cx))),
                            ),
                    ))
                })
                .child(wide(
                    Button::new("quit-cancel")
                        .label("Cancel")
                        .disabled(!state.phase.accepts(Answer::Cancel, can_stop))
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.defer(|cx| answer(Answer::Cancel, cx))),
                        ),
                )),
        );
    Some(
        deferred(
            div()
                .id("quit-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .bg(p.shadow.opacity(0.35))
                .when(owner, |d| d.child(card))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                        cx.defer(|cx| answer(Answer::Cancel, cx));
                    }),
                ),
        )
        .with_priority(10)
        .into_any_element(),
    )
}

/// "Huan MacBook", "Huan MacBook and lab", "a, b and c"; `None` for none.
fn device_list(mut names: Vec<String>) -> Option<String> {
    let last = names.pop()?;
    Some(if names.is_empty() {
        last
    } else {
        format!("{} and {last}", names.join(", "))
    })
}

#[cfg(target_os = "macos")]
mod native {
    use gpui_kit::*;
    use objc2::{
        runtime::{AnyClass, AnyObject, Imp, Sel},
        sel,
    };
    use std::cell::{Cell, RefCell};
    use tokio::sync::mpsc::{Sender, channel};

    type Terminate = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject);
    thread_local! {
        static APPROVED: Cell<bool> = const { Cell::new(false) };
        static ORIGINAL: Cell<Option<Terminate>> = const { Cell::new(None) };
        static REQUESTS: RefCell<Option<Sender<()>>> = const { RefCell::new(None) };
    }

    // GPUI only exposes an irreversible will-terminate observer. Intercept the public
    // NSApplication termination entrypoint, retaining its implementation for approved
    // quits so GPUI's own shutdown and window restoration still run normally.
    pub(super) fn install(cx: &mut App) {
        let (sender, mut requests) = channel(1);
        REQUESTS.with(|s| *s.borrow_mut() = Some(sender));
        let method = AnyClass::get(c"NSApplication")
            .expect("AppKit application class")
            .instance_method(sel!(terminate:))
            .expect("NSApplication terminate:");
        unsafe {
            let original =
                method.set_implementation(std::mem::transmute::<Terminate, Imp>(terminate));
            ORIGINAL.with(|s| s.set(Some(std::mem::transmute::<Imp, Terminate>(original))));
        }
        cx.spawn(async move |cx| {
            while requests.recv().await.is_some() {
                cx.update(super::request);
            }
        })
        .detach();
    }

    unsafe extern "C-unwind" fn terminate(
        app: *mut AnyObject,
        command: Sel,
        sender: *mut AnyObject,
    ) {
        if APPROVED.with(Cell::get) {
            if let Some(original) = ORIGINAL.with(Cell::get) {
                unsafe { original(app, command, sender) };
            }
        } else {
            REQUESTS.with(|s| {
                if let Some(requests) = s.borrow().as_ref() {
                    let _ = requests.try_send(());
                }
            });
        }
    }

    pub(super) fn approve() {
        APPROVED.with(|s| s.set(true));
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    pub(super) fn install(_: &mut gpui_kit::App) {}
    pub(super) fn approve() {}
}

#[cfg(test)]
mod tests {
    use super::{Answer, Phase, key_answer};
    use gpui_kit::{AppContext, TestAppContext, WindowOptions};
    use std::sync::Arc;

    use crate::shell::app_window::{AppWindow, Kind, Place, Places, Windows};

    #[gpui_kit::test]
    fn statusbar_resources_release_workbench_before_reading_connections(cx: &mut TestAppContext) {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let place = Place {
            host: sandhill_link::LOCAL.into(),
            path: "/statusbar-resource-regression".into(),
        };
        let (_, shell) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            // Avoid native quit interception and writes to the user's saved windows.
            cx.set_global(super::QuitState::default());
            cx.set_global(Windows {
                rt: rt.clone(),
                open: Vec::new(),
                main: None,
                welcome: None,
                quick: Vec::new(),
                quitting: true,
            });
            // Construction must not connect a daemon or open the requested project.
            crate::welcome::resource_manager::block_host(&place.host, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    AppWindow::new(
                        rt,
                        Places {
                            tabs: vec![place.clone()],
                            current: Some(place),
                        },
                        Kind::Main,
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        let workbench = cx.read(|cx| shell.read(cx).tabs[0].workbench.clone());
        cx.update(|cx| {
            workbench.update(cx, |workbench, cx| {
                assert!(matches!(workbench.link, crate::workbench::Link::Failed(_)));
                assert!(workbench.project.is_none());
                // This is the statusbar item's handler, under its actual update lease.
                workbench.manage_resources(cx);
                assert!(shell.read(cx).resource_manager.is_none());
            });
        });
        // App::update drains deferred effects only after returning the workbench lease.
        // Resources and Storage is a page in Welcome's own window.
        cx.read(|cx| {
            let welcome = cx
                .global::<Windows>()
                .welcome
                .clone()
                .expect("Welcome opened");
            let welcome = welcome.view.upgrade().unwrap();
            assert!(welcome.read(cx).resource_manager.is_some());
            assert!(shell.read(cx).resource_manager.is_none());
        });
    }

    #[test]
    fn default_and_dismiss_keys_never_stop_background_services() {
        assert_eq!(key_answer("enter"), Some(Answer::Keep));
        assert_eq!(key_answer("space"), Some(Answer::Keep));
        assert_eq!(key_answer("escape"), Some(Answer::Cancel));
        assert_eq!(key_answer("s"), None);
    }

    #[test]
    fn quit_gate_rejects_stopping_before_save_or_after_a_stop_failure() {
        assert!(Phase::Asking.accepts(Answer::Stop, true));
        assert!(!Phase::Asking.accepts(Answer::Stop, false));
        for phase in [
            Phase::Idle,
            Phase::Checking,
            Phase::Saving,
            Phase::Stopping,
            Phase::StopFailed,
        ] {
            assert!(!phase.accepts(Answer::Stop, true));
        }
        assert!(Phase::StopFailed.accepts(Answer::Keep, true));
        assert!(Phase::StopFailed.accepts(Answer::Force, true));
        assert!(!Phase::Asking.accepts(Answer::Force, true));
        assert!(!Phase::StopFailed.accepts(Answer::Force, false));
        assert!(Phase::Saving.accepts(Answer::Cancel, true));
        assert!(!Phase::Stopping.accepts(Answer::Cancel, true));
    }
}
