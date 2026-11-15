//! Explicit, device-scoped maintenance. Inventory never connects an unselected host.
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::Call;
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::look::{self, Palette, icon};
use crate::shell::app_window::{AppWindow, quit};

#[derive(Default)]
struct Maintenance {
    blocked: BTreeSet<String>,
    active: BTreeSet<String>,
}
impl Global for Maintenance {}

pub(crate) fn host_blocked(host: &str, cx: &App) -> bool {
    cx.try_global::<Maintenance>()
        .is_some_and(|state| state.blocked.contains(host))
}
pub(crate) fn block_host(host: &str, cx: &mut App) {
    if cx.try_global::<Maintenance>().is_none() {
        cx.set_global(Maintenance::default());
    }
    cx.global_mut::<Maintenance>().blocked.insert(host.into());
}
pub(crate) fn allow_host(host: &str, cx: &mut App) -> bool {
    if cx
        .try_global::<Maintenance>()
        .is_some_and(|state| state.active.contains(host))
    {
        return false;
    }
    if cx.try_global::<Maintenance>().is_some() {
        cx.global_mut::<Maintenance>().blocked.remove(host);
    }
    true
}

fn hold_host(host: &str, cx: &mut App) -> bool {
    if cx
        .try_global::<Maintenance>()
        .is_some_and(|state| state.active.contains(host))
    {
        return false;
    }
    block_host(host, cx);
    cx.global_mut::<Maintenance>().active.insert(host.into());
    true
}

fn release_host(host: &str, cx: &mut App) {
    cx.global_mut::<Maintenance>().active.remove(host);
}

pub(crate) fn resources_stopped(value: &Value) -> bool {
    value["stopped"].as_bool() == Some(true)
        && value["remaining"].as_array().is_some_and(Vec::is_empty)
        && value["errors"].as_array().is_some_and(Vec::is_empty)
}
fn operation_succeeded(value: &Value) -> bool {
    value["success"].as_bool() == Some(true)
        && value["errors"].as_array().is_some_and(Vec::is_empty)
        && value
            .get("remaining")
            .is_none_or(|remaining| remaining.as_array().is_some_and(Vec::is_empty))
        && value.get("results").is_none_or(|results| {
            results.as_array().is_some_and(|results| {
                results.iter().all(|result| {
                    result["removed"].as_bool() != Some(false)
                        && result.get("error").is_none_or(Value::is_null)
                })
            })
        })
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string())
        })
        .collect()
}
pub(crate) fn stop_failure(value: &Value) -> String {
    let mut detail = strings(&value["errors"]);
    for resource in value["remaining"].as_array().into_iter().flatten() {
        detail.push(format!(
            "{}: {} ({})",
            text(resource, "kind"),
            text(resource, "label"),
            text(resource, "state")
        ));
    }
    if detail.is_empty() {
        detail.push("The daemon did not confirm an empty resource inventory".into());
    }
    detail.join("\n")
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").to_string()
}
fn bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024. * 1024.))
    } else {
        format!("{:.2} GiB", bytes as f64 / (1024. * 1024. * 1024.))
    }
}

fn local_inventory() -> Value {
    let mut inventory = crate::prefs::storage_inventory();
    match crate::editor::media::cache::inventory() {
        Ok(media) => {
            if let Some(entries) = inventory["entries"].as_array_mut() {
                entries.extend(media["entries"].as_array().into_iter().flatten().cloned());
            }
            if let Some(errors) = inventory["errors"].as_array_mut() {
                errors.extend(media["errors"].as_array().into_iter().flatten().cloned());
            }
        }
        Err(error) => inventory["errors"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!("Media previews: {error:#}"))),
    }
    inventory
}

fn local_categories_only(categories: &BTreeSet<String>) -> bool {
    !categories.is_empty()
        && categories
            .iter()
            .all(|category| matches!(category.as_str(), "app_settings" | "media_previews"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Stop,
    Force,
    Clean,
    Uninstall,
}
impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Stop => "Stop all background resources",
            Self::Force => "Force stop all background resources",
            Self::Clean => "Clean selected storage",
            Self::Uninstall => "Uninstall components",
        }
    }
}
struct Device {
    selected: bool,
    daemon: Option<Rc<Daemon>>,
    resources: Option<Value>,
    storage: Option<Value>,
    projects: Vec<String>,
    status: String,
    errors: Vec<String>,
    force_available: bool,
    uninstalled: bool,
}
pub(crate) struct Manager {
    devices: BTreeMap<String, Device>,
    categories: BTreeSet<String>,
    delete_data: bool,
    trash_app: bool,
    app_path: Option<String>,
    confirm: Option<Action>,
    busy: bool,
    message: Option<String>,
    /// Opened lists: a device's projects (its host) or a storage category
    /// (`host\0category`).
    expanded: BTreeSet<String>,
    focus: FocusHandle,
    focus_on_open: std::cell::Cell<bool>,
}

impl AppWindow {
    pub(crate) fn open_resource_manager(&mut self, current: String, cx: &mut Context<Self>) {
        if quit::active(cx) {
            return;
        }
        let connected = quit::connections(self, cx.entity_id(), cx);
        let hosts: BTreeSet<String> = crate::prefs::servers()
            .into_iter()
            .chain(connected.keys().cloned())
            .chain([sandhill_link::LOCAL.to_string(), current.clone()])
            .collect();
        let devices = hosts
            .into_iter()
            .map(|host| {
                let daemon = connected.get(&host).cloned();
                let status = if daemon.is_some() {
                    "Connected. Inventory not read."
                } else {
                    "Not connected. Select and read inventory to connect explicitly."
                }
                .into();
                let selected = host == current;
                let storage = (host == sandhill_link::LOCAL).then(local_inventory);
                (
                    host,
                    Device {
                        selected,
                        daemon,
                        resources: None,
                        storage,
                        projects: Vec::new(),
                        status,
                        errors: Vec::new(),
                        force_available: false,
                        uninstalled: false,
                    },
                )
            })
            .collect();
        let (app_path, message) = match crate::welcome::app_uninstall::preview_path() {
            Ok(Some(path)) => (Some(path.display().to_string()), None),
            Ok(None) => (
                None,
                Some("Current app Trash is unavailable for this development executable.".into()),
            ),
            Err(error) => (
                None,
                Some(format!("Current app Trash is unavailable: {error:#}")),
            ),
        };
        self.resource_manager = Some(Manager {
            devices,
            categories: BTreeSet::new(),
            delete_data: false,
            trash_app: false,
            app_path,
            confirm: None,
            busy: false,
            message,
            expanded: BTreeSet::new(),
            focus: cx.focus_handle(),
            focus_on_open: std::cell::Cell::new(true),
        });
        self.read_resource_inventory(false, cx);
        cx.notify();
    }

    fn read_resource_inventory(&mut self, connect_selected: bool, cx: &mut Context<Self>) {
        let Some(manager) = &mut self.resource_manager else {
            return;
        };
        if manager.busy {
            return;
        }
        manager.busy = true;
        manager.confirm = None;
        let selected: Vec<_> = manager
            .devices
            .iter_mut()
            .filter(|(_, device)| device.selected && !device.uninstalled)
            .map(|(host, device)| {
                device.errors.clear();
                device.status = "Reading inventory…".into();
                (host.clone(), device.daemon.clone())
            })
            .collect();
        let rt = self.rt.clone();
        cx.spawn(async move |this, cx| {
            for (host, existing) in selected {
                let blocked = cx.update(|cx| host_blocked(&host, cx));
                let restarting = existing.is_none() && blocked && connect_selected;
                let daemon = match existing {
                    Some(daemon) => Ok(daemon),
                    None if blocked && !connect_selected => Err("This device was stopped. Click Read selected inventories to explicitly restart only its daemon for inspection or cleanup.".into()),
                    None if connect_selected => {
                        match Target::parse(&host) {
                            Ok(target) => Daemon::connect(rt.clone(), target).await.map(|(daemon, _)| Rc::new(daemon)).map_err(|error| format!("{error:#}")),
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    None => Err("Not connected. Read selected inventories to explicitly connect this device. Connecting can install or start its backend.".into()),
                };
                let result = match daemon {
                    Ok(daemon) => {
                        let resources = daemon.call_within::<Value>(Call::ResourceInventory, Duration::from_secs(30)).await;
                        let storage = daemon.call_within::<Value>(Call::StorageInventory, Duration::from_secs(30)).await;
                        let projects = daemon.call_within::<Vec<sandhill_proto::ProjectInfo>>(Call::ListRecentProjects, Duration::from_secs(10)).await;
                        Ok((daemon, resources, storage, projects))
                    }
                    Err(error) => Err(error),
                };
                this.update(cx, |this, cx| {
                    let Some(manager) = &mut this.resource_manager else { return; };
                    let Some(device) = manager.devices.get_mut(&host) else { return; };
                    match result {
                        Ok((daemon, resources, storage, projects)) => {
                            if restarting && !allow_host(&host, cx) {
                                device.errors.push("Another window is maintaining this device; it remains blocked.".into());
                            }
                            quit::retain_daemon(host.clone(), daemon.clone(), cx);
                            device.daemon = Some(daemon);
                            device.status = "Inventory read".into();
                            match resources {
                                Ok(value) => { device.errors.extend(strings(&value["errors"])); device.resources = Some(value); }
                                Err(error) => { device.resources = None; device.errors.push(format!("Resources: {error:#}")); }
                            }
                            match storage {
                                Ok(mut value) => {
                                    if host == sandhill_link::LOCAL {
                                        let local = local_inventory();
                                        if let Some(entries) = value["entries"].as_array_mut() {
                                            entries.extend(local["entries"].as_array().into_iter().flatten().cloned());
                                        }
                                        device.errors.extend(strings(&local["errors"]));
                                    }
                                    device.errors.extend(strings(&value["errors"])); device.storage = Some(value);
                                }
                                Err(error) => {
                                    device.storage = (host == sandhill_link::LOCAL).then(local_inventory);
                                    device.errors.push(format!("Storage: {error:#}"));
                                }
                            }
                            match projects {
                                Ok(projects) => device.projects = projects.into_iter().map(|project| project.path).collect(),
                                Err(error) => device.errors.push(format!("Projects: {error:#}")),
                            }
                        }
                        Err(error) => {
                            device.status = "Not connected".into();
                            device.errors.push(error);
                            if host == sandhill_link::LOCAL { device.storage = Some(local_inventory()); }
                        }
                    }
                    cx.notify();
                }).ok();
            }
            this.update(cx, |this, cx| { if let Some(manager) = &mut this.resource_manager { manager.busy = false; } cx.notify(); }).ok();
        }).detach();
        cx.notify();
    }

    fn request_resource_action(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(manager) = &mut self.resource_manager else {
            return;
        };
        if manager.busy {
            return;
        }
        let selected: Vec<_> = manager
            .devices
            .iter()
            .filter(|(_, device)| device.selected)
            .collect();
        let local_only = action == Action::Clean && local_categories_only(&manager.categories);
        if selected.is_empty()
            || selected.iter().any(|(host, device)| {
                (device.daemon.is_none() || device.uninstalled)
                    && !(local_only && host.as_str() == sandhill_link::LOCAL)
            })
        {
            manager.message = Some("Read the inventory of every selected device first. Disconnected or uninstalled devices cannot be reported as successful.".into());
        } else if action == Action::Clean && manager.categories.is_empty() {
            manager.message = Some("Select at least one removable storage category.".into());
        } else if action == Action::Force
            && !selected.iter().any(|(_, device)| device.force_available)
        {
            manager.message = Some("Try Stop resources first. Force only targets selected devices whose graceful stop was not confirmed.".into());
        } else {
            manager.message = None;
            manager.confirm = Some(action);
        }
        cx.notify();
    }

    fn run_resource_action(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = &mut self.resource_manager else {
            return;
        };
        let Some(action) = manager.confirm.take() else {
            return;
        };
        if manager.busy {
            return;
        }
        manager.busy = true;
        manager.message = Some("Saving edited documents before maintenance…".into());
        let mut selected: Vec<_> = manager
            .devices
            .iter()
            .filter(|(_, device)| {
                device.selected && (action != Action::Force || device.force_available)
            })
            .map(|(host, device)| (host.clone(), device.daemon.clone()))
            .collect();
        // Remote errors must prevent local app settings removal and app Trash.
        selected.sort_by_key(|(host, _)| host == sandhill_link::LOCAL);
        let categories = manager.categories.clone();
        let delete_data = manager.delete_data;
        let trash_app = manager.trash_app && action == Action::Uninstall;
        let workbenches = quit::workbenches_from(self, cx.entity_id(), cx);
        let tasks: Vec<_> = workbenches
            .iter()
            .flat_map(|wb| wb.update(cx, |wb, cx| wb.save_all_tasks(cx)))
            .collect();
        cx.spawn(async move |this, cx| {
            for task in tasks { task.await; }
            let saved = cx.update(|cx| !workbenches.iter().any(|wb| wb.read(cx).any_dirty()));
            let mut all_success = saved;
            let mut local_settings_cleaned = false;
            if !saved {
                this.update(cx, |this, cx| {
                    if let Some(manager) = &mut this.resource_manager { manager.message = Some("Could not save every edited document. No maintenance request was sent.".into()); manager.busy = false; }
                    cx.notify();
                }).ok();
                return;
            }
            for (host, daemon) in selected {
                let local_only = host == sandhill_link::LOCAL && action == Action::Clean && local_categories_only(&categories);
                if Daemon::connecting(&host) {
                    all_success = false;
                    this.update(cx, |this, cx| {
                        if let Some(device) = this.resource_manager.as_mut().and_then(|manager| manager.devices.get_mut(&host)) {
                            device.status = "Not changed".into(); device.errors.push("A connection is still opening. Wait before maintenance.".into());
                        }
                        cx.notify();
                    }).ok();
                    continue;
                }
                let teardown = matches!(action, Action::Stop | Action::Force | Action::Uninstall);
                if teardown && !cx.update(|cx| hold_host(&host, cx)) {
                    all_success = false;
                    this.update(cx, |this, cx| {
                        if let Some(device) = this.resource_manager.as_mut().and_then(|manager| manager.devices.get_mut(&host)) {
                            device.status = "Not changed".into();
                            device.errors.push("Another window is already maintaining this device.".into());
                        }
                        cx.notify();
                    }).ok();
                    continue;
                }
                let call = match action {
                    Action::Stop | Action::Force => Call::StopResources { force: action == Action::Force },
                    Action::Clean => Call::CleanStorage { categories: categories.iter().filter(|category| !matches!(category.as_str(), "app_settings" | "media_previews")).cloned().collect() },
                    Action::Uninstall => Call::UninstallComponents { delete_data },
                };
                let mut result = if local_only {
                    Ok(json!({"success": true, "results": [], "errors": [], "backend_requested": false}))
                } else if let Some(daemon) = &daemon {
                    daemon.call_within::<Value>(call, Duration::from_secs(120)).await.map_err(|error| format!("{error:#}"))
                } else {
                    Err("Not connected. No backend operation was requested.".into())
                };
                let mut success = result.as_ref().is_ok_and(|value| match action {
                    Action::Stop | Action::Force => resources_stopped(value),
                    _ => operation_succeeded(value),
                });
                if host == sandhill_link::LOCAL && action == Action::Clean {
                    if categories.contains("media_previews") {
                        match crate::editor::media::cache::clean() {
                            Ok(value) => { success &= operation_succeeded(&value); if let Ok(result) = &mut result { result["local_media"] = value; } }
                            Err(error) => { success = false; result = Err(format!("Media preview cleanup: {error:#}")); }
                        }
                    }
                    if categories.contains("app_settings") {
                        if all_success && success {
                            let value = crate::prefs::clean_storage();
                            local_settings_cleaned = operation_succeeded(&value);
                            success &= local_settings_cleaned;
                            if let Ok(result) = &mut result { result["local_settings"] = value; }
                        } else {
                            success = false;
                            if let Ok(result) = &mut result { result["local_settings"] = json!({"success":false,"errors":["Preserved settings because another selected operation failed"]}); }
                        }
                    }
                }
                if teardown && success
                    && let Some(daemon) = &daemon
                    && let Err(error) = daemon.stop_confirmed().await
                {
                    success = false;
                    if let Ok(value) = &mut result { value["daemon_stop_error"] = json!(format!("{error:#}")); }
                }
                all_success &= success;
                let detach = teardown && success;
                if detach { cx.update(|cx| quit::forget_host(&host, cx)); }
                if teardown { cx.update(|cx| release_host(&host, cx)); }
                this.update(cx, |this, cx| {
                    if let Some(device) = this.resource_manager.as_mut().and_then(|manager| manager.devices.get_mut(&host)) {
                        device.status = if success { format!("{}: confirmed", action.label()) } else { format!("{}: not fully confirmed", action.label()) };
                        device.errors.clear();
                        match result {
                            Ok(value) => {
                                if matches!(action, Action::Stop | Action::Force) {
                                    device.force_available = !success;
                                    device.resources = Some(json!({"resources": value["remaining"], "errors": value["errors"]}));
                                    if !resources_stopped(&value) { device.errors.push(stop_failure(&value)); }
                                    if let Some(error) = value["daemon_stop_error"].as_str() {
                                        device.errors.push(format!("Resources stopped, but daemon stop was not confirmed: {error}"));
                                    }
                                } else {
                                    device.errors.push(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()));
                                    if action == Action::Clean { device.storage = None; }
                                }
                            }
                            Err(error) => { device.errors.push(format!("Disconnected or request failed. Completion is unknown: {error}")); device.force_available = matches!(action, Action::Stop | Action::Force); }
                        }
                        if detach {
                            device.daemon = None;
                            device.uninstalled = action == Action::Uninstall;
                        }
                    }
                    cx.notify();
                }).ok();
            }
            let trash_result = if trash_app && all_success { Some(crate::welcome::app_uninstall::move_to_trash().map_err(|error| format!("{error:#}"))) } else { None };
            if trash_result.as_ref().is_some_and(Result::is_err) { all_success = false; }
            this.update(cx, |this, cx| {
                if let Some(manager) = &mut this.resource_manager {
                    manager.busy = false;
                    manager.message = Some(match trash_result {
                        Some(Ok(path)) => format!("All selected devices confirmed uninstall. Moved {} to Trash. This running app can now be closed.", path.display()),
                        Some(Err(error)) => format!("Backend results are shown below. The app was not moved to Trash: {error}"),
                        None if trash_app => "The app was not moved to Trash because at least one selected device did not confirm success.".into(),
                        None if all_success => "Completed on every selected device. Review the per-device results below.".into(),
                        None => "Not every selected operation succeeded. No success is assumed for disconnected devices. Review the per-device results below.".into(),
                    });
                }
                cx.notify();
                if local_settings_cleaned {
                    crate::prefs::suspend_saves_for_exit();
                    quit::finish_after_cleanup(cx);
                }
            }).ok();
        }).detach();
        cx.notify();
    }

    /// Welcome's Resources and Storage page: the ticked devices' work and
    /// storage as cards, then the actions, each with what it does.
    pub(crate) fn resource_page(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(manager) = self.resource_manager.as_ref() else {
            return div()
                .text_color(p.muted)
                .child("Reading devices…")
                .into_any_element();
        };
        if manager.focus_on_open.replace(false) {
            window.focus(&manager.focus, cx);
        }
        let busy = manager.busy;
        let mut ticks = h_flex().flex_wrap().items_center().gap(px(16.));
        for (index, (host, device)) in manager.devices.iter().enumerate() {
            let host = host.clone();
            ticks = ticks.child(
                Checkbox::new(("manage-device", index))
                    .label(crate::welcome::start_screen::host_label(&host))
                    .checked(device.selected)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(manager) = &mut this.resource_manager {
                            if let Some(device) = manager.devices.get_mut(&host) {
                                device.selected = *on;
                            }
                            manager.confirm = None;
                        }
                        cx.notify();
                    })),
            );
        }
        let header = v_flex()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::BOLD)
                    .child("Resources and Storage"),
            )
            .child(div().text_size(px(14.)).text_color(p.muted).child(
                "Only the devices you tick are affected. Project folders are never removed.",
            ))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .child(ticks.flex_1().min_w_0())
                    .child(
                        Button::new("manage-inventory")
                            .small()
                            .label(if busy { "Working…" } else { "Read" })
                            .tooltip("Read the ticked devices. A device that isn't connected is connected now, which can install or start its backend.")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.read_resource_inventory(true, cx)
                            })),
                    ),
            );
        let cards: Vec<AnyElement> = manager
            .devices
            .iter()
            .enumerate()
            .filter(|(_, (_, device))| device.selected)
            .map(|(index, (host, device))| self.device_card(index, host, device, cx))
            .collect();
        let none_ticked = cards.is_empty();
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(16.))
            .track_focus(&manager.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && let Some(manager) = &mut this.resource_manager
                    && !manager.busy
                    && manager.confirm.take().is_some()
                {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(header)
            .children(cards)
            .when(none_ticked, |d| {
                d.child(
                    div()
                        .text_color(p.muted)
                        .child("Tick a device to see what runs there and what it stores."),
                )
            })
            .child(self.resource_actions(cx))
            .into_any_element()
    }

    /// One ticked device: what runs, what is stored, and its projects.
    fn device_card(
        &self,
        index: usize,
        host: &str,
        device: &Device,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let manager = self.resource_manager.as_ref().unwrap();
        let busy = manager.busy;
        let section = |title: &str, extra: String| {
            h_flex()
                .gap(px(8.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(div().text_size(px(12.5)).text_color(p.muted).child(extra))
        };
        let mut card = v_flex()
            .w_full()
            .min_w_0()
            .gap(px(12.))
            .p(px(14.))
            .rounded(px(look::RADIUS))
            .border_1()
            .border_color(p.line)
            .bg(p.panel)
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        icon(if host == sandhill_link::LOCAL {
                            "device.laptop"
                        } else {
                            "device.server"
                        })
                        .size(px(16.)),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::welcome::start_screen::host_label(host)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(p.muted)
                            .child(device.status.clone()),
                    ),
            );
        for (i, error) in device.errors.iter().enumerate() {
            // Clean and uninstall report their per-item results as JSON.
            card = card.child(if error.trim_start().starts_with('{') {
                div()
                    .id(SharedString::from(format!("manage-result-{index}-{i}")))
                    .max_h(px(160.))
                    .overflow_y_scroll()
                    .p(px(8.))
                    .rounded(px(6.))
                    .bg(p.chunk)
                    .font_family(look::MONO)
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(error.clone())
                    .into_any_element()
            } else {
                div()
                    .text_color(p.err)
                    .whitespace_normal()
                    .child(error.clone())
                    .into_any_element()
            });
        }
        // Running.
        match &device.resources {
            None => {
                card = card.child(
                    v_flex()
                        .gap(px(4.))
                        .child(section("Running", String::new()))
                        .child(
                            div()
                                .text_color(p.muted)
                                .child("Read the device to see what runs there."),
                        ),
                )
            }
            Some(resources) => {
                let resources: Vec<&Value> = resources["resources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .collect();
                let mut list = v_flex()
                    .gap(px(2.))
                    .child(section("Running", resources.len().to_string()));
                if resources.is_empty() {
                    list = list.child(div().text_color(p.muted).child("Nothing running"));
                }
                for resource in resources {
                    list = list.child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap(px(8.))
                            .py(px(4.))
                            .child(
                                icon(kind_icon(&text(resource, "kind")))
                                    .size(px(14.))
                                    .text_color(p.muted),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(text(resource, "label")),
                                    )
                                    .when_some(resource["project"].as_str(), |d, path| {
                                        d.child(
                                            div()
                                                .text_size(px(12.))
                                                .font_family(look::MONO)
                                                .text_color(p.muted)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(path.to_string()),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.5))
                                    .text_color(p.muted)
                                    .child(text(resource, "state")),
                            ),
                    );
                }
                card = card.child(list);
            }
        }
        // Storage.
        if let Some(storage) = &device.storage {
            let entries = storage["entries"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let sum = |keep: &dyn Fn(&Value) -> bool| -> u64 {
                entries
                    .iter()
                    .filter(|e| keep(e))
                    .filter_map(|e| e["bytes"].as_u64())
                    .sum()
            };
            let total = sum(&|_| true);
            let removable = sum(&|e| e["removable"].as_bool() == Some(true));
            let active = sum(&|e| e["active"].as_bool() == Some(true));
            let protected = total.saturating_sub(removable).saturating_sub(active);
            let protected_color = p.ink(28.);
            let share = |part: u64| (part as f32 / total.max(1) as f32).clamp(0., 1.);
            let legend = |color: Hsla, label: String| {
                h_flex().gap(px(5.)).child(look::dot(color)).child(label)
            };
            let mut list = v_flex()
                .gap(px(6.))
                .child(section("Storage", bytes(total)))
                .when(total > 0, |d| {
                    d.child(
                        h_flex()
                            .w_full()
                            .h(px(6.))
                            .rounded_full()
                            .overflow_hidden()
                            .bg(p.ink(8.))
                            .child(div().h_full().w(relative(share(removable))).bg(p.accent))
                            .child(div().h_full().w(relative(share(active))).bg(p.busy))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(share(protected)))
                                    .bg(protected_color),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(14.))
                        .text_size(px(12.5))
                        .text_color(p.muted)
                        .child(legend(p.accent, format!("{} removable", bytes(removable))))
                        .child(legend(p.busy, format!("{} in use", bytes(active))))
                        .child(legend(
                            protected_color,
                            format!("{} protected", bytes(protected)),
                        )),
                );
            // One row per category, its items listed when opened.
            let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
            for entry in entries {
                let category = text(entry, "category");
                match groups.iter_mut().find(|(c, _)| *c == category) {
                    Some((_, items)) => items.push(entry),
                    None => groups.push((category, vec![entry])),
                }
            }
            for (i, (category, items)) in groups.into_iter().enumerate() {
                let size: u64 = items.iter().filter_map(|e| e["bytes"].as_u64()).sum();
                let is_removable = items.iter().any(|e| e["removable"].as_bool() == Some(true));
                let in_use = items.iter().any(|e| e["active"].as_bool() == Some(true));
                let checked = manager.categories.contains(&category);
                let key = format!("{host}\u{0}{category}");
                let open = manager.expanded.contains(&key);
                let toggle = category.clone();
                let summary = if items.len() == 1 {
                    text(items[0], "path")
                } else {
                    format!("{} items", items.len())
                };
                list = list.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(p.line)
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .items_start()
                                .gap(px(10.))
                                .child(div().w(px(18.)).flex_none().pt(px(2.)).when(
                                    is_removable,
                                    |d| {
                                        d.child(
                                            Checkbox::new(SharedString::from(format!(
                                                "manage-category-{index}-{i}"
                                            )))
                                            .checked(checked)
                                            .disabled(busy)
                                            .accessibility_label(category_label(&category))
                                            .on_click(cx.listener(
                                                move |this, on: &bool, _, cx| {
                                                    if let Some(manager) =
                                                        &mut this.resource_manager
                                                    {
                                                        if *on {
                                                            manager
                                                                .categories
                                                                .insert(toggle.clone());
                                                        } else {
                                                            manager.categories.remove(&toggle);
                                                        }
                                                        manager.confirm = None;
                                                    }
                                                    cx.notify();
                                                },
                                            )),
                                        )
                                    },
                                ))
                                .child(
                                    v_flex()
                                        .id(SharedString::from(format!(
                                            "manage-storage-{index}-{i}"
                                        )))
                                        .flex_1()
                                        .min_w_0()
                                        .cursor_pointer()
                                        .child(category_label(&category))
                                        .child(
                                            h_flex()
                                                .min_w_0()
                                                .gap(px(4.))
                                                .text_size(px(12.))
                                                .text_color(p.muted)
                                                .child(
                                                    icon(if open {
                                                        "chevron.down"
                                                    } else {
                                                        "chevron.right"
                                                    })
                                                    .size(px(11.)),
                                                )
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .whitespace_nowrap()
                                                        .when(items.len() == 1, |d| {
                                                            d.font_family(look::MONO)
                                                        })
                                                        .child(summary),
                                                ),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(manager) = &mut this.resource_manager
                                                && !manager.expanded.remove(&key)
                                            {
                                                manager.expanded.insert(key.clone());
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(v_flex().flex_none().items_end().child(bytes(size)).when(
                                    !is_removable,
                                    |d| {
                                        d.child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(p.muted)
                                                .child(if in_use { "In use" } else { "Protected" }),
                                        )
                                    },
                                )),
                        )
                        .when(open, |d| {
                            d.child(v_flex().pl(px(28.)).pt(px(6.)).gap(px(6.)).children(
                                items.into_iter().map(|item| {
                                    v_flex()
                                        .min_w_0()
                                        .text_size(px(12.))
                                        .text_color(p.muted)
                                        .child(
                                            h_flex()
                                                .min_w_0()
                                                .gap(px(8.))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .whitespace_nowrap()
                                                        .font_family(look::MONO)
                                                        .child(text(item, "path")),
                                                )
                                                .child(div().flex_none().child(bytes(
                                                    item["bytes"].as_u64().unwrap_or(0),
                                                ))),
                                        )
                                        .when_some(item["reason"].as_str(), |d, reason| {
                                            d.child(
                                                div().whitespace_normal().child(reason.to_string()),
                                            )
                                        })
                                }),
                            ))
                        }),
                );
            }
            for error in storage["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                list = list.child(
                    div()
                        .text_color(p.err)
                        .child(format!("Storage measurement incomplete: {error}")),
                );
            }
            card = card.child(list);
        }
        // Projects: never removed, so one line unless opened.
        if !device.projects.is_empty() {
            let open = manager.expanded.contains(host);
            let toggle = host.to_string();
            card = card.child(
                v_flex()
                    .gap(px(4.))
                    .child(
                        h_flex()
                            .id(("manage-projects", index))
                            .gap(px(6.))
                            .cursor_pointer()
                            .text_color(p.muted)
                            .child(
                                icon(if open {
                                    "chevron.down"
                                } else {
                                    "chevron.right"
                                })
                                .size(px(12.)),
                            )
                            .child(format!(
                                "{} project{}, never removed",
                                device.projects.len(),
                                if device.projects.len() == 1 { "" } else { "s" }
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(manager) = &mut this.resource_manager
                                    && !manager.expanded.remove(&toggle)
                                {
                                    manager.expanded.insert(toggle.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .when(open, |d| {
                        d.children(device.projects.iter().map(|path| {
                            div()
                                .pl(px(18.))
                                .text_size(px(12.))
                                .font_family(look::MONO)
                                .text_color(p.muted)
                                .child(path.clone())
                        }))
                    }),
            );
        }
        card.into_any_element()
    }

    /// The actions, each beside what it does, or the confirmation of one.
    fn resource_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let manager = self.resource_manager.as_ref().unwrap();
        let busy = manager.busy;
        let mut actions = v_flex()
            .gap(px(10.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Actions"))
            .when_some(manager.message.clone(), |d, message| {
                d.child(div().text_color(p.muted).whitespace_normal().child(message))
            });
        if let Some(action) = manager.confirm {
            return actions
                .child(self.resource_confirmation(action, cx))
                .into_any_element();
        }
        let row =
            |title: &str, detail: String, extra: Option<AnyElement>, buttons: Vec<AnyElement>| {
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(px(14.))
                    .p(px(12.))
                    .rounded(px(look::RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(p.muted)
                                    .whitespace_normal()
                                    .child(detail),
                            )
                            .children(extra),
                    )
                    .child(h_flex().flex_none().gap(px(6.)).children(buttons))
            };
        let force_available = manager
            .devices
            .values()
            .any(|device| device.selected && device.force_available);
        let mut stop_buttons = vec![
            Button::new("manage-stop")
                .small()
                .label("Stop…")
                .disabled(busy)
                .on_click(
                    cx.listener(|this, _, _, cx| this.request_resource_action(Action::Stop, cx)),
                )
                .into_any_element(),
        ];
        if force_available {
            stop_buttons.push(
                Button::new("manage-force")
                    .small()
                    .danger()
                    .label("Force Stop…")
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.request_resource_action(Action::Force, cx)
                        }),
                    )
                    .into_any_element(),
            );
        }
        actions = actions.child(row(
            "Stop background work",
            "Stops kernels, terminals, jobs and agents on the ticked devices, then their backend. Nothing is deleted. Read restarts only the backend.".into(),
            None,
            stop_buttons,
        ));
        let ticked: u64 = manager
            .devices
            .values()
            .filter(|device| device.selected)
            .filter_map(|device| device.storage.as_ref())
            .flat_map(|storage| storage["entries"].as_array().into_iter().flatten())
            .filter(|entry| manager.categories.contains(&text(entry, "category")))
            .filter_map(|entry| entry["bytes"].as_u64())
            .sum();
        let clean_detail = if manager.categories.is_empty() {
            "Tick removable storage above. History needs its own tick; cleaning app settings closes Sandhill.".to_string()
        } else {
            format!(
                "{}: {} on the ticked devices. Ticked history cannot be recovered.",
                manager
                    .categories
                    .iter()
                    .map(|c| category_label(c))
                    .collect::<Vec<_>>()
                    .join(", "),
                bytes(ticked)
            )
        };
        actions =
            actions.child(row(
                "Clean ticked storage",
                clean_detail,
                None,
                vec![
                    Button::new("manage-clean")
                        .small()
                        .label("Clean…")
                        .disabled(busy || manager.categories.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.request_resource_action(Action::Clean, cx)
                        }))
                        .into_any_element(),
                ],
            ));
        let options = v_flex()
            .gap(px(6.))
            .pt(px(4.))
            .child(
                Checkbox::new("manage-delete-data")
                    .label("Also delete backend data (history and caches)")
                    .checked(manager.delete_data)
                    .disabled(busy)
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        if let Some(manager) = &mut this.resource_manager {
                            manager.delete_data = *on;
                            manager.confirm = None;
                        }
                        cx.notify();
                    })),
            )
            .when_some(manager.app_path.clone(), |d, path| {
                d.child(
                    Checkbox::new("manage-trash-app")
                        .label("Move this app to Trash afterwards")
                        .checked(manager.trash_app)
                        .disabled(busy)
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(manager) = &mut this.resource_manager {
                                manager.trash_app = *on;
                                manager.confirm = None;
                            }
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .pl(px(26.))
                        .text_size(px(12.))
                        .font_family(look::MONO)
                        .text_color(p.muted)
                        .child(path),
                )
            });
        actions = actions.child(row(
            "Uninstall Sandhill components",
            "Stops everything and removes Sandhill's backend from the ticked devices. Project folders stay. The app goes to Trash only after every ticked device succeeds.".into(),
            Some(options.into_any_element()),
            vec![
                Button::new("manage-uninstall")
                    .small()
                    .danger()
                    .label("Uninstall…")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.request_resource_action(Action::Uninstall, cx)
                    }))
                    .into_any_element(),
            ],
        ));
        actions.into_any_element()
    }

    /// What a confirmed action will do, on which devices.
    fn resource_confirmation(&self, action: Action, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let manager = self.resource_manager.as_ref().unwrap();
        let selected = manager
            .devices
            .iter()
            .filter(|(_, device)| {
                device.selected && (action != Action::Force || device.force_available)
            })
            .map(|(host, _)| crate::welcome::start_screen::host_label(host))
            .collect::<Vec<_>>()
            .join(", ");
        let detail = match action {
            Action::Stop => {
                "Stop all owned resources, including kernels, agents, terminals and jobs. Stop the daemon last, only after no resources remain. Views disconnect without reconnecting automatically. To inspect or clean storage afterward, click Read to explicitly restart only the daemon."
            }
            Action::Force => {
                "Force termination can lose in-memory work. This is a separate request after an unsuccessful graceful stop. Stop the daemon last, only after no resources remain, then disconnect all views."
            }
            Action::Clean => {
                "Delete files only in the ticked removable categories. Ticked history is irreversible. App settings cleanup closes the app."
            }
            Action::Uninstall => {
                "Stop resources and remove backend components. Every ticked device must confirm success before the current app can be moved to Trash. Reconnecting later is explicit."
            }
        };
        v_flex()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(look::RADIUS))
            .bg(p.chunk)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Confirm {} on {selected}?", action.label())),
            )
            .child(div().whitespace_normal().child(detail))
            .when(action == Action::Clean, |d| {
                d.child(div().child(format!(
                    "Ticked: {}",
                    manager
                        .categories
                        .iter()
                        .map(|c| category_label(c))
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            })
            .when(action == Action::Uninstall, |d| {
                d.child(div().child(format!(
                    "Backend data: {}. This app: {}.",
                    if manager.delete_data {
                        "DELETE"
                    } else {
                        "preserve"
                    },
                    if manager.trash_app {
                        "move to Trash after success"
                    } else {
                        "keep"
                    }
                )))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("manage-confirm-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(manager) = &mut this.resource_manager {
                                    manager.confirm = None;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("manage-confirm")
                            .small()
                            .map(|b| {
                                if matches!(action, Action::Force | Action::Uninstall) {
                                    b.danger()
                                } else {
                                    b.primary()
                                }
                            })
                            .label(format!("Confirm {}", action.label()))
                            .on_click(cx.listener(|this, _, _, cx| this.run_resource_action(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Manager {
    /// An action or inventory read is under way: the page keeps its state.
    pub(crate) fn busy(&self) -> bool {
        self.busy
    }
}

/// "Media previews" for `media_previews`.
fn category_label(category: &str) -> String {
    let words = category.replace('_', " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A resource's icon by its kind: a kernel, the Terminal, a job or an agent.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "kernel" => "cpu",
        sandhill_proto::TERMINAL_JOB => "run",
        sandhill_proto::TERMINAL_SHELL => "terminal",
        _ => "agent",
    }
}

#[cfg(test)]
mod tests {
    use super::{local_categories_only, operation_succeeded, resources_stopped};
    use serde_json::json;
    #[test]
    fn stop_requires_explicit_empty_inventory_and_no_errors() {
        assert!(resources_stopped(
            &json!({"stopped":true,"remaining":[],"errors":[]})
        ));
        assert!(!resources_stopped(
            &json!({"stopped":true,"remaining":[{"id":"kernel:1"}],"errors":[]})
        ));
        assert!(!resources_stopped(
            &json!({"stopped":true,"remaining":[],"errors":["unconfirmed"]})
        ));
        assert!(!resources_stopped(&json!({"stopped":true})));
    }
    #[test]
    fn uninstall_leftovers_or_unknown_results_are_not_success() {
        assert!(operation_succeeded(
            &json!({"success":true,"remaining":[],"errors":[]})
        ));
        assert!(!operation_succeeded(
            &json!({"success":true,"remaining":["daemon"],"errors":[]})
        ));
        assert!(!operation_succeeded(
            &json!({"success":true,"errors":["disconnected"]})
        ));
        assert!(!operation_succeeded(
            &json!({"success":true,"remaining":[],"errors":[],"results":[{"id":"daemon","removed":false}]})
        ));
        assert!(!operation_succeeded(&json!({})));
    }
    #[test]
    fn disconnected_local_cleanup_never_includes_backend_history_or_projects() {
        assert!(!local_categories_only(&Default::default()));
        assert!(local_categories_only(
            &["app_settings".to_string(), "media_previews".to_string()]
                .into_iter()
                .collect()
        ));
        assert!(!local_categories_only(
            &["app_settings".to_string(), "history".to_string()]
                .into_iter()
                .collect()
        ));
        assert!(!local_categories_only(
            &["projects".to_string()].into_iter().collect()
        ));
    }
}
