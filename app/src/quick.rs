//! Explore a Table in a session's window (`docs/design/quick-tools-plan.md`):
//! the session's R kernel, its tables loaded into R and recognised, the
//! What we know card, and the plots Sandhill's R templates make. The window
//! is drawn by `view::quick`.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use sandhill_proto::{
    Call, DirEntry, EntryKind, GeneIds, Known, QuickCard, QuickCardTable, QuickComparison,
    QuickCutoffs, QuickFrom, QuickPlot, QuickRequest, QuickResult, QuickSettings, QuickTable,
    QuickTableKind, QuickValues,
};
use serde_json::Value;

use crate::workbench::{Session, Workbench};

/// The table files Sandhill explores, as the daemon's `quick::folder`.
const TABLE_EXTENSIONS: &[&str] = &["csv", "tsv", "tab", "txt", "xlsx", "xls"];

/// A fact on the card that can be opened to change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    Kind,
    Gene,
    Fc,
    Padj,
    Mean,
    Values,
    Term,
    Count,
    Ratio,
    Organism,
    Groups,
    Comparison,
    Cutoffs,
}

/// A change to the card.
pub(crate) enum Edit {
    Kind(QuickTableKind),
    Column(Item, String),
    Values(QuickValues),
    Organism(Option<String>),
    /// `(sample, group)`; no group takes the sample out.
    Group(String, Option<String>),
    AddGroup(String),
    Comparison(String, String),
    Cutoffs(f64, f64),
}

/// The fields the card's forms type into.
pub(crate) struct Inputs {
    pub higher: Entity<InputState>,
    pub lower: Entity<InputState>,
    pub group: Entity<InputState>,
    pub gene: Entity<InputState>,
    pub padj: Entity<InputState>,
    pub lfc: Entity<InputState>,
    /// The kept folder's name.
    pub keep_name: Entity<InputState>,
}

/// Why the user is asked to keep the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepReason {
    /// Closing the window after making something.
    Close,
    /// Save Session….
    Save,
    /// Export… writes files, so they need a folder.
    Export,
    /// Open as Project needs a folder to open.
    OpenProject,
}

/// The question "Keep the plots and steps?", showing.
pub(crate) struct Keeping {
    pub reason: KeepReason,
    /// The folder the kept one goes in: the first table's at first.
    pub parent: String,
    pub busy: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub(crate) struct Quick {
    pub card: QuickCard,
    /// The card has been read from the session folder.
    pub card_read: bool,
    /// The tables loaded into the session, as described when loaded.
    pub tables: Vec<QuickTable>,
    /// Tables the templates made (top genes): `(R variable, made from)`.
    pub outputs: Vec<(String, String)>,
    /// The card table picked: what the card describes and plots use.
    pub shown: Option<String>,
    /// The R variable the Data Explorer shows: the picked table, or a
    /// table made from it.
    pub viewing: Option<String>,
    /// Group names added on the card that no sample is in yet.
    pub added_groups: Vec<String>,
    /// The session the tables were loaded into.
    pub loaded_in: Option<String>,
    /// What is happening, in a few words.
    pub busy: Option<String>,
    pub error: Option<String>,
    /// R or its kernel is missing: Welcome → Getting Ready sets it up.
    pub not_ready: bool,
    /// What the last plot shows.
    pub sentence: Option<String>,
    pub open: Option<Item>,
    pub inputs: Option<Inputs>,
    /// Where the last export went.
    pub exported: Option<String>,
    /// A plot or table was made, or something exported: closing asks
    /// whether to keep the session (Huan, 2026-11-19).
    pub made: bool,
    pub keeping: Option<Keeping>,
    /// The window is closing after the question was answered.
    pub closing: bool,
}

pub(crate) fn is_table(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && TABLE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    })
}

fn is_excel(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xlsx") || lower.ends_with(".xls")
}

/// An R variable for a table: its file's name (and sheet), made a plain R
/// name, numbered when taken: `de_results`, `samples_Sheet1`, `de_results_2`.
pub(crate) fn variable_name(file: &str, sheet: Option<&str>, taken: &[String]) -> String {
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    let raw = match sheet {
        Some(sheet) => format!("{stem}_{sheet}"),
        None => stem.to_string(),
    };
    let mut name: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let mut name = name.trim_matches('_').to_string();
    if !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name.insert_str(0, "table_");
    }
    name.truncate(48);
    let mut candidate = name.clone();
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = format!("{name}_{n}");
        n += 1;
    }
    candidate
}

/// A table's facts as recognised, all marked Found.
pub(crate) fn card_table(table: &QuickTable) -> QuickCardTable {
    let r = &table.recognition;
    let found = |v: &Option<String>| v.clone().map(Known::found);
    QuickCardTable {
        name: table.name.clone(),
        file: table.file.clone(),
        sheet: table.sheet.clone(),
        kind: Known::found(r.kind),
        gene: found(&r.gene),
        gene_ids: r.gene_ids.map(Known::found),
        fc: found(&r.fc),
        padj: found(&r.padj),
        mean: found(&r.mean),
        mean_log: r.mean_log,
        samples: (!r.samples.is_empty()).then(|| Known::found(r.samples.clone())),
        values: r.values.map(Known::found),
        term: found(&r.term),
        count: found(&r.count),
        ratio: found(&r.ratio),
        dates: r.dates,
        question: r.question.clone(),
    }
}

/// Adds a loaded table to the card, keeping what the card already says
/// about it.
pub(crate) fn merge(card: &mut QuickCard, table: &QuickTable) {
    match card
        .tables
        .iter_mut()
        .find(|t| t.file == table.file && t.sheet == table.sheet)
    {
        Some(known) => known.name = table.name.clone(),
        None => card.tables.push(card_table(table)),
    }
    let r = &table.recognition;
    if card.organism.is_none()
        && let Some(organism) = &r.organism
    {
        card.organism = Some(Known::found(organism.clone()));
    }
    if card.groups.is_none() && !r.groups.is_empty() {
        card.groups = Some(Known::found(r.groups.clone()));
    }
}

pub(crate) fn kind_label(kind: QuickTableKind) -> &'static str {
    match kind {
        QuickTableKind::Results => "Differential expression results",
        QuickTableKind::Counts => "Raw counts",
        QuickTableKind::Normalized => "Normalized expression",
        QuickTableKind::SampleSheet => "Sample sheet",
        QuickTableKind::Enrichment => "Enrichment results",
        QuickTableKind::Unknown => "Not recognised",
    }
}

pub(crate) fn values_label(values: QuickValues) -> &'static str {
    match values {
        QuickValues::Counts => "Raw counts",
        QuickValues::Normalized => "Normalized (CPM, TPM, FPKM)",
        QuickValues::Logged => "Already on a log scale",
    }
}

pub(crate) fn ids_label(ids: GeneIds) -> &'static str {
    match ids {
        GeneIds::Ensembl => "Ensembl IDs",
        GeneIds::Symbol => "gene symbols",
        GeneIds::Entrez => "Entrez IDs",
    }
}

pub(crate) fn plot_label(plot: QuickPlot) -> &'static str {
    match plot {
        QuickPlot::Volcano => "Volcano plot",
        QuickPlot::Ma => "MA plot",
        QuickPlot::UpDown => "Genes up and down",
        QuickPlot::TopGenes => "Top genes table",
        QuickPlot::ResultsHeatmap => "Heatmap of top genes",
        QuickPlot::LibrarySizes => "Library sizes",
        QuickPlot::Pca => "PCA of samples",
        QuickPlot::Correlation => "Sample correlation",
        QuickPlot::VariableHeatmap => "Heatmap of variable genes",
        QuickPlot::GeneByGroup => "One gene by group",
        QuickPlot::EnrichDot => "Dot plot of terms",
        QuickPlot::EnrichBar => "Bar plot of terms",
    }
}

/// What can be made from a kind of table.
pub(crate) fn plots_for(kind: QuickTableKind) -> &'static [QuickPlot] {
    match kind {
        QuickTableKind::Results => &[
            QuickPlot::Volcano,
            QuickPlot::Ma,
            QuickPlot::UpDown,
            QuickPlot::TopGenes,
            QuickPlot::ResultsHeatmap,
        ],
        QuickTableKind::Counts => &[
            QuickPlot::LibrarySizes,
            QuickPlot::Pca,
            QuickPlot::Correlation,
            QuickPlot::VariableHeatmap,
            QuickPlot::GeneByGroup,
        ],
        QuickTableKind::Normalized => &[
            QuickPlot::Pca,
            QuickPlot::Correlation,
            QuickPlot::VariableHeatmap,
            QuickPlot::GeneByGroup,
        ],
        QuickTableKind::Enrichment => &[QuickPlot::EnrichDot, QuickPlot::EnrichBar],
        QuickTableKind::SampleSheet | QuickTableKind::Unknown => &[],
    }
}

fn usable<T: Clone>(known: &Option<Known<T>>) -> Option<T> {
    known.as_ref().and_then(|k| k.usable().cloned())
}

/// The groups of `table`'s samples.
fn groups_of(card: &QuickCard, table: &QuickCardTable) -> Vec<(String, String)> {
    let samples = usable(&table.samples).unwrap_or_default();
    usable(&card.groups)
        .unwrap_or_default()
        .into_iter()
        .filter(|(sample, _)| samples.contains(sample))
        .collect()
}

/// What a plot still needs, in plain words; empty when it can be made.
pub(crate) fn missing(
    table: &QuickCardTable,
    plot: QuickPlot,
    show_gene: &str,
) -> Vec<&'static str> {
    let mut needs = Vec::new();
    let mut need = |ok: bool, what: &'static str| {
        if !ok {
            needs.push(what);
        }
    };
    let samples = usable(&table.samples).map_or(0, |s| s.len());
    match plot {
        QuickPlot::Volcano | QuickPlot::UpDown | QuickPlot::TopGenes => {
            need(usable(&table.fc).is_some(), "the fold change column");
            need(usable(&table.padj).is_some(), "the adjusted p-value column");
        }
        QuickPlot::Ma => {
            need(usable(&table.fc).is_some(), "the fold change column");
            need(usable(&table.padj).is_some(), "the adjusted p-value column");
            need(
                usable(&table.mean).is_some(),
                "the average expression column",
            );
        }
        QuickPlot::ResultsHeatmap => {
            need(usable(&table.fc).is_some(), "the fold change column");
            need(usable(&table.padj).is_some(), "the adjusted p-value column");
            need(samples >= 2, "a value for each sample");
            need(
                usable(&table.values).is_some(),
                "what the sample values are",
            );
        }
        QuickPlot::LibrarySizes => {
            need(samples >= 2, "the sample columns");
            need(
                usable(&table.values) == Some(QuickValues::Counts),
                "raw counts",
            );
        }
        QuickPlot::Pca | QuickPlot::Correlation | QuickPlot::VariableHeatmap => {
            need(samples >= 2, "the sample columns");
            need(usable(&table.values).is_some(), "what the values are");
        }
        QuickPlot::GeneByGroup => {
            need(samples >= 2, "the sample columns");
            need(usable(&table.values).is_some(), "what the values are");
            need(usable(&table.gene).is_some(), "the gene column");
            need(!show_gene.trim().is_empty(), "a gene to show");
        }
        QuickPlot::EnrichDot => {
            need(usable(&table.term).is_some(), "the term column");
            need(usable(&table.padj).is_some(), "the adjusted p-value column");
            need(
                usable(&table.count).is_some() || usable(&table.ratio).is_some(),
                "the gene count or ratio column",
            );
        }
        QuickPlot::EnrichBar => {
            need(usable(&table.term).is_some(), "the term column");
            need(usable(&table.padj).is_some(), "the adjusted p-value column");
        }
    }
    needs
}

/// What a template is told: the card's usable facts only.
pub(crate) fn settings(card: &QuickCard, table: &QuickCardTable, show_gene: &str) -> QuickSettings {
    QuickSettings {
        gene: usable(&table.gene),
        fc: usable(&table.fc),
        padj: usable(&table.padj),
        mean: usable(&table.mean),
        mean_log: table.mean_log,
        samples: usable(&table.samples).unwrap_or_default(),
        values: usable(&table.values),
        groups: groups_of(card, table),
        comparison: usable(&card.comparison),
        cutoffs: card.cutoffs,
        term: usable(&table.term),
        count: usable(&table.count),
        ratio: usable(&table.ratio),
        show_gene: (!show_gene.trim().is_empty()).then(|| show_gene.trim().to_string()),
    }
}

/// Applies a change to the card; table facts change on `table`.
pub(crate) fn apply(card: &mut QuickCard, table: Option<&str>, edit: Edit) {
    let at = table.and_then(|name| card.tables.iter().position(|t| t.name == name));
    match edit {
        Edit::Kind(kind) => {
            if let Some(ix) = at {
                card.tables[ix].kind = Known::told(kind);
                card.tables[ix].question = None;
            }
        }
        Edit::Column(item, column) => {
            if let Some(ix) = at {
                let t = &mut card.tables[ix];
                let slot = match item {
                    Item::Gene => &mut t.gene,
                    Item::Fc => &mut t.fc,
                    Item::Padj => &mut t.padj,
                    Item::Mean => &mut t.mean,
                    Item::Term => &mut t.term,
                    Item::Count => &mut t.count,
                    Item::Ratio => &mut t.ratio,
                    _ => return,
                };
                *slot = Some(Known::told(column));
            }
        }
        Edit::Values(values) => {
            if let Some(ix) = at {
                card.tables[ix].values = Some(Known::told(values));
                card.tables[ix].question = None;
            }
        }
        Edit::Organism(organism) => card.organism = organism.map(Known::told),
        Edit::Group(sample, group) => {
            let mut groups = card.groups.take().map(|g| g.value).unwrap_or_default();
            groups.retain(|(s, _)| *s != sample);
            if let Some(group) = group {
                groups.push((sample, group));
            }
            card.groups = Some(Known::told(groups));
        }
        Edit::AddGroup(_) => {}
        Edit::Comparison(higher, lower) => {
            let (higher, lower) = (higher.trim().to_string(), lower.trim().to_string());
            card.comparison = (!higher.is_empty() && !lower.is_empty())
                .then(|| Known::told(QuickComparison { higher, lower }));
        }
        Edit::Cutoffs(padj, lfc) => card.cutoffs = QuickCutoffs { padj, lfc },
    }
}

/// The group names on the card, in first-seen order, and any added.
pub(crate) fn group_names(card: &QuickCard, added: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, group) in usable(&card.groups).unwrap_or_default() {
        if !names.contains(&group) {
            names.push(group);
        }
    }
    for name in added {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

/// The workbook's Read_me rows: what each table is and what the numbers
/// mean.
pub(crate) fn readme(card: &QuickCard, session: &str) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Made with".to_string(),
        format!(
            "Sandhill, Explore a Table session “{session}”. steps.R in the session folder repeats it."
        ),
    )];
    match usable(&card.comparison) {
        Some(c) => rows.push((
            "Comparison".into(),
            format!(
                "{} vs {}: a positive log2 fold change means higher in {}.",
                c.higher, c.lower, c.higher
            ),
        )),
        None => rows.push((
            "Comparison".into(),
            "Not recorded: the table doesn't say which group a positive fold change is higher in."
                .into(),
        )),
    }
    rows.push((
        "Cutoffs".into(),
        format!(
            "Changed genes have an adjusted p-value below {} and a log2 fold change of at least {} either way.",
            card.cutoffs.padj, card.cutoffs.lfc
        ),
    ));
    for table in &card.tables {
        let from = match &table.sheet {
            Some(sheet) => format!("{} (sheet {sheet})", table.file),
            None => table.file.clone(),
        };
        rows.push((
            table.name.clone(),
            format!("{}, from {from}.", kind_label(table.kind.value)),
        ));
    }
    rows
}

/// The methods paragraph written beside the workbook.
pub(crate) fn methods(card: &QuickCard) -> String {
    let mut text = String::from(
        "Methods\n\nTables were read and plotted in R with Sandhill's fixed templates (ggplot2, ggrepel, pheatmap, readxl, writexl).",
    );
    text.push_str(&format!(
        " Genes were counted as changed when their adjusted p-value was below {} and their absolute log2 fold change was at least {}.",
        card.cutoffs.padj, card.cutoffs.lfc
    ));
    match usable(&card.comparison) {
        Some(c) => text.push_str(&format!(
            " Positive log2 fold changes mean higher expression in {} than in {}.",
            c.higher, c.lower
        )),
        None => text.push_str(
            " The results table did not record which group a positive fold change is higher in.",
        ),
    }
    text.push_str(
        " Sample-level plots use log2(counts per million + 1) for raw counts and log2(value + 1) for normalized values; heatmaps scale each gene to its own mean.\n\nThe exact steps are in steps.R in the session folder.\n",
    );
    text
}

impl Workbench {
    fn quick_mut(&mut self) -> Option<&mut Quick> {
        self.quick.as_mut()
    }

    /// The session's start: its card, its fields, and an R session.
    pub(crate) fn quick_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        if let Some(quick) = &mut self.quick
            && quick.inputs.is_none()
        {
            let field = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
            };
            let inputs = Inputs {
                higher: field("treated", window, cx),
                lower: field("control", window, cx),
                group: field("New group", window, cx),
                gene: field("Gene, like STAT1", window, cx),
                padj: field("0.05", window, cx),
                lfc: field("1", window, cx),
                keep_name: field("Folder name", window, cx),
            };
            let gene = inputs.gene.clone();
            cx.subscribe_in(&gene, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
                if let InputEvent::PressEnter { .. } = event {
                    this.quick_make(QuickPlot::GeneByGroup, cx);
                }
            })
            .detach();
            quick.inputs = Some(inputs);
        }
        self.recover_console_sessions(cx);
        let kernel = quick_kernel(&self.kernels);
        match kernel {
            Some(kernel) => {
                if let Some(quick) = self.quick_mut() {
                    quick.not_ready = false;
                    quick.busy = Some("Starting R…".into());
                }
                self.choose_console_kernel(kernel, cx);
            }
            None => {
                if let Some(quick) = self.quick_mut() {
                    quick.not_ready = true;
                }
            }
        }
        let read = daemon.call::<QuickCard>(Call::QuickCard { project });
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some(quick) = this.quick_mut() {
                    match read {
                        Ok(card) => quick.card = card,
                        Err(err) => quick.error = Some(format!("The card can't be read: {err:#}")),
                    }
                    quick.card_read = true;
                }
                this.quick_session_changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The R session became ready (or another one did): the tables are read
    /// into it.
    pub(crate) fn quick_session_changed(&mut self, cx: &mut Context<Self>) {
        let Session::Ready(session) = self.loose.clone() else {
            if let (Some(quick), Session::Failed(err)) = (&mut self.quick, &self.loose) {
                quick.busy = None;
                quick.error = Some(format!("R didn't start: {err}"));
            }
            return;
        };
        let Some(quick) = &self.quick else {
            return;
        };
        if !quick.card_read || quick.loaded_in.as_deref() == Some(&session) {
            return;
        }
        if let Some(quick) = self.quick_mut() {
            quick.loaded_in = Some(session.clone());
        }
        self.quick_load(session, false, cx);
    }

    /// Reads the folder's tables into `session`: every one, or only those
    /// not on the card yet.
    fn quick_load(&mut self, session: String, only_new: bool, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(quick) = &mut self.quick else {
            return;
        };
        quick.busy = Some("Reading your tables…".into());
        quick.error = None;
        let known: Vec<QuickCardTable> = quick.card.tables.clone();
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: project.clone(),
            sizes: false,
            marks: false,
        });
        let handle = self.window_handle;
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut problems = Vec::new();
            let files: Vec<String> = match listed.await {
                Ok(entries) => {
                    let mut files: Vec<String> = entries
                        .into_iter()
                        .filter(|e| e.kind == EntryKind::File && is_table(&e.name))
                        .map(|e| e.name)
                        .collect();
                    files.sort();
                    files
                }
                Err(err) => {
                    problems.push(format!("The session folder can't be read: {err:#}"));
                    Vec::new()
                }
            };
            let run = |request: QuickRequest| {
                daemon.call::<QuickResult>(Call::QuickRun {
                    session: session.clone(),
                    project: project.clone(),
                    request,
                })
            };
            let mut wanted: Vec<(String, Option<String>, String)> = Vec::new();
            if !only_new {
                wanted.extend(
                    known
                        .iter()
                        .filter(|t| files.contains(&t.file))
                        .map(|t| (t.file.clone(), t.sheet.clone(), t.name.clone())),
                );
            }
            let mut taken: Vec<String> = known.iter().map(|t| t.name.clone()).collect();
            for file in files
                .iter()
                .filter(|f| !known.iter().any(|t| &t.file == *f))
            {
                let sheets: Vec<Option<String>> = if is_excel(file) {
                    match run(QuickRequest::Sheets { file: file.clone() }).await {
                        Ok(result) => result.sheets.into_iter().map(Some).collect(),
                        Err(err) => {
                            problems.push(format!("{file}: {err:#}"));
                            continue;
                        }
                    }
                } else {
                    vec![None]
                };
                let many = sheets.len() > 1;
                for sheet in sheets {
                    let name = variable_name(file, sheet.as_deref().filter(|_| many), &taken);
                    taken.push(name.clone());
                    wanted.push((file.clone(), sheet, name));
                }
            }
            let mut loaded = Vec::new();
            for (file, sheet, name) in wanted {
                match run(QuickRequest::Load {
                    file: file.clone(),
                    sheet,
                    name,
                })
                .await
                {
                    Ok(QuickResult {
                        table: Some(table), ..
                    }) => loaded.push(table),
                    Ok(_) => problems.push(format!("{file}: the table didn't come back")),
                    Err(err) => problems.push(format!("{file}: {err:#}")),
                }
            }
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.quick_loaded(session, loaded, problems, window, cx);
                    }
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    fn quick_loaded(
        &mut self,
        session: String,
        loaded: Vec<QuickTable>,
        problems: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(quick) = &mut self.quick else {
            return;
        };
        quick.busy = None;
        quick.error = (!problems.is_empty()).then(|| problems.join("\n"));
        for table in loaded {
            merge(&mut quick.card, &table);
            quick.tables.retain(|t| t.name != table.name);
            quick.tables.push(table);
        }
        let shown = quick
            .shown
            .clone()
            .filter(|name| quick.tables.iter().any(|t| &t.name == name))
            .or_else(|| quick.card.tables.first().map(|t| t.name.clone()));
        self.quick_save_card(cx);
        if let Some(name) = shown {
            self.quick_show(session, name, window, cx);
        }
        // A sample sheet with a condition column gives the groups.
        if self.quick.as_ref().is_some_and(|q| q.card.groups.is_none())
            && let Some((table, key, value)) = self.quick.as_ref().and_then(|q| {
                sheet_columns(q)
                    .into_iter()
                    .find_map(|(t, k, cols)| guess_group_column(&cols).map(|v| (t, k, v)))
            })
        {
            self.quick_groups_from(table, key, value, QuickFrom::Found, cx);
        }
        cx.notify();
    }

    /// Sets the groups from two columns of a sample sheet.
    pub(crate) fn quick_groups_from(
        &mut self,
        table: String,
        key: String,
        value: String,
        from: QuickFrom,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(session) = self.quick.as_ref().and_then(|q| q.loaded_in.clone()) else {
            return;
        };
        let read = daemon.call::<QuickResult>(Call::QuickRun {
            session,
            project,
            request: QuickRequest::Pairs { table, key, value },
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                let Some(quick) = &mut this.quick else {
                    return;
                };
                match read {
                    Ok(result) => {
                        let samples: Vec<String> = quick
                            .card
                            .tables
                            .iter()
                            .filter_map(|t| t.samples.as_ref())
                            .flat_map(|s| s.value.iter().cloned())
                            .collect();
                        let pairs: Vec<(String, String)> = result
                            .pairs
                            .into_iter()
                            .filter(|(sample, _)| samples.contains(sample))
                            .collect();
                        if pairs.is_empty() {
                            if from == QuickFrom::Told {
                                quick.error = Some(
                                    "None of the sample sheet's samples are columns of the tables."
                                        .into(),
                                );
                            }
                        } else if from == QuickFrom::Told || quick.card.groups.is_none() {
                            quick.card.groups = Some(Known { value: pairs, from });
                            this.quick_save_card(cx);
                        }
                    }
                    Err(err) => quick.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a table (or a made one) in the Data Explorer.
    fn quick_show(
        &mut self,
        session: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(quick) = self.quick_mut() {
            quick.shown = Some(name.clone());
            quick.viewing = Some(name.clone());
            quick.open = None;
            quick.sentence = None;
        }
        self.open_session_explorer(session, "r".into(), vec![name], window, cx);
    }

    /// Shows a table a template made, keeping the card on its source.
    pub(crate) fn quick_view_output(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.quick.as_ref().and_then(|q| q.loaded_in.clone()) else {
            return;
        };
        if let Some(quick) = self.quick_mut() {
            quick.viewing = Some(name.clone());
        }
        self.open_session_explorer(session, "r".into(), vec![name], window, cx);
    }

    /// The title bar's table capsules pick the table shown.
    pub(crate) fn quick_pick(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.quick.as_ref().and_then(|q| q.loaded_in.clone()) else {
            return;
        };
        self.quick_show(session, name, window, cx);
    }

    fn quick_save_card(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project), Some(quick)) =
            (self.daemon(), self.project.clone(), &self.quick)
        else {
            return;
        };
        let saved = daemon.call::<Value>(Call::SaveQuickCard {
            project,
            card: quick.card.clone(),
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = saved.await {
                this.update(cx, |this, cx| {
                    if let Some(quick) = this.quick_mut() {
                        quick.error = Some(format!("The card couldn't be saved: {err:#}"));
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The card entry of the table shown.
    pub(crate) fn quick_table(&self) -> Option<&QuickCardTable> {
        let quick = self.quick.as_ref()?;
        let shown = quick.shown.as_deref()?;
        quick.card.tables.iter().find(|t| t.name == shown)
    }

    pub(crate) fn quick_edit(&mut self, edit: Edit, cx: &mut Context<Self>) {
        let Some(quick) = &mut self.quick else {
            return;
        };
        // Placing samples in groups takes several clicks: the form stays.
        let stays = matches!(edit, Edit::Group(..) | Edit::AddGroup(_));
        if let Edit::AddGroup(name) = &edit {
            let name = name.trim().to_string();
            if !name.is_empty() && !quick.added_groups.contains(&name) {
                quick.added_groups.push(name);
            }
            cx.notify();
            return;
        }
        let shown = quick.shown.clone();
        apply(&mut quick.card, shown.as_deref(), edit);
        if !stays {
            quick.open = None;
        }
        self.quick_save_card(cx);
        cx.notify();
    }

    pub(crate) fn quick_toggle(&mut self, item: Item, cx: &mut Context<Self>) {
        if let Some(quick) = &mut self.quick {
            quick.open = if quick.open == Some(item) {
                None
            } else {
                Some(item)
            };
            cx.notify();
        }
    }

    /// The text typed in one of the card's fields.
    pub(crate) fn quick_input(&self, pick: fn(&Inputs) -> &Entity<InputState>, cx: &App) -> String {
        self.quick
            .as_ref()
            .and_then(|q| q.inputs.as_ref())
            .map(|i| pick(i).read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// Makes a plot (or the top genes table) from the table shown.
    pub(crate) fn quick_make(&mut self, plot: QuickPlot, cx: &mut Context<Self>) {
        let show_gene = self.quick_input(|i| &i.gene, cx);
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(quick) = &self.quick else {
            return;
        };
        let (Some(session), Some(table)) = (quick.loaded_in.clone(), self.quick_table().cloned())
        else {
            return;
        };
        if quick.busy.is_some() || !missing(&table, plot, &show_gene).is_empty() {
            return;
        }
        let settings = settings(&quick.card, &table, &show_gene);
        let output = (plot == QuickPlot::TopGenes).then(|| {
            let taken: Vec<String> = quick
                .tables
                .iter()
                .map(|t| t.name.clone())
                .chain(quick.outputs.iter().map(|(n, _)| n.clone()))
                .collect();
            variable_name(&format!("{}_top_genes", table.name), None, &taken)
        });
        let made = daemon.call::<QuickResult>(Call::QuickRun {
            session: session.clone(),
            project,
            request: QuickRequest::Plot {
                plot,
                table: table.name.clone(),
                settings: Box::new(settings),
                output,
            },
        });
        if let Some(quick) = self.quick_mut() {
            quick.busy = Some(format!("Making the {}…", plot_label(plot).to_lowercase()));
            quick.error = None;
            quick.sentence = None;
        }
        let handle = self.window_handle;
        let generation = self.generation;
        let from = table.name.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let made = made.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    let mut show = None;
                    if let Some(quick) = this.quick_mut() {
                        quick.busy = None;
                        match made {
                            Ok(result) => {
                                quick.made = true;
                                quick.sentence = result.sentence;
                                if let Some(output) = result.output {
                                    quick.outputs.retain(|(n, _)| *n != output);
                                    quick.outputs.push((output.clone(), from));
                                    show = Some(output);
                                }
                            }
                            Err(err) => quick.error = Some(format!("{err:#}")),
                        }
                    }
                    if let Some(name) = show {
                        this.quick_view_output(name, window, cx);
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// Writes every table, with a Read_me sheet, to `results/<session>.xlsx`
    /// and the methods paragraph to `results/methods.txt`.
    pub(crate) fn quick_export(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(quick) = &self.quick else {
            return;
        };
        let Some(session) = quick.loaded_in.clone() else {
            return;
        };
        if quick.busy.is_some() {
            return;
        }
        let name = self.quick_name();
        let workbook = format!("results/{}.xlsx", file_safe(&name));
        let mut sheets: Vec<(String, String)> = quick
            .card
            .tables
            .iter()
            .filter(|t| quick.tables.iter().any(|l| l.name == t.name))
            .map(|t| (t.name.clone(), t.name.clone()))
            .collect();
        sheets.extend(quick.outputs.iter().map(|(n, _)| (n.clone(), n.clone())));
        let readme = readme(&quick.card, &name);
        let methods_text = methods(&quick.card);
        let workbook_call = QuickRequest::Workbook {
            path: workbook.clone(),
            readme,
            sheets,
        };
        // methods.txt first, so the workbook's run copies both to a kept
        // folder.
        let methods_path = format!("{project}/results/methods.txt");
        let methods_written = daemon.call::<Value>(Call::WriteFile {
            path: methods_path,
            text: methods_text,
            expected_mtime_ms: None,
            base64: false,
            create: false,
        });
        let shown_in = quick.card.kept.clone().unwrap_or_else(|| project.clone());
        if let Some(quick) = self.quick_mut() {
            quick.busy = Some("Saving the workbook…".into());
            quick.error = None;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let written = match methods_written.await {
                Ok(_) => daemon
                    .call::<QuickResult>(Call::QuickRun {
                        session,
                        project: project.clone(),
                        request: workbook_call,
                    })
                    .await
                    .map(|_| ()),
                Err(err) => Err(err),
            };
            this.update(cx, |this, cx| {
                if let Some(quick) = this.quick_mut() {
                    quick.busy = None;
                    match written {
                        Ok(()) => {
                            quick.made = true;
                            quick.exported = Some(format!("{shown_in}/{workbook}"));
                        }
                        Err(err) => quick.error = Some(format!("Saving didn't work: {err:#}")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Tables dropped onto the window join the session.
    pub(crate) fn quick_add(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let tables: Vec<String> = paths.into_iter().filter(|p| is_table(p)).collect();
        if tables.is_empty() {
            if let Some(quick) = self.quick_mut() {
                quick.error = Some("Sandhill explores CSV, TSV, text and Excel tables.".into());
            }
            cx.notify();
            return;
        }
        let added = daemon.call::<Vec<String>>(Call::QuickAddTables { project, tables });
        cx.spawn(async move |this, cx| {
            let added = added.await;
            this.update(cx, |this, cx| {
                match added {
                    Ok(_) => {
                        if let Some(session) = this.quick.as_ref().and_then(|q| q.loaded_in.clone())
                        {
                            this.quick_load(session, true, cx);
                        }
                    }
                    Err(err) => {
                        if let Some(quick) = this.quick_mut() {
                            quick.error = Some(format!("{err:#}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The session's name: the kept folder's, else the first table's.
    pub(crate) fn quick_name(&self) -> String {
        let Some(quick) = &self.quick else {
            return String::new();
        };
        if let Some(kept) = &quick.card.kept {
            return crate::panes::files::base_name(kept).to_string();
        }
        if quick.card.scratch
            && let Some(first) = quick.card.sources.first()
        {
            let base = crate::panes::files::base_name(first);
            return base
                .rsplit_once('.')
                .map_or(base, |(stem, _)| stem)
                .to_string();
        }
        self.project
            .as_deref()
            .map(session_name)
            .unwrap_or_default()
    }

    /// Still only in Sandhill's storage: keeping asks where first.
    pub(crate) fn quick_unkept(&self) -> bool {
        self.quick
            .as_ref()
            .is_some_and(|q| q.card.scratch && q.card.kept.is_none())
    }

    /// The folder the session's files are in for the user: where it was
    /// kept, else its own.
    pub(crate) fn quick_folder(&self) -> Option<String> {
        let quick = self.quick.as_ref()?;
        if quick.card.scratch {
            quick.card.kept.clone()
        } else {
            self.project.clone()
        }
    }

    /// Asks where to keep the session, and under which name.
    pub(crate) fn quick_ask_keep(
        &mut self,
        reason: KeepReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = format!("{} (Sandhill)", self.quick_name());
        let Some(quick) = &mut self.quick else {
            return;
        };
        let parent = quick
            .card
            .sources
            .first()
            .and_then(|s| std::path::Path::new(s).parent())
            .map(|p| p.to_string_lossy().into_owned())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_default();
        if let Some(inputs) = &quick.inputs {
            inputs
                .keep_name
                .update(cx, |input, cx| input.set_value(name, window, cx));
        }
        quick.keeping = Some(Keeping {
            reason,
            parent,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    /// Export…, Save Session… and Open as Project need a folder: they ask
    /// to keep the session first when it isn't kept.
    pub(crate) fn quick_needs_folder(
        &mut self,
        reason: KeepReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.quick_unkept() {
            self.quick_ask_keep(reason, window, cx);
        } else {
            self.quick_after_keep(reason, window, cx);
        }
    }

    pub(crate) fn quick_keep_change_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Keep It Here".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(keeping) = this.quick.as_mut().and_then(|q| q.keeping.as_mut()) {
                    keeping.parent = folder.to_string_lossy().into_owned();
                    keeping.error = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Keep: makes the folder the user named, then does what asked for it.
    pub(crate) fn quick_keep(&mut self, cx: &mut Context<Self>) {
        let name = self.quick_input(|i| &i.keep_name, cx);
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(keeping) = self.quick.as_mut().and_then(|q| q.keeping.as_mut()) else {
            return;
        };
        if keeping.busy {
            return;
        }
        keeping.busy = true;
        keeping.error = None;
        let reason = keeping.reason;
        let kept = daemon.call::<String>(Call::QuickKeep {
            project,
            parent: keeping.parent.clone(),
            name,
        });
        let handle = self.window_handle;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let kept = kept.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let Some(quick) = &mut this.quick else {
                        return;
                    };
                    match kept {
                        Ok(folder) => {
                            crate::prefs::remember_quick_session(&folder);
                            quick.card.kept = Some(folder);
                            quick.keeping = None;
                            this.quick_after_keep(reason, window, cx);
                        }
                        Err(err) => {
                            if let Some(keeping) = &mut quick.keeping {
                                keeping.busy = false;
                                keeping.error = Some(format!("{err:#}"));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// What the folder was wanted for, once there is one.
    fn quick_after_keep(
        &mut self,
        reason: KeepReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match reason {
            KeepReason::Save => {}
            KeepReason::Export => self.quick_export(cx),
            KeepReason::Close => self.quick_finish_close(window, cx),
            KeepReason::OpenProject => {
                let Some(folder) = self.quick_folder() else {
                    return;
                };
                let host = self.host();
                self.quick_finish_close(window, cx);
                cx.defer(move |cx| crate::shell::app_window::open_in_main(host, folder, cx));
            }
        }
    }

    /// Don't Keep: the session leaves nothing behind.
    pub(crate) fn quick_dont_keep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(quick) = &mut self.quick {
            quick.keeping = None;
        }
        self.quick_finish_close(window, cx);
    }

    pub(crate) fn quick_cancel_keep(&mut self, cx: &mut Context<Self>) {
        if let Some(quick) = &mut self.quick
            && quick.keeping.as_ref().is_some_and(|k| !k.busy)
        {
            quick.keeping = None;
            cx.notify();
        }
    }

    /// The window is asked to close. With something made and not kept, it
    /// asks whether to keep it, and stays open (`true`). Otherwise the
    /// session is ended and the window may close (`false`).
    pub(crate) fn quick_close_requested(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(quick) = &self.quick else {
            return false;
        };
        if quick.closing {
            return false;
        }
        if quick.keeping.is_some() {
            return true;
        }
        if quick.made && self.quick_unkept() {
            self.quick_ask_keep(KeepReason::Close, window, cx);
            return true;
        }
        self.quick_end_session(cx);
        false
    }

    /// Ends the session's R and removes its copy in Sandhill's storage; a
    /// kept folder has everything made so far.
    fn quick_end_session(&mut self, cx: &mut Context<Self>) {
        let Some(quick) = &mut self.quick else {
            return;
        };
        quick.closing = true;
        let scratch = quick.card.scratch;
        self.shut_down_project_kernels(cx);
        if scratch && let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) {
            // Sent at once; the reply isn't needed.
            drop(daemon.call::<Value>(Call::QuickDiscard { project }));
        }
    }

    fn quick_finish_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.quick_end_session(cx);
        window.defer(cx, |window, _| window.remove_window());
    }
}

/// Each sample sheet loaded: its R variable, its sample column, and its
/// other text columns, which may hold groups.
pub(crate) fn sheet_columns(quick: &Quick) -> Vec<(String, String, Vec<String>)> {
    quick
        .card
        .tables
        .iter()
        .filter(|t| t.kind.usable() == Some(&QuickTableKind::SampleSheet))
        .filter_map(|t| {
            let loaded = quick.tables.iter().find(|l| l.name == t.name)?;
            let key = loaded.recognition.sample_column.clone()?;
            let columns = loaded
                .columns
                .iter()
                .filter(|c| c.name != key && c.kind != sandhill_proto::QuickColumnKind::Number)
                .filter(|c| c.distinct >= 2 && c.distinct < loaded.rows.max(2))
                .map(|c| c.name.clone())
                .collect();
            Some((t.name.clone(), key, columns))
        })
        .collect()
}

/// The column that most likely holds the groups.
pub(crate) fn guess_group_column(columns: &[String]) -> Option<String> {
    const NAMES: &[&str] = &[
        "condition",
        "group",
        "treatment",
        "genotype",
        "status",
        "disease",
    ];
    NAMES.iter().find_map(|wanted| {
        columns
            .iter()
            .find(|c| c.to_ascii_lowercase().contains(wanted))
            .cloned()
    })
}

/// The R kernel a session starts: IRkernel before Ark, the newest R first.
pub(crate) fn quick_kernel(kernels: &[sandhill_proto::KernelspecInfo]) -> Option<String> {
    let key = |v: &Option<String>| -> Vec<u32> {
        v.as_deref()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    kernels
        .iter()
        .filter(|k| k.language.eq_ignore_ascii_case("r"))
        .max_by(|a, b| {
            let irkernel =
                |k: &sandhill_proto::KernelspecInfo| k.engine.as_deref() == Some("IRkernel");
            irkernel(a)
                .cmp(&irkernel(b))
                .then_with(|| key(&a.version).cmp(&key(&b.version)))
        })
        .map(|k| k.name.clone())
}

/// "de_results (Sandhill) 2" → "de_results".
pub(crate) fn session_name(folder: &str) -> String {
    let base = crate::panes::files::base_name(folder);
    match base.rfind(" (Sandhill)") {
        Some(at) => base[..at].to_string(),
        None => base.to_string(),
    }
}

fn file_safe(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' || c == ':' { '_' } else { c })
        .collect()
}

/// A fact's origin, as the card says it.
pub(crate) fn from_label(from: QuickFrom) -> &'static str {
    match from {
        QuickFrom::Found => "found in the table",
        QuickFrom::Told => "you said",
        QuickFrom::Suggested => "suggested",
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's own `test` macro would replace `#[test]`.
    use super::{
        Edit, GeneIds, Item, Known, QuickCard, QuickComparison, QuickFrom, QuickPlot, QuickTable,
        QuickTableKind, apply, group_names, is_table, merge, missing, session_name, settings,
        variable_name,
    };
    use sandhill_proto::{QuickColumn, QuickColumnKind, QuickRecognition};

    fn recognition(kind: QuickTableKind) -> QuickRecognition {
        QuickRecognition {
            kind,
            tool: None,
            gene: Some("id".into()),
            gene_ids: Some(GeneIds::Ensembl),
            organism: Some("human".into()),
            fc: Some("log2FoldChange".into()),
            padj: Some("padj".into()),
            pvalue: None,
            mean: None,
            mean_log: false,
            samples: vec![],
            values: None,
            groups: vec![],
            sample_column: None,
            term: None,
            count: None,
            ratio: None,
            dates: 0,
            question: None,
        }
    }

    fn table(name: &str, kind: QuickTableKind) -> QuickTable {
        QuickTable {
            name: name.into(),
            file: format!("{name}.csv"),
            sheet: None,
            rows: 10,
            columns: vec![QuickColumn {
                name: "id".into(),
                kind: QuickColumnKind::Text,
                missing: 0,
                distinct: 10,
                whole: false,
                min: None,
                max: None,
                examples: vec![],
                patterns: Default::default(),
            }],
            recognition: recognition(kind),
        }
    }

    #[test]
    fn variable_names_are_plain_and_unique() {
        assert_eq!(variable_name("de results.csv", None, &[]), "de_results");
        assert_eq!(
            variable_name("2024 counts.xlsx", Some("Sheet 1"), &[]),
            "table_2024_counts_Sheet_1"
        );
        assert_eq!(
            variable_name("de.csv", None, &["de".into(), "de_2".into()]),
            "de_3"
        );
    }

    #[test]
    fn a_volcano_waits_only_for_what_it_needs() {
        let mut card = QuickCard::default();
        merge(&mut card, &table("de", QuickTableKind::Results));
        let t = card.tables[0].clone();
        assert!(missing(&t, QuickPlot::Volcano, "").is_empty());
        assert_eq!(
            missing(&t, QuickPlot::Ma, ""),
            vec!["the average expression column"]
        );
        // Without a comparison, the plot is made with neutral wording.
        assert!(settings(&card, &t, "").comparison.is_none());
    }

    #[test]
    fn suggestions_reach_no_plot_until_accepted() {
        let mut card = QuickCard::default();
        merge(&mut card, &table("de", QuickTableKind::Results));
        card.comparison = Some(Known {
            value: QuickComparison {
                higher: "treated".into(),
                lower: "control".into(),
            },
            from: QuickFrom::Suggested,
        });
        card.tables[0].fc = Some(Known {
            value: "log2FoldChange".into(),
            from: QuickFrom::Suggested,
        });
        let t = card.tables[0].clone();
        assert_eq!(
            missing(&t, QuickPlot::Volcano, ""),
            vec!["the fold change column"]
        );
        assert!(settings(&card, &t, "").comparison.is_none());
    }

    #[test]
    fn what_the_user_says_survives_reloading() {
        let mut card = QuickCard::default();
        merge(&mut card, &table("de", QuickTableKind::Results));
        apply(&mut card, Some("de"), Edit::Column(Item::Fc, "lfc".into()));
        merge(&mut card, &table("de", QuickTableKind::Results));
        assert_eq!(card.tables.len(), 1);
        assert_eq!(card.tables[0].fc, Some(Known::told("lfc".to_string())));
        assert_eq!(card.organism, Some(Known::found("human".to_string())));
    }

    #[test]
    fn groups_change_one_sample_at_a_time() {
        let mut card = QuickCard::default();
        apply(
            &mut card,
            None,
            Edit::Group("S1".into(), Some("control".into())),
        );
        apply(
            &mut card,
            None,
            Edit::Group("S2".into(), Some("treated".into())),
        );
        apply(
            &mut card,
            None,
            Edit::Group("S1".into(), Some("treated".into())),
        );
        apply(&mut card, None, Edit::Group("S2".into(), None));
        assert_eq!(
            card.groups,
            Some(Known::told(vec![("S1".to_string(), "treated".to_string())]))
        );
        assert_eq!(group_names(&card, &["new".into()]), vec!["treated", "new"]);
    }

    #[test]
    fn session_names_drop_the_folder_suffix() {
        assert_eq!(session_name("/a/de_results (Sandhill) 2"), "de_results");
        assert_eq!(session_name("/a/other"), "other");
    }

    #[test]
    fn only_tables_are_read() {
        assert!(is_table("de.CSV"));
        assert!(is_table("book.xlsx"));
        assert!(!is_table("steps.R"));
        assert!(!is_table(".csv"));
    }
}
