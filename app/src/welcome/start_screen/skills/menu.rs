use super::*;

impl SkillsView {
    pub(super) fn toggle_menu(
        &mut self,
        menu: Menu,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .menu
            .as_ref()
            .is_some_and(|(open, _)| match (open, &menu) {
                (Menu::Add, Menu::Add) => true,
                (Menu::Skill(a) | Menu::Copy(a), Menu::Skill(b)) => a == b,
                _ => false,
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
            "skill-add"
        } else {
            "skill-row"
        };
        let mut body = crate::agents::composer_view::popup(&p)
            .id("skills-menu")
            .track_focus(&self.focus)
            .occlude()
            .min_w(px(220.))
            .max_w(px(360.))
            .text_size(px(13.))
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
                body =
                    body.child(item("skill-github", "From GitHub…", true, &p).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_card(Card::Github, window, cx);
                            this.input.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                        }),
                    ))
                    .child(
                        item("skill-upload", "From a Folder or Zip…", true, &p).on_click(
                            cx.listener(|this, _, window, cx| this.choose_upload(window, cx)),
                        ),
                    );
            }
            Menu::Skill(name) => {
                let skill = self
                    .list
                    .as_ref()?
                    .installed
                    .iter()
                    .find(|s| s.name == name)?
                    .clone();
                let update_name = name.clone();
                let copy_name = name.clone();
                let git = matches!(skill.source, SkillSource::Git { .. });
                body = body.child(item("skill-files", "Show Files", true, &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.show_files(skill.clone(), window, cx)
                    }),
                ));
                if git {
                    body = body.child(
                        item("skill-update", "Check for Updates…", true, &p).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.preview(
                                    SkillFetch::Update {
                                        name: update_name.clone(),
                                    },
                                    window,
                                    cx,
                                )
                            },
                        )),
                    );
                }
                let connected = !self.destinations.is_empty();
                body = body
                    .child(
                        item("skill-copy", "Copy to…", connected, &p)
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if connected {
                                    "Copy to another connected device"
                                } else {
                                    "Connect another device to copy this skill"
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
                        item("skill-remove", "Remove", true, &p).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_card(Card::Remove(name.clone()), window, cx)
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
                    body = body.child(note("Connect another device to copy this skill.", &p));
                }
                for (ix, destination) in self.destinations.iter().enumerate() {
                    let name = name.clone();
                    let destination = destination.clone();
                    body = body.child(
                        item(
                            SharedString::from(format!("skill-copy-device-{ix}")),
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

fn item(id: impl Into<ElementId>, label: &str, enabled: bool, p: &Palette) -> Stateful<Div> {
    let hover = p.ink(6.);
    h_flex()
        .id(id)
        .h(px(30.))
        .px(px(8.))
        .rounded(px(look::RADIUS - 4.))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |d| d.bg(hover))
                .active(|d| d.opacity(0.8))
        })
        .when(!enabled, |d| d.opacity(0.4).cursor_default())
        .child(label.to_string())
}
