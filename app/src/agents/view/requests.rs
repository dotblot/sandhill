use super::*;
use gpui_kit::component::Disableable;

impl Agents {
    /// What waits on the user: a permission, a question or a plan.
    pub(super) fn render_ask(&self, ask: &Ask, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let view = chat::ask_view(ask, None, agent);
        let pending = ask.answer.is_none();
        let sent = self.sent.contains(&ask.id);
        let mut card = v_flex()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.accent.opacity(0.55))
            .bg(p.accent.opacity(0.06))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        icon(if !view.questions.is_empty() {
                            "help"
                        } else if view.plan.is_some() {
                            "plan"
                        } else {
                            "ask"
                        })
                        .size(px(18.))
                        .text_color(p.accent),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(view.title.clone()),
                            )
                            .when_some(view.detail.clone(), |d, detail| {
                                d.child(
                                    div()
                                        .text_size(px(look::SMALL))
                                        .text_color(p.muted)
                                        .child(detail),
                                )
                            }),
                    )
                    .when_some(view.view.added.filter(|n| *n > 0), |d, n| {
                        d.child(
                            div()
                                .text_size(px(look::TINY))
                                .text_color(p.ok)
                                .child(format!("+{n}")),
                        )
                    }),
            );
        if let Some(plan) = &view.plan {
            card = card.child(
                div()
                    .max_h(px(320.))
                    .p(px(8.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(p.line)
                    .bg(p.bg)
                    .child(TextView::markdown(
                        SharedString::from(format!("plan-{}", ask.id)),
                        plan.clone(),
                    )),
            );
        } else if view.questions.is_empty() {
            card = card.child(step_card(
                &view.view,
                &format!("ask-{}", ask.id),
                cx.entity().downgrade(),
                &p,
                cx,
            ));
        }
        if !pending {
            let answer = ask.answer.as_ref().and_then(|a| a["behavior"].as_str());
            let answers = ask
                .answer
                .as_ref()
                .and_then(|a| a["answers"].as_object())
                .map(|all| {
                    // In the order they were asked.
                    view.questions
                        .iter()
                        .filter_map(|q| all.get(&q.question).and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("; ")
                });
            let option = ask.answered_option();
            let text = if let Some(option) = option {
                option.name.clone()
            } else {
                match (answers, answer) {
                    (Some(answers), _) => answers,
                    (None, Some("allow")) if view.plan.is_some() => "Carried out".into(),
                    (None, Some("allow")) => "Allowed".into(),
                    (None, Some("deny")) if !view.questions.is_empty() => "Skipped".into(),
                    (None, Some("deny")) if view.plan.is_some() => "Kept planning".into(),
                    (None, Some("deny")) => "Denied".into(),
                    _ => "No longer waiting".to_string(),
                }
            };
            return card
                .child(
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child(text),
                )
                .into_any_element();
        }
        let mut ready = true;
        let mut answers = serde_json::Map::new();
        for (qi, question) in view.questions.iter().enumerate() {
            let key = question_key(&ask.id, &question.question);
            let answer = self.answer_of(&key, cx);
            ready &= !answer.is_empty();
            answers.insert(question.question.clone(), Value::String(answer));
            let picked = self.picks.get(&key).cloned().unwrap_or_default();
            let multi = question.multi_select;
            let options = question
                .options
                .iter()
                .enumerate()
                .map(|(oi, (label, desc))| {
                    let on = picked.contains(label);
                    let key = key.clone();
                    let label_owned = label.clone();
                    let hover = p.faint;
                    v_flex()
                        .id(SharedString::from(format!("q-{}-{qi}-{oi}", ask.id)))
                        .px(px(9.))
                        .py(px(5.))
                        .rounded(px(RADIUS))
                        .border_1()
                        .bg(p.bg)
                        .cursor_pointer()
                        .map(|d| {
                            if on {
                                d.border_color(p.accent).bg(p.accent_soft)
                            } else {
                                d.border_color(p.line).hover(move |d| d.border_color(hover))
                            }
                        })
                        .child(div().font_weight(FontWeight::MEDIUM).child(label.clone()))
                        .when_some(desc.clone(), |d, desc| {
                            d.child(
                                div()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.muted)
                                    .child(desc),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pick_option(key.clone(), label_owned.clone(), multi, cx)
                        }))
                });
            let other = self.others.get(&key).cloned();
            card = card.child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(question.question.clone()),
                            )
                            .when_some(question.header.clone(), |d, h| {
                                d.child(
                                    div()
                                        .px(px(6.))
                                        .rounded(px(5.))
                                        .bg(p.ink(7.))
                                        .text_size(px(look::SMALL))
                                        .text_color(p.muted)
                                        .child(h),
                                )
                            }),
                    )
                    .when(multi, |d| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child("Pick any that apply"),
                        )
                    })
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .children(options)
                            .when_some(other, |d, other| d.child(Input::new(&other).small())),
                    ),
            );
        }
        if !view.questions.is_empty() {
            let id = ask.id.clone();
            let send = json!({"behavior": "allow", "answers": answers});
            card = card.child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        look::button(SharedString::from(format!("q-answer-{id}")), "Answer", None, ButtonSize::Card)
                            .primary()
                            .font_weight(FontWeight::SEMIBOLD)
                            .disabled(!ready || sent)
                            .on_click(Self::reply(id.clone(), send, cx)),
                    )
                    .child(div().flex_1())
                    .child(
                        look::button(SharedString::from(format!("q-skip-{id}")), "Skip", None, ButtonSize::Card)
                            .on_click(Self::reply(
                                id,
                                json!({"behavior": "deny", "message": "The user skipped your questions in Sandhill. Ask them in the chat instead."}),
                                cx,
                            )),
                    ),
            );
        }
        if view.questions.is_empty() {
            let scopes = ask.scopes.clone();
            let can = |scope: &str| scopes.as_ref().is_none_or(|s| s.iter().any(|x| x == scope));
            let plan = view.plan.is_some();
            let mut buttons = h_flex().gap(px(6.)).flex_wrap();
            let button = |id: &str, label: &str, primary: bool, deny: bool| {
                look::button(
                    SharedString::from(format!("{id}-{}", ask.id)),
                    SharedString::from(label.to_owned()),
                    None,
                    ButtonSize::Card,
                )
                .when(primary, |button| {
                    button.primary().font_weight(FontWeight::SEMIBOLD)
                })
                .when(deny, |button| button.danger())
                .disabled(sent)
            };
            if let Some(options) = &ask.options {
                for (index, option) in options.iter().enumerate() {
                    buttons = buttons.child(
                        button(
                            &format!("approval-{index}"),
                            &option.name,
                            option.kind == "allow_once",
                            option.rejects(),
                        )
                        .on_click(Self::reply(
                            ask.id.clone(),
                            option.answer(),
                            cx,
                        )),
                    );
                }
                return card.child(buttons).into_any_element();
            }
            let id = &ask.id;
            buttons = buttons.child(
                button(
                    "allow",
                    if plan {
                        "Yes, carry it out"
                    } else {
                        "Allow once"
                    },
                    true,
                    false,
                )
                .on_click(Self::reply(
                    id.clone(),
                    json!({"behavior": "allow", "scope": "once"}),
                    cx,
                )),
            );
            if plan && !ask.remember.is_empty() {
                buttons = buttons.child(
                    button("allow-edits", "Yes, and allow edits", false, false).on_click(
                        Self::reply(
                            id.clone(),
                            json!({"behavior": "allow", "scope": "chat"}),
                            cx,
                        ),
                    ),
                );
            }
            if !plan && can("chat") {
                buttons = buttons.child(
                    button("allow-chat", "For this chat", false, false).on_click(Self::reply(
                        id.clone(),
                        json!({"behavior": "allow", "scope": "chat"}),
                        cx,
                    )),
                );
            }
            if !plan && can("project") {
                buttons = buttons.child(
                    button("allow-project", "For this project", false, false).on_click(
                        Self::reply(
                            id.clone(),
                            json!({"behavior": "allow", "scope": "project"}),
                            cx,
                        ),
                    ),
                );
            }
            buttons = buttons.child(
                button(
                    "deny",
                    if plan { "Keep planning" } else { "Deny" },
                    false,
                    !plan,
                )
                .on_click(Self::reply(
                    id.clone(),
                    if plan {
                        json!({"behavior": "deny", "message": "The user wants to keep planning. Ask them what to change in the plan."})
                    } else {
                        json!({"behavior": "deny"})
                    },
                    cx,
                )),
            );
            card = card.child(buttons);
            if !ask.remember.is_empty() && !plan {
                card = card.child(div().text_size(px(look::SMALL)).text_color(p.faint).child(
                    format!(
                        "Allowing it for the chat or project also allows: {}",
                        ask.remember.join(", ")
                    ),
                ));
            }
        }
        card.into_any_element()
    }

    fn reply(id: String, answer: Value, cx: &mut Context<Self>) -> ClickHandler {
        let entity = cx.entity().downgrade();
        Box::new(move |_, _, cx| {
            entity
                .update(cx, |this, cx| {
                    if !this.sent.contains(&id) {
                        this.answer(&id, answer.clone(), cx)
                    }
                })
                .ok();
        })
    }
}
