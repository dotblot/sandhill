use sandhill_proto::{
    QuickCard, QuickCardTable, QuickComparison, QuickDeBatch, QuickDeCombined, QuickDeContrast,
    QuickDeDesign, QuickDeMethod, QuickDeOutput, QuickDeSettings, QuickFactor, QuickRequest,
};

use super::{factors, file_safe, usable, variable_name, variable_name_with_limit};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Question {
    TwoGroups,
    EachReference,
    AnyGroup,
    WithinGenotype,
    GenotypePerDose,
    SameAtEveryDose,
    Interaction,
    RespondAtAll,
    Trend,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum BatchChoice {
    #[default]
    None,
    Model,
    Pairs,
    Sva,
    Ruv,
    CombatSeq,
    CombatRef,
}

impl BatchChoice {
    pub(crate) const ALL: [Self; 7] = [
        Self::None,
        Self::Model,
        Self::Pairs,
        Self::Sva,
        Self::Ruv,
        Self::CombatSeq,
        Self::CombatRef,
    ];

    pub(crate) const fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::None => ("None", "No batch correction."),
            Self::Model => (
                "Batch in the model",
                "Recommended when batches were recorded. The test separates batch from the comparison.",
            ),
            Self::Pairs => (
                "Pairs or donors in the model",
                "For measurements before and after, or matched samples.",
            ),
            Self::Sva => (
                "Unknown batches: SVA",
                "svaseq estimates hidden factors when batches were not recorded.",
            ),
            Self::Ruv => (
                "Unknown batches: RUVSeq",
                "RUVg estimates hidden factors using the least changed genes from a first pass.",
            ),
            Self::CombatSeq => (
                "ComBat-seq",
                "Corrects the counts before testing. Batch in the model is usually preferred.",
            ),
            Self::CombatRef => (
                "ComBat-ref",
                "Corrects the counts toward the batch with the least dispersion, which is kept unchanged.",
            ),
        }
    }

    pub(crate) const fn named(self) -> bool {
        matches!(
            self,
            Self::Model | Self::Pairs | Self::CombatSeq | Self::CombatRef
        )
    }

    fn request(self, factor: Option<&str>) -> Option<QuickDeBatch> {
        Some(match self {
            Self::None => QuickDeBatch::None,
            Self::Model => QuickDeBatch::Model {
                factor: factor?.into(),
            },
            Self::Pairs => QuickDeBatch::Pairs {
                factor: factor?.into(),
            },
            Self::Sva => QuickDeBatch::Sva,
            Self::Ruv => QuickDeBatch::Ruv,
            Self::CombatSeq => QuickDeBatch::CombatSeq {
                factor: factor?.into(),
            },
            Self::CombatRef => QuickDeBatch::CombatRef {
                factor: factor?.into(),
            },
        })
    }
}

#[derive(Default)]
pub(crate) struct DeForm {
    pub method: QuickDeMethod,
    pub first: Option<String>,
    pub second: Option<String>,
    pub question: Option<Question>,
    pub effect: Option<QuickDeContrast>,
    pub own: bool,
    pub own_lrt: bool,
    pub by_coefficient: bool,
    pub coefficients: Option<(QuickRequest, Vec<String>)>,
    pub batch: BatchChoice,
    pub batch_factor: Option<String>,
    pub shrink: bool,
    pub quality_weights: bool,
}

#[derive(Clone)]
pub(crate) struct Effect {
    pub(crate) contrast: QuickDeContrast,
    pub(crate) comparison: QuickComparison,
}

impl Effect {
    pub(crate) fn label(&self) -> String {
        format!("{} vs {}", self.comparison.higher, self.comparison.lower)
    }

    fn levels(factor: &str, higher: &str, lower: &str) -> Self {
        Self {
            comparison: QuickComparison {
                higher: higher.into(),
                lower: lower.into(),
            },
            contrast: QuickDeContrast::Levels {
                factor: factor.into(),
                higher: higher.into(),
                lower: lower.into(),
                at: vec![],
            },
        }
    }

    pub(crate) fn coefficient(name: &str) -> Self {
        Self {
            contrast: QuickDeContrast::Coefficient { name: name.into() },
            comparison: QuickComparison {
                higher: name.into(),
                lower: "zero".into(),
            },
        }
    }
}

pub(crate) struct ReadyDesign {
    pub question: Question,
    pub title: &'static str,
    pub note: &'static str,
    pub formula: String,
    pub reduced: Option<String>,
    pub combined: Option<QuickDeCombined>,
    pub numeric: Option<String>,
    pub effects: Vec<Effect>,
    /// Run only the effect selected from this list rather than every effect.
    pub pick_effect: bool,
}

impl ReadyDesign {
    fn new(
        question: Question,
        title: &'static str,
        note: &'static str,
        formula: String,
        effects: Vec<Effect>,
    ) -> Self {
        Self {
            question,
            title,
            note,
            formula,
            reduced: None,
            combined: None,
            numeric: None,
            effects,
            pick_effect: false,
        }
    }

    fn design(&self, card: &QuickCard) -> QuickDeDesign {
        QuickDeDesign {
            formula: self.formula.clone(),
            factors: factors(card).to_vec(),
            combined: self.combined.clone(),
            numeric: self.numeric.clone(),
        }
    }
}

pub(crate) fn r_term(name: &str) -> String {
    let mut chars = name.chars();
    if chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'))
        && !matches!(
            name,
            "if" | "else"
                | "repeat"
                | "while"
                | "function"
                | "for"
                | "in"
                | "next"
                | "break"
                | "TRUE"
                | "FALSE"
                | "NULL"
                | "Inf"
                | "NaN"
                | "NA"
                | "NA_integer_"
                | "NA_real_"
                | "NA_complex_"
                | "NA_character_"
        )
    {
        name.into()
    } else {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
    }
}

pub(crate) fn all_pairs(factor: &QuickFactor) -> Vec<Effect> {
    factor
        .levels
        .iter()
        .flat_map(|lower| {
            factor
                .levels
                .iter()
                .filter(move |higher| *higher != lower)
                .map(move |higher| Effect::levels(&factor.name, higher, lower))
        })
        .collect()
}

pub(crate) fn ready_designs(
    first: &QuickFactor,
    second: Option<&QuickFactor>,
    card: &QuickCard,
) -> Vec<ReadyDesign> {
    let a = r_term(&first.name);
    let form = |terms: &str| format!("~ {terms}");
    let reference = |factor: &QuickFactor| {
        factor
            .levels
            .iter()
            .skip(1)
            .map(|higher| Effect::levels(&factor.name, higher, &factor.levels[0]))
            .collect()
    };
    let mut designs = Vec::new();
    if let Some(second) = second {
        let b = r_term(&second.name);
        let mut group = "group".to_string();
        let mut i = 2;
        while factors(card).iter().any(|f| f.name == group) {
            group = format!("group_{i}");
            i += 1;
        }
        let combined = QuickDeCombined {
            name: group.clone(),
            first: first.name.clone(),
            second: second.name.clone(),
        };
        let pair = |a: &str, b: &str| format!("{a}_{b}");
        let within: Vec<_> = first
            .levels
            .iter()
            .flat_map(|a| {
                second.levels.iter().skip(1).map(|higher| {
                    Effect::levels(&group, &pair(a, higher), &pair(a, &second.levels[0]))
                })
            })
            .collect();
        let across = second
            .levels
            .iter()
            .flat_map(|b| {
                first.levels.iter().skip(1).map(|higher| {
                    Effect::levels(&group, &pair(higher, b), &pair(&first.levels[0], b))
                })
            })
            .collect();
        let mut within_design = ReadyDesign::new(
            Question::WithinGenotype,
            "Effect of treatment within each genotype",
            "Recommended start: compare each dose with its reference separately in each genotype.",
            form(&group),
            within,
        );
        within_design.combined = Some(combined.clone());
        designs.push(within_design);
        let mut across_design = ReadyDesign::new(
            Question::GenotypePerDose,
            "Effect of genotype at each dose",
            "Use when each dose may show a different genotype effect.",
            form(&group),
            across,
        );
        across_design.combined = Some(combined);
        designs.push(across_design);
        let mut additive = ReadyDesign::new(
            Question::SameAtEveryDose,
            "Genotype effect, the same at every dose",
            "Use when the genotype is expected to change genes the same way at every dose.",
            form(&format!("{b} + {a}")),
            all_pairs(first),
        );
        additive.pick_effect = true;
        designs.push(additive);
        let full = form(&format!("{a} + {b} + {a}:{b}"));
        let interaction = first
            .levels
            .iter()
            .skip(1)
            .flat_map(|a| {
                second.levels.iter().skip(1).map(move |b| Effect {
                    contrast: QuickDeContrast::Interaction {
                        first: first.name.clone(),
                        second: second.name.clone(),
                        first_higher: a.clone(),
                        second_higher: b.clone(),
                    },
                    comparison: QuickComparison {
                        higher: format!("{b} effect in {a}"),
                        lower: format!("{b} effect in {}", first.levels[0]),
                    },
                })
            })
            .collect();
        let mut interact = ReadyDesign::new(
            Question::Interaction,
            "Does treatment act differently by genotype?",
            "Use to test whether the dose response differs by genotype.",
            full.clone(),
            interaction,
        );
        if second.levels.len() > 2 {
            interact.reduced = Some(form(&format!("{a} + {b}")));
            interact.pick_effect = true;
        }
        designs.push(interact);
        let respond = first
            .levels
            .iter()
            .flat_map(|a| {
                second.levels.iter().skip(1).map(|dose| {
                    let mut effect = Effect::levels(&second.name, dose, &second.levels[0]);
                    effect.comparison = QuickComparison {
                        higher: pair(a, dose),
                        lower: pair(a, &second.levels[0]),
                    };
                    if let QuickDeContrast::Levels { at, .. } = &mut effect.contrast {
                        at.push((first.name.clone(), a.clone()));
                    }
                    effect
                })
            })
            .collect();
        let mut response = ReadyDesign::new(
            Question::RespondAtAll,
            "Genes that respond to treatment in either genotype",
            "Use to find a response at any dose, whether or not genotypes respond differently.",
            full,
            respond,
        );
        response.reduced = Some(form(&a));
        response.pick_effect = true;
        designs.push(response);
        let numeric = if is_numeric(second) {
            Some((second, Some(&a)))
        } else if is_numeric(first) {
            Some((first, Some(&b)))
        } else {
            None
        };
        if let Some((dose, covariate)) = numeric {
            designs.push(trend(dose, covariate.map(String::as_str)));
        }
    } else {
        let mut two = ReadyDesign::new(
            Question::TwoGroups,
            "Compare two groups",
            "Use for one comparison, such as treated vs control.",
            form(&a),
            all_pairs(first),
        );
        two.pick_effect = true;
        designs.push(two);
        designs.push(ReadyDesign::new(
            Question::EachReference,
            "Each group against the reference",
            "Use for one table per group compared with the first level.",
            form(&a),
            reference(first),
        ));
        if first.levels.len() >= 3 {
            let mut any = ReadyDesign::new(
                Question::AnyGroup,
                "Any difference among the groups",
                "Use to find whether any of three or more groups differ.",
                form(&a),
                all_pairs(first),
            );
            any.reduced = Some("~ 1".into());
            any.pick_effect = true;
            designs.push(any);
        }
        if is_numeric(first) {
            designs.push(trend(first, None));
        }
    }
    designs
}

fn is_numeric(factor: &QuickFactor) -> bool {
    let parsed: Vec<_> = factor
        .levels
        .iter()
        .filter_map(|level| level.parse::<f64>().ok().filter(|x| x.is_finite()))
        .collect();
    parsed.len() == factor.levels.len()
        && parsed
            .iter()
            .enumerate()
            .all(|(i, x)| !parsed[..i].contains(x))
}

fn trend(dose: &QuickFactor, covariate: Option<&str>) -> ReadyDesign {
    let name = r_term(&dose.name);
    let formula = match covariate {
        Some(covariate) => format!("~ {covariate} + {name}"),
        None => format!("~ {name}"),
    };
    let mut effect = Effect::coefficient(&name);
    effect.comparison.higher = format!("one more unit of {}", dose.name);
    effect.comparison.lower = "the starting dose".into();
    let mut design = ReadyDesign::new(
        Question::Trend,
        "Genes that rise or fall with dose",
        "Use when a steady increase or decrease per unit of dose is plausible.",
        formula,
        vec![effect],
    );
    design.numeric = Some(dose.name.clone());
    design
}

pub(crate) fn batch_formula(formula: &str, choice: BatchChoice, factor: Option<&str>) -> String {
    match (choice, factor, formula.trim().strip_prefix('~')) {
        (BatchChoice::Model | BatchChoice::Pairs, Some(factor), Some(rhs)) => {
            format!("~ {} + {}", r_term(factor), rhs.trim())
        }
        _ => formula.to_string(),
    }
}

pub(crate) fn test_label(method: QuickDeMethod, lrt: bool) -> &'static str {
    match (method, lrt) {
        (QuickDeMethod::Deseq2, false) => "Wald test",
        (QuickDeMethod::Deseq2, true) => "Likelihood ratio test (LRT)",
        (QuickDeMethod::Edger, _) => "Quasi-likelihood F-test",
        (QuickDeMethod::Limma, false) => "Moderated t-test",
        (QuickDeMethod::Limma, true) => "Moderated F-test",
    }
}

impl DeForm {
    pub(crate) fn choices<'a>(
        &self,
        card: &'a QuickCard,
        table: &QuickCardTable,
    ) -> Vec<&'a QuickFactor> {
        let samples = table.samples.as_ref().and_then(|k| k.usable());
        factors(card)
            .iter()
            .filter(|f| {
                f.levels.len() >= 2
                    && samples.is_some_and(|samples| {
                        f.assignments.iter().any(|(s, _)| samples.contains(s))
                    })
            })
            .collect()
    }

    pub(crate) fn chosen<'a>(
        &self,
        choices: &[&'a QuickFactor],
    ) -> (Option<&'a QuickFactor>, Option<&'a QuickFactor>) {
        let first = choices
            .iter()
            .copied()
            .find(|f| Some(&f.name) == self.first.as_ref())
            .or_else(|| {
                choices.iter().copied().find(|f| {
                    ["condition", "genotype", "treatment", "group"]
                        .iter()
                        .any(|name| f.name.eq_ignore_ascii_case(name))
                })
            })
            .or_else(|| choices.first().copied());
        let second = choices.iter().copied().find(|f| {
            Some(&f.name) == self.second.as_ref()
                && Some(f.name.as_str()) != first.map(|f| f.name.as_str())
        });
        (first, second)
    }

    pub(crate) fn ready<'a>(&self, designs: &'a [ReadyDesign]) -> Option<&'a ReadyDesign> {
        designs
            .iter()
            .find(|d| Some(d.question) == self.question)
            .or_else(|| designs.first())
    }

    pub(crate) fn batch_factor<'a>(
        &self,
        choices: &[&'a QuickFactor],
        first: Option<&QuickFactor>,
        second: Option<&QuickFactor>,
    ) -> Option<&'a str> {
        if !self.batch.named() {
            return None;
        }
        let candidates = || {
            choices.iter().copied().filter(|f| {
                self.own
                    || (Some(f.name.as_str()) != first.map(|f| f.name.as_str())
                        && Some(f.name.as_str()) != second.map(|f| f.name.as_str()))
            })
        };
        candidates()
            .find(|f| Some(&f.name) == self.batch_factor.as_ref())
            .or_else(|| {
                candidates().find(|f| match self.batch {
                    BatchChoice::Pairs => ["donor", "pair", "subject"]
                        .iter()
                        .any(|word| f.name.to_ascii_lowercase().contains(word)),
                    _ => f.name.to_ascii_lowercase().contains("batch"),
                })
            })
            .or_else(|| candidates().next())
            .map(|f| f.name.as_str())
    }

    pub(crate) fn pick<'a>(&self, effects: &'a [Effect]) -> Option<&'a Effect> {
        effects
            .iter()
            .find(|e| Some(&e.contrast) == self.effect.as_ref())
            .or_else(|| effects.first())
    }

    pub(crate) fn own_design(&self, card: &QuickCard, formula: &str) -> QuickDeDesign {
        QuickDeDesign {
            formula: formula.trim().into(),
            factors: factors(card).to_vec(),
            numeric: None,
            combined: None,
        }
    }

    pub(crate) fn coefficients_request(
        &self,
        card: &QuickCard,
        table: &QuickCardTable,
        formula: &str,
        batch: Option<&str>,
    ) -> Option<QuickRequest> {
        Some(QuickRequest::DeCoefficients {
            samples: usable(&table.samples)?,
            design: Box::new(self.own_design(card, formula)),
            batch: self.batch.request(batch)?,
        })
    }

    pub(crate) fn coefficients_for(&self, request: &Option<QuickRequest>) -> &[String] {
        self.coefficients
            .as_ref()
            .filter(|(old, _)| Some(old) == request.as_ref())
            .map_or(&[], |(_, names)| names)
    }

    pub(crate) fn active_design(
        &self,
        card: &QuickCard,
        table: &QuickCardTable,
        formula: &str,
    ) -> Option<(QuickDeDesign, Option<String>)> {
        let choices = self.choices(card, table);
        let (first, second) = self.chosen(&choices);
        let batch = self
            .batch_factor(&choices, first, second)
            .map(str::to_owned);
        let design = if self.own {
            if formula.trim().is_empty() {
                return None;
            }
            self.own_design(card, formula)
        } else {
            self.ready(&ready_designs(first?, second, card))?
                .design(card)
        };
        Some((design, batch))
    }

    pub(crate) fn request(
        &self,
        card: &QuickCard,
        table: &QuickCardTable,
        formula: &str,
        reduced: &str,
        taken: &mut Vec<String>,
    ) -> Option<QuickRequest> {
        let choices = self.choices(card, table);
        let (first, second) = self.chosen(&choices);
        let batch_name = self.batch_factor(&choices, first, second);
        let batch = self.batch.request(batch_name)?;
        let (design, reduced, effects) = if self.own {
            if formula.trim().is_empty() || (self.own_lrt && reduced.trim().is_empty()) {
                return None;
            }
            let effects = if self.by_coefficient {
                self.coefficients_for(&self.coefficients_request(card, table, formula, batch_name))
                    .iter()
                    .map(|name| Effect::coefficient(name))
                    .collect()
            } else {
                all_pairs(first?)
            };
            (
                self.own_design(card, formula),
                self.own_lrt.then(|| reduced.trim().into()),
                vec![self.pick(&effects)?.clone()],
            )
        } else {
            let designs = ready_designs(first?, second, card);
            let ready = self.ready(&designs)?;
            let effects = if ready.pick_effect {
                vec![self.pick(&ready.effects)?.clone()]
            } else {
                ready.effects.clone()
            };
            (ready.design(card), ready.reduced.clone(), effects)
        };
        let samples = usable(&table.samples)?;
        if samples.len() < 3 {
            return None;
        }
        let gene = usable(&table.gene)?;
        let method = match self.method {
            QuickDeMethod::Deseq2 => "deseq2",
            QuickDeMethod::Edger => "edger",
            QuickDeMethod::Limma => "limma",
        };
        // Reserve room for the method and comparison even with a long input name.
        let short = |name: &str, width: usize| -> String { name.chars().take(width).collect() };
        let stem = format!("{}_{method}", short(&table.name, 20));
        let outputs = effects
            .into_iter()
            .map(|effect| {
                let base = file_safe(&format!(
                    "{stem}_{}_vs_{}.csv",
                    short(&effect.comparison.higher, 14),
                    short(&effect.comparison.lower, 14)
                ));
                let name = variable_name_with_limit(&base, None, taken, 60);
                taken.push(name.clone());
                QuickDeOutput {
                    name,
                    contrast: effect.contrast,
                    comparison: effect.comparison,
                }
            })
            .collect();
        let normalized = variable_name(&format!("{stem}_normalized.csv"), None, taken);
        Some(QuickRequest::Differential {
            table: table.name.clone(),
            settings: Box::new(QuickDeSettings {
                method: self.method,
                design,
                reduced,
                batch,
                shrink: self.shrink && self.method == QuickDeMethod::Deseq2,
                quality_weights: self.quality_weights && self.method == QuickDeMethod::Limma,
                gene,
                label: usable(&table.labels),
                samples,
                cutoffs: card.cutoffs,
                outputs,
                normalized,
                directory: "results".into(),
            }),
        })
    }
}
