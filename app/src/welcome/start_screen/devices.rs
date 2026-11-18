//! Device (machine) connection, server, and rename handling for the start screen.

use super::*;

impl AppWindow {
    /// Render-time connects respect per-server preferences; explicit actions do not.
    pub(super) fn ensure_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        if auto_connect_host(host, &self.start.auto_connect) {
            self.connect_machine(host, cx);
        }
    }

    pub(super) fn connect_machine(&mut self, host: &str, cx: &mut Context<Self>) {
        if crate::shell::app_window::quit::active(cx)
            || crate::welcome::resource_manager::host_blocked(host, cx)
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

    pub(super) fn welcome_connected(
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

    pub(super) fn reconnect_welcome(&mut self, host: &str, cx: &mut Context<Self>) {
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

    pub(super) fn disconnect_welcome(&mut self, host: &str, cx: &mut Context<Self>) {
        if host == sandhill_link::LOCAL || crate::shell::app_window::host_has_project(host, cx) {
            return;
        }
        // Dropping the attach process closes only this link, not the daemon.
        self.start
            .set_machine(host.into(), MachineLink::Disconnected);
        cx.notify();
    }

    pub(super) fn machine_daemon(&self, host: &str) -> Option<Rc<Daemon>> {
        match &self.start.machines.get(host)?.link {
            MachineLink::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    pub(super) fn add_server(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn remove_server(&mut self, host: &str, cx: &mut Context<Self>) {
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

    pub(super) fn welcome_connection_button(
        &self,
        host: String,
        id: &'static str,
        connect_label: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let link = self.start.machines.get(&host).map(|m| &m.link);
        let connected = matches!(link, Some(MachineLink::Ready(_)));
        let connecting = matches!(link, Some(MachineLink::Connecting));
        let project_open = connected && crate::shell::app_window::host_has_project(&host, cx);
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

    pub(super) fn render_device_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        let blocked = connected && crate::shell::app_window::host_has_project(&host, cx);
        let connect_host = host.clone();
        let rename_host = host.clone();
        let local = host == sandhill_link::LOCAL;
        let menu = crate::agents::composer_view::popup(&p)
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
                menu.child(crate::panes::tree_actions::separator(&p))
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
                    .child(crate::panes::tree_actions::separator(&p))
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

    pub(super) fn render_start_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = welcome_palette(cx);
        let mut card = v_flex()
            .id("start-dialog-card")
            .occlude()
            .w(px(560.))
            .max_w(relative(0.9))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(look::GAP_12))
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
                        .text_size(px(look::TITLE))
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
                        .gap(px(look::GAP_8))
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
            let mut grid = h_flex().flex_wrap().gap(px(look::GAP_6));
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
                        .rounded(px(look::CONTROL_RADIUS))
                        .border_1()
                        .border_color(if on { p.accent } else { p.line })
                        .when(on, |d| d.bg(p.accent.opacity(0.12)))
                        .cursor_pointer()
                        .hover(move |d| d.bg(p.ink(look::ROW_HOVER)))
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
                        .text_size(px(look::TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Icon for {}", crate::panes::files::base_name(path))),
                )
                .child(div().text_color(p.muted).child(
                    "Shown for this project in Sandhill on this Mac. The project folder is not changed.",
                ))
                .child(grid)
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(look::GAP_8))
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
                .child(div().text_size(px(look::TITLE)).font_weight(FontWeight::SEMIBOLD).child("Connect to Server"))
                .child(Input::new(&self.start.server_input).small().disabled(connecting))
                .child(div().text_color(p.muted).child(
                    "Sandhill connects with ssh, using your ~/.ssh/config and keys, so a name from there works. The first time, it installs sandhilld in ~/.local/bin on the server.",
                ))
                .when_some(self.start.server_problem.clone(), |d, error| {
                    d.child(div().text_color(p.err).child(error))
                })
                .child(
                    h_flex().justify_end().gap(px(look::GAP_8))
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
                        .text_size(px(look::TITLE))
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
    pub(super) fn ask_rename(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn save_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(host) = self.start.renaming.clone() else {
            return;
        };
        let name = self.start.rename_input.read(cx).value().to_string();
        crate::prefs::save_device_name(&host, &name);
        self.start.close_dialogs(window, cx);
        // Other windows name it in their title bar and status bar too.
        cx.refresh_windows();
    }
}
