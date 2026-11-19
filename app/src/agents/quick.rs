//! The Explore window owns its live card and the R session the templates use.

use anyhow::Context as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, v_flex};
use gpui_kit::*;
use sandhill_proto::{AgentInfo, DirEntry, EntryKind};
use sandhill_proto::{
    AppRequest, Call, Known, QuickAgentTool, QuickCard, QuickFrom, QuickMake, QuickSuggestion,
    QuickTable, QuickTableKind,
};
use serde_json::{Value, json};

use super::{Agents, Mention, Show, turned_off};
use crate::look::{self, Palette};
use crate::quick::{
    Quick, from_label, ids_label, kind_label, missing, plot_label, plots_for, values_label,
};
use crate::workbench::Workbench;

const PROVIDERS: [&str; 2] = ["codex", "claude"];

impl Agents {
    pub(crate) fn explore_ready(&self) -> bool {
        self.explore_ready.into_iter().any(|ready| ready)
    }

    fn explore_kind_ready(&self, kind: &str) -> bool {
        PROVIDERS
            .iter()
            .position(|&provider| provider == kind)
            .is_some_and(|i| self.explore_ready[i])
    }

    pub(super) fn pick_ready_explore_agent(&mut self, cx: &mut Context<Self>) {
        if !self.explore {
            return;
        }
        self.explore_ready = PROVIDERS.map(|kind| {
            if turned_off(kind)
                || !self
                    .installed
                    .iter()
                    .any(|agent| agent.kind == kind && agent.path.is_some())
            {
                return false;
            }
            let account = crate::prefs::agent_account(&self.host, kind);
            let id = account.as_deref().unwrap_or(kind);
            self.accounts
                .iter()
                .any(|account| account.id == id && account.signed_in == Some(true))
        });
        if !self.explore_kind_ready(self.new_kind)
            && self.new_choice.model.is_none()
            && let Some(kind) = PROVIDERS
                .into_iter()
                .find(|kind| self.explore_kind_ready(kind))
        {
            self.choose_new_kind(kind, cx);
        }
    }

    pub(crate) fn refresh_explore_status(&mut self, cx: &mut Context<Self>) {
        let listed = self.daemon.call::<Vec<AgentInfo>>(Call::ListAgents);
        self.refresh_accounts(cx);
        cx.spawn(async move |this, cx| {
            if let Ok(installed) = listed.await {
                _ = this.update(cx, |this, cx| {
                    for agent in &installed {
                        if agent.path.is_some()
                            && !turned_off(&agent.kind)
                            && !this
                                .installed
                                .iter()
                                .any(|old| old.kind == agent.kind && old.path == agent.path)
                        {
                            this.list_models(agent.kind.clone(), cx);
                        }
                    }
                    this.installed = installed;
                    this.pick_ready_explore_agent(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn read_explore_files(&mut self, cx: &mut Context<Self>) {
        let listed = self.daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: self.project.clone(),
            sizes: false,
            marks: false,
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            _ = this.update(cx, |this, cx| {
                match listed {
                    Ok(entries) => {
                        let mut files: Vec<_> = entries
                            .into_iter()
                            .filter(|e| {
                                e.kind == EntryKind::File && crate::quick::is_table(&e.name)
                            })
                            .map(|e| e.name)
                            .collect();
                        files.sort_unstable();
                        this.explore_files.get_or_insert_with(Vec::new);
                        this.add_explore_files(files, cx);
                    }
                    Err(err) => {
                        this.problem =
                            Some(format!("The session's tables couldn't be listed: {err:#}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn add_explore_files(&mut self, added: Vec<String>, cx: &mut Context<Self>) {
        if let Some(files) = &mut self.explore_files {
            for file in added {
                if files.contains(&file) {
                    continue;
                }
                if self.explore_seeded {
                    let chip = Mention::file(&file).item;
                    let refs = if self.draft_key == "new" {
                        &mut self.refs
                    } else {
                        &mut self.drafts.entry("new".into()).or_default().1
                    };
                    if !refs
                        .iter()
                        .any(|item| super::composer::same_ref(item, &chip))
                    {
                        refs.push(chip);
                    }
                }
                files.push(file);
            }
            cx.notify();
        }
    }

    pub(super) fn seed_explore_draft(&mut self) {
        if self.show == Show::New
            && self.starting.is_none()
            && !self.explore_seeded
            && let Some(files) = &self.explore_files
        {
            for file in files {
                let chip = Mention::file(file).item;
                if !self
                    .refs
                    .iter()
                    .any(|item| super::composer::same_ref(item, &chip))
                {
                    self.refs.push(chip);
                }
            }
            self.explore_seeded = true;
        }
    }

    pub(in crate::agents) fn explore_new_note(&self, name: &str) -> Option<String> {
        if !self.explore {
            return None;
        }
        if self.explore_files.is_none() {
            return Some("Wait for Sandhill to list the session's tables.".into());
        }
        (self
            .installed
            .iter()
            .any(|agent| agent.kind == self.new_kind && agent.path.is_some())
            && !self.explore_kind_ready(self.new_kind))
        .then(|| format!("Sign in to {name} in Welcome → Agents before sending a message."))
    }

    pub(super) fn explore_notice(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.explore || self.explore_sent {
            return None;
        }
        let provider = match self.box_kind() {
            "claude" => "Anthropic (Claude Code)",
            "codex" => "OpenAI (Codex)",
            _ => return None,
        };
        Some(v_flex()
            .flex_none()
            .mx(px(look::PANE_PAD_X))
            .mt(px(look::GAP_4))
            .gap(px(look::GAP_4))
            .text_size(px(look::small()))
            .text_color(p.muted)
            .child(format!("Your messages, and any part of your tables the AI reads, are sent to {provider}."))
            .child(Button::new("explore-without-ai").small().ghost().label("Continue Without AI")
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(workbench) = this.workbench.upgrade() {
                        workbench.update(cx, |workbench, cx| {
                            if let Some(quick) = &mut workbench.quick {
                                quick.ai_hidden = true;
                                window.focus(&workbench.focus, cx);
                                cx.notify();
                            }
                        });
                    }
                })))
            .into_any_element())
    }
}

impl Workbench {
    pub(crate) fn on_quick_request(
        &mut self,
        tool: QuickAgentTool,
        request: AppRequest,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else { return };
        let id = request.id;
        let outcome = (|| -> anyhow::Result<Value> {
            let quick = self
                .quick
                .as_mut()
                .context("Open this session in Explore a Table first.")?;
            match tool {
                QuickAgentTool::State => Ok(state(quick)),
                QuickAgentTool::Suggest => {
                    anyhow::ensure!(
                        quick.card_read,
                        "Wait for the Explore card to finish loading."
                    );
                    let input = serde_json::from_value::<QuickSuggestion>(request.params)
                        .context("The suggestion does not match the Explore card's facts.")?;
                    let name = input.table.as_deref().or(quick.shown.as_deref());
                    let table =
                        name.and_then(|name| quick.tables.iter().find(|table| table.name == name));
                    anyhow::ensure!(
                        input.table.is_none() || table.is_some(),
                        "No loaded table has that name. Use a table name from quick_state."
                    );
                    let reply = suggest(&mut quick.card, table, input);
                    Ok(reply)
                }
                QuickAgentTool::Make => {
                    let input = serde_json::from_value::<QuickMake>(request.params)
                        .context("Choose a plot name listed by quick_state.")?;
                    self.quick_run_plot(
                        input.plot,
                        input.table.as_deref(),
                        input.gene.as_deref().unwrap_or_default(),
                        Some(id),
                        cx,
                    )?;
                    Ok(Value::Null)
                }
            }
        })();
        let (result, error) = match outcome {
            Ok(_) if tool == QuickAgentTool::Make => return, // The template answers when it finishes.
            Ok(result)
                if tool == QuickAgentTool::Suggest
                    && result["set"].as_array().is_some_and(|set| !set.is_empty()) =>
            {
                self.quick_save_card_answer(Some((id, result)), cx);
                cx.notify();
                return;
            }
            Ok(result) => (Some(result), None),
            Err(err) => (None, Some(format!("{err:#}"))),
        };
        daemon.fire(Call::Answer { id, result, error });
    }

    pub(crate) fn quick_accept(
        &mut self,
        pick: Option<(Option<&str>, &str)>,
        cx: &mut Context<Self>,
    ) {
        if let Some(quick) = &mut self.quick
            && accept(&mut quick.card, pick)
        {
            self.quick_save_card_answer(None, cx);
            cx.notify();
        }
    }
}

fn state(quick: &Quick) -> Value {
    let card = &quick.card;
    let templates: Vec<_> = quick
        .tables
        .iter()
        .filter_map(|table| {
            let known = card.tables.iter().find(|known| known.name == table.name)?;
            let kind = known
                .kind
                .usable()
                .copied()
                .unwrap_or(QuickTableKind::Unknown);
            let plots: Vec<_> = plots_for(kind)
                .iter()
                .map(|&plot| {
                    json!({
                        "plot": plot,
                        "name": plot_label(plot),
                        "missing": missing(known, plot, ""),
                    })
                })
                .collect();
            Some(json!({"table": table.name, "plots": plots}))
        })
        .collect();
    json!({
        "card": {
            "tables": card.tables,
            "organism": card.organism,
            "comparison": card.comparison,
            "groups": card.groups,
            "cutoffs": {
                "value": card.cutoffs,
                "from": if card.cutoffs == Default::default() { "default" } else { "told" },
            },
        },
        "tables": quick.tables,
        "shown": quick.shown,
        "templates": templates,
        "busy": quick.busy,
        "ready": quick.card_read && quick.loaded_in.is_some() && quick.busy.is_none(),
        "error": quick.error,
    })
}

#[derive(Default)]
struct Suggested {
    set: Vec<&'static str>,
    skipped: Vec<Value>,
}

impl Suggested {
    fn skip(&mut self, fact: &'static str, reason: &'static str) {
        self.skipped.push(json!({"fact": fact, "reason": reason}));
    }

    fn set<T>(
        &mut self,
        field: &'static str,
        slot: &mut Option<Known<T>>,
        value: Option<T>,
        check: impl FnOnce(&T) -> bool,
        invalid: &'static str,
    ) -> bool {
        let Some(value) = value else { return false };
        if let Some(known) = slot.as_ref().filter(|k| k.usable().is_some()) {
            self.skip(field, from_label(known.from));
        } else if !check(&value) {
            self.skip(field, invalid);
        } else {
            *slot = Some(Known {
                value,
                from: QuickFrom::Suggested,
            });
            self.set.push(field);
            return true;
        }
        false
    }
}

fn suggest(card: &mut QuickCard, loaded: Option<&QuickTable>, input: QuickSuggestion) -> Value {
    let mut reply = Suggested::default();
    let text = |name: &String| !name.trim().is_empty();
    let mut table =
        loaded.and_then(|loaded| card.tables.iter_mut().find(|t| t.name == loaded.name));
    reply.set(
        "organism",
        &mut card.organism,
        input.organism,
        text,
        "Name an organism.",
    );
    reply.set(
        "comparison",
        &mut card.comparison,
        input.comparison,
        |c| text(&c.higher) && text(&c.lower) && c.higher.trim() != c.lower.trim(),
        "Name two different groups.",
    );
    reply.set(
        "groups",
        &mut card.groups,
        input.groups,
        |groups| {
            !groups.is_empty()
                && groups.iter().enumerate().all(|(i, (sample, group))| {
                    text(sample)
                        && text(group)
                        && !groups[..i].iter().any(|(other, _)| other == sample)
                })
        },
        "Give each sample one group.",
    );
    macro_rules! table_fact {
        ($field:ident, $check:expr, $invalid:expr) => {{
            if let Some(table) = table.as_deref_mut() {
                reply.set(
                    stringify!($field),
                    &mut table.$field,
                    input.$field,
                    $check,
                    $invalid,
                )
            } else {
                if input.$field.is_some() {
                    reply.skip(stringify!($field), "Choose a loaded table for this fact.");
                }
                false
            }
        }};
    }
    if let Some(kind) = input.kind {
        match table.as_deref_mut() {
            // Found Unknown records that no kind could be found, not an identified kind.
            Some(t)
                if t.kind.from == QuickFrom::Suggested
                    || (t.kind.from == QuickFrom::Found
                        && t.kind.value == QuickTableKind::Unknown) =>
            {
                if kind == QuickTableKind::Unknown {
                    reply.skip("kind", "Suggest a kind the table could be.");
                } else {
                    t.kind = Known {
                        value: kind,
                        from: QuickFrom::Suggested,
                    };
                    reply.set.push("kind");
                }
            }
            Some(t) => reply.skip("kind", from_label(t.kind.from)),
            None => reply.skip("kind", "Choose a loaded table for this fact."),
        }
    }
    let column =
        |name: &String| loaded.is_some_and(|t| t.columns.iter().any(|col| col.name == *name));
    let bad_column = "Use a column name from the loaded table.";
    table_fact!(gene, column, bad_column);
    table_fact!(fc, column, bad_column);
    table_fact!(padj, column, bad_column);
    if table_fact!(mean, column, bad_column) {
        table
            .as_deref_mut()
            .expect("a mean was set on the table")
            .mean_log = input.mean_log.unwrap_or(false);
        if input.mean_log.is_some() {
            reply.set.push("mean_log");
        }
    } else if input.mean_log.is_some() {
        reply.skip("mean_log", "It goes with a new average expression column.");
    }
    table_fact!(term, column, bad_column);
    table_fact!(count, column, bad_column);
    table_fact!(ratio, column, bad_column);
    table_fact!(gene_ids, |_| true, "");
    table_fact!(values, |_| true, "");
    table_fact!(
        samples,
        |samples: &Vec<String>| !samples.is_empty()
            && samples
                .iter()
                .enumerate()
                .all(|(i, sample)| column(sample) && !samples[..i].contains(sample)),
        "List distinct sample columns from the loaded table."
    );
    json!({"set": reply.set, "skipped": reply.skipped})
}

pub(crate) struct Suggestion<'a> {
    pub table: Option<&'a str>,
    pub field: &'static str,
    pub label: &'static str,
    pub value: String,
}

pub(crate) fn suggestions(card: &QuickCard) -> Vec<Suggestion<'_>> {
    let mut rows = Vec::new();
    macro_rules! row {
        ($table:expr, $field:ident, $known:expr, $label:literal, $text:expr) => {
            if let Some(known) = $known.filter(|k| k.from == QuickFrom::Suggested) {
                rows.push(Suggestion {
                    table: $table,
                    field: stringify!($field),
                    label: $label,
                    value: ($text)(&known.value),
                });
            }
        };
    }
    row!(
        None,
        organism,
        card.organism.as_ref(),
        "Organism",
        String::clone
    );
    row!(
        None,
        comparison,
        card.comparison.as_ref(),
        "Positive means",
        |c: &sandhill_proto::QuickComparison| format!("higher in {} than in {}", c.higher, c.lower)
    );
    row!(
        None,
        groups,
        card.groups.as_ref(),
        "Groups",
        |groups: &Vec<(String, String)>| groups
            .iter()
            .map(|(sample, group)| format!("{sample}: {group}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for t in &card.tables {
        let table = Some(t.name.as_str());
        row!(
            table,
            kind,
            Some(&t.kind),
            "Table kind",
            |k: &QuickTableKind| kind_label(*k).to_string()
        );
        row!(table, gene, t.gene.as_ref(), "Genes", String::clone);
        row!(
            table,
            gene_ids,
            t.gene_ids.as_ref(),
            "Gene IDs",
            |i: &sandhill_proto::GeneIds| { ids_label(*i).to_string() }
        );
        row!(table, fc, t.fc.as_ref(), "Fold change", String::clone);
        row!(table, padj, t.padj.as_ref(), "Adjusted p", String::clone);
        row!(
            table,
            mean,
            t.mean.as_ref(),
            "Average expression",
            |name: &String| if t.mean_log {
                format!("{name} (already logged)")
            } else {
                format!("{name} (not logged)")
            }
        );
        row!(
            table,
            samples,
            t.samples.as_ref(),
            "Sample columns",
            |s: &Vec<String>| s.join(", ")
        );
        row!(
            table,
            values,
            t.values.as_ref(),
            "Sample values",
            |v: &sandhill_proto::QuickValues| { values_label(*v).to_string() }
        );
        row!(table, term, t.term.as_ref(), "Terms", String::clone);
        row!(table, count, t.count.as_ref(), "Gene count", String::clone);
        row!(table, ratio, t.ratio.as_ref(), "Gene ratio", String::clone);
    }
    rows
}

fn accept(card: &mut QuickCard, pick: Option<(Option<&str>, &str)>) -> bool {
    fn fact<T>(known: &mut Known<T>) -> bool {
        if known.from != QuickFrom::Suggested {
            return false;
        }
        known.from = QuickFrom::Told;
        true
    }
    let mut changed = false;
    let picked = |table, field| pick.is_none_or(|pick| pick == (table, field));
    macro_rules! fields {
        ($table:expr, $parent:ident; $($field:ident),+ $(,)?) => {
            $(if picked($table, stringify!($field)) {
                changed |= $parent.$field.as_mut().is_some_and(fact);
            })+
        };
    }
    fields!(None, card; organism, comparison, groups);
    for t in &mut card.tables {
        let table = Some(t.name.as_str());
        if picked(table, "kind") && fact(&mut t.kind) {
            t.question = None;
            changed = true;
        }
        fields!(table, t; gene, gene_ids, fc, padj, mean, samples, term, count, ratio);
        if picked(table, "values") && t.values.as_mut().is_some_and(fact) {
            t.question = None;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{accept, suggest, suggestions};
    use sandhill_proto::{Known, QuickCard, QuickComparison, QuickSuggestion};
    use serde_json::json;

    #[test]
    fn suggestions_cannot_replace_known_facts_or_enter_templates_unaccepted() {
        let mut card = QuickCard {
            organism: Some(Known::found("human".into())),
            ..Default::default()
        };
        let guess = QuickComparison {
            higher: "treated".into(),
            lower: "control".into(),
        };
        let report = suggest(
            &mut card,
            None,
            QuickSuggestion {
                organism: Some("mouse".into()),
                comparison: Some(guess.clone()),
                ..Default::default()
            },
        );
        assert_eq!(report["set"], json!(["comparison"]));
        assert_eq!(report["skipped"][0]["fact"], "organism");
        assert_eq!(card.organism.as_ref().unwrap().value, "human");
        assert_eq!(suggestions(&card)[0].field, "comparison");
        assert!(card.comparison.as_ref().unwrap().usable().is_none());
        assert!(accept(&mut card, Some((None, "comparison"))));
        assert_eq!(card.comparison.unwrap(), Known::told(guess));
    }
}
