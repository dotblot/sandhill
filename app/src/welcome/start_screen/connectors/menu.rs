use super::*;

impl ConnectorsView {
    pub(super) fn toggle_menu(
        &mut self,
        menu: Menu,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = self.menu.as_ref().is_some_and(|(open, _)| {
            open == &menu || matches!((open, &menu), (Menu::Copy(a), Menu::Connector(b)) if a == b)
        });
        self.notice = None;
        self.menu = if same { None } else { Some((menu, at)) };
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (menu, at) = self.menu.clone()?;
        let p = welcome_palette(cx);
        let trigger = if matches!(menu, Menu::Add) {
            "connector-add"
        } else {
            "connector-row"
        };
        let mut body = crate::agents::composer_view::popup(&p)
            .id("connectors-menu")
            .track_focus(&self.focus)
            .occlude()
            .min_w(px(220.))
            .max_w(px(360.))
            .text_size(px(look::SMALL))
            .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if !this.triggers.contains(trigger, event.position) {
                    this.menu = None;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.menu = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }));
        match menu {
            Menu::Add => {
                body = body
                    .child(item("connector-web", "Web Address…", true, &p).on_click(
                        cx.listener(|this, _, window, cx| this.open_form(false, window, cx)),
                    ))
                    .child(item("connector-command", "Command…", true, &p).on_click(
                        cx.listener(|this, _, window, cx| this.open_form(true, window, cx)),
                    ));
            }
            Menu::Connector(name) => {
                let connector = self
                    .list
                    .as_ref()?
                    .added
                    .iter()
                    .find(|c| c.name == name)?
                    .clone();
                let tools = connector.clone();
                let test_name = name.clone();
                let copy_name = name.clone();
                let connected = !self.destinations.is_empty();
                body = body
                    .child(
                        item("connector-tools", "Tools", true, &p).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_card(Card::Tools(tools.clone()), window, cx)
                            },
                        )),
                    )
                    .child(
                        item("connector-test", "Test", true, &p).on_click(cx.listener(
                            move |this, _, window, cx| this.test(test_name.clone(), window, cx),
                        )),
                    )
                    .child(
                        item("connector-copy", "Copy to…", connected, &p)
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if connected {
                                    "Copy to another connected device"
                                } else {
                                    "Connect another device to copy this connector"
                                })
                                .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if connected {
                                    this.menu = Some((Menu::Copy(copy_name.clone()), at));
                                    cx.notify();
                                }
                            })),
                    )
                    .child(crate::panes::tree_actions::separator(&p))
                    .child(
                        item("connector-remove", "Remove", true, &p).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_card(Card::Remove(connector.clone()), window, cx)
                            },
                        )),
                    );
            }
            Menu::Copy(name) => {
                body = body.child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .text_color(p.muted)
                        .child("Copy to…"),
                );
                if self.destinations.is_empty() {
                    body = body.child(grey("Connect another device to copy this connector.", &p));
                }
                for (ix, destination) in self.destinations.iter().enumerate() {
                    let name = name.clone();
                    let destination = destination.clone();
                    body = body.child(
                        item(
                            SharedString::from(format!("connector-copy-device-{ix}")),
                            &host_label(&destination.host),
                            true,
                            &p,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.copy(name.clone(), destination.clone(), window, cx)
                            },
                        )),
                    );
                }
            }
        }
        Some(
            deferred(anchored().position(at).snap_to_window().child(body))
                .with_priority(5)
                .into_any_element(),
        )
    }
}
