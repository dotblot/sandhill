//! Recognise Quick Tools tables from the column summaries returned by R.

use std::collections::HashMap;

use sandhill_proto::{
    GeneIds, QuickColumn, QuickColumnKind, QuickRecognition, QuickTableKind, QuickValues,
};

const NUMBERS_QUESTION: &str = "What are these numbers?";

pub fn recognise(columns: &[QuickColumn]) -> QuickRecognition {
    let mut scan = Scan::new(columns);
    let mut found = QuickRecognition {
        kind: QuickTableKind::Unknown,
        tool: None,
        gene: None,
        gene_ids: None,
        organism: None,
        fc: None,
        padj: None,
        pvalue: None,
        mean: None,
        mean_log: false,
        samples: Vec::new(),
        values: None,
        groups: Vec::new(),
        sample_column: None,
        term: None,
        count: None,
        ratio: None,
        dates: 0,
        question: None,
    };

    let fc = scan.log_fc.or(scan.signed_fc);
    if fc.is_some() && (scan.adjusted_p().is_some() || scan.pvalue.is_some()) {
        found.kind = QuickTableKind::Results;
        let tool = scan.results_tool();
        scan.keep_result_samples(tool);
        found.tool = tool.map(str::to_owned);
        found.fc = column_name(fc);
        found.padj = column_name(scan.adjusted_p());
        found.pvalue = column_name(scan.pvalue);
        if let Some((mean, scale)) = scan.mean {
            found.mean = Some(mean.name.clone());
            found.mean_log = matches!(scale, Mean::LogCpm | Mean::AveExpr);
        }
        set_gene(&mut found, scan.gene);
        if !scan.samples.is_empty() {
            set_samples(&mut found, &scan.samples);
        }
    } else if let Some((term, adjusted, tool)) = scan.enrichment() {
        found.kind = QuickTableKind::Enrichment;
        found.tool = tool.map(str::to_owned);
        found.term = Some(term.name.clone());
        found.padj = Some(adjusted.name.clone());
        found.count = column_name(scan.count.or(scan.intersection));
        found.ratio = column_name(scan.gene_ratio.or(scan.ratio));
    } else if let Some(sample) = scan.sample_sheet_id() {
        found.kind = QuickTableKind::SampleSheet;
        found.sample_column = Some(sample.name.clone());
    } else if scan.is_expression() {
        set_gene(&mut found, scan.gene);
        set_samples(&mut found, &scan.samples);
        if found.values == Some(QuickValues::Counts) {
            found.kind = QuickTableKind::Counts;
            if scan.feature_counts == FEATURE_COUNTS_COLUMNS {
                found.tool = Some("featureCounts".to_owned());
            }
        } else {
            // Logged is a value scale, not a separate table kind.
            found.kind = QuickTableKind::Normalized;
        }
    } else {
        set_gene(
            &mut found,
            scan.gene.filter(|gene| gene.supports_expression()),
        );
        found.question = Some(NUMBERS_QUESTION.to_owned());
    }
    found
}

fn column_name(column: Option<&QuickColumn>) -> Option<String> {
    column.map(|column| column.name.clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mean {
    Plain,
    Deseq2,
    LogCpm,
    AveExpr,
}

const FEATURE_COUNTS_COLUMNS: u8 = 0b1_1111;

#[derive(Default)]
struct Scan<'a> {
    log_fc: Option<&'a QuickColumn>,
    signed_fc: Option<&'a QuickColumn>,
    adjusted: Option<&'a QuickColumn>,
    qvalue: Option<&'a QuickColumn>,
    pvalue: Option<&'a QuickColumn>,
    mean: Option<(&'a QuickColumn, Mean)>,
    lfc_se: bool,
    limma_t: Option<&'a QuickColumn>,
    limma_b: Option<&'a QuickColumn>,
    f_stat: Option<&'a QuickColumn>,
    statistics: usize,
    samples: Vec<&'a QuickColumn>,
    feature_counts: u8,
    gene: Option<Gene<'a>>,
    gene_evidence: bool,
    sample_id: Option<(bool, &'a QuickColumn)>,
    category_distinct: Option<u64>,
    term: Option<&'a QuickColumn>,
    term_id: Option<&'a QuickColumn>,
    term_named_id: Option<&'a QuickColumn>,
    count: Option<&'a QuickColumn>,
    intersection: Option<&'a QuickColumn>,
    ratio: Option<&'a QuickColumn>,
    gene_ratio: Option<&'a QuickColumn>,
    background_ratio: bool,
    gene_lists: bool,
    source: bool,
    category: bool,
    david_term: bool,
    benjamini: Option<&'a QuickColumn>,
    gprofiler_p: Option<&'a QuickColumn>,
}

impl<'a> Scan<'a> {
    fn new(columns: &'a [QuickColumn]) -> Self {
        let mut scan = Self::default();
        let mut key = String::new();
        for (index, column) in columns.iter().enumerate() {
            key.clear();
            key.extend(
                column
                    .name
                    .chars()
                    .filter(|c| !separator(*c))
                    .map(|c| c.to_ascii_lowercase()),
            );
            scan.observe(column, &key, index == 0);
        }
        scan
    }

    fn observe(&mut self, column: &'a QuickColumn, key: &str, first: bool) {
        let numeric = (column.kind == QuickColumnKind::Number).then_some(column);
        let text = column.kind == QuickColumnKind::Text;
        let mean = match key {
            "basemean" => Some(Mean::Deseq2),
            "mean" | "meanexpr" | "meanexpression" | "averageexpression" => Some(Mean::Plain),
            "logcpm" => Some(Mean::LogCpm),
            "aveexpr" => Some(Mean::AveExpr),
            _ => None,
        };
        if let Some(mean) = mean {
            if let Some(column) = numeric
                && self
                    .mean
                    .is_none_or(|(_, old)| old == Mean::Plain && mean != Mean::Plain)
            {
                self.mean = Some((column, mean));
            }
        } else {
            match key {
                "log2foldchange" | "log2fc" | "logfoldchange" | "logfc" => {
                    self.log_fc = self.log_fc.or(numeric)
                }
                "foldchange" | "fc" => {
                    if column.min.is_some_and(|min| min < 0.0) {
                        self.signed_fc = self.signed_fc.or(numeric);
                    }
                }
                "padj" | "padjust" | "padjusted" | "padjval" | "padjvalue" | "adjp" | "adjpval"
                | "adjpvalue" | "adjustedp" | "adjustedpval" | "adjustedpvalue" | "fdr"
                | "benjaminihochberg" | "bhfdr" => self.adjusted = self.adjusted.or(numeric),
                "qval" | "qvalue" => self.qvalue = self.qvalue.or(numeric),
                "benjamini" => self.benjamini = self.benjamini.or(numeric),
                "pvalue" | "pval" | "rawpvalue" | "rawpval" | "p" => {
                    self.pvalue = self.pvalue.or(numeric);
                    if key == "pvalue" {
                        self.gprofiler_p = self.gprofiler_p.or(numeric);
                    }
                }
                "lfcse" => self.lfc_se |= numeric.is_some(),
                "t" | "b" | "f" => {
                    // A short statistic name can also be a sample name in an expression matrix.
                    match key {
                        "t" => self.limma_t = self.limma_t.or(numeric),
                        "b" => self.limma_b = self.limma_b.or(numeric),
                        _ => self.f_stat = self.f_stat.or(numeric),
                    }
                    self.samples.extend(numeric);
                    return;
                }
                "stat" | "statistic" | "lr" | "qlf" | "se" | "lfcmle" | "logodds" | "zscore"
                | "means" | "rank" => {}
                _ => {
                    self.observe_other(column, key, first, text, numeric);
                    return;
                }
            }
        }
        if numeric.is_some() {
            self.statistics += 1;
        }
    }

    fn observe_other(
        &mut self,
        column: &'a QuickColumn,
        key: &str,
        first: bool,
        text: bool,
        numeric: Option<&'a QuickColumn>,
    ) {
        match key {
            "chr" => self.feature_counts |= 0b0_0001,
            "start" => self.feature_counts |= 0b0_0010,
            "end" => self.feature_counts |= 0b0_0100,
            "strand" => self.feature_counts |= 0b0_1000,
            "length" => self.feature_counts |= 0b1_0000,
            "chrom" | "chromosome" | "position" | "width" | "genelength" | "genebiotype"
            | "biotype" | "type" | "index" | "rowid" | "genedescription" | "annotation" => {}
            "term" | "description" | "termname" | "pathway" if text => {
                self.term = self.term.or(Some(column));
                self.david_term |= key == "term";
            }
            "termid" | "native" | "goid" | "pathwayid" if text => {
                self.term_id = self.term_id.or(Some(column));
                if key == "termid" || key == "native" {
                    self.term_named_id = self.term_named_id.or(Some(column));
                }
            }
            "count" | "genecount" | "overlapcount" => self.count = self.count.or(numeric),
            "intersectionsize" => self.intersection = self.intersection.or(numeric),
            "generatio" => self.gene_ratio = self.gene_ratio.or(Some(column)),
            "bgratio" => self.background_ratio = true,
            "ratio" => self.ratio = self.ratio.or(Some(column)),
            "termsize" | "querysize" | "effectivedomainsize" | "precision" | "recall" => {}
            "source" => self.source |= text,
            "condition" | "group" | "treatment" | "batch" | "sex" | "gender" | "replicate"
            | "genotype" | "phenotype" | "tissue" | "timepoint" | "category" => {
                self.category |= key == "category" && text;
                if column.distinct > 0 {
                    self.category_distinct = Some(
                        self.category_distinct
                            .map_or(column.distinct, |n| n.min(column.distinct)),
                    );
                }
            }
            _ => {
                let gene_label = matches!(
                    key,
                    "gene"
                        | "genes"
                        | "geneid"
                        | "genename"
                        | "symbol"
                        | "genesymbol"
                        | "ensembl"
                        | "ensemblid"
                        | "ensemblgeneid"
                        | "entrez"
                        | "entrezid"
                        | "entrezgeneid"
                );
                let sample_label = matches!(
                    key,
                    "sample"
                        | "sampleid"
                        | "samplename"
                        | "samplelabel"
                        | "sampleaccession"
                        | "run"
                        | "runid"
                        | "library"
                        | "libraryid"
                        | "libraryname"
                );
                let unlabeled_id =
                    key == "id" || (first && matches!(key, "" | "x" | "1" | "rownames"));
                if text {
                    let patterns = &column.patterns;
                    if patterns.go.max(patterns.pathway) > 0
                        && patterns.go.saturating_add(patterns.pathway) >= column.distinct
                    {
                        self.term_id = self.term_id.or(Some(column));
                    } else if patterns.gene_lists > 0 && patterns.gene_lists >= column.distinct {
                        self.gene_lists |= gene_label;
                    } else if !sample_label
                        && let Some(gene) = Gene::new(column, gene_label, unlabeled_id)
                    {
                        self.gene_evidence |= gene.blocks_sample_sheet();
                        if self
                            .gene
                            .is_none_or(|current| gene.score() > current.score())
                        {
                            self.gene = Some(gene);
                        }
                    }
                    if !gene_label
                        && column.distinct > 0
                        && (sample_label || examples_look_like_samples(column))
                        && self.sample_id.is_none_or(|(named, old)| {
                            (sample_label, column.distinct) > (named, old.distinct)
                        })
                    {
                        self.sample_id = Some((sample_label, column));
                    }
                } else if !gene_label
                    && !sample_label
                    && !unlabeled_id
                    && let Some(column) = numeric
                {
                    self.samples.push(column);
                }
            }
        }
    }

    fn results_tool(&self) -> Option<&'static str> {
        match self.mean.map(|(_, scale)| scale) {
            Some(Mean::Deseq2) => Some("DESeq2"),
            Some(Mean::LogCpm) => Some("edgeR"),
            Some(Mean::AveExpr) => Some("limma"),
            _ if self.lfc_se => Some("DESeq2"),
            _ if self.limma_t.is_some() && self.limma_b.is_some() => Some("limma"),
            _ => None,
        }
    }

    fn keep_result_samples(&mut self, tool: Option<&str>) {
        let b_stat = self.limma_b.filter(|_| tool == Some("limma"));
        let stats = [self.limma_t, b_stat, self.f_stat];
        if stats.iter().any(Option::is_some) {
            self.samples.retain(|col| {
                !stats
                    .into_iter()
                    .flatten()
                    .any(|stat| std::ptr::eq(*col, stat))
            });
        }
    }

    fn adjusted_p(&self) -> Option<&'a QuickColumn> {
        self.adjusted.or(self.qvalue).or(self.benjamini)
    }

    fn enrichment(&self) -> Option<(&'a QuickColumn, &'a QuickColumn, Option<&'static str>)> {
        let gprofiler = self.term_named_id.is_some() && self.intersection.is_some() && self.source;
        let term = self.term.or(self.term_id)?;
        let tool = if gprofiler {
            Some("g:Profiler")
        } else if self.gene_ratio.is_some()
            && (self.background_ratio || (self.gene_lists && self.count.is_some()))
        {
            Some("clusterProfiler")
        } else if self.category && self.david_term && self.benjamini.is_some() {
            Some("DAVID")
        } else {
            None
        };
        let adjusted = match tool {
            Some("g:Profiler") => self.gprofiler_p.or(self.adjusted_p()),
            Some("DAVID") => self.benjamini,
            _ => self.adjusted_p(),
        }?;
        Some((term, adjusted, tool))
    }

    fn sample_sheet_id(&self) -> Option<&'a QuickColumn> {
        let (_, sample) = self.sample_id?;
        (!self.gene_evidence && self.category_distinct.is_some_and(|n| sample.distinct >= n))
            .then_some(sample)
    }

    fn is_expression(&self) -> bool {
        if self.samples.len() <= self.statistics {
            return false;
        }
        self.feature_counts == FEATURE_COUNTS_COLUMNS
            || self.gene.is_some_and(|gene| gene.supports_expression())
            || (self.samples.len() >= 2
                && self.samples.iter().all(|sample| sample_token(&sample.name)))
    }
}

#[derive(Clone, Copy)]
struct Gene<'a> {
    column: &'a QuickColumn,
    ids: Option<GeneIds>,
    hits: u64,
    labeled: bool,
    unlabeled_id: bool,
}

impl<'a> Gene<'a> {
    fn new(column: &'a QuickColumn, labeled: bool, unlabeled_id: bool) -> Option<Self> {
        let p = &column.patterns;
        let ensembl = p
            .ensembl_human
            .saturating_add(p.ensembl_mouse)
            .saturating_add(p.ensembl_other);
        // The symbol pattern is broad enough to also match Ensembl IDs.
        let symbols = p.symbol.saturating_sub(ensembl);
        let ids = if ensembl > 0 && ensembl >= symbols.max(p.entrez) {
            Some(GeneIds::Ensembl)
        } else if p.entrez > 0 && p.entrez >= symbols {
            Some(GeneIds::Entrez)
        } else if symbols > 0 {
            Some(GeneIds::Symbol)
        } else {
            None
        };
        let hits = ensembl
            .max(p.symbol)
            .saturating_add(p.entrez)
            .max(p.datelike);
        if !labeled && (hits == 0 || (!unlabeled_id && hits <= column.distinct / 2)) {
            return None;
        }
        Some(Self {
            column,
            ids,
            hits,
            labeled,
            unlabeled_id,
        })
    }

    fn score(self) -> (u64, bool, bool, bool) {
        (
            self.hits,
            self.ids == Some(GeneIds::Ensembl),
            self.labeled,
            self.unlabeled_id,
        )
    }

    fn blocks_sample_sheet(self) -> bool {
        self.labeled || matches!(self.ids, Some(GeneIds::Ensembl | GeneIds::Entrez))
    }

    fn supports_expression(self) -> bool {
        self.blocks_sample_sheet() || self.unlabeled_id
    }
}

fn set_gene(found: &mut QuickRecognition, gene: Option<Gene<'_>>) {
    let Some(gene) = gene else { return };
    found.gene = Some(gene.column.name.clone());
    found.gene_ids = gene.ids;
    let p = &gene.column.patterns;
    found.dates = p.datelike;
    if gene.ids == Some(GeneIds::Ensembl) {
        if p.ensembl_human > p.ensembl_mouse.saturating_add(p.ensembl_other) {
            found.organism = Some("human".to_owned());
        } else if p.ensembl_mouse > p.ensembl_human.saturating_add(p.ensembl_other) {
            found.organism = Some("mouse".to_owned());
        }
    }
}

fn examples_look_like_samples(column: &QuickColumn) -> bool {
    column.distinct > 1
        && !column.examples.is_empty()
        && column.examples.iter().all(|value| sample_token(value))
}

fn sample_token(value: &str) -> bool {
    let mut letter = false;
    let mut digit = false;
    for c in value.trim().chars() {
        if c.is_ascii_alphabetic() {
            letter = true;
        } else if c.is_ascii_digit() {
            digit = true;
        } else if !separator(c) {
            return false;
        }
    }
    letter && digit
}

fn separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | '_' | '-')
}

fn set_samples(found: &mut QuickRecognition, samples: &[&QuickColumn]) {
    let (values, uncertain) = value_scale(samples);
    found.values = Some(values);
    if uncertain {
        found.question = Some(NUMBERS_QUESTION.to_owned());
    }
    found.groups = guess_groups(samples);
    found.samples = samples.iter().map(|sample| sample.name.clone()).collect();
}

fn value_scale(samples: &[&QuickColumn]) -> (QuickValues, bool) {
    if samples
        .iter()
        .all(|sample| sample.whole && sample.min.is_some_and(|min| min >= 0.0))
    {
        return (QuickValues::Counts, false);
    }
    let mut fractional = false;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut complete_range = true;
    let mut logged_labels = 0;
    let mut normalized_labels = 0;
    for sample in samples {
        fractional |= !sample.whole;
        if let (Some(low), Some(high)) = (sample.min, sample.max) {
            min = min.min(low);
            max = max.max(high);
        } else {
            complete_range = false;
        }
        match scale_label(&sample.name) {
            Some(QuickValues::Logged) => logged_labels += 1,
            Some(QuickValues::Normalized) => normalized_labels += 1,
            _ => {}
        }
    }
    if logged_labels == samples.len() {
        return (QuickValues::Logged, false);
    }
    if normalized_labels == samples.len() && min >= 0.0 {
        return (QuickValues::Normalized, false);
    }
    if max.is_finite() && max <= 25.0 {
        // Small, positive numbers can be logged, normalized or estimated counts.
        (
            QuickValues::Logged,
            !fractional || !complete_range || min >= 0.0 || normalized_labels > 0,
        )
    } else {
        // Decimals on a large scale do not distinguish TPM from estimated counts.
        (QuickValues::Normalized, true)
    }
}

fn scale_label(name: &str) -> Option<QuickValues> {
    let mut normalized = false;
    for word in name.split(separator) {
        if [
            "log", "log2", "logcpm", "log2cpm", "logtpm", "log2tpm", "vst", "rlog",
        ]
        .iter()
        .any(|unit| word.eq_ignore_ascii_case(unit))
        {
            return Some(QuickValues::Logged);
        }
        normalized |= ["tpm", "fpkm", "rpkm", "cpm", "normalized", "normalised"]
            .iter()
            .any(|unit| word.eq_ignore_ascii_case(unit));
    }
    normalized.then_some(QuickValues::Normalized)
}

fn guess_groups(samples: &[&QuickColumn]) -> Vec<(String, String)> {
    let mut sizes = HashMap::new();
    for sample in samples {
        if let Some(group) = group_stem(&sample.name) {
            *sizes.entry(group).or_insert(0usize) += 1;
        }
    }
    if sizes.values().filter(|&&size| size >= 2).take(2).count() < 2 {
        return Vec::new();
    }
    samples
        .iter()
        .filter_map(|sample| {
            let group = group_stem(&sample.name)?;
            (sizes[group] >= 2).then(|| (sample.name.clone(), group.to_owned()))
        })
        .collect()
}

fn group_stem(name: &str) -> Option<&str> {
    let before_digits = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if before_digits.len() == name.len() {
        return None;
    }
    let mut stem = before_digits.trim_end_matches(separator);
    if let Some(prefix) = strip_ascii_suffix(stem, "rep") {
        stem = prefix.trim_end_matches(separator);
    } else if let Some(prefix) = strip_ascii_suffix(stem, "r")
        && prefix.ends_with(separator)
    {
        stem = prefix.trim_end_matches(separator);
    }
    (!stem.is_empty()).then_some(stem)
}

fn strip_ascii_suffix<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let (prefix, tail) = name.split_at_checked(name.len().checked_sub(suffix.len())?)?;
    tail.eq_ignore_ascii_case(suffix).then_some(prefix)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::Value;

    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        columns: Vec<QuickColumn>,
        expect: BTreeMap<String, Value>,
    }

    #[test]
    fn recognises_table_fixtures() {
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../../../tests/fixtures/quick_tables.json"))
                .unwrap();
        for case in cases {
            let actual = serde_json::to_value(recognise(&case.columns)).unwrap();
            for (field, expected) in case.expect {
                assert_eq!(actual[&field], expected, "{}: {field}", case.name);
            }
        }
    }
}
