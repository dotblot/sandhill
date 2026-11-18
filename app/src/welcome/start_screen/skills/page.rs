use super::*;
use gpui_kit::base::ElementExt as _;

impl AppWindow {
    pub(in crate::welcome::start_screen) fn skills_page(
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
                    .skills
                    .as_ref()
                    .is_some_and(|(h, a, _)| h == &host && *a == attempt)
                {
                    let view = cx.new(|cx| SkillsView::new(daemon, host.clone(), window, cx));
                    cx.observe(&view, |_, _, cx| cx.notify()).detach();
                    self.start.skills = Some((host.clone(), attempt, view));
                }
            }
            None => self.start.skills = None,
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
        if let Some((_, _, view)) = &self.start.skills {
            view.update(cx, |view, _| view.destinations = destinations);
        }
        let mut tabs = look::tab_capsule(&p).gap(px(1.));
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let selected = device == host;
            tabs = tabs.child(
                crate::workbench::Workbench::tab(
                    ("skills-device", ix),
                    None,
                    host_label(&device),
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !selected {
                        this.start.selected_host = device.clone();
                        this.start.skills = None;
                        this.ensure_machine(&device, cx);
                        cx.notify();
                    }
                })),
            );
        }
        let add = self.start.skills.as_ref().map(|(_, _, view)| {
            let view = view.clone();
            let disabled = view.read(cx).busy || view.read(cx).problem.is_some();
            let triggers = view.read(cx).triggers.clone();
            Button::new("skills-add")
                .small()
                .label("+ Add Skill ⌄")
                .disabled(disabled)
                .on_prepaint(triggers.track("skill-add"))
                .on_click(cx.listener(move |_, event: &ClickEvent, window, cx| {
                    view.update(cx, |view, cx| {
                        view.toggle_menu(Menu::Add, event.position(), window, cx)
                    });
                }))
        });
        let mut page = v_flex().w_full().gap(px(look::GAP_12))
            .child(h_flex().w_full().gap(px(look::GAP_8))
                .child(div().flex_1().text_size(px(look::WELCOME_TITLE)).font_weight(FontWeight::BOLD).child("Skills"))
                .children(add))
            .child(div().text_size(px(look::TAB_TEXT)).text_color(p.muted).child("Skills are instructions agents load when the work calls for them. They are kept on each device and given to every agent."))
            .child(h_flex().child(tabs));
        if let Some((_, _, view)) = &self.start.skills {
            page = page.child(view.clone());
        } else {
            let message = match self.start.machines.get(&host).map(|m| &m.link) {
                Some(MachineLink::Failed(error)) => error.clone(),
                Some(MachineLink::Connecting) => "Connecting…".into(),
                _ => "Connect to this device to manage its skills.".into(),
            };
            page = page
                .child(
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(message),
                )
                .child(self.welcome_connection_button(host, "skills-connect", "Connect", cx));
        }
        page.into_any_element()
    }

    pub(in crate::welcome::start_screen) fn skills_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (_, _, view) = self.start.skills.as_ref()?;
        view.update(cx, |view, cx| view.render_overlay(cx))
    }
}
fn detail(name: String, description: String, source: String, p: &Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(look::GAP_2))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
        .child(
            div()
                .text_color(p.muted)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(description),
        )
        .child(
            div()
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(source),
        )
}
impl Render for SkillsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = welcome_palette(cx);
        let mut page = v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .text_size(px(look::SMALL));
        if let Some(error) = &self.problem {
            return page
                .child(div().text_color(p.err).child(error.clone()))
                .child(
                    Button::new("skills-retry")
                        .small()
                        .label("Refresh")
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                );
        }
        let Some(list) = &self.list else {
            return page.child(note("Reading…", &p));
        };
        if list.installed.is_empty() {
            page = page.child(note("No skills installed on this device yet.", &p));
        } else {
            page = page.child(div().font_weight(FontWeight::SEMIBOLD).child("Installed"));
            for skill in &list.installed {
                let name = skill.name.clone();
                let menu_name = name.clone();
                let menu_open = self.menu.as_ref().is_some_and(
                    |(m, _)| matches!(m, Menu::Skill(n) | Menu::Copy(n) if n == &name),
                );
                let switch_name = name.clone();
                page = page.child(
                    h_flex()
                        .w_full()
                        .min_h(px(look::ROW))
                        .gap(px(look::GAP_12))
                        .py(px(look::GAP_6))
                        .child(detail(
                            name.clone(),
                            skill.description.clone(),
                            match self.notice.as_ref().filter(|(n, _)| n == &skill.name) {
                                Some((_, text)) => {
                                    format!("{} · {text}", source_label(skill))
                                }
                                None => source_label(skill),
                            },
                            &p,
                        ))
                        .child(
                            Switch::new(SharedString::from(format!("skill-switch-{name}")))
                                .small()
                                .color(p.accent)
                                .checked(skill.enabled)
                                .disabled(self.busy || self.loading)
                                .accessibility_label(format!("Enable {name}"))
                                .on_change(cx.listener(move |this, enabled: &bool, window, cx| {
                                    this.focus.focus(window, cx);
                                    this.mutate(
                                        Call::SetSkillEnabled {
                                            name: switch_name.clone(),
                                            enabled: *enabled,
                                        },
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            icon_button(
                                SharedString::from(format!("skill-menu-{name}")),
                                "more",
                                &p,
                                false,
                            )
                            .when(menu_open, |d| {
                                d.on_prepaint(self.triggers.track("skill-row"))
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
                                            Menu::Skill(menu_name.clone()),
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
        }
        let suggested: Vec<_> = list
            .suggested
            .iter()
            .filter(|s| !s.installed && !list.installed.iter().any(|i| i.name == s.name))
            .collect();
        if !suggested.is_empty() {
            page = page.child(
                div()
                    .mt(px(look::GAP_8))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Suggested"),
            );
            for skill in suggested {
                let id = skill.id.clone();
                page = page.child(
                    h_flex()
                        .w_full()
                        .min_h(px(look::ROW))
                        .gap(px(look::GAP_12))
                        .py(px(look::GAP_6))
                        .child(detail(
                            skill.name.clone(),
                            skill.description.clone(),
                            "Anthropic · Apache-2.0".into(),
                            &p,
                        ))
                        .child(
                            Button::new(SharedString::from(format!("skill-suggested-{id}")))
                                .small()
                                .label("Add")
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.preview(
                                        SkillFetch::Suggested { id: id.clone() },
                                        window,
                                        cx,
                                    )
                                })),
                        ),
                );
            }
        }
        page = page.child(
            div()
                .mt(px(look::GAP_8))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Built in"),
        );
        for guide in &list.built_in {
            page = page.child(detail(
                guide.name.clone(),
                guide.description.clone(),
                "Built into Sandhill".into(),
                &p,
            ));
        }
        page
    }
}
