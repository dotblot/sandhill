use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::InputState;
use sandhill_proto::{QuickCardTable, QuickDeMethod, QuickFrom};

use super::*;
use crate::quick::{Inputs, Quick, differential as de};

fn update<T: Clone + 'static>(
    id: &'static str,
    items: impl IntoIterator<Item = (String, bool, T)>,
    set: fn(&mut Quick, T),
    cx: &mut Context<Workbench>,
) -> Div {
    h_flex()
        .flex_wrap()
        .gap(px(look::GAP_4))
        .children(
            items
                .into_iter()
                .enumerate()
                .map(|(ix, (label, selected, value))| {
                    let button = Button::new((id, ix)).small().label(label);
                    (if selected { button.primary() } else { button }).on_click(cx.listener(
                        move |this, _, _, cx| {
                            if let Some(quick) = &mut this.quick {
                                set(quick, value.clone());
                                cx.notify();
                            }
                        },
                    ))
                }),
        )
}

fn note(p: &Palette, text: impl Into<SharedString>) -> Div {
    div().text_color(p.muted).child(text.into())
}

impl Workbench {
    pub(super) fn render_sample_factors(
        &self,
        table: &QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let p = *Palette::get(cx);
        let factors = quick::factors(&q.card);
        let selected = factors
            .iter()
            .find(|f| Some(&f.name) == q.editing_factor.as_ref())
            .or_else(|| factors.first());
        let factor_name = selected.map_or("group", |f| f.name.as_str());
        let samples = table
            .samples
            .as_ref()
            .and_then(Known::usable)
            .map_or(&[][..], Vec::as_slice);
        let sheets = quick::sheet_columns(q);
        let mut body = v_flex()
            .gap(px(look::GAP_6))
            .children(
                (!sheets.is_empty())
                    .then(|| note(&p, "Take all factors from a loaded sample sheet:")),
            )
            .children(sheets.into_iter().enumerate().map(|(ix, (sheet, key))| {
                Button::new(("quick-factor-sheet", ix))
                    .small()
                    .label(sheet.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_factors_from(sheet.clone(), key.clone(), QuickFrom::Told, cx)
                    }))
            }))
            .child(
                Button::new("quick-import-sheet")
                    .small()
                    .label("Import a Sample Sheet…")
                    .on_click(cx.listener(|this, _, _, cx| this.quick_import_sheet(cx))),
            )
            .children((!factors.is_empty()).then(|| {
                update(
                    "quick-edit-factors",
                    factors
                        .iter()
                        .map(|f| (f.name.clone(), f.name == factor_name, Some(f.name.clone()))),
                    |q, name| q.editing_factor = name,
                    cx,
                )
            }));
        if let Some(inputs) = &q.inputs {
            let edit_factor = factor_name.to_owned();
            body = body
                .child(Self::factor_input(
                    "quick-add-factor",
                    "Add Factor",
                    inputs,
                    |i| &i.factor,
                    Edit::AddFactor,
                    cx,
                ))
                .child(Self::factor_input(
                    "quick-add-level",
                    "Add Level",
                    inputs,
                    |i| &i.group,
                    move |value| Edit::AddLevel(edit_factor.clone(), value),
                    cx,
                ));
        }
        if let Some(factor) = selected.filter(|f| !f.levels.is_empty()) {
            body = body.child(note(
                &p,
                "The first level is the reference. Change the order before comparing groups.",
            ));
            for (ix, level) in factor.levels.iter().enumerate() {
                let name = factor.name.clone();
                let other = name.clone();
                let empty = !factor.assignments.iter().any(|(_, value)| value == level);
                let remove = name.clone();
                body = body.child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(look::GAP_4))
                        .child(div().flex_1().child(if ix == 0 {
                            format!("{level} (reference)")
                        } else {
                            level.clone()
                        }))
                        .child(
                            Button::new(("quick-level-up", ix))
                                .small()
                                .label("Up")
                                .disabled(ix == 0)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_edit(
                                        Edit::MoveLevel(name.clone(), ix, ix.saturating_sub(1)),
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new(("quick-level-down", ix))
                                .small()
                                .label("Down")
                                .disabled(ix + 1 == factor.levels.len())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_edit(Edit::MoveLevel(other.clone(), ix, ix + 1), cx)
                                })),
                        )
                        .children(empty.then(|| {
                            Button::new(("quick-level-remove", ix))
                                .small()
                                .label("Remove")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_edit(Edit::RemoveLevel(remove.clone(), ix), cx)
                                }))
                        })),
                );
            }
            let levels = Rc::new(factor.levels.clone());
            let owner = cx.entity().downgrade();
            let assigned: std::collections::HashMap<_, _> = factor
                .assignments
                .iter()
                .map(|(sample, level)| (sample.as_str(), level.as_str()))
                .collect();
            for (ix, sample) in samples.iter().enumerate() {
                let levels = levels.clone();
                let owner = owner.clone();
                let name = factor.name.clone();
                let key = sample.clone();
                let current = assigned
                    .get(sample.as_str())
                    .copied()
                    .unwrap_or("Unassigned");
                body = body.child(
                    h_flex()
                        .gap(px(look::GAP_4))
                        .child(note(&p, sample.clone()).flex_1())
                        .child(
                            Button::new(("quick-factor-sample", ix))
                                .small()
                                .label(current.to_string())
                                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                    levels
                                        .iter()
                                        .map(|level| (level.as_str(), Some(level.clone())))
                                        .chain([("Unassigned", None)])
                                        .fold(menu, |menu, (label, level)| {
                                            let (owner, name, key) =
                                                (owner.clone(), name.clone(), key.clone());
                                            menu.item(
                                                PopupMenuItem::new(label.to_string()).on_click(
                                                    move |_, _, cx| {
                                                        let _ = owner.update(cx, |this, cx| {
                                                            this.quick_edit(
                                                                Edit::Group(
                                                                    name.clone(),
                                                                    key.clone(),
                                                                    level.clone(),
                                                                ),
                                                                cx,
                                                            )
                                                        });
                                                    },
                                                ),
                                            )
                                        })
                                }),
                        ),
                );
            }
        }
        body.into_any_element()
    }

    fn factor_input(
        id: &'static str,
        label: &'static str,
        inputs: &Inputs,
        field: fn(&Inputs) -> &Entity<InputState>,
        edit: impl Fn(String) -> Edit + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let input = field(inputs).clone();
        h_flex()
            .gap(px(look::GAP_4))
            .child(div().flex_1().min_w_0().child(Input::new(&input).small()))
            .child(Button::new(id).small().label(label).on_click(cx.listener(
                move |this, _, window, cx| {
                    let name = input.read(cx).value().trim().to_owned();
                    if !name.is_empty() {
                        this.quick_edit(edit(name), cx);
                        input.update(cx, |input, cx| input.set_value("", window, cx));
                    }
                },
            )))
    }

    pub(super) fn render_colour_choice(
        &self,
        table: &QuickCardTable,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let q = self.quick.as_ref()?;
        let samples = table.samples.as_ref()?.usable()?;
        let p = *Palette::get(cx);
        let choices: Vec<_> = quick::factors(&q.card)
            .iter()
            .filter(|f| f.assignments.iter().any(|(s, _)| samples.contains(s)))
            .collect();
        if choices.len() <= 1 {
            return None;
        }
        let chosen = quick::colour_factor(&q.card, table, q.colour_by.as_deref());
        Some(
            v_flex()
                .gap(px(look::GAP_4))
                .child(note(&p, "Colour samples by"))
                .child(update(
                    "quick-plot-factor",
                    choices.into_iter().map(|f| {
                        (
                            f.name.clone(),
                            chosen.is_some_and(|c| c.name == f.name),
                            Some(f.name.clone()),
                        )
                    }),
                    |q, choice| q.colour_by = choice,
                    cx,
                )),
        )
    }

    pub(super) fn render_find_changed_genes(
        &self,
        table: &QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let p = *Palette::get(cx);
        let form = &q.de;
        let mut body = v_flex().gap(px(look::GAP_6));
        if !q.show_differential {
            return body
                .child(
                    Button::new("quick-open-de")
                        .small()
                        .label("Set Up a Comparison…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(q) = &mut this.quick {
                                q.show_differential = true;
                                cx.notify();
                            }
                        })),
                )
                .into_any_element();
        }
        body = body.child(
            Button::new("quick-close-de")
                .small()
                .label("Hide Setup")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(q) = &mut this.quick {
                        q.show_differential = false;
                        cx.notify();
                    }
                })),
        );
        let choices = form.choices(&q.card, table);
        let (first, second) = form.chosen(&choices);
        let Some(first) = first else {
            return body.child(note(&p, "Add a sample factor with at least two levels in What we know. You can type levels or import a sample sheet."))
                .child(Button::new("quick-de-set-factors").small().label("Edit Sample Factors")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(q) = &mut this.quick { q.open = Some(Item::Groups); cx.notify(); }
                    }))).into_any_element();
        };
        let busy = q.busy.is_some() || q.loaded_in.is_none();
        let batch_factor = form.batch_factor(&choices, Some(first), second);
        body = body.child(heading("Method", &p)).child(update(
            "quick-de-method",
            [
                QuickDeMethod::Deseq2,
                QuickDeMethod::Edger,
                QuickDeMethod::Limma,
            ]
            .into_iter()
            .map(|m| (m.label().into(), form.method == m, m)),
            |q, m| q.de.method = m,
            cx,
        ));
        body = match form.method {
            QuickDeMethod::Deseq2 => body.child(
                Checkbox::new("quick-de-shrink")
                    .label("Shrink fold changes with apeglm")
                    .checked(form.shrink)
                    .on_click(cx.listener(|this, yes: &bool, _, cx| {
                        if let Some(q) = &mut this.quick {
                            q.de.shrink = *yes;
                            cx.notify();
                        }
                    })),
            ),
            QuickDeMethod::Limma => body.child(
                Checkbox::new("quick-de-weights")
                    .label("Estimate sample quality weights")
                    .checked(form.quality_weights)
                    .on_click(cx.listener(|this, yes: &bool, _, cx| {
                        if let Some(q) = &mut this.quick {
                            q.de.quality_weights = *yes;
                            cx.notify();
                        }
                    })),
            ),
            QuickDeMethod::Edger => body,
        };
        body = body.child(heading("What do you want to find?", &p))
            .child(update("quick-de-form", [("Ready-made design".into(), !form.own, false), ("Write my own formula".into(), form.own, true)],
                |q, own| { q.de.own = own; q.de.effect = None; }, cx))
            .child(note(&p, "A Wald test compares two things and gives their fold change. An LRT asks if several terms matter together. Its p-value answers that wider question; you pick the comparison for the fold change. edgeR and limma use F-tests for the wider question."));

        let can_run;
        if form.own {
            let formula = self.quick_input(|i| &i.formula, cx);
            body = body.child(note(
                &p,
                format!(
                    "Use the sample factors: {}.",
                    quick::factors(&q.card)
                        .iter()
                        .map(|f| de::r_term(&f.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            if let Some(inputs) = &q.inputs {
                body = body.child(Input::new(&inputs.formula).small());
            }
            body = body.child(update(
                "quick-de-test",
                [
                    (
                        de::test_label(form.method, false).into(),
                        !form.own_lrt,
                        false,
                    ),
                    (
                        de::test_label(form.method, true).to_string() + " (full vs reduced)",
                        form.own_lrt,
                        true,
                    ),
                ],
                |q, yes| q.de.own_lrt = yes,
                cx,
            ));
            if form.own_lrt {
                body = body.child(note(
                    &p,
                    "Reduced formula: leave out the terms you want to test.",
                ));
                if let Some(inputs) = &q.inputs {
                    body = body.child(Input::new(&inputs.reduced).small());
                }
            }
            if form.batch.named()
                && matches!(form.batch, de::BatchChoice::Model | de::BatchChoice::Pairs)
                && !formula.trim().is_empty()
            {
                body = body.child(note(
                    &p,
                    format!(
                        "With batch: {}",
                        de::batch_formula(&formula, form.batch, batch_factor)
                    ),
                ));
            }
            body = body
                .child(heading(
                    if form.own_lrt {
                        "Fold change to show"
                    } else {
                        "Contrast to test"
                    },
                    &p,
                ))
                .child(update(
                    "quick-de-own-contrast",
                    [
                        ("Two levels".into(), !form.by_coefficient, false),
                        ("One coefficient".into(), form.by_coefficient, true),
                    ],
                    |q, coefficient| {
                        q.de.by_coefficient = coefficient;
                        q.de.effect = None;
                    },
                    cx,
                ));
            let effects = if form.by_coefficient {
                let request = form.coefficients_request(&q.card, table, &formula, batch_factor);
                let effects: Vec<_> = form
                    .coefficients_for(&request)
                    .iter()
                    .map(|name| de::Effect::coefficient(name))
                    .collect();
                body = body.child(
                    Button::new("quick-de-coefficients")
                        .small()
                        .label("List Coefficients from R")
                        .disabled(busy || formula.trim().is_empty() || request.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(request) = request.clone() {
                                this.quick_run_analysis(request, cx);
                            }
                        })),
                );
                effects
            } else {
                body = body.child(Self::de_factor_choice(
                    "quick-de-contrast-factor",
                    &choices,
                    first,
                    cx,
                ));
                de::all_pairs(first)
            };
            can_run = !effects.is_empty()
                && !formula.trim().is_empty()
                && (!form.own_lrt || !self.quick_input(|i| &i.reduced, cx).trim().is_empty());
            body = body.child(Self::de_effects(form, effects, cx));
        } else {
            body = body
                .child(note(&p, "First factor (genotype for two factors)"))
                .child(Self::de_factor_choice(
                    "quick-de-factor-first",
                    &choices,
                    first,
                    cx,
                ));
            if choices.len() > 1 {
                body = body
                    .child(note(&p, "Second factor, if needed (dose or treatment)"))
                    .child(update(
                        "quick-de-factor-second",
                        std::iter::once(("None".into(), second.is_none(), None)).chain(
                            choices.iter().filter(|f| f.name != first.name).map(|f| {
                                (
                                    f.name.clone(),
                                    second.is_some_and(|s| s.name == f.name),
                                    Some(f.name.clone()),
                                )
                            }),
                        ),
                        |q, name| {
                            q.de.second = name;
                            q.de.question = None;
                            q.de.effect = None;
                        },
                        cx,
                    ));
            }
            let designs = de::ready_designs(first, second, &q.card);
            let chosen = form.ready(&designs);
            for (ix, design) in designs.iter().enumerate() {
                let selected = chosen.is_some_and(|d| d.question == design.question);
                let question = design.question;
                let formula = de::batch_formula(&design.formula, form.batch, batch_factor);
                let reduced = design.reduced.as_ref().map_or_else(String::new, |r| {
                    format!(
                        " against {}",
                        de::batch_formula(r, form.batch, batch_factor)
                    )
                });
                let combined = design.combined.as_ref().map_or_else(String::new, |g| {
                    format!(" ({} joins {} and {})", g.name, g.first, g.second)
                });
                let button = Button::new(("quick-de-question", ix))
                    .small()
                    .label(design.title);
                body = body.child(
                    v_flex()
                        .gap(px(look::GAP_2))
                        .child((if selected { button.primary() } else { button }).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(q) = &mut this.quick {
                                    q.de.question = Some(question);
                                    q.de.effect = None;
                                    cx.notify();
                                }
                            }),
                        ))
                        .child(note(
                            &p,
                            format!(
                                "{formula}{reduced}{combined}; {}",
                                de::test_label(form.method, design.reduced.is_some())
                            ),
                        ))
                        .child(note(&p, design.note)),
                );
            }
            can_run = chosen.is_some_and(|d| !d.effects.is_empty());
            if let Some(chosen) = chosen {
                if chosen.pick_effect {
                    body = body
                        .child(heading(
                            if chosen.reduced.is_some() {
                                "Fold change to show"
                            } else {
                                "Comparison to test"
                            },
                            &p,
                        ))
                        .child(Self::de_effects(form, chosen.effects.clone(), cx));
                } else {
                    body = body.child(note(
                        &p,
                        format!(
                            "{} results {}.",
                            chosen.effects.len(),
                            if chosen.effects.len() == 1 {
                                "table"
                            } else {
                                "tables"
                            }
                        ),
                    ));
                }
            }
        }

        body = body
            .child(heading("Batch handling", &p))
            .child(update(
                "quick-de-batch",
                de::BatchChoice::ALL
                    .into_iter()
                    .map(|batch| (batch.labels().0.into(), form.batch == batch, batch)),
                |q, batch| {
                    q.de.batch = batch;
                    q.de.effect = None;
                },
                cx,
            ))
            .child(note(&p, form.batch.labels().1));
        if form.batch.named() {
            body = body.child(note(
                &p,
                if form.batch == de::BatchChoice::Pairs {
                    "Which factor identifies pairs or donors?"
                } else {
                    "Which factor identifies the batches?"
                },
            ));
            let candidates = choices.iter().copied().filter(|f| {
                form.own || (f.name != first.name && !second.is_some_and(|s| s.name == f.name))
            });
            body = body.child(update(
                "quick-de-batch-factor",
                candidates.map(|f| {
                    (
                        f.name.clone(),
                        batch_factor == Some(&f.name),
                        Some(f.name.clone()),
                    )
                }),
                |q, factor| {
                    q.de.batch_factor = factor;
                    q.de.effect = None;
                },
                cx,
            ));
            if batch_factor.is_none() {
                body = body.child(note(
                    &p,
                    "Add a separate factor for batches or pairs in What we know.",
                ));
            }
        }
        let has_gene = table.gene.as_ref().and_then(Known::usable).is_some();
        if !has_gene {
            body = body.child(note(
                &p,
                "Choose the gene identifier column in What we know.",
            ));
        }
        body.child(
            Button::new("quick-run-de")
                .small()
                .primary()
                .w_full()
                .label("Find Changed Genes")
                .disabled(
                    busy || !can_run || !has_gene || (form.batch.named() && batch_factor.is_none()),
                )
                .on_click(cx.listener(|this, _, _, cx| this.quick_analyze(cx))),
        )
        .into_any_element()
    }

    fn de_factor_choice(
        id: &'static str,
        choices: &[&sandhill_proto::QuickFactor],
        first: &sandhill_proto::QuickFactor,
        cx: &mut Context<Self>,
    ) -> Div {
        update(
            id,
            choices
                .iter()
                .map(|f| (f.name.clone(), f.name == first.name, Some(f.name.clone()))),
            |q, name| {
                q.de.first = name;
                q.de.effect = None;
            },
            cx,
        )
    }

    fn de_effects(form: &de::DeForm, effects: Vec<de::Effect>, cx: &mut Context<Self>) -> Div {
        let Some(selected) = form.pick(&effects) else {
            return div();
        };
        let owner = cx.entity().downgrade();
        div().child(
            Button::new("quick-de-effect")
                .small()
                .label(selected.label())
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    effects.iter().fold(menu, |menu, effect| {
                        let owner = owner.clone();
                        let contrast = effect.contrast.clone();
                        menu.item(
                            PopupMenuItem::new(effect.label()).on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if let Some(q) = &mut this.quick {
                                        q.de.effect = Some(contrast.clone());
                                        cx.notify();
                                    }
                                });
                            }),
                        )
                    })
                }),
        )
    }
}
