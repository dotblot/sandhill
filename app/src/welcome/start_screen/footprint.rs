//! A read-only footprint, followed by the only whole-app uninstall flow.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::time::Duration;

use gpui_kit::component::checkbox::Checkbox;
use sandhill_link::LOCAL;
use sandhill_proto::{
    Footprint, FootprintItem, FootprintKind, InstalledKey, QuickInstalled, UninstallChoice,
    UninstallPlan, UninstallReport,
};

use super::*;
use crate::shell::app_window::quit;
use crate::welcome::{app_uninstall, resource_manager as maintenance};

const READ_LIMIT: Duration = Duration::from_secs(60);
const RUN_LIMIT: Duration = Duration::from_secs(900);

#[derive(Default)]
pub(super) struct FootprintPage {
    shown: Option<String>,
    devices: BTreeMap<String, Device>,
    app: Option<AppRemoval>,
    reading_app: bool,
    delete_history: bool,
    phase: Phase,
    commands: BTreeSet<String>,
    message: Option<String>,
}

#[derive(Default)]
struct Device {
    daemon: Option<Rc<Daemon>>,
    footprint: Option<Result<Footprint, String>>,
    selected: bool,
    skipped: HashSet<InstalledKey>,
    agents: BTreeSet<String>,
}

impl Device {
    fn choice(&self, keep_history: bool) -> Option<UninstallChoice> {
        let footprint = self.footprint.as_ref()?.as_ref().ok()?;
        Some(UninstallChoice {
            installed: external_installs(footprint)
                .map(installed_key)
                .filter(|key| !self.skipped.contains(key))
                .collect(),
            agents: footprint
                .agents
                .iter()
                .filter(|agent| self.agents.contains(&agent.name))
                .map(|agent| agent.name.clone())
                .collect(),
            keep_history,
        })
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Choose,
    Reviewing,
    Review(Vec<Reviewed>),
    Running,
    Report(Vec<DeviceResult>, Option<Exit>),
}

struct Reviewed {
    host: String,
    daemon: Rc<Daemon>,
    plan: Result<UninstallPlan, String>,
}

type DeviceResult = (String, Result<UninstallReport, String>);

#[derive(Clone, Copy)]
enum Exit {
    Trash,
    Quit,
}

enum AppRemoval {
    Trash { path: String, size: Option<u64> },
    Package(&'static str),
    Development,
    Unavailable(String),
}

impl AppRemoval {
    fn read() -> Self {
        match app_uninstall::preview_path() {
            Ok(Some(path)) => Self::Trash {
                size: bundle_bytes(&path),
                path: path.display().to_string(),
            },
            Ok(None) => app_uninstall::package_removal().map_or(Self::Development, Self::Package),
            Err(error) => Self::Unavailable(format!("{error:#}")),
        }
    }

    fn line(&self) -> String {
        match self {
            Self::Trash { .. } => "When all ticked devices confirm success, you can move this app to the Trash and quit.".into(),
            Self::Package(command) => format!("To remove the app itself after quitting, run: {command}"),
            Self::Development => "This copy is running from source. Sandhill will quit without moving the executable to the Trash.".into(),
            Self::Unavailable(error) => format!("Sandhill cannot move the app to the Trash: {error}"),
        }
    }
}

fn bundle_bytes(path: &Path) -> Option<u64> {
    let meta = path.symlink_metadata().ok()?;
    if !meta.is_dir() {
        return Some(meta.len());
    }
    path.read_dir().ok()?.try_fold(0_u64, |sum, entry| {
        Some(sum.saturating_add(bundle_bytes(&entry.ok()?.path())?))
    })
}

// Keep the window and the local connection in place until the report has been read.
struct UninstallGuard;
impl Global for UninstallGuard {}

pub(crate) fn uninstall_running(cx: &App) -> bool {
    cx.has_global::<UninstallGuard>()
}

impl AppWindow {
    fn sync_footprint(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.start.footprint.phase, Phase::Choose) {
            return;
        }
        let mut connected = quit::connections(self, cx.entity_id(), cx);
        connected
            .retain(|host, daemon| !daemon.is_closed() && !maintenance::host_blocked(host, cx));
        let page = &mut self.start.footprint;
        page.devices.entry(LOCAL.into()).or_insert_with(|| Device {
            selected: true,
            ..Device::default()
        });
        page.devices.retain(|host, device| {
            host == LOCAL || device.selected || connected.contains_key(host)
        });
        for host in connected.keys() {
            if !page.devices.contains_key(host) {
                page.devices.insert(host.clone(), Device::default());
            }
        }
        let mut read = Vec::new();
        for (host, device) in &mut page.devices {
            let live = connected.remove(host);
            if !same_connection(&device.daemon, &live) {
                device.footprint = None;
                device.daemon = live;
                if let Some(daemon) = &device.daemon {
                    read.push((host.clone(), daemon.clone()));
                }
            }
        }
        for (host, daemon) in read {
            self.read_footprint(host, daemon, cx);
        }
        let page = &mut self.start.footprint;
        if page.app.is_none() && !page.reading_app {
            page.reading_app = true;
            let read = cx.background_spawn(async { AppRemoval::read() });
            cx.spawn(async move |this, cx| {
                let app = read.await;
                this.update(cx, |this, cx| {
                    this.start.footprint.app = Some(app);
                    this.start.footprint.reading_app = false;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn read_footprint(&mut self, host: String, daemon: Rc<Daemon>, cx: &mut Context<Self>) {
        let read = daemon.call_within::<Footprint>(Call::Footprint, READ_LIMIT);
        cx.spawn(async move |this, cx| {
            let reply = read
                .await
                .map_err(|error| format!("Couldn't read this device: {error}"));
            this.update(cx, |this, cx| {
                if let Some(device) = this.start.footprint.devices.get_mut(&host)
                    && device
                        .daemon
                        .as_ref()
                        .is_some_and(|live| Rc::ptr_eq(live, &daemon))
                {
                    device.footprint = Some(reply);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn review_uninstall(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.start.footprint;
        if !matches!(page.phase, Phase::Choose) || quit::active(cx) || uninstall_running(cx) {
            return;
        }
        let Some(targets) = page
            .devices
            .iter()
            .filter(|(_, device)| device.selected)
            .map(|(host, device)| {
                Some((
                    host.clone(),
                    device.daemon.clone()?,
                    device.choice(!page.delete_history)?,
                ))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if targets.is_empty() || page.app.is_none() {
            return;
        }
        page.phase = Phase::Reviewing;
        page.commands.clear();
        page.message = None;
        self.start.content_scroll.set_offset(Point::default());
        let requests = targets
            .into_iter()
            .map(|(host, daemon, choice)| {
                let request =
                    daemon.call_within::<UninstallPlan>(Call::PlanUninstall { choice }, READ_LIMIT);
                (host, daemon, request)
            })
            .collect::<Vec<_>>();
        cx.spawn(async move |this, cx| {
            let mut plans = Vec::with_capacity(requests.len());
            for (host, daemon, request) in requests {
                let plan = request.await.map_err(|error| {
                    format!(
                        "Couldn't review this device. Check its connection and try again: {error}"
                    )
                });
                plans.push(Reviewed { host, daemon, plan });
            }
            this.update(cx, |this, cx| {
                this.start.footprint.phase = Phase::Review(plans);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run_uninstall(&mut self, cx: &mut Context<Self>) {
        if quit::active(cx) || uninstall_running(cx) {
            return;
        }
        let page = &mut self.start.footprint;
        if self
            .resource_manager
            .as_ref()
            .is_some_and(maintenance::Manager::busy)
        {
            page.message = Some("Wait for Resources and Storage to finish, then try again.".into());
            cx.notify();
            return;
        }
        let Phase::Review(reviewed) = &page.phase else {
            return;
        };
        if !reviewed.iter().all(can_run) {
            return;
        }
        let Phase::Review(mut reviewed) = std::mem::replace(&mut page.phase, Phase::Running) else {
            unreachable!();
        };
        reviewed.sort_by_key(|review| review.host == LOCAL);
        page.message = None;
        let packaged = matches!(page.app, Some(AppRemoval::Trash { .. }));
        cx.set_global(UninstallGuard);
        cx.refresh_windows();
        let workbenches = quit::workbenches_from(self, cx.entity_id(), cx);
        let saves: Vec<_> = workbenches
            .iter()
            .flat_map(|wb| wb.update(cx, |wb, cx| wb.save_all_tasks(cx)))
            .collect();
        cx.spawn(async move |this, cx| {
            for save in saves {
                save.await;
            }
            if cx.update(|cx| workbenches.iter().any(|wb| wb.read(cx).any_dirty())) {
                cx.update(|cx| {
                    cx.remove_global::<UninstallGuard>();
                    cx.refresh_windows();
                });
                this.update(cx, |this, cx| {
                    this.start.footprint.phase = Phase::Review(reviewed);
                    this.start.footprint.message = Some("Some edited files couldn't be saved. Nothing was removed. Save the files and try again.".into());
                    cx.notify();
                }).ok();
                return;
            }
            let mut results: Vec<DeviceResult> = Vec::with_capacity(reviewed.len());
            let mut exit = None;
            for Reviewed { host, daemon, plan } in reviewed {
                let local = host == LOCAL;
                if local && !results.iter().all(|(_, result)| confirmed(result)) {
                    results.push((host, Err("Not started. Check the server reports before uninstalling from this computer.".into())));
                    continue;
                }
                if daemon.is_closed() {
                    results.push((host, Err("Not started. This device went offline. Connect and review again.".into())));
                    continue;
                }
                if Daemon::connecting(&host) || !cx.update(|cx| maintenance::hold_host(&host, cx)) {
                    results.push((host, Err("Not started. Wait for the connection or other maintenance to finish, then review again.".into())));
                    continue;
                }
                // Remember before sending the request, in case the reply is lost as the server shuts down.
                if let Err(error) = crate::prefs::set_server_uninstalled(&host, true) {
                    cx.update(|cx| {
                        maintenance::release_host(&host, cx);
                        maintenance::allow_host(&host, cx);
                    });
                    results.push((host, Err(format!("Not started. {error}"))));
                    continue;
                }
                let result = daemon.call_within::<UninstallReport>(Call::RunUninstall {
                    choice: plan.expect("only approved plans run").choice,
                    app_pid: local.then(std::process::id),
                }, RUN_LIMIT).await;
                let may_be_finishing = match &result {
                    Ok(report) => !report.finishing.is_empty(),
                    Err(error) => !matches!(error.downcast_ref(), Some(sandhill_proto::client::ClientError::Remote(_))),
                };
                let result = result.map_err(|error| {
                    if let Some(sandhill_proto::client::ClientError::Remote(reason)) = error.downcast_ref() {
                        reason.clone()
                    } else {
                        format!("Removal could not be confirmed: {error}. Check this device before trying again.")
                    }
                });
                if local && (confirmed(&result) || may_be_finishing) {
                    exit = Some(if packaged && confirmed(&result) { Exit::Trash } else { Exit::Quit });
                    crate::prefs::suspend_saves_for_exit();
                }
                cx.update(|cx| {
                    if !local || exit.is_some() {
                        quit::forget_host(&host, cx);
                    }
                    if !local || exit.is_none() {
                        maintenance::release_host(&host, cx);
                        if local {
                            maintenance::allow_host(&host, cx);
                        }
                    }
                });
                results.push((host, result));
            }
            if exit.is_none() {
                cx.update(|cx| {
                    cx.remove_global::<UninstallGuard>();
                    cx.refresh_windows();
                });
            }
            this.update(cx, |this, cx| {
                for (host, _) in &results {
                    if let Some(device) = this.start.footprint.devices.get_mut(host) {
                        device.daemon = None;
                        device.footprint = None;
                    }
                }
                this.start.footprint.phase = Phase::Report(results, exit);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn finish_uninstall(&mut self, exit: Exit, cx: &mut Context<Self>) {
        if matches!(exit, Exit::Trash)
            && let Err(error) = app_uninstall::move_to_trash()
        {
            self.start.footprint.message = Some(format!(
                "The app wasn't moved to the Trash: {error:#}. Quit and move it to the Trash yourself."
            ));
            if let Phase::Report(_, pending) = &mut self.start.footprint.phase {
                *pending = Some(Exit::Quit);
            }
            cx.notify();
            return;
        }
        quit::finish_after_cleanup(cx);
    }

    pub(super) fn footprint_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.sync_footprint(cx);
        let p = welcome_palette(cx);
        let page = &self.start.footprint;
        let current = page.shown.as_deref().unwrap_or(&self.start.selected_host);
        let current = if page.devices.contains_key(current) {
            current
        } else {
            LOCAL
        };
        let mut selectors = h_flex().flex_wrap().gap(px(look::GAP_6));
        for (index, (host, device)) in local_first(&page.devices).enumerate() {
            if host != LOCAL && device.daemon.is_none() && host != current {
                continue;
            }
            let picked = host.clone();
            selectors = selectors.child(
                Button::new(("footprint-device", index))
                    .small()
                    .when(host == current, |b| b.primary())
                    .label(host_label(host))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start.footprint.shown = Some(picked.clone());
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(look::GAP_12))
            .child(
                div()
                    .text_size(px(look::welcome_title()))
                    .font_weight(FontWeight::BOLD)
                    .whitespace_normal()
                    .child("What Sandhill Has Put on This Computer"),
            )
            .when(matches!(page.phase, Phase::Choose), |d| {
                d.child(selectors)
                    .child(self.footprint_inventory(current, &p, cx))
            })
            .child(self.uninstall_card(&p, cx))
            .into_any_element()
    }

    fn footprint_inventory(&self, host: &str, p: &Palette, cx: &mut Context<Self>) -> Div {
        let local = host == LOCAL;
        let page = &self.start.footprint;
        let Some(device) = page.devices.get(host) else {
            return div();
        };
        let reading = device.daemon.is_some() && device.footprint.is_none();
        let reload = device.daemon.as_ref().map(|daemon| {
            let host = host.to_owned();
            let daemon = daemon.clone();
            Button::new("footprint-reload")
                .small()
                .label("Read Again")
                .disabled(reading || !matches!(page.phase, Phase::Choose))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(device) = this.start.footprint.devices.get_mut(&host) {
                        device.footprint = None;
                    }
                    this.read_footprint(host.clone(), daemon.clone(), cx);
                    cx.notify();
                }))
        });
        let mut inventory = v_flex().min_w_0().gap(px(look::GAP_12));
        let footprint = match &device.footprint {
            Some(Ok(footprint)) => footprint,
            other => return inventory.child(h_flex().gap(px(look::GAP_8))
                .child(div().flex_1().min_w_0().text_color(p.muted).whitespace_normal().child(match other {
                    Some(Err(error)) => error.clone(),
                    _ if reading => "Reading what Sandhill put here…".into(),
                    _ => "This device isn't connected. Connect to see what Sandhill put here.".into(),
                }))
                .children(reload)),
        };
        let bundle = if local { page.app.as_ref() } else { None };
        let total = footprint.size_mb.saturating_add(match bundle {
            Some(AppRemoval::Trash {
                size: Some(bytes), ..
            }) => bytes.div_ceil(1024 * 1024),
            _ => 0,
        });
        inventory = inventory.child(
            h_flex()
                .w_full()
                .gap(px(look::GAP_8))
                .child(div().flex_1().min_w_0().whitespace_normal().child(format!(
                    "Sandhill's folder: {}. Listed files: {}.",
                    footprint.folder,
                    size_mb(total)
                )))
                .when(local, |d| {
                    let folder = footprint.folder.clone();
                    d.child(
                        Button::new("footprint-reveal-folder")
                            .small()
                            .label(look::mac_or("Show in Finder", "Show in File Manager"))
                            .on_click(move |_, _, cx| cx.reveal_path(Path::new(&folder))),
                    )
                })
                .children(reload),
        );
        for (kind, label) in [
            (FootprintKind::Folder, "Sandhill's folder"),
            (FootprintKind::System, "Placed by the system"),
            (FootprintKind::AgentSetting, "Agent settings"),
            (FootprintKind::OldPlace, "Old places"),
        ] {
            let rows: Vec<_> = footprint
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.kind == kind)
                .map(|(index, item)| item_row(("footprint-place", index), item, local, p))
                .collect();
            if !rows.is_empty() {
                inventory = inventory.child(look::settings_group(label, rows, p));
            }
        }
        if let Some(AppRemoval::Trash { path, size }) = bundle {
            inventory = inventory.child(look::settings_group(
                "The app",
                vec![path_row(
                    "footprint-bundle",
                    "Sandhill.app",
                    path,
                    size.map(bundle_size),
                    "",
                    true,
                    p,
                )],
                p,
            ));
        }
        let mut installed_rows: Vec<_> = external_installs(footprint)
            .enumerate()
            .map(|(index, item)| install_row(("footprint-installed", index), item, local, p))
            .collect();
        if installed_rows.is_empty() {
            installed_rows
                .push(look::settings_row("No installs recorded", None).into_any_element());
        }
        if local {
            installed_rows.push(
                look::settings_row("Remove only some of these installs", None)
                    .child(
                        Button::new("footprint-what-installed")
                            .small()
                            .label("What Sandhill Installed")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.quick.forget_check();
                                this.start.show_install_record = true;
                                this.show_page(Page::Ready, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        inventory
            .child(look::settings_group(
                "Installed into your R and Python",
                installed_rows,
                p,
            ))
            .child(look::settings_group(
                "Agents Sandhill installed",
                if footprint.agents.is_empty() {
                    vec![look::settings_row("No agents recorded", None).into_any_element()]
                } else {
                    footprint
                        .agents
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            install_row(("footprint-agent", index), item, local, p)
                        })
                        .collect()
                },
                p,
            ))
    }

    fn uninstall_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let page = &self.start.footprint;
        let card = v_flex()
            .min_w_0()
            .gap(px(look::GAP_12))
            .p(px(look::PANE_PAD_X))
            .rounded(px(look::RADIUS))
            .border_1()
            .border_color(p.line)
            .child(title("Uninstall Sandhill"))
            .when_some(page.message.as_ref(), |d, message| {
                d.child(
                    div()
                        .text_color(p.err)
                        .whitespace_normal()
                        .child(message.clone()),
                )
            });
        match &page.phase {
            Phase::Choose => card.child(self.uninstall_choices(p, cx)),
            Phase::Reviewing => card.child("Preparing a review for each ticked device…"),
            Phase::Review(reviewed) => card.child(self.uninstall_review(reviewed, p, cx)),
            Phase::Running => card.child(
                "Saving edited files and uninstalling Sandhill. Please keep this window open.",
            ),
            Phase::Report(results, exit) => {
                card.child(self.uninstall_report(results, *exit, p, cx))
            }
        }
    }

    fn uninstall_choices(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let page = &self.start.footprint;
        let mut ticks = h_flex().flex_wrap().gap(px(look::GAP_8));
        let mut lists = v_flex().gap(px(look::GAP_12));
        let mut ready = true;
        let mut selected = 0;
        for (index, (host, device)) in local_first(&page.devices).enumerate() {
            let picked = host.clone();
            ticks = ticks.child(
                Checkbox::new(("uninstall-device", index))
                    .label(host_label(host))
                    .checked(device.selected)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(device) = this.start.footprint.devices.get_mut(&picked) {
                            device.selected = *on;
                            cx.notify();
                        }
                    })),
            );
            if !device.selected {
                continue;
            }
            selected += 1;
            let Some(Ok(footprint)) = &device.footprint else {
                ready = false;
                let state = if device.daemon.is_none() {
                    "Not connected. Connect or untick this device."
                } else if device.footprint.is_some() {
                    "Couldn't read the installs. Choose this device above and click Read Again."
                } else {
                    "Reading its installs…"
                };
                lists = lists.child(look::settings_detail(
                    format!("{}: {state}", host_label(host)),
                    p,
                ));
                continue;
            };
            let id =
                |part: &str, i: usize| SharedString::from(format!("uninstall-{index}-{part}-{i}"));
            let mut installs = v_flex().gap(px(look::GAP_6)).child(title(host_label(host)));
            let mut external = external_installs(footprint).enumerate().peekable();
            if external.peek().is_some() {
                installs =
                    installs.child(look::settings_detail("Remove from your R and Python", p));
                for (i, item) in external {
                    let key = installed_key(item);
                    let tick = !device.skipped.contains(&key);
                    let picked = host.clone();
                    installs = installs.child(
                        Checkbox::new(id("install", i))
                            .label(install_choice_label(item))
                            .checked(tick)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                if let Some(device) = this.start.footprint.devices.get_mut(&picked)
                                {
                                    if *on {
                                        device.skipped.remove(&key);
                                    } else {
                                        device.skipped.insert(key.clone());
                                    }
                                    cx.notify();
                                }
                            })),
                    );
                }
            }
            if !footprint.agents.is_empty() {
                installs = installs.child(look::settings_detail(
                    "Remove agents Sandhill installed. Your sign-ins stay.",
                    p,
                ));
                for (i, item) in footprint.agents.iter().enumerate() {
                    let picked = host.clone();
                    let agent = item.name.clone();
                    installs = installs.child(
                        Checkbox::new(id("agent", i))
                            .label(install_choice_label(item))
                            .checked(device.agents.contains(&agent))
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                if let Some(device) = this.start.footprint.devices.get_mut(&picked)
                                {
                                    if *on {
                                        device.agents.insert(agent.clone());
                                    } else {
                                        device.agents.remove(&agent);
                                    }
                                    cx.notify();
                                }
                            })),
                    );
                }
            }
            if external_installs(footprint).next().is_none() && footprint.agents.is_empty() {
                installs =
                    installs.child(look::settings_detail("No optional installs to remove.", p));
            }
            lists = lists.child(installs);
        }
        let local_selected = page
            .devices
            .get(LOCAL)
            .is_some_and(|device| device.selected);
        v_flex().gap(px(look::GAP_12))
            .child(div().whitespace_normal().child("Sandhill stops background work and removes its files from the ticked devices. Your projects, data and results stay."))
            .child(title("Devices"))
            .child(ticks)
            .child(lists)
            .child(Checkbox::new("uninstall-delete-history")
                .label("Also delete Sandhill's history (conversations, file history, outputs)?")
                .checked(page.delete_history)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.start.footprint.delete_history = *on;
                    cx.notify();
                })))
            .child(look::settings_detail(if local_selected {
                page.app.as_ref().map_or_else(|| "Checking this copy of Sandhill…".into(), AppRemoval::line)
            } else {
                "Sandhill stays on this computer.".into()
            }, p))
            .child(h_flex().justify_end().child(Button::new("uninstall-review").small().primary()
                .label("Review…").disabled(!ready || selected == 0 || page.app.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.review_uninstall(cx)))))
    }

    fn uninstall_review(&self, reviewed: &[Reviewed], p: &Palette, cx: &mut Context<Self>) -> Div {
        let page = &self.start.footprint;
        let mut review = v_flex()
            .gap(px(look::GAP_12))
            .child(title("Review before uninstalling"));
        for (index, Reviewed { host, plan, .. }) in reviewed.iter().enumerate() {
            let mut device = v_flex().gap(px(look::GAP_8)).child(title(host_label(host)));
            match plan {
                Ok(plan) => {
                    let local_app = (host == LOCAL).then_some(page.app.as_ref()).flatten();
                    device = device
                        .child(plan_items("Will remove", &plan.remove, local_app, p))
                        .child(plan_items("Will keep", &plan.keep, None, p))
                        .children(
                            plan.notes
                                .iter()
                                .map(|note| look::settings_detail(note.clone(), p)),
                        )
                        .when_some(plan.refused.as_ref(), |d, reason| {
                            d.child(
                                div()
                                    .text_color(p.err)
                                    .whitespace_normal()
                                    .child(reason.clone()),
                            )
                        });
                    if !plan.steps.is_empty() {
                        let shown = page.commands.contains(host);
                        let target = host.clone();
                        device = device.child(
                            h_flex().child(
                                Button::new(("uninstall-commands", index))
                                    .small()
                                    .label(if shown {
                                        "Hide commands"
                                    } else {
                                        "Show commands"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let commands = &mut this.start.footprint.commands;
                                        if !commands.remove(&target) {
                                            commands.insert(target.clone());
                                        }
                                        cx.notify();
                                    })),
                            ),
                        );
                        if shown {
                            device = device.children(plan.steps.iter().map(|step| {
                                v_flex()
                                    .min_w_0()
                                    .gap(px(look::GAP_4))
                                    .child(look::settings_detail(step.what.clone(), p))
                                    .child(crate::install_confirm::command_block(&step.command, p))
                            }));
                        }
                    }
                }
                Err(error) => {
                    device = device.child(
                        div()
                            .text_color(p.err)
                            .whitespace_normal()
                            .child(error.clone()),
                    );
                }
            }
            review = review.child(device);
        }
        review.child(
            h_flex()
                .justify_end()
                .gap(px(look::GAP_8))
                .child(
                    Button::new("uninstall-cancel")
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.reset_uninstall_review(cx))),
                )
                .child(
                    Button::new("uninstall-confirm")
                        .small()
                        .danger()
                        .label("Uninstall")
                        .disabled(!reviewed.iter().all(can_run))
                        .on_click(cx.listener(|this, _, _, cx| this.run_uninstall(cx))),
                ),
        )
    }

    fn reset_uninstall_review(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.start.footprint;
        if matches!(page.phase, Phase::Report(..)) {
            for device in page.devices.values_mut() {
                device.selected = false;
            }
            if let Some(local) = page.devices.get_mut(LOCAL) {
                local.selected = true;
            }
            page.delete_history = false;
        }
        page.message = None;
        page.phase = Phase::Choose;
        self.start.content_scroll.set_offset(Point::default());
        cx.notify();
    }

    fn uninstall_report(
        &self,
        results: &[DeviceResult],
        exit: Option<Exit>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let local = results.iter().find(|(host, _)| host == LOCAL);
        let all_done = results.iter().all(|(_, result)| confirmed(result));
        let mut panel = v_flex()
            .gap(px(look::GAP_12))
            .child(title("Uninstall report"));
        for (host, result) in results {
            let mut device = v_flex().gap(px(look::GAP_6)).child(title(host_label(host)));
            match result {
                Ok(report) => {
                    if !report.removed.is_empty() {
                        device = device.child(path_list("Removed", &report.removed, p));
                    }
                    if !report.left.is_empty() {
                        device = device.child(plan_items("Couldn't remove", &report.left, None, p));
                    } else if !report.success {
                        device = device.child(look::settings_detail(
                            "This device did not confirm removal.",
                            p,
                        ));
                    }
                    if !report.finishing.is_empty() {
                        device = device.child(path_list(
                            if host == LOCAL {
                                "Finishes after Sandhill quits"
                            } else {
                                "Scheduled for removal after the server connection closes"
                            },
                            &report.finishing,
                            p,
                        ));
                    }
                }
                Err(error) => {
                    device = device.child(
                        div()
                            .text_color(p.err)
                            .whitespace_normal()
                            .child(error.clone()),
                    );
                }
            }
            panel = panel.child(device);
        }
        if results.iter().any(|(_, result)| {
            result
                .as_ref()
                .is_ok_and(|report| !report.finishing.is_empty())
        }) {
            panel = panel.child(look::settings_detail(
                "If anything cannot be removed after Sandhill quits, look for sandhill-uninstall-leftovers.txt on that device's Desktop (Mac) or in its home folder (Linux).", p));
        }
        if local.is_none() {
            let servers = results
                .iter()
                .filter(|(_, result)| confirmed(result))
                .map(|(host, _)| host_label(host))
                .collect::<Vec<_>>();
            panel = panel.child(look::settings_detail(
                if servers.is_empty() {
                    "Sandhill stays on this computer.".into()
                } else {
                    format!(
                        "Sandhill stays on this computer. Removal was confirmed on {}.",
                        servers.join(", ")
                    )
                },
                p,
            ));
        } else if !all_done {
            panel = panel.child(look::settings_detail("The app stays in place because not every ticked device confirmed removal. Review the reasons above before trying again.", p));
        } else if let Some(app) = &self.start.footprint.app {
            panel = panel.child(look::settings_detail(app.line(), p));
        }
        panel.child(
            h_flex().justify_end().child(match exit {
                Some(exit) => Button::new("uninstall-finish")
                    .small()
                    .primary()
                    .label(match exit {
                        Exit::Trash => "Move to Trash and Quit",
                        Exit::Quit => "Quit Sandhill",
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.finish_uninstall(exit, cx))),
                None => Button::new("uninstall-done")
                    .small()
                    .label("Done")
                    .on_click(cx.listener(|this, _, _, cx| this.reset_uninstall_review(cx))),
            }),
        )
    }
}

fn can_run(review: &Reviewed) -> bool {
    review
        .plan
        .as_ref()
        .is_ok_and(|plan| plan.refused.is_none())
}

fn confirmed(result: &Result<UninstallReport, String>) -> bool {
    result
        .as_ref()
        .is_ok_and(|report| report.success && report.left.is_empty())
}

fn same_connection(a: &Option<Rc<Daemon>>, b: &Option<Rc<Daemon>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn local_first(devices: &BTreeMap<String, Device>) -> impl Iterator<Item = (&String, &Device)> {
    devices
        .get_key_value(LOCAL)
        .into_iter()
        .chain(devices.iter().filter(|(host, _)| host.as_str() != LOCAL))
}

fn external_installs(footprint: &Footprint) -> impl Iterator<Item = &QuickInstalled> {
    footprint.installed.iter().filter(|item| {
        !matches!(
            item.kind.as_str(),
            "agent" | "own-environment" | "agent-folder" | "agent-setting"
        )
    })
}

fn installed_key(item: &QuickInstalled) -> InstalledKey {
    InstalledKey {
        kind: item.kind.clone(),
        name: item.name.clone(),
        place: item.place.clone(),
    }
}

fn install_name(item: &QuickInstalled) -> &str {
    match (item.kind.as_str(), item.name.as_str()) {
        ("agent", "claude") => "Claude Code",
        ("agent", "codex") => "Codex",
        (_, name) => name,
    }
}

fn install_choice_label(item: &QuickInstalled) -> String {
    let name = install_name(item);
    if item.kind != "agent" && !item.place.is_empty() {
        format!("{name} ({})", item.place)
    } else {
        name.into()
    }
}

fn install_row(
    id: impl Into<ElementId>,
    item: &QuickInstalled,
    local: bool,
    p: &Palette,
) -> AnyElement {
    let name = if item.version.is_empty() {
        install_name(item).into()
    } else {
        format!("{} {}", install_name(item), item.version)
    };
    let tool = match item.tool.as_str() {
        "brew" => "Homebrew",
        "r" => "R's own installer",
        "conda" => "mamba or conda",
        "native" => "Official installer",
        tool => tool,
    };
    let changed = if item.ours {
        ""
    } else {
        "Changed since Sandhill installed it. It stays."
    };
    let note = [tool, &item.date, &item.reason, changed]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    path_row(id, name, &item.place, None, &note, local, p)
}

fn item_row(
    id: impl Into<ElementId>,
    item: &FootprintItem,
    local: bool,
    p: &Palette,
) -> AnyElement {
    path_row(
        id,
        item.what.clone(),
        &item.path,
        item.size_mb.map(size_mb),
        &item.note,
        local,
        p,
    )
}

fn path_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    path: &str,
    size: Option<String>,
    note: &str,
    local: bool,
    p: &Palette,
) -> AnyElement {
    let path_text = path.to_owned();
    let details = v_flex()
        .gap(px(look::GAP_2))
        .when(!path.is_empty(), |d| {
            d.child(path_text_line(path_text.clone(), p))
        })
        .when(!note.is_empty(), |d| {
            d.child(look::settings_detail(note.to_owned(), p))
        });
    look::settings_row(name, Some(details))
        .children(size.map(|size| look::settings_detail(size, p).flex_none()))
        .when(local && Path::new(path).is_absolute(), |d| {
            d.child(
                Button::new(id)
                    .small()
                    .label(look::mac_or("Show in Finder", "Show in File Manager"))
                    .on_click(move |_, _, cx| cx.reveal_path(Path::new(&path_text))),
            )
        })
        .into_any_element()
}

fn plan_items(
    label: &'static str,
    items: &[FootprintItem],
    app: Option<&AppRemoval>,
    p: &Palette,
) -> Div {
    v_flex()
        .gap(px(look::GAP_4))
        .child(title(label))
        .when(items.is_empty() && app.is_none(), |d| {
            d.child(look::settings_detail("Nothing", p))
        })
        .children(items.iter().map(|item| {
            v_flex()
                .min_w_0()
                .child(div().whitespace_normal().child(item.what.clone()))
                .when(!item.path.is_empty(), |d| {
                    d.child(path_text_line(item.path.clone(), p))
                })
                .when(!item.note.is_empty(), |d| {
                    d.child(look::settings_detail(item.note.clone(), p))
                })
        }))
        .when_some(app, |d, app| match app {
            AppRemoval::Trash { path, .. } => d
                .child("Sandhill.app")
                .child(path_text_line(path.clone(), p)),
            _ => d.child(look::settings_detail(app.line(), p)),
        })
}

fn path_list(label: &'static str, paths: &[String], p: &Palette) -> Div {
    v_flex()
        .gap(px(look::GAP_4))
        .child(title(label))
        .children(paths.iter().map(|path| path_text_line(path.clone(), p)))
}

fn path_text_line(text: String, p: &Palette) -> Div {
    look::settings_detail(text, p).font_family(look::mono())
}

fn title(text: impl Into<SharedString>) -> Div {
    div()
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_normal()
        .child(text.into())
}

fn size_mb(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.)
    } else if mb == 0 {
        "Less than 1 MB".into()
    } else {
        format!("{mb} MB")
    }
}

fn bundle_size(bytes: u64) -> String {
    size_mb(bytes / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::{Device, confirmed, installed_key};
    use sandhill_proto::{
        Footprint, FootprintItem, FootprintKind, QuickInstalled, UninstallChoice, UninstallReport,
    };

    fn installed(kind: &str, name: &str) -> QuickInstalled {
        QuickInstalled {
            kind: kind.into(),
            name: name.into(),
            version: String::new(),
            place: "/example".into(),
            ours: true,
            date: String::new(),
            tool: String::new(),
            reason: String::new(),
        }
    }

    #[test]
    fn choices_default_to_external_installs_but_leave_agents_and_history() {
        let package = installed("r-package", "jsonlite");
        let key = installed_key(&package);
        let mut device = Device {
            footprint: Some(Ok(Footprint {
                folder: "/example/.sandhill".into(),
                size_mb: 1,
                items: Vec::new(),
                installed: vec![package, installed("own-environment", "sandhill")],
                agents: vec![installed("agent", "codex")],
            })),
            ..Device::default()
        };
        assert_eq!(
            device.choice(true),
            Some(UninstallChoice {
                installed: vec![key.clone()],
                agents: Vec::new(),
                keep_history: true,
            })
        );
        device.skipped.insert(key);
        device.agents.insert("codex".into());
        assert_eq!(
            device.choice(false),
            Some(UninstallChoice {
                installed: Vec::new(),
                agents: vec!["codex".into()],
                keep_history: false,
            })
        );
    }

    #[test]
    fn trash_requires_confirmed_removal_without_leftovers() {
        let mut report = UninstallReport {
            removed: Vec::new(),
            left: Vec::new(),
            finishing: vec!["/example/.sandhill".into()],
            success: false,
        };
        assert!(!confirmed(&Err("Offline".into())));
        assert!(!confirmed(&Ok(report.clone())));
        report.success = true;
        assert!(confirmed(&Ok(report.clone())));
        report.left.push(FootprintItem {
            path: "/example/left".into(),
            what: "An install".into(),
            kind: FootprintKind::OldPlace,
            size_mb: None,
            note: "Could not remove".into(),
        });
        assert!(!confirmed(&Ok(report)));
    }
}
