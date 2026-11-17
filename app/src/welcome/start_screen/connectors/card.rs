use super::*;

impl ConnectorsView {
    pub(super) fn render_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.card.is_none() {
            return self.render_menu(cx);
        }
        let card = self.card.as_ref()?;
        let p = welcome_palette(cx);
        let (title, primary) = match card {
            Card::Suggested(s) => (format!("Add {}?", s.title), Some("Add")),
            Card::Web => ("Add a Web Address".into(), Some("Add")),
            Card::Command => ("Add a Command".into(), Some("Add")),
            Card::Secret { destination, .. } => (
                format!("Copy to {}", host_label(&destination.host)),
                Some("Copy"),
            ),
            Card::Busy(text) => (text.clone(), None),
            Card::Tools(c) => (format!("{} tools", c.title), None),
            Card::Remove(c) => (format!("Remove {}?", c.title), Some("Remove")),
            Card::Error(_) => ("Could not manage connectors".into(), None),
        };
        let mut body = v_flex()
            .id("connector-card-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(12.));
        match card {
            Card::Suggested(s) => {
                let terms = s.terms.clone();
                body = body
                    .child(grey(&s.action, &p))
                    .child(grey(&s.description, &p).line_clamp(4))
                    .child(grey(&format!("Licence: {}", s.licence), &p))
                    .child(
                        grey("Each database has its own terms: read them", &p)
                            .text_color(p.accent)
                            .id("connector-terms")
                            .cursor_pointer()
                            .hover(|d| d.underline())
                            .on_click(move |_, _, cx| cx.open_url(&terms)),
                    );
            }
            Card::Web => {
                body = body
                    .child(field("Name", Input::new(&self.form.name), &p))
                    .child(field("Address", Input::new(&self.form.address), &p))
                    .child(field(
                        "Header name (optional)",
                        Input::new(&self.form.header),
                        &p,
                    ))
                    .child(field("Header value", Input::new(&self.form.secret), &p))
                    .children(
                        self.form_error
                            .clone()
                            .map(|e| div().text_color(p.err).child(e)),
                    )
            }
            Card::Command => {
                body = body
                    .child(field("Name", Input::new(&self.form.name), &p))
                    .child(field("Command line", Input::new(&self.form.command), &p))
                    .child(field(
                        "Environment (optional)",
                        Textarea::new(&self.form.env).h(px(100.)),
                        &p,
                    ))
                    .child(grey(
                        &format!("It runs as you on {}.", host_label(&self.host)),
                        &p,
                    ))
                    .children(
                        self.form_error
                            .clone()
                            .map(|e| div().text_color(p.err).child(e)),
                    )
            }
            Card::Secret {
                add, destination, ..
            } => {
                let header = match add {
                    ConnectorAdd::Custom {
                        spec:
                            ConnectorSpec::Http {
                                header: Some(header),
                                ..
                            },
                        ..
                    } => header.as_str(),
                    _ => "Header value",
                };
                body = body
                    .child(grey(
                        &format!(
                            "Enter the value for {}. The secret is not copied.",
                            host_label(&destination.host)
                        ),
                        &p,
                    ))
                    .child(field(header, Input::new(&self.form.secret), &p));
            }
            Card::Tools(c) => {
                if c.tools.is_empty() {
                    body = body.child(grey("No tools reported.", &p));
                }
                for tool in &c.tools {
                    body = body.child(
                        v_flex()
                            .min_w_0()
                            .gap(px(3.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tool.name.clone()),
                            )
                            .child(grey(&tool.description, &p).line_clamp(4))
                            .when(tool.read_only, |d| d.child(grey("looks up", &p))),
                    );
                }
            }
            Card::Remove(_) => {
                body = body.child(grey(
                    &format!(
                        "Agents on {} will no longer reach it.",
                        host_label(&self.host)
                    ),
                    &p,
                ))
            }
            Card::Error(error) => body = body.child(div().text_color(p.err).child(error.clone())),
            Card::Busy(_) => {}
        }
        let close_only = matches!(card, Card::Tools(_) | Card::Error(_));
        let shell = card_shell("connector-card", &self.focus, &p)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.dismiss(cx),
                    "enter" => {
                        if this.form.env.focus_handle(cx).is_focused(window) {
                            return;
                        }
                        this.primary(window, cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(12.))
                    .child(div().flex_none().child(icon("link").size(px(32.))))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(body)
            .child(
                v_flex()
                    .flex_none()
                    .gap(px(8.))
                    .children(primary.map(|label| {
                        Button::new("connector-primary")
                            .primary()
                            .w_full()
                            .label(label)
                            .disabled(self.busy || !self.form.ready(card, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx)))
                    }))
                    .child(
                        Button::new("connector-cancel")
                            .w_full()
                            .label(if close_only { "Close" } else { "Cancel" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("connector-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .bg(p.shadow.opacity(0.35))
                    .child(shell)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss(cx);
                        }),
                    ),
            )
            .with_priority(10)
            .into_any_element(),
        )
    }

    pub(super) fn primary(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(card) = self.card.as_ref() else {
            return;
        };
        if !self.form.ready(card, cx) {
            return;
        }
        match card {
            Card::Suggested(s) => self.add(
                ConnectorAdd::Suggested { id: s.id.clone() },
                self.daemon.clone(),
                Completion::Reload,
                cx,
            ),
            Card::Web => self.add(
                self.form.web(cx),
                self.daemon.clone(),
                Completion::Reload,
                cx,
            ),
            Card::Command => match self.form.command(cx) {
                Ok(add) => self.add(add, self.daemon.clone(), Completion::Reload, cx),
                Err(error) => self.fail(error, cx),
            },
            Card::Secret {
                add,
                destination,
                name,
            } => {
                let mut add = add.clone();
                if let ConnectorAdd::Custom { secret, .. } = &mut add {
                    *secret = Some(self.form.secret.read(cx).value().to_string());
                }
                let completion = Completion::Copied {
                    name: name.clone(),
                    device: host_label(&destination.host),
                };
                self.add(add, destination.daemon.clone(), completion, cx);
            }
            Card::Remove(c) => self.mutate(
                Call::RemoveConnector {
                    name: c.name.clone(),
                },
                cx,
            ),
            Card::Tools(_) | Card::Error(_) => self.dismiss(cx),
            Card::Busy(_) => {}
        }
    }
}

fn field(label: &str, input: impl IntoElement, p: &Palette) -> Div {
    v_flex().gap(px(4.)).child(grey(label, p)).child(input)
}
