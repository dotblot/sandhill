//! The Agents page and trusted folders list.

use super::*;

impl AppWindow {
    pub(super) fn agent_accounts_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
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
                let accounts = cx.new(|cx| AccountManager::new(daemon, host.clone(), cx));
                cx.observe(&accounts, |_, _, cx| cx.notify()).detach();
                self.start.accounts = Some((host.clone(), attempt, accounts));
            }
        } else {
            self.start.accounts = None;
        }
        // Accounts belong to a device: one capsule tab each.
        let mut devices = look::tab_capsule(&p).gap(px(1.));
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let selected = device == host;
            devices = devices.child(
                crate::workbench::Workbench::tab(
                    ("agents-device", ix),
                    None,
                    host_label(&device),
                    selected,
                    &p,
                )
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
        let refresh = self.start.accounts.as_ref().map(|(_, _, accounts)| {
            let accounts = accounts.clone();
            let refreshing = accounts.read(cx).refreshing;
            Button::new("agents-refresh")
                .small()
                .label(if refreshing {
                    "Refreshing…"
                } else {
                    "Refresh"
                })
                .disabled(refreshing)
                .on_click(cx.listener(move |_, _, _, cx| {
                    accounts.update(cx, |accounts, cx| accounts.refresh_all(cx))
                }))
        });
        let mut page = v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(look::GAP_8))
                    .child(div().flex_1().text_size(px(look::welcome_title())).font_weight(FontWeight::BOLD).child("Agents"))
                    .children(refresh),
            )
            .child(div().text_size(px(look::tab_text())).text_color(p.muted).child(
                "Choose which agents Sandhill offers, and the accounts they run on. Switching accounts in a conversation is always your choice.",
            ))
            .child(h_flex().child(devices));
        if host == sandhill_link::LOCAL && self.start.accounts.is_some() {
            page = page.child(self.quick_ai_helpers(&p, cx));
        }
        if let Some((_, _, accounts)) = &self.start.accounts {
            page = page.child(accounts.clone());
        } else {
            let message = match self.start.machines.get(&host).map(|machine| &machine.link) {
                Some(MachineLink::Failed(error)) => error.clone(),
                Some(MachineLink::Connecting) => "Connecting…".into(),
                _ => "Connect to this device to manage its agent accounts.".into(),
            };
            page = page
                .child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.muted)
                        .child(message),
                )
                .child(self.welcome_connection_button(host, "accounts-connect", "Connect", cx));
        }
        page.into_any_element()
    }

    /// Manage → Trusted Folders: reads every connected device's list.
    pub(super) fn show_trusted_folders(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn untrust_folder(&mut self, host: String, path: String, cx: &mut Context<Self>) {
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
    pub(super) fn trusted_folders(
        &self,
        list_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                                .min_h(px(look::row()))
                                .px(px(look::PANE_PAD_X))
                                .py(px(look::GAP_2))
                                .gap(px(look::GAP_4))
                                .when(row > 0, |d| d.border_t_1().border_color(p.line))
                                .child(
                                    icon(if host == sandhill_link::LOCAL {
                                        "device.laptop"
                                    } else {
                                        "device.server"
                                    })
                                    .size(px(look::ICON))
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
                                        .font_family(look::mono())
                                        .text_size(px(look::small()))
                                        .when(!folder.exists, |d| d.text_color(p.muted))
                                        .child(folder.path.clone()),
                                )
                                .when(!folder.exists, |d| {
                                    d.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(look::small()))
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
            .gap(px(look::GAP_12))
            .child(
                v_flex()
                    .gap(px(look::GAP_6))
                    .child(
                        div()
                            .text_size(px(look::welcome_title()))
                            .font_weight(FontWeight::BOLD)
                            .child("Trusted Folders"),
                    )
                    .child(div().text_size(px(look::tab_text())).text_color(p.muted).child(
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
                    .map(|text| div().text_size(px(look::small())).text_color(p.muted).child(text)),
            )
            .into_any_element()
    }
}
