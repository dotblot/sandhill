//! The Projects page: project rows, menus, pins, and opening projects.

use super::*;

impl AppWindow {
    pub(super) fn forget_project(&mut self, host: String, path: String, cx: &mut Context<Self>) {
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

    pub(super) fn set_project_pin(
        &mut self,
        host: &str,
        path: &str,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
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
    pub(super) fn open_from_start(
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
    pub(super) fn prepare_folder_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let chosen = crate::welcome::folder_dialog::choose_local_folder("Open", cx);
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
                let parent = crate::panes::files::parent_of(&project.path);
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

    /// A project row's menu (its ⋯ button, or a right-click): save its
    /// files, stop its kernels, close it, change its icon. Actions that
    /// don't apply are dimmed.
    pub(super) fn render_project_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.start.project_menu.clone()?;
        let p = welcome_palette(cx);
        let (icon_host, icon_path) = (menu.host.clone(), menu.path.clone());
        let unsaved = menu.open.map_or(0, |id| {
            if self.welcome {
                crate::shell::app_window::main_unsaved(id, cx)
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
        let body = crate::agents::composer_view::popup(&p)
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
                                crate::shell::app_window::with_main(cx, move |main, _, cx| {
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
    pub(super) fn project_kernel_ids(&self, host: &str, path: &str) -> Vec<String> {
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
    pub(super) fn ask_stop_kernels(
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
        let name = crate::panes::files::base_name(&path).to_string();
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
    pub(super) fn ask_forget_project(
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
        let name = crate::panes::files::base_name(&path).to_string();
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
                        crate::shell::app_window::with_welcome(cx, move |welcome, _, cx| {
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
    pub(super) fn more_button(
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

    pub(super) fn show_project_menu(
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

    /// Projects: pinned ones from every connected device, then those open
    /// here, then those with live kernels. Each appears once, in the first
    /// group that has it; its status says the rest.
    pub(super) fn projects_page(&self, list_height: Pixels, cx: &mut Context<Self>) -> AnyElement {
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
                        || crate::panes::files::base_name(path).to_string(),
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
                        || match crate::panes::files::base_name(path) {
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
        let mut column = v_flex().w_full().flex_none().gap(px(16.)).child(
            v_flex()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(look::WELCOME_TITLE))
                        .font_weight(FontWeight::BOLD)
                        .child("Projects"),
                )
                .child(
                    div()
                        .text_size(px(look::TAB_TEXT))
                        .text_color(p.muted)
                        .child(if summary.is_empty() {
                            "Nothing open, running or pinned".to_string()
                        } else {
                            summary.join(" · ")
                        }),
                ),
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
                            .text_size(px(look::SMALL))
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
            column = column.child(div().text_size(px(look::SMALL)).text_color(p.muted).child(
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
                            .text_size(px(look::CHROME))
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
                            .text_size(px(look::SMALL))
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
                    .text_size(px(look::SMALL))
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

    /// The selected device's status, preferences and grouped recent projects.
    pub(super) fn host_projects(
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
                    .text_size(px(look::WELCOME_TITLE))
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
                    .text_size(px(look::TAB_TEXT))
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
                        look::button(
                            "start-open",
                            "Open Folder…",
                            Some("folder"),
                            look::ButtonSize::Welcome,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.show_start(StartMode::Picking(host.clone()), window, cx);
                            },
                        )),
                    )
                    .when(local, |d| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.muted)
                                .child("or drag folders here from Finder"),
                        )
                    }),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn recent_row(
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
                            .text_size(px(look::CHROME))
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
                            .text_size(px(look::SMALL))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(project.path.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(look::SMALL))
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

#[cfg(test)]
mod tests {
    use super::pinned_order;

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
}
