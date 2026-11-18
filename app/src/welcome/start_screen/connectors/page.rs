use super::*;
use gpui_kit::base::ElementExt as _;

impl AppWindow {
    pub(in crate::welcome::start_screen) fn connectors_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = welcome_palette(cx);
        let host = self.start.selected_host.clone();
        let ready = self.start.machines.get(&host).and_then(|m| match &m.link {
            MachineLink::Ready(daemon) => Some((m.attempt, daemon.clone())),
            _ => None,
        });
        match ready {
            Some((attempt, daemon)) => {
                if !self
                    .start
                    .connectors
                    .as_ref()
                    .is_some_and(|(h, a, _)| h == &host && *a == attempt)
                {
                    let view = cx.new(|cx| ConnectorsView::new(daemon, host.clone(), window, cx));
                    cx.observe(&view, |_, _, cx| cx.notify()).detach();
                    self.start.connectors = Some((host.clone(), attempt, view));
                }
            }
            None => self.start.connectors = None,
        }
        let destinations = self
            .start
            .hosts()
            .into_iter()
            .filter(|h| h != &host)
            .filter_map(|h| match self.start.machines.get(&h).map(|m| &m.link) {
                Some(MachineLink::Ready(daemon)) => Some(Destination {
                    host: h,
                    daemon: daemon.clone(),
                }),
                _ => None,
            })
            .collect();
        if let Some((_, _, view)) = &self.start.connectors {
            view.update(cx, |view, _| view.destinations = destinations);
        }
        let mut tabs = look::tab_capsule(&p).gap(px(1.));
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let selected = device == host;
            tabs = tabs.child(
                crate::workbench::Workbench::tab(
                    ("connectors-device", ix),
                    None,
                    host_label(&device),
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !selected {
                        this.start.selected_host = device.clone();
                        this.start.connectors = None;
                        this.ensure_machine(&device, cx);
                        cx.notify();
                    }
                })),
            );
        }
        let add = self.start.connectors.as_ref().map(|(_, _, view)| {
            let view = view.clone();
            let disabled = view.read(cx).busy || view.read(cx).problem.is_some();
            let triggers = view.read(cx).triggers.clone();
            Button::new("connectors-add")
                .small()
                .label("+ Add Connector ⌄")
                .disabled(disabled)
                .on_prepaint(triggers.track("connector-add"))
                .on_click(cx.listener(move |_, event: &ClickEvent, window, cx| {
                    view.update(cx, |view, cx| {
                        view.toggle_menu(Menu::Add, event.position(), window, cx)
                    });
                }))
        });
        let mut page = v_flex().w_full().gap(px(16.))
            .child(h_flex().w_full().gap(px(12.))
                .child(div().flex_1().text_size(px(look::WELCOME_TITLE)).font_weight(FontWeight::BOLD).child("Connectors"))
                .children(add))
            .child(div().text_size(px(look::TAB_TEXT)).text_color(p.muted).child("Connectors let agents look things up in databases and services. They are kept on each device and shared by every agent."))
            .child(h_flex().child(tabs));
        if let Some((_, _, view)) = &self.start.connectors {
            page = page.child(view.clone());
        } else {
            let message = match self.start.machines.get(&host).map(|m| &m.link) {
                Some(MachineLink::Failed(error)) => error.clone(),
                Some(MachineLink::Connecting) => "Connecting…".into(),
                _ => "Connect to this device to manage its connectors.".into(),
            };
            page = page
                .child(grey(&message, &p))
                .child(self.welcome_connection_button(host, "connectors-connect", "Connect", cx));
        }
        page.into_any_element()
    }

    pub(in crate::welcome::start_screen) fn connectors_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, _, view) = self.start.connectors.as_ref()?;
        view.update(cx, |view, cx| view.render_overlay(cx))
    }
}

fn detail(title: &str, description: &str, caption: AnyElement, p: &Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(3.))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_string()),
        )
        .child(
            grey(description, p)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap(),
        )
        .child(caption)
}

impl Render for ConnectorsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = welcome_palette(cx);
        let mut page = v_flex().w_full().gap(px(16.)).text_size(px(look::SMALL));
        if let Some(error) = &self.problem {
            return page
                .child(div().text_color(p.err).child(error.clone()))
                .child(
                    Button::new("connectors-retry")
                        .small()
                        .label("Refresh")
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                );
        }
        let Some(list) = &self.list else {
            return page.child(grey("Reading…", &p));
        };
        page = page.child(div().font_weight(FontWeight::SEMIBOLD).child("Added"));
        if list.added.is_empty() {
            page = page.child(grey("No connectors on this device yet.", &p));
        }
        for connector in &list.added {
            let name = connector.name.clone();
            let menu_name = name.clone();
            let switch_name = name.clone();
            let sign_in_name = name.clone();
            let needs_sign_in = connector
                .error
                .as_ref()
                .is_some_and(|error| error.contains(CONNECTOR_SIGN_IN));
            let menu_open = self.menu.as_ref().is_some_and(
                |(m, _)| matches!(m, Menu::Connector(n) | Menu::Copy(n) if n == &name),
            );
            let suggested = connector
                .suggested
                .as_ref()
                .and_then(|id| list.suggested.iter().find(|s| &s.id == id));
            let description = if suggested.is_some() {
                connector.description.clone()
            } else {
                match &connector.spec {
                    ConnectorSpec::Http { url, .. } => url.clone(),
                    ConnectorSpec::Command { command, args, .. } => {
                        let mut line = command.clone();
                        for arg in args {
                            line.push(' ');
                            line.push_str(arg);
                        }
                        line
                    }
                }
            };
            let (caption, error) = if let Some((_, text, error)) =
                self.notice.as_ref().filter(|(n, _, _)| n == &name)
            {
                (text.clone(), *error)
            } else if let Some(error) = &connector.error {
                (error.clone(), true)
            } else {
                (
                    suggested
                        .map(|s| format!("{} · {}", s.by, s.licence))
                        .unwrap_or_else(|| {
                            format!(
                                "{} · {} tools",
                                match connector.spec {
                                    ConnectorSpec::Http { .. } => "Web address",
                                    ConnectorSpec::Command { .. } => "Command",
                                },
                                connector.tools.len()
                            )
                        }),
                    false,
                )
            };
            page = page.child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .py(px(6.))
                    .child(detail(
                        &connector.title,
                        &description,
                        grey(&caption, &p)
                            .when(error, |d| d.text_color(p.err))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .into_any_element(),
                        &p,
                    ))
                    .when(needs_sign_in, |row| {
                        row.child(
                            Button::new(SharedString::from(format!("connector-sign-in-{name}")))
                                .small()
                                .label("Sign In Again")
                                .disabled(self.busy || self.loading)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_sign_in(
                                        SignInTarget::Existing {
                                            name: sign_in_name.clone(),
                                        },
                                        this.daemon.clone(),
                                        Completion::Reload,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .child(
                        Switch::new(SharedString::from(format!("connector-switch-{name}")))
                            .small()
                            .color(p.accent)
                            .checked(connector.enabled)
                            .disabled(self.busy || self.loading)
                            .accessibility_label(format!("Enable {}", connector.title))
                            .on_change(cx.listener(move |this, enabled: &bool, window, cx| {
                                this.focus.focus(window, cx);
                                this.mutate(
                                    Call::SetConnectorEnabled {
                                        name: switch_name.clone(),
                                        enabled: *enabled,
                                    },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("connector-menu-{name}")),
                            "more",
                            &p,
                            !self.busy,
                        )
                        .when(menu_open, |d| {
                            d.on_prepaint(self.triggers.track("connector-row"))
                        })
                        .when(!menu_open, |d| {
                            d.tooltip(|w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new("More").build(w, cx)
                            })
                        })
                        .on_click(cx.listener(
                            move |this, event: &ClickEvent, window, cx| {
                                if !this.busy {
                                    this.toggle_menu(
                                        Menu::Connector(menu_name.clone()),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )),
                    ),
            );
        }
        let suggested: Vec<_> = list.suggested.iter().filter(|s| !s.added).collect();
        if !suggested.is_empty() {
            page = page.child(
                div()
                    .mt(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Suggested"),
            );
        }
        for connector in suggested {
            let suggestion = connector.clone();
            let caption = if let Some(needs) = &connector.needs {
                let url = connector.needs_url.clone();
                grey(&format!("Needs {needs} on this device"), &p)
                    .id(SharedString::from(format!(
                        "connector-needs-{}",
                        connector.id
                    )))
                    .when(url.is_some(), |d| {
                        d.cursor_pointer().hover(|d| d.underline())
                    })
                    .on_click(move |_, _, cx| {
                        if let Some(url) = &url {
                            cx.open_url(url);
                        }
                    })
                    .into_any_element()
            } else {
                grey(&format!("{} · {}", connector.by, connector.licence), &p).into_any_element()
            };
            page = page.child(
                h_flex()
                    .w_full()
                    .gap(px(12.))
                    .py(px(6.))
                    .child(detail(
                        &connector.title,
                        &connector.description,
                        caption,
                        &p,
                    ))
                    .child(
                        Button::new(SharedString::from(format!(
                            "connector-suggested-{}",
                            connector.id
                        )))
                        .small()
                        .label("Add")
                        .disabled(self.busy || connector.needs.is_some())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_card(Card::Suggested(suggestion.clone()), window, cx)
                            },
                        )),
                    ),
            );
        }
        page
    }
}
