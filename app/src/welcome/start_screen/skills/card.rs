use super::*;
use gpui_kit::component::checkbox::Checkbox;

impl SkillsView {
    pub(super) fn render_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.card.is_none() {
            return self.render_menu(cx);
        }
        let card = self.card.as_ref()?;
        let p = welcome_palette(cx);
        let (title, primary) = match card {
            Card::Github => ("Add from GitHub".into(), Some("Continue")),
            Card::Busy(text) => (text.clone(), None),
            Card::Confirm(confirm) => {
                let title = confirm.title.clone().unwrap_or_else(|| {
                    if confirm.preview.skills.len() == 1 {
                        format!("Install {}?", confirm.preview.skills[0].name)
                    } else {
                        format!("Install {} skills?", confirm.preview.skills.len())
                    }
                });
                (
                    title,
                    Some(if confirm.update { "Update" } else { "Install" }),
                )
            }
            Card::Files { skill, .. } => (skill.name.clone(), None),
            Card::Remove(name) => (format!("Remove {name}?"), Some("Remove")),
            Card::Error(_) => ("Could not manage skills".into(), None),
        };
        let mut body = v_flex()
            .id("skill-card-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(look::GAP_12));
        match card {
            Card::Github => body = body.child(Input::new(&self.input)),
            Card::Busy(_) => {}
            Card::Error(error) => body = body.child(div().text_color(p.err).child(error.clone())),
            Card::Remove(_) => {
                body = body.child(grey(
                    &format!(
                        "Agents on {} will no longer have it.",
                        host_label(&self.host)
                    ),
                    &p,
                ))
            }
            Card::Files { skill, paths } => {
                body = body
                    .child(grey(&source_label(skill), &p))
                    .child(grey(&skill.description, &p).line_clamp(4))
                    .child(file_list("installed-skill-files".into(), paths, &p));
                if !skill.scripts.is_empty() {
                    body = body.child(scripts(&skill.scripts, &p));
                }
            }
            Card::Confirm(confirm) => {
                body = body.child(grey(&confirm.preview.source, &p));
                let several = confirm.preview.skills.len() > 1;
                for (ix, skill) in confirm.preview.skills.iter().enumerate() {
                    let mut candidate = v_flex().min_w_0().gap(px(look::GAP_6));
                    if several {
                        candidate = candidate.child(
                            h_flex()
                                .items_start()
                                .gap(px(look::GAP_4))
                                .child(
                                    h_flex().h(rems(1.25)).flex_none().items_center().child(
                                        Checkbox::new(SharedString::from(format!(
                                            "skill-pick-{ix}"
                                        )))
                                        .checked(confirm.selected[ix])
                                        .on_click(
                                            cx.listener(move |this, checked: &bool, _, cx| {
                                                if let Some(Card::Confirm(confirm)) = &mut this.card
                                                {
                                                    confirm.selected[ix] = *checked;
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(skill.name.clone()),
                                ),
                        );
                    }
                    candidate = candidate
                        .child(grey(&skill.description, &p).line_clamp(4))
                        .child(licence(&skill.licence, &p))
                        .child(file_list(
                            SharedString::from(format!("skill-preview-files-{ix}")),
                            &skill.files,
                            &p,
                        ));
                    if !skill.scripts.is_empty() {
                        candidate = candidate.child(scripts(&skill.scripts, &p));
                    }
                    if skill.replaces {
                        candidate = candidate.child(grey("Replaces the installed copy.", &p));
                    }
                    body = body.child(candidate);
                }
                for refusal in &confirm.preview.refused {
                    body = body.child(
                        div()
                            .text_color(p.faint)
                            .child(format!("{}: {}", refusal.path, refusal.reason)),
                    );
                }
            }
        }
        let disabled = self.busy
            || match card {
                Card::Confirm(c) => !c.selected.iter().any(|s| *s),
                Card::Github => self.input.read(cx).value().trim().is_empty(),
                _ => false,
            };
        let close_only = matches!(card, Card::Files { .. } | Card::Error(_));
        let card = card_shell("skill-card", &self.focus, &p)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.dismiss(cx),
                    "enter" => this.primary(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(look::GAP_12))
                    .child(
                        div()
                            .flex_none()
                            .child(icon("skill").size(px(look::ICON_LARGE))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(look::title()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(body)
            .child(
                v_flex()
                    .flex_none()
                    .gap(px(look::GAP_8))
                    .children(primary.map(|label| {
                        Button::new("skill-primary")
                            .primary()
                            .w_full()
                            .label(label)
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx)))
                    }))
                    .child(
                        Button::new("skill-cancel")
                            .w_full()
                            .label(if close_only { "Close" } else { "Cancel" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("skill-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
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

    fn primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.card.as_ref() {
            Some(Card::Github) => self.continue_github(window, cx),
            Some(Card::Confirm(_)) => self.install(cx),
            Some(Card::Remove(name)) => self.mutate(Call::RemoveSkill { name: name.clone() }, cx),
            Some(Card::Error(_) | Card::Files { .. }) => self.dismiss(cx),
            _ => {}
        }
    }
}

fn licence(licence: &Option<String>, p: &Palette) -> Div {
    grey(
        &licence
            .as_ref()
            .map_or_else(|| "No licence file".into(), |l| format!("Licence: {l}")),
        p,
    )
}

fn file_list(id: SharedString, paths: &[String], p: &Palette) -> Stateful<Div> {
    v_flex()
        .id(id)
        .max_h(px(160.))
        .overflow_y_scroll()
        .gap(px(look::GAP_2))
        .text_size(px(look::small()))
        .text_color(p.muted)
        .children(paths.iter().map(|path| div().child(path.clone())))
}

fn scripts(paths: &[String], p: &Palette) -> Div {
    let mut names = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > 3 {
        names.push_str(&format!(" and {} more", paths.len() - 3));
    }
    div()
        .text_color(p.warn)
        .child(format!("Contains scripts an agent may run: {names}"))
}
