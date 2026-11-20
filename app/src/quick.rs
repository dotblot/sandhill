//! Explore a Table in a session's window (`docs/design/quick-tools-plan.md`):
//! the session's R kernel, its tables loaded into R and recognised, the
//! What we know card, and the plots Sandhill's R templates make. The window
//! is drawn by `view::quick`.

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use sandhill_proto::{
    Call, DirEntry, EntryKind, GeneIds, Known, QuickCard, QuickCardTable, QuickComparison,
    QuickCutoffs, QuickDifferentialTable, QuickFactor, QuickFrom, QuickPlot, QuickPlotBatch,
    QuickRequest, QuickResult, QuickSettings, QuickTable, QuickTableKind, QuickValues,
};
use sandhill_proto::{
    QuickArchiveCounts, QuickArchiveOpened, QuickArchiveSelection, quick_archive_stem,
    quick_table_stem,
};
use serde_json::{Value, json};

use crate::view::quick_archive::{ArchivePicker, TABLE_TYPES};

use crate::workbench::{Session, Workbench};

pub(crate) mod differential;

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
    /// `(factor, sample, level)`; no level takes the sample out.
    Group(String, String, Option<String>),
    AddLevel(String, String),
    AddFactor(String),
    MoveLevel(String, usize, usize),
    RemoveLevel(String, usize),
    Comparison(String, String),
    Cutoffs(f64, f64),
    /// The column genes are labelled by (gene symbols), or none: by ID.
    Labels(Option<String>),
}

/// The fields the card's forms type into.
pub(crate) struct Inputs {
    pub higher: Entity<InputState>,
    pub lower: Entity<InputState>,
    pub group: Entity<InputState>,
    pub factor: Entity<InputState>,
    pub formula: Entity<InputState>,
    pub reduced: Entity<InputState>,
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
    pub editing_factor: Option<String>,
    pub colour_by: Option<String>,
    pub de: differential::DeForm,
    pub show_differential: bool,
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
    /// The next table loaded was imported as a sample sheet.
    pub sheet_wanted: bool,
    /// The user continued without AI in this window, not a Mac-wide choice.
    pub ai_hidden: bool,
    pub ai_ready: bool,
    pub archive_picker: Option<ArchivePicker>,
    pub pending_counts: Option<QuickArchiveCounts>,
}

pub(crate) fn is_table(name: &str) -> bool {
    quick_table_stem(name).is_some()
}

pub(crate) fn is_archive(name: &str) -> bool {
    quick_archive_stem(name).is_some()
}

fn is_excel(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("xlsx") || ext.eq_ignore_ascii_case("xls"))
}

/// An R variable for a table: its file's name (and sheet), made a plain R
/// name, numbered when taken: `de_results`, `samples_Sheet1`, `de_results_2`.
pub(crate) fn variable_name(file: &str, sheet: Option<&str>, taken: &[String]) -> String {
    variable_name_with_limit(file, sheet, taken, 48)
}

fn variable_name_with_limit(
    file: &str,
    sheet: Option<&str>,
    taken: &[String],
    limit: usize,
) -> String {
    let base = crate::panes::files::base_name(file);
    let stem = quick_table_stem(base)
        .unwrap_or_else(|| base.rsplit_once('.').map_or(base, |(stem, _)| stem));
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
    name.truncate(limit);
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
        labels: None,
        differential: None,
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
    if card.factors.is_none() && !r.groups.is_empty() {
        card.factors = Some(Known::found(vec![QuickFactor::from_pairs(
            "group".into(),
            r.groups.clone(),
        )]));
    }
}

fn add_differential_table(
    quick: &mut Quick,
    output: QuickDifferentialTable,
    gene_ids: Option<GeneIds>,
) {
    let mut card = card_table(&output.table);
    card.gene = Some(Known::found(output.gene));
    card.gene_ids = gene_ids.map(Known::found).or(card.gene_ids);
    card.labels = output.label.map(Known::found);
    card.samples = Some(Known::found(output.samples));
    card.values = Some(Known::found(QuickValues::Logged));
    card.differential = Some(output.info);
    card.question = None;
    quick.card.tables.push(card);
    quick.tables.push(output.table);
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

pub(crate) fn factors(card: &QuickCard) -> &[QuickFactor] {
    card.factors
        .as_ref()
        .and_then(Known::usable)
        .map_or(&[], Vec::as_slice)
}

pub(crate) fn comparison<'a>(
    card: &'a QuickCard,
    table: &'a QuickCardTable,
) -> Option<&'a Known<QuickComparison>> {
    match &table.differential {
        Some(info) => info.comparison.as_ref(),
        None => card.comparison.as_ref(),
    }
}

pub(crate) fn colour_factor<'a>(
    card: &'a QuickCard,
    table: &QuickCardTable,
    chosen: Option<&str>,
) -> Option<&'a QuickFactor> {
    let samples = table.samples.as_ref()?.usable()?;
    let choices = || {
        factors(card).iter().filter(|f| {
            f.assignments
                .iter()
                .any(|(sample, _)| samples.contains(sample))
        })
    };
    choices()
        .find(|f| Some(f.name.as_str()) == chosen)
        .or_else(|| choices().next())
}

fn apply_colour(settings: &mut QuickSettings, factor: Option<&QuickFactor>) {
    if let Some(factor) = factor {
        settings.groups = factor
            .assignments
            .iter()
            .filter(|(sample, _)| settings.samples.contains(sample))
            .cloned()
            .collect();
        settings.group_levels.clone_from(&factor.levels);
        settings.group_name = Some(factor.name.clone());
    }
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
    let info = table.differential.as_ref();
    let expression = info
        .and_then(|i| i.expression_table.as_ref())
        .and_then(|name| card.tables.iter().find(|t| &t.name == name));
    let plotted = expression.unwrap_or(table).differential.as_ref();
    let mut settings = QuickSettings {
        gene: usable(&table.gene),
        fc: usable(&table.fc),
        padj: usable(&table.padj),
        mean: usable(&table.mean),
        mean_log: table.mean.as_ref().and_then(Known::usable).is_some() && table.mean_log,
        samples: usable(&table.samples).unwrap_or_default(),
        values: usable(&table.values),
        comparison: comparison(card, table).and_then(Known::usable).cloned(),
        expression_table: expression.map(|t| t.name.clone()),
        plot_batch: plotted.and_then(|i| i.plot_batch.clone()),
        batch_note: plotted.and_then(|i| i.batch_note.clone()),
        omnibus: info.is_some_and(|i| i.omnibus),
        cutoffs: card.cutoffs,
        term: usable(&table.term),
        count: usable(&table.count),
        ratio: usable(&table.ratio),
        show_gene: (!show_gene.trim().is_empty()).then(|| show_gene.trim().to_string()),
        label: usable(&table.labels),
        ..QuickSettings::default()
    };
    apply_colour(&mut settings, colour_factor(card, table, None));
    settings
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
                if item == Item::Mean
                    && slot
                        .as_ref()
                        .is_some_and(|k| k.from == QuickFrom::Suggested)
                {
                    t.mean_log = false;
                }
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
        Edit::Group(name, sample, group) => change_factor(card, &name, |factor| {
            factor.assignments.retain(|(s, _)| *s != sample);
            if let Some(group) = group {
                if !factor.levels.contains(&group) {
                    factor.levels.push(group.clone());
                }
                factor.assignments.push((sample, group));
            }
        }),
        Edit::AddFactor(name) => change_factor(card, &name, |_| {}),
        Edit::AddLevel(name, level) => change_factor(card, &name, |factor| {
            if !factor.levels.contains(&level) {
                factor.levels.push(level);
            }
        }),
        Edit::MoveLevel(name, from, to) => change_factor(card, &name, |factor| {
            if from < factor.levels.len() && to < factor.levels.len() {
                factor.levels.swap(from, to);
            }
        }),
        Edit::RemoveLevel(name, index) => change_factor(card, &name, |factor| {
            if let Some(level) = factor.levels.get(index)
                && !factor.assignments.iter().any(|(_, value)| value == level)
            {
                factor.levels.remove(index);
            }
        }),
        Edit::Comparison(higher, lower) => {
            let (higher, lower) = (higher.trim().to_string(), lower.trim().to_string());
            let comparison = (!higher.is_empty() && !lower.is_empty())
                .then(|| Known::told(QuickComparison { higher, lower }));
            if let Some(info) = at.and_then(|ix| card.tables[ix].differential.as_mut()) {
                info.comparison = comparison;
            } else {
                card.comparison = comparison;
            }
        }
        Edit::Cutoffs(padj, lfc) => card.cutoffs = QuickCutoffs { padj, lfc },
        Edit::Labels(column) => {
            if let Some(ix) = at {
                card.tables[ix].labels = column.map(Known::told);
            }
        }
    }
}

/// The run that adds gene symbols for `table`'s gene IDs (Huan, 2026-11-19;
/// `docs/design/gene-symbols-plan.md`): only for an Ensembl or Entrez gene
/// column and an organism with symbols. `output` is the column to write,
/// for a session reopened later.
pub(crate) fn symbols_request(
    table: &QuickCardTable,
    organism: Option<&str>,
    output: Option<String>,
) -> Option<QuickRequest> {
    let ids =
        usable(&table.gene_ids).filter(|ids| matches!(ids, GeneIds::Ensembl | GeneIds::Entrez))?;
    let organism = organism.filter(|o| sandhill_proto::symbols_package(o).is_some())?;
    Some(QuickRequest::Symbols {
        table: table.name.clone(),
        column: usable(&table.gene)?,
        ids,
        organism: organism.to_string(),
        output,
    })
}

fn change_factor(card: &mut QuickCard, name: &str, change: impl FnOnce(&mut QuickFactor)) {
    if name.is_empty() || name == "." {
        return;
    }
    let mut factors = card
        .factors
        .take()
        .filter(|k| k.usable().is_some())
        .map(|k| k.value)
        .unwrap_or_default();
    let index = factors
        .iter()
        .position(|f| f.name == name)
        .unwrap_or_else(|| {
            factors.push(QuickFactor::from_pairs(name.into(), Vec::new()));
            factors.len() - 1
        });
    change(&mut factors[index]);
    card.factors = Some(Known::told(factors));
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
        let comparison = comparison(card, table)
            .and_then(Known::usable)
            .map_or_else(String::new, |c| {
                format!(" Fold change: {} vs {}.", c.higher, c.lower)
            });
        rows.push((
            table.name.clone(),
            format!("{}, from {from}.{comparison}", kind_label(table.kind.value)),
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
        " For a single comparison, genes were counted as changed when their adjusted p-value was below {} and their absolute log2 fold change was at least {}. Omnibus tests use only the adjusted p-value cutoff.",
        card.cutoffs.padj, card.cutoffs.lfc
    ));
    if let Some(c) = usable(&card.comparison) {
        text.push_str(&format!(
            " Positive log2 fold changes mean higher expression in {} than in {}.",
            c.higher, c.lower
        ));
    }
    for table in &card.tables {
        if let Some(methods) = table
            .differential
            .as_ref()
            .and_then(|info| info.methods.as_deref())
        {
            text.push_str(&format!("\n\n{}: {methods}", table.name));
            if let Some(c) = comparison(card, table).and_then(Known::usable) {
                text.push_str(&format!(
                    " The fold change shows {} vs {}.",
                    c.higher, c.lower
                ));
            }
        }
    }
    if card.tables.iter().any(|t| usable(&t.labels).is_some())
        && let Some(package) = usable(&card.organism)
            .as_deref()
            .and_then(sandhill_proto::symbols_package)
    {
        text.push_str(&format!(
            " Gene symbols were added for Ensembl or Entrez IDs with AnnotationDbi::mapIds and {package} (the first symbol when an ID has several); genes without one keep their ID."
        ));
    }
    text.push_str(
        "\n\nSample-level plots use log2(counts per million + 1) for raw counts and log2(value + 1) for unlogged normalized values; analysis tables use the method's variance-stabilized or logged values directly. Where marked, PCA and heatmaps remove the recorded batch from plotted values only. Heatmaps scale each gene to its own mean.\n\nThe exact steps are in steps.R in the session folder.\n",
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
                group: field("New level", window, cx),
                factor: field("New factor name", window, cx),
                formula: field("~ genotype * dose", window, cx),
                reduced: field("~ genotype", window, cx),
                gene: field("Gene, like STAT1", window, cx),
                padj: field("0.05", window, cx),
                lfc: field("1", window, cx),
                keep_name: field("Folder name", window, cx),
            };
            for field in [&inputs.formula, &inputs.reduced] {
                cx.subscribe_in(field, window, |_, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
                .detach();
            }
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
        let combine = quick.pending_counts.take();
        quick.busy = Some(
            if combine.is_some() {
                "Combining counts…"
            } else {
                "Reading your tables…"
            }
            .into(),
        );
        quick.error = None;
        let known: Vec<QuickCardTable> = quick.card.tables.clone();
        let organism = usable(&quick.card.organism);
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: project.clone(),
            sizes: false,
            marks: false,
        });
        let results =
            (!only_new && known.iter().any(|t| t.file.starts_with("results/"))).then(|| {
                daemon.call::<Vec<DirEntry>>(Call::ListDir {
                    path: format!("{project}/results"),
                    sizes: false,
                    marks: false,
                })
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
            let saved: Vec<String> = if let Some(results) = results {
                match results.await {
                    Ok(entries) => entries
                        .into_iter()
                        .filter(|e| e.kind == EntryKind::File)
                        .map(|e| format!("results/{}", e.name))
                        .collect(),
                    Err(err) => {
                        problems.push(format!("The results folder can't be read: {err:#}"));
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
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
                        .filter(|t| files.contains(&t.file) || saved.contains(&t.file))
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
                    }) => {
                        // Gene symbols added before are added again.
                        let again = known.iter().find(|t| t.name == table.name).and_then(|t| {
                            let label = usable(&t.labels)?;
                            if t.differential.is_some()
                                && table.columns.iter().any(|c| c.name == label)
                            {
                                return None;
                            }
                            symbols_request(t, organism.as_deref(), Some(label))
                        });
                        if let Some(request) = again
                            && let Err(err) = run(request).await
                        {
                            problems.push(format!("{file}: the gene symbols: {err:#}"));
                        }
                        loaded.push(table);
                    }
                    Ok(_) => problems.push(format!("{file}: the table didn't come back")),
                    Err(err) => problems.push(format!("{file}: {err:#}")),
                }
            }
            let mut combined = None;
            if let Some(QuickArchiveCounts { files, file }) = combine {
                let name = variable_name(&file, None, &taken);
                match run(QuickRequest::CombineCounts { files, file, name }).await {
                    Ok(QuickResult {
                        table: Some(table),
                        sentence,
                        ..
                    }) => {
                        combined = Some((table.name.clone(), table.file.clone(), sentence));
                        loaded.push(table);
                    }
                    Ok(_) => problems.push("The combined counts table didn't come back.".into()),
                    Err(err) => problems.push(format!("Could not combine the counts: {err:#}")),
                }
            }
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        let sentence = if let Some((name, file, sentence)) = combined {
                            if let Some(quick) = &mut this.quick {
                                quick.shown = Some(name);
                                quick.made = true;
                            }
                            if let Some(agents) = &this.agents {
                                agents.update(cx, |agents, cx| {
                                    agents.add_explore_files(vec![file], cx)
                                });
                            }
                            sentence
                        } else {
                            None
                        };
                        this.quick_loaded(session, loaded, problems, window, cx);
                        if let Some(quick) = &mut this.quick
                            && sentence.is_some()
                        {
                            quick.sentence = sentence;
                        }
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
        let new_names: Vec<String> = loaded.iter().map(|t| t.name.clone()).collect();
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
        // A sample sheet provides every column other than the sample names.
        let wanted = self
            .quick
            .as_mut()
            .is_some_and(|q| std::mem::take(&mut q.sheet_wanted));
        let from_import = if wanted {
            self.quick
                .as_mut()
                .and_then(|q| take_imported_sheet(q, &new_names))
        } else {
            None
        };
        let found = from_import.clone().or_else(|| {
            let q = self.quick.as_ref()?;
            if wanted
                || q.card
                    .factors
                    .as_ref()
                    .is_some_and(|f| f.from == QuickFrom::Told)
            {
                return None;
            }
            sheet_columns(q)
                .into_iter()
                .find(|(table, _)| new_names.contains(table))
        });
        if let Some((table, key)) = found {
            let from = if from_import.is_some() {
                QuickFrom::Told
            } else {
                QuickFrom::Found
            };
            if from == QuickFrom::Told {
                self.quick_save_card(cx);
            }
            self.quick_factors_from(table, key, from, cx);
        } else if wanted && let Some(quick) = self.quick_mut() {
            quick.error = Some(
                "A sample sheet needs a column with sample names and at least one other column for a factor.".into(),
            );
        }
        cx.notify();
    }

    /// Reads sample factors from a sheet in the order they were written.
    pub(crate) fn quick_factors_from(
        &mut self,
        table: String,
        key: String,
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
            request: QuickRequest::Factors { table, key },
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                let Some(quick) = &mut this.quick else {
                    return;
                };
                match read {
                    Ok(result) if !result.factors.is_empty() => {
                        let overlaps = quick.card.tables.iter()
                            .filter_map(|t| t.samples.as_ref().and_then(Known::usable))
                            .flatten()
                            .any(|sample| result.factors.iter().any(|f| f.assignments.iter().any(|(s, _)| s == sample)));
                        if !overlaps && from == QuickFrom::Told && quick.card.tables.iter().any(|t| t.samples.is_some()) {
                            quick.error = Some("None of the sample sheet's samples match the sample columns in this session.".into());
                        } else if from == QuickFrom::Told || quick.card.factors.as_ref().is_none_or(|f| f.from != QuickFrom::Told) {
                            let mut incoming = result.factors;
                            let previous = usable(&quick.card.factors).unwrap_or_default();
                            for factor in previous {
                                if !incoming.iter().any(|f| f.name == factor.name) {
                                    incoming.push(factor);
                                }
                            }
                            quick.card.factors = Some(Known { value: incoming, from });
                            this.quick_save_card(cx);
                        }
                    }
                    Ok(_) => quick.error = Some("The sample sheet has no sample factors.".into()),
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
        self.quick_save_card_answer(None, cx);
    }

    pub(crate) fn quick_save_card_answer(
        &mut self,
        answer: Option<(u64, Value)>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project), Some(quick)) =
            (self.daemon(), self.project.clone(), &self.quick)
        else {
            return;
        };
        let saved = daemon.call_in_order::<Value>(Call::SaveQuickCard {
            project,
            card: quick.card.clone(),
        });
        cx.spawn(async move |this, cx| {
            let saved = saved
                .await
                .map_err(|err| format!("The card couldn't be saved: {err:#}"));
            if let Some((id, result)) = answer {
                daemon.fire(Call::Answer {
                    id,
                    result: saved.as_ref().ok().map(|_| result),
                    error: saved.as_ref().err().cloned(),
                });
            }
            if let Err(err) = saved {
                this.update(cx, move |this, cx| {
                    if let Some(quick) = this.quick_mut() {
                        quick.error = Some(err);
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
        let stays = matches!(
            edit,
            Edit::Group(..)
                | Edit::AddLevel(..)
                | Edit::AddFactor(_)
                | Edit::MoveLevel(..)
                | Edit::RemoveLevel(..)
        );
        if let Edit::AddFactor(name) = &edit
            && !name.is_empty()
            && name != "."
        {
            quick.editing_factor = Some(name.clone());
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

    /// Adds gene symbols beside the shown table's gene IDs; plots then
    /// label genes by them.
    pub(crate) fn quick_add_symbols(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(quick) = &self.quick else {
            return;
        };
        let (Some(session), None) = (quick.loaded_in.clone(), &quick.busy) else {
            return;
        };
        let organism = usable(&quick.card.organism);
        let Some(request) = self
            .quick_table()
            .and_then(|t| symbols_request(t, organism.as_deref(), None))
        else {
            return;
        };
        let QuickRequest::Symbols { table, .. } = &request else {
            return;
        };
        let table = table.clone();
        let added = daemon.call::<QuickResult>(Call::QuickRun {
            session,
            project,
            request,
        });
        if let Some(quick) = self.quick_mut() {
            quick.busy = Some("Adding gene symbols…".into());
            quick.error = None;
            quick.sentence = None;
        }
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let added = added.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let Some(quick) = this.quick_mut() else {
                    return;
                };
                quick.busy = None;
                match added {
                    Ok(result) => {
                        let column = result.facts["column"].as_str().map(str::to_string);
                        quick.sentence = result.sentence;
                        quick.made = true;
                        apply(&mut quick.card, Some(&table), Edit::Labels(column));
                        this.quick_save_card(cx);
                    }
                    Err(err) => quick.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn quick_analyze(&mut self, cx: &mut Context<Self>) {
        let (Some(quick), Some(table)) = (&self.quick, self.quick_table()) else {
            return;
        };
        let mut taken: Vec<_> = quick
            .card
            .tables
            .iter()
            .map(|t| t.name.clone())
            .chain(quick.outputs.iter().map(|(name, _)| name.clone()))
            .collect();
        let formula = self.quick_input(|i| &i.formula, cx);
        let reduced = self.quick_input(|i| &i.reduced, cx);
        if let Some(request) = quick
            .de
            .request(&quick.card, table, &formula, &reduced, &mut taken)
        {
            self.quick_run_analysis(request, cx);
        } else if let Some(quick) = self.quick_mut() {
            quick.error = Some("Choose a counts table, sample factors and a comparison. For your own formula, fill in the design and list coefficients before choosing one.".into());
            cx.notify();
        }
    }

    pub(crate) fn quick_run_analysis(&mut self, request: QuickRequest, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project), Some(quick)) =
            (self.daemon(), self.project.clone(), &mut self.quick)
        else {
            return;
        };
        let (Some(session), None) = (quick.loaded_in.clone(), &quick.busy) else {
            return;
        };
        let reading = matches!(&request, QuickRequest::DeCoefficients { .. });
        let gene_ids = match &request {
            QuickRequest::Differential { table, .. } => quick
                .card
                .tables
                .iter()
                .find(|t| &t.name == table)
                .and_then(|t| usable(&t.gene_ids)),
            _ => None,
        };
        quick.busy = Some(
            if reading {
                "Checking the design…"
            } else {
                "Finding changed genes…"
            }
            .into(),
        );
        quick.error = None;
        quick.sentence = None;
        let run = daemon.call::<QuickResult>(Call::QuickRun {
            session: session.clone(),
            project,
            request: request.clone(),
        });
        let handle = self.window_handle;
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let run = run.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    let Some(quick) = this.quick_mut() else {
                        return;
                    };
                    quick.busy = None;
                    match run {
                        Ok(result) if reading => {
                            quick.de.coefficients = Some((request, result.coefficients))
                        }
                        Ok(result) => {
                            quick.made = true;
                            let sentence = result.sentence;
                            let first = result.differential.first().map(|r| r.table.name.clone());
                            for output in result.differential {
                                add_differential_table(quick, output, gene_ids);
                            }
                            this.quick_save_card(cx);
                            if let Some(name) = first {
                                this.quick_show(session, name, window, cx);
                            }
                            if let Some(quick) = this.quick_mut() {
                                quick.sentence = sentence;
                            }
                        }
                        Err(err) => quick.error = Some(format!("{err:#}")),
                    }
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// Makes a plot (or the top genes table) from the table shown.
    pub(crate) fn quick_make(&mut self, plot: QuickPlot, cx: &mut Context<Self>) {
        let show_gene = self.quick_input(|i| &i.gene, cx);
        let _ = self.quick_run_plot(plot, None, &show_gene, None, cx);
    }

    pub(crate) fn quick_run_plot(
        &mut self,
        plot: QuickPlot,
        table: Option<&str>,
        show_gene: &str,
        answer: Option<u64>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        use anyhow::{Context as _, bail};

        let daemon = self
            .daemon()
            .context("The Explore window is disconnected.")?;
        let project = self
            .project
            .clone()
            .context("There is no Explore session here.")?;
        let quick = self
            .quick
            .as_ref()
            .context("There is no Explore session here.")?;
        if let Some(busy) = &quick.busy {
            bail!("{busy} Wait for it to finish, then try again.");
        }
        let session = quick
            .loaded_in
            .clone()
            .context("R isn't ready to make plots yet.")?;
        let name = table
            .or(quick.shown.as_deref())
            .context("Choose a table first.")?;
        let table = quick
            .card
            .tables
            .iter()
            .find(|table| table.name == name && quick.tables.iter().any(|t| t.name == name))
            .with_context(|| format!("There is no loaded table named {name}."))?;
        let kind = table
            .kind
            .usable()
            .copied()
            .unwrap_or(QuickTableKind::Unknown);
        if !plots_for(kind).contains(&plot) {
            bail!(
                "{} is not available for this table. Check what the card says the table is.",
                plot_label(plot)
            );
        }
        let needs = missing(table, plot, show_gene);
        if !needs.is_empty() {
            bail!("{} needs {}.", plot_label(plot), needs.join(", "));
        }
        let mut settings = settings(&quick.card, table, show_gene);
        if let Some(chosen) = &quick.colour_by {
            apply_colour(
                &mut settings,
                colour_factor(&quick.card, table, Some(chosen)),
            );
        }
        if matches!(
            plot,
            QuickPlot::Pca | QuickPlot::VariableHeatmap | QuickPlot::Correlation
        ) && table.differential.is_none()
            && let Some((design, Some(factor))) =
                quick
                    .de
                    .active_design(&quick.card, table, &self.quick_input(|i| &i.formula, cx))
        {
            settings.plot_batch = Some(Box::new(QuickPlotBatch { design, factor }));
        }
        let output = (plot == QuickPlot::TopGenes).then(|| {
            let taken: Vec<String> = quick
                .tables
                .iter()
                .map(|t| t.name.clone())
                .chain(quick.outputs.iter().map(|(n, _)| n.clone()))
                .collect();
            variable_name(&format!("{}_top_genes", table.name), None, &taken)
        });
        let from = table.name.clone();
        let made = daemon.call::<QuickResult>(Call::QuickRun {
            session: session.clone(),
            project,
            request: QuickRequest::Plot {
                plot,
                table: from.clone(),
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
        cx.notify();
        cx.spawn(async move |this, cx| {
            let made = made.await;
            if let Some(id) = answer {
                let (result, error) = match &made {
                    Ok(result) => (
                        Some(json!({
                            "plot": plot, "table": from, "summary": result.sentence,
                            "numbers": result.facts, "output": result.output,
                        })),
                        None,
                    ),
                    Err(err) => (None, Some(format!("{err:#}"))),
                };
                daemon.fire(Call::Answer { id, result, error });
            }
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
        Ok(())
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

    /// Files dropped onto the window join the session.
    pub(crate) fn quick_add(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.quick_add_paths(paths, false, cx);
    }

    fn quick_add_paths(&mut self, paths: Vec<String>, sample_sheet: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(quick) = self.quick_mut() else {
            return;
        };
        if quick.busy.is_some() || quick.archive_picker.is_some() || quick.keeping.is_some() {
            quick.error = Some("Wait for this step to finish before adding more tables.".into());
            cx.notify();
            return;
        }
        let (tables, archives): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .filter(|p| is_table(p) || is_archive(p))
            .partition(|p| is_table(p));
        if archives.is_empty() {
            if tables.is_empty() {
                quick.error = Some(TABLE_TYPES.into());
                cx.notify();
            } else {
                self.quick_add_selected(tables, Vec::new(), false, sample_sheet, cx);
            }
            return;
        }
        quick.busy = Some("Reading the archive…".into());
        quick.error = None;
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picker = ArchivePicker::read(daemon, tables, archives, sample_sheet).await;
            this.update(cx, |this, cx| {
                if this.generation == generation
                    && let Some(quick) = this.quick_mut()
                {
                    quick.busy = None;
                    match picker {
                        Ok(picker) => quick.archive_picker = Some(picker),
                        Err(err) => {
                            quick.error = Some(format!("Could not read the archive: {err:#}"))
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn quick_open_archive_selection(&mut self, combine: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.quick_mut().and_then(|q| q.archive_picker.take()) {
            let (tables, archives, sample_sheet) = picker.into_selection();
            self.quick_add_selected(tables, archives, combine, sample_sheet, cx);
        }
    }

    fn quick_add_selected(
        &mut self,
        tables: Vec<String>,
        archives: Vec<QuickArchiveSelection>,
        combine: bool,
        sample_sheet: bool,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        if let Some(quick) = self.quick_mut() {
            quick.busy = Some("Adding tables…".into());
            quick.error = None;
        }
        let generation = self.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let added = if archives.is_empty() {
                daemon
                    .call::<Vec<String>>(Call::QuickAddTables { project, tables })
                    .await
                    .map(|files| (files, None))
            } else {
                daemon
                    .call::<QuickArchiveOpened>(Call::QuickOpenArchives {
                        project: Some(project),
                        tables,
                        archives,
                        combine,
                    })
                    .await
                    .map(|opened| (opened.files, opened.combine))
            };
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                let Some(quick) = &mut this.quick else { return };
                quick.busy = None;
                match added {
                    Ok((files, counts)) => {
                        quick.sheet_wanted = sample_sheet;
                        if counts.is_some() {
                            quick.pending_counts = counts;
                        }
                        let session = quick.loaded_in.clone();
                        if let Some(agents) = &this.agents {
                            agents.update(cx, |agents, cx| agents.add_explore_files(files, cx));
                        }
                        if let Some(session) = session {
                            this.quick_load(session, true, cx);
                        }
                    }
                    Err(err) => {
                        quick.error = Some(format!("Could not add the tables: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn quick_choose_tables(&mut self, cx: &mut Context<Self>) {
        self.quick_choose_files(false, cx);
    }

    /// Import a Sample Sheet…: a table of samples and their groups, added
    /// to the session and used for the groups.
    pub(crate) fn quick_import_sheet(&mut self, cx: &mut Context<Self>) {
        self.quick_choose_files(true, cx);
    }

    fn quick_choose_files(&mut self, sample_sheet: bool, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: !sample_sheet,
            prompt: Some(if sample_sheet { "Import" } else { "Open" }.into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let paths: Vec<String> = paths
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            this.update(cx, |this, cx| this.quick_add_paths(paths, sample_sheet, cx))
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
            return quick_table_stem(base)
                .or_else(|| quick_archive_stem(base))
                .unwrap_or(base)
                .to_owned();
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

/// Pickable sample sheets and their sample-name columns.
pub(crate) fn sheet_columns(quick: &Quick) -> Vec<(String, String)> {
    quick
        .card
        .tables
        .iter()
        .filter(|t| t.kind.usable() == Some(&QuickTableKind::SampleSheet))
        .filter_map(|t| {
            let loaded = quick.tables.iter().find(|l| l.name == t.name)?;
            Some((t.name.clone(), sheet_key(loaded, &quick.card)?))
        })
        .collect()
}

fn sheet_key(table: &QuickTable, card: &QuickCard) -> Option<String> {
    if table.columns.len() < 2 {
        return None;
    }
    let samples: std::collections::HashSet<_> = card
        .tables
        .iter()
        .filter_map(|t| t.samples.as_ref().and_then(Known::usable))
        .flatten()
        .map(String::as_str)
        .collect();
    let matched = table
        .columns
        .iter()
        .filter_map(|column| {
            let n = column
                .examples
                .iter()
                .filter(|s| samples.contains(s.as_str()))
                .count();
            (n > 0).then_some((n, &column.name))
        })
        .max_by_key(|&(n, _)| n)
        .map(|(_, name)| name.clone());
    matched
        .or_else(|| table.recognition.sample_column.clone())
        .or_else(|| Some(table.columns[0].name.clone()))
}

/// Mark an explicitly imported sheet and choose its sample-name column.
fn take_imported_sheet(quick: &mut Quick, new_names: &[String]) -> Option<(String, String)> {
    for name in new_names {
        let Some(loaded) = quick.tables.iter().find(|t| &t.name == name) else {
            continue;
        };
        let Some(key) = sheet_key(loaded, &quick.card) else {
            continue;
        };
        if let Some(entry) = quick.card.tables.iter_mut().find(|t| &t.name == name) {
            entry.kind = Known::told(QuickTableKind::SampleSheet);
            entry.question = None;
        }
        return Some((name.clone(), key));
    }
    None
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
        QuickTableKind, apply, factors, is_archive, is_excel, is_table, merge, missing,
        session_name, settings, symbols_request, variable_name,
    };
    use sandhill_proto::{QuickColumn, QuickColumnKind, QuickRecognition, QuickRequest};

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
    fn gene_symbols_need_ids_and_human_or_mouse_then_label_plots() {
        let mut card = QuickCard::default();
        merge(&mut card, &table("de", QuickTableKind::Results));
        let t = card.tables[0].clone();
        let Some(QuickRequest::Symbols {
            column,
            ids,
            organism,
            output,
            ..
        }) = symbols_request(&t, Some("human"), None)
        else {
            panic!("Ensembl IDs of a human table get symbols");
        };
        assert_eq!(
            (column.as_str(), ids, organism.as_str(), output),
            ("id", GeneIds::Ensembl, "human", None)
        );
        assert!(symbols_request(&t, Some("zebrafish"), None).is_none());
        assert!(symbols_request(&t, None, None).is_none());
        let mut named = t.clone();
        named.gene_ids = Some(Known::found(GeneIds::Symbol));
        assert!(symbols_request(&named, Some("human"), None).is_none());

        // Once added, plots label by the symbols and still match by ID;
        // Use IDs Instead goes back.
        apply(&mut card, Some("de"), Edit::Labels(Some("symbol".into())));
        let s = settings(&card, &card.tables[0], "");
        assert_eq!(
            (s.label.as_deref(), s.gene.as_deref()),
            (Some("symbol"), Some("id"))
        );
        apply(&mut card, Some("de"), Edit::Labels(None));
        assert!(settings(&card, &card.tables[0], "").label.is_none());
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
            Edit::Group("condition".into(), "S1".into(), Some("control".into())),
        );
        apply(
            &mut card,
            None,
            Edit::Group("condition".into(), "S2".into(), Some("treated".into())),
        );
        apply(
            &mut card,
            None,
            Edit::Group("condition".into(), "S1".into(), Some("treated".into())),
        );
        apply(
            &mut card,
            None,
            Edit::Group("condition".into(), "S2".into(), None),
        );
        apply(&mut card, None, Edit::MoveLevel("condition".into(), 1, 0));
        let condition = &factors(&card)[0];
        assert_eq!(condition.levels, ["treated", "control"]);
        assert_eq!(condition.assignments, [("S1".into(), "treated".into())]);
    }

    #[test]
    fn an_old_card_reads_groups_as_a_factor() {
        let card: QuickCard = serde_json::from_value(serde_json::json!({
            "tables": [],
            "groups": {"from": "told", "value": [["S1", "control"], ["S2", "treated"], ["S3", "control"]]}
        })).unwrap();
        assert_eq!(factors(&card)[0].name, "group");
        assert_eq!(factors(&card)[0].levels, ["control", "treated"]);
        let saved = serde_json::to_value(card).unwrap();
        assert!(saved.get("groups").is_none());
        assert_eq!(saved["factors"]["value"][0]["assignments"][1][0], "S2");
    }

    #[test]
    fn ready_designs_cover_one_factor_and_two_factor_questions() {
        use super::differential::{Question, ready_designs};
        fn f(name: &str, levels: &[&str]) -> sandhill_proto::QuickFactor {
            sandhill_proto::QuickFactor {
                name: name.into(),
                levels: levels.iter().map(|s| (*s).into()).collect(),
                assignments: vec![],
            }
        }
        let empty = QuickCard::default();
        let condition = f("condition", &["control", "GBM", "metastasis"]);
        let one = ready_designs(&condition, None, &empty);
        assert_eq!(one.len(), 3);
        assert_eq!(one[0].effects[0].label(), "GBM vs control");
        assert_eq!(one[1].effects.len(), 2);
        assert_eq!(one[2].reduced.as_deref(), Some("~ 1"));
        assert!(one[2].pick_effect);

        let genotype = f("genotype", &["WT", "KO"]);
        let dose = f("dose", &["0", "1", "10"]);
        let two = ready_designs(&genotype, Some(&dose), &empty);
        assert_eq!(two.len(), 6);
        assert_eq!(two[0].formula, "~ group");
        assert_eq!(two[0].effects[2].label(), "KO_1 vs KO_0");
        assert_eq!(two[1].effects[2].label(), "KO_10 vs WT_10");
        assert_eq!(two[2].formula, "~ dose + genotype");
        assert_eq!(two[3].formula, "~ genotype + dose + genotype:dose");
        assert_eq!(two[3].reduced.as_deref(), Some("~ genotype + dose"));
        assert_eq!(two[4].reduced.as_deref(), Some("~ genotype"));
        assert_eq!(two[4].effects[3].label(), "KO_10 vs KO_0");
        assert_eq!(two[5].numeric.as_deref(), Some("dose"));
        assert_eq!(two[5].formula, "~ genotype + dose");
        let binary = f("dose", &["vehicle", "drug"]);
        let two_levels = ready_designs(&genotype, Some(&binary), &empty);
        assert_eq!(two_levels.len(), 5);
        assert!(
            two_levels
                .iter()
                .find(|d| d.question == Question::Interaction)
                .unwrap()
                .reduced
                .is_none()
        );
    }

    #[test]
    fn choices_and_generated_comparisons_survive_the_request_and_card() {
        use super::differential::{BatchChoice, DeForm, Question};
        use super::{Quick, add_differential_table, comparison};
        use sandhill_proto::{
            QuickDeBatch, QuickDeMethod, QuickDifferentialInfo, QuickDifferentialTable, QuickFactor,
        };

        let mut q = Quick::default();
        merge(&mut q.card, &table("counts", QuickTableKind::Counts));
        let samples: Vec<String> = ["S1", "S2", "S3", "S4"]
            .into_iter()
            .map(str::to_string)
            .collect();
        q.card.tables[0].samples = Some(Known::found(samples.clone()));
        q.card.tables[0].labels = Some(Known::told("symbol".into()));
        let f = |name: &str, values: [&str; 4]| {
            QuickFactor::from_pairs(
                name.into(),
                samples
                    .iter()
                    .cloned()
                    .zip(values.map(str::to_string))
                    .collect(),
            )
        };
        q.card.factors = Some(Known::told(vec![
            f("condition", ["control", "control", "GBM", "GBM"]),
            f("batch", ["one", "two", "one", "two"]),
        ]));
        let form = DeForm {
            method: QuickDeMethod::Limma,
            question: Some(Question::EachReference),
            batch: BatchChoice::Model,
            quality_weights: true,
            shrink: true,
            ..Default::default()
        };
        let Some(QuickRequest::Differential { settings: opts, .. }) = form.request(
            &q.card,
            &q.card.tables[0],
            "",
            "",
            &mut vec!["counts".into()],
        ) else {
            panic!("a ready design builds a request")
        };
        assert_eq!(opts.design.formula, "~ condition");
        assert_eq!(
            opts.batch,
            QuickDeBatch::Model {
                factor: "batch".into()
            }
        );
        assert!(opts.quality_weights && !opts.shrink);
        assert_eq!(opts.label.as_deref(), Some("symbol"));
        assert_eq!(opts.outputs[0].name, "counts_limma_GBM_vs_control");

        let mut result = table(&opts.outputs[0].name, QuickTableKind::Results);
        result.file = format!("results/{}.csv", result.name);
        add_differential_table(
            &mut q,
            QuickDifferentialTable {
                table: result.clone(),
                gene: "id".into(),
                label: Some("symbol".into()),
                samples,
                info: QuickDifferentialInfo {
                    comparison: Some(Known::found(opts.outputs[0].comparison.clone())),
                    expression_table: Some(opts.normalized.clone()),
                    ..Default::default()
                },
            },
            Some(GeneIds::Ensembl),
        );
        merge(&mut q.card, &result);
        let entry = q.card.tables.last().unwrap();
        assert_eq!(comparison(&q.card, entry).unwrap().value.higher, "GBM");
        assert_eq!(
            settings(&q.card, entry, "").label.as_deref(),
            Some("symbol")
        );
        assert!(missing(entry, QuickPlot::ResultsHeatmap, "").is_empty());
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
        for name in [
            "sample.csv.gz",
            "sample.TSV.GZ",
            "sample.txt.gz",
            "sample.tab.gz",
        ] {
            assert!(is_table(name), "{name}");
            assert!(!is_excel(name), "{name}");
            assert_eq!(variable_name(name, None, &[]), "sample");
        }
        assert_eq!(
            variable_name("/data/my sample.CSV.GZ", None, &["my_sample".into()]),
            "my_sample_2"
        );
        assert!(!is_table("book.xlsx.gz"));
        assert!(!is_table("samples.tar.gz"));
        assert!(is_archive("samples.tar.gz"));
        assert!(!is_table("steps.R"));
        assert!(!is_table(".csv"));
    }
}
