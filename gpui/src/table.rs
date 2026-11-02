//! The Data Explorer's model, from the Tauri app's `model/table.ts`: how
//! cells and filters read, which filters a column takes, and the R or
//! Python code that does what the view does. Filtering and sorting
//! themselves run in the kernel (`TableRows`).

use sandhill_proto::{ColumnSummary, TableColumn, TableFilter, TableSort};
use serde_json::Value;

/// How many values a condition takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Takes {
    None,
    One,
    Two,
    Many,
}

#[derive(Clone, Copy, Debug)]
pub struct Op {
    pub op: &'static str,
    pub label: &'static str,
    pub takes: Takes,
}

const fn op(op: &'static str, label: &'static str, takes: Takes) -> Op {
    Op { op, label, takes }
}

const LESS: [Op; 4] = [
    op("<", "<", Takes::One),
    op("<=", "≤", Takes::One),
    op(">", ">", Takes::One),
    op(">=", "≥", Takes::One),
];
const BETWEEN: Op = op("between", "between", Takes::Two);
const EQUAL: [Op; 2] = [op("=", "is", Takes::One), op("!=", "is not", Takes::One)];
const SET: [Op; 2] = [
    op("in", "is one of", Takes::Many),
    op("not in", "is none of", Takes::Many),
];
const MISSING: [Op; 2] = [
    op("missing", "is missing", Takes::None),
    op("present", "is not missing", Takes::None),
];
const CONTAINS: Op = op("contains", "contains", Takes::One);

/// The filters a column of `kind` takes, the usual one first.
pub fn ops_for(kind: &str) -> Vec<Op> {
    let mut ops = Vec::new();
    match kind {
        "number" => {
            ops.extend(LESS);
            ops.push(BETWEEN);
            ops.extend(EQUAL);
        }
        "date" => {
            ops.extend(LESS);
            ops.push(BETWEEN);
        }
        "category" | "logical" => ops.extend(SET),
        "text" => {
            ops.push(CONTAINS);
            ops.extend(EQUAL);
            ops.extend(SET);
        }
        _ => ops.push(CONTAINS),
    }
    ops.extend(MISSING);
    ops
}

pub fn op_label(op: &str) -> &str {
    ["number", "text", "category"]
        .iter()
        .flat_map(|kind| ops_for(kind))
        .find(|o| o.op == op)
        .map_or(op, |o| o.label)
}

fn column_name(columns: &[TableColumn], column: u32) -> String {
    columns
        .get(column as usize)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("column {}", column + 1))
}

/// A filter as it reads on its chip: "percent.mt < 5", "cell_type is one
/// of B, NK".
pub fn filter_text(filter: &TableFilter, columns: &[TableColumn]) -> String {
    let name = column_name(columns, filter.column);
    let v = |i: usize| filter.values.get(i).cloned().unwrap_or_default();
    match filter.op.as_str() {
        "missing" | "present" => format!("{name} {}", op_label(&filter.op)),
        "between" => format!("{name} between {} and {}", v(0), v(1)),
        "in" | "not in" => format!(
            "{name} {} {}",
            op_label(&filter.op),
            filter.values.join(", ")
        ),
        "contains" => format!("{name} contains \"{}\"", v(0)),
        _ => format!("{name} {} {}", op_label(&filter.op), v(0)),
    }
}

/// Whether a filter's values make sense for its column: numbers where
/// numbers go.
pub fn filter_error(filter: &TableFilter, column: Option<&TableColumn>) -> Option<&'static str> {
    let kind = column.map_or("other", |c| c.kind.as_str());
    let Some(op) = ops_for(kind).into_iter().find(|o| o.op == filter.op) else {
        return Some("Pick a condition.");
    };
    let wanted = match op.takes {
        Takes::None => 0,
        Takes::One => 1,
        Takes::Two => 2,
        Takes::Many => filter.values.len().max(1),
    };
    if filter.values.len() < wanted || filter.values[..wanted].iter().any(|v| v.trim().is_empty()) {
        return Some("Give a value.");
    }
    if kind == "number"
        && filter.op != "in"
        && filter.op != "not in"
        && filter
            .values
            .iter()
            .any(|v| !v.trim().parse::<f64>().is_ok_and(f64::is_finite))
    {
        return Some("Give a number.");
    }
    None
}

/// A new filter on `column`: the usual condition for its kind.
pub fn new_filter(
    column: u32,
    columns: &[TableColumn],
    summary: Option<&ColumnSummary>,
) -> TableFilter {
    let kind = columns
        .get(column as usize)
        .map_or("other", |c| c.kind.as_str());
    let op = ops_for(kind)[0].op;
    let values = match summary.and_then(|s| s.median) {
        Some(median) if kind == "number" && op == "<" => vec![median.to_string()],
        _ => Vec::new(),
    };
    TableFilter {
        column,
        op: op.into(),
        values,
    }
}

pub fn missing_text(language: &str) -> &'static str {
    if language.eq_ignore_ascii_case("r") {
        "NA"
    } else {
        "NaN"
    }
}

/// A cell as shown, and whether it's missing: numbers with digit grouping
/// and at most 4 decimals.
pub fn format_cell(value: &Value, language: &str) -> (String, bool) {
    match value {
        Value::Null => (missing_text(language).into(), true),
        Value::String(s) => (s.clone(), false),
        Value::Number(n) => (
            n.as_f64().map_or_else(|| n.to_string(), format_number),
            false,
        ),
        Value::Bool(b) => (if *b { "TRUE" } else { "FALSE" }.into(), false),
        other => (other.to_string(), false),
    }
}

/// As JavaScript's `toLocaleString("en-US")` with at most 4 decimals;
/// very large and very small numbers in exponent form.
pub fn format_number(n: f64) -> String {
    if !n.is_finite() {
        return if n.is_nan() {
            "NaN".into()
        } else if n > 0. {
            "Inf".into()
        } else {
            "-Inf".into()
        };
    }
    if n.fract() == 0. && n.abs() < 1e15 {
        return group(&format!("{:.0}", n.abs()), n < 0.);
    }
    let size = n.abs();
    if size != 0. && !(1e-4..1e9).contains(&size) {
        let s = format!("{n:.3e}");
        // Rust writes 1.234e-5; JavaScript 1.234e-5 too, but e+9 for big.
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    let s = format!("{size:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut out = group(whole, n < 0.);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

fn group(digits: &str, negative: bool) -> String {
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "Sorted by nCount_RNA, descending, then cluster".
pub fn sort_text(sort: &[TableSort], columns: &[TableColumn]) -> String {
    if sort.is_empty() {
        return "Not sorted; click a column's name to sort by it".into();
    }
    let keys: Vec<String> = sort
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let name = columns
                .get(s.column as usize)
                .map_or_else(|| s.column.to_string(), |c| c.name.clone());
            let how = if s.descending {
                ", descending"
            } else if i == 0 && sort.len() == 1 {
                ", ascending"
            } else {
                ""
            };
            format!("{name}{how}")
        })
        .collect();
    format!("Sorted by {}", keys.join(", then "))
}

/// What clicking a column's name does to the sort: ascending, then
/// descending, then off. With `add` (shift-click) it becomes another key.
pub fn next_sort(sort: &[TableSort], column: u32, add: bool) -> Vec<TableSort> {
    let at = sort.iter().position(|s| s.column == column);
    let next = match at.map(|i| &sort[i]) {
        None => Some(TableSort {
            column,
            descending: false,
        }),
        Some(s) if !s.descending => Some(TableSort {
            column,
            descending: true,
        }),
        Some(_) => None,
    };
    if !add {
        return next.into_iter().collect();
    }
    match (next, at) {
        (None, _) => sort
            .iter()
            .filter(|s| s.column != column)
            .cloned()
            .collect(),
        (Some(n), Some(_)) => sort
            .iter()
            .map(|s| {
                if s.column == column {
                    n.clone()
                } else {
                    s.clone()
                }
            })
            .collect(),
        (Some(n), None) => sort.iter().cloned().chain([n]).collect(),
    }
}

/// A width for a column, from its name and some of its cells, in px: the
/// Tauri app's, for text 2px larger.
pub fn column_width(column: &TableColumn, cells: &[String]) -> f32 {
    let longest = cells
        .iter()
        .map(|c| c.chars().count())
        .chain([
            column.name.chars().count() + 2,
            column.kind_detail.chars().count(),
        ])
        .max()
        .unwrap_or(0);
    (longest as f32 * 8.4 + 24.).clamp(72., 360.).round()
}

/// "dbl", "fct · 9 values", "chr · 2 missing".
pub fn column_note(column: &TableColumn, summary: Option<&ColumnSummary>) -> String {
    let mut parts = vec![column.kind_detail.clone()];
    if let Some(s) = summary {
        if let Some(distinct) = s.distinct
            && column.kind != "number"
        {
            parts.push(if distinct == 1 {
                "1 unique".into()
            } else {
                format!("{} values", format_number(distinct as f64))
            });
        }
        if s.missing > 0 {
            parts.push(format!("{} missing", format_number(s.missing as f64)));
        }
    }
    parts.join(" · ")
}

/// A file name for exporting: the table's last part, made safe.
pub fn export_name(path: &[String]) -> String {
    let last = path.last().map_or("table", String::as_str);
    let mut name = String::new();
    for c in last.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
            name.push(c);
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_start_matches(['-', '.']).trim_end_matches('-');
    format!("{}.csv", if name.is_empty() { "table" } else { name })
}

/// Colours of a category's values, as in the mockup.
pub const COLORS: [u32; 10] = [
    0x4e79a7, 0xf28e2b, 0xe15759, 0x76b7b2, 0x59a14f, 0xedc948, 0xb07aa1, 0xff9da7, 0x9c755f,
    0xbab0ac,
];

/// Code that does what the view does, in the session's language: the
/// filters, then the sort, on the table `code` names.
pub fn table_code(
    language: &str,
    code: &str,
    matrix: bool,
    columns: &[TableColumn],
    sort: &[TableSort],
    filters: &[TableFilter],
) -> String {
    if language.eq_ignore_ascii_case("r") {
        r_code(code, matrix, columns, sort, filters)
    } else {
        python_code(code, matrix, columns, sort, filters)
    }
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn is_number(columns: &[TableColumn], i: u32) -> bool {
    columns.get(i as usize).is_some_and(|c| c.kind == "number")
}

fn r_name_ok(name: &str) -> bool {
    if name == "." {
        return true;
    }
    let mut chars = name.chars();
    let ok_rest = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_';
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => chars.all(ok_rest),
        Some('.') => {
            let rest: Vec<char> = chars.collect();
            matches!(rest.first(), Some(c) if c.is_ascii_alphabetic() || *c == '.' || *c == '_')
                && rest.into_iter().all(ok_rest)
        }
        _ => false,
    }
}

fn r_code(
    code: &str,
    matrix: bool,
    columns: &[TableColumn],
    sort: &[TableSort],
    filters: &[TableFilter],
) -> String {
    let mut lines = vec![format!("x <- {code}")];
    let col = |i: u32| {
        let name = columns.get(i as usize).map_or("", |c| c.name.as_str());
        if matrix {
            let positional = name.starts_with("[,")
                && name.ends_with(']')
                && name[2..name.len() - 1].chars().all(|c| c.is_ascii_digit())
                && name.len() > 3;
            if positional {
                format!("x[, {}]", i + 1)
            } else {
                format!("x[, {}]", quoted(name))
            }
        } else if r_name_ok(name) {
            format!("x${name}")
        } else {
            format!("x[[{}]]", quoted(name))
        }
    };
    let value = |i: u32, v: &str| {
        if is_number(columns, i) {
            v.to_string()
        } else {
            quoted(v)
        }
    };
    let tests: Vec<String> = filters
        .iter()
        .map(|f| {
            let c = col(f.column);
            let v = |i: usize| f.values.get(i).map_or("", String::as_str);
            let list = || {
                f.values
                    .iter()
                    .map(|v| quoted(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            match f.op.as_str() {
                "missing" => format!("is.na({c})"),
                "present" => format!("!is.na({c})"),
                "between" => format!(
                    "{c} >= {} & {c} <= {}",
                    value(f.column, v(0)),
                    value(f.column, v(1))
                ),
                "in" => format!("{c} %in% c({})", list()),
                "not in" => format!("!({c} %in% c({})) & !is.na({c})", list()),
                "contains" => format!(
                    "grepl({}, tolower({c}), fixed = TRUE)",
                    quoted(&v(0).to_lowercase())
                ),
                op => format!(
                    "{c} {} {}",
                    if op == "=" { "==" } else { op },
                    value(f.column, v(0))
                ),
            }
        })
        .collect();
    if !tests.is_empty() {
        lines.push(format!(
            "x <- x[which({}), , drop = FALSE]",
            tests.join(" & ")
        ));
    }
    if !sort.is_empty() {
        let keys: Vec<String> = sort
            .iter()
            .map(|s| {
                if s.descending {
                    format!("-xtfrm({})", col(s.column))
                } else {
                    col(s.column)
                }
            })
            .collect();
        lines.push(format!(
            "x <- x[order({}), , drop = FALSE]",
            keys.join(", ")
        ));
    }
    format!("{}\n", lines.join("\n"))
}

fn python_code(
    code: &str,
    matrix: bool,
    columns: &[TableColumn],
    sort: &[TableSort],
    filters: &[TableFilter],
) -> String {
    let mut lines = if matrix {
        vec![
            "import pandas as pd".to_string(),
            format!("x = pd.DataFrame({code})"),
        ]
    } else {
        vec![format!("x = {code}")]
    };
    let name = |i: u32| columns.get(i as usize).map_or("", |c| c.name.as_str());
    // Columns named by position (an array's, or digits) are read by position.
    let numbered = |i: u32| {
        let n = name(i);
        !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
    };
    let col = |i: u32| {
        if numbered(i) {
            format!("x.iloc[:, {i}]")
        } else {
            format!("x[{}]", quoted(name(i)))
        }
    };
    let value = |i: u32, v: &str| {
        if is_number(columns, i) {
            v.to_string()
        } else {
            quoted(v)
        }
    };
    let tests: Vec<String> = filters
        .iter()
        .map(|f| {
            let c = col(f.column);
            let text = format!("{c}.astype(str)");
            let v = |i: usize| f.values.get(i).map_or("", String::as_str);
            let list = || {
                f.values
                    .iter()
                    .map(|v| quoted(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            match f.op.as_str() {
                "missing" => format!("{c}.isna()"),
                "present" => format!("{c}.notna()"),
                "between" => format!(
                    "{c}.between({}, {})",
                    value(f.column, v(0)),
                    value(f.column, v(1))
                ),
                "in" => format!("{text}.isin([{}]) & {c}.notna()", list()),
                "not in" => format!("~{text}.isin([{}]) & {c}.notna()", list()),
                "contains" => format!(
                    "{text}.str.contains({}, case=False, regex=False) & {c}.notna()",
                    quoted(v(0))
                ),
                op => {
                    let op = if op == "=" { "==" } else { op };
                    let kind = columns
                        .get(f.column as usize)
                        .map_or("", |c| c.kind.as_str());
                    match kind {
                        "number" => format!("{c} {op} {}", v(0)),
                        "date" => format!("{c} {op} {}", quoted(v(0))),
                        _ => format!("{text} {op} {}", quoted(v(0))),
                    }
                }
            }
        })
        .collect();
    if !tests.is_empty() {
        let many = tests.len() > 1;
        let parts: Vec<String> = tests
            .iter()
            .map(|t| {
                if many || t.contains(" & ") {
                    format!("({t})")
                } else {
                    t.clone()
                }
            })
            .collect();
        lines.push(format!("x = x[{}]", parts.join(" & ")));
    }
    if !sort.is_empty() {
        let by: Vec<String> = sort
            .iter()
            .map(|s| {
                if numbered(s.column) {
                    format!("x.columns[{}]", s.column)
                } else {
                    quoted(name(s.column))
                }
            })
            .collect();
        let ascending: Vec<&str> = sort
            .iter()
            .map(|s| if s.descending { "False" } else { "True" })
            .collect();
        if sort.len() == 1 {
            lines.push(format!(
                "x = x.sort_values({}, ascending={})",
                by[0], ascending[0]
            ));
        } else {
            lines.push(format!(
                "x = x.sort_values([{}], ascending=[{}])",
                by.join(", "),
                ascending.join(", ")
            ));
        }
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{
        column_note, export_name, filter_error, filter_text, format_cell, format_number, next_sort,
        ops_for, sort_text, table_code,
    };
    use sandhill_proto::{ColumnSummary, TableColumn, TableFilter, TableSort};
    use serde_json::json;

    fn column(name: &str, kind: &str, detail: &str) -> TableColumn {
        TableColumn {
            name: name.into(),
            kind: kind.into(),
            kind_detail: detail.into(),
        }
    }

    fn filter(column: u32, op: &str, values: &[&str]) -> TableFilter {
        TableFilter {
            column,
            op: op.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
        }
    }

    fn sort(column: u32, descending: bool) -> TableSort {
        TableSort { column, descending }
    }

    #[test]
    fn formats_numbers_as_the_tauri_app() {
        assert_eq!(format_number(2.0), "2");
        assert_eq!(format_number(1234567.0), "1,234,567");
        assert_eq!(format_number(-1234.5), "-1,234.5");
        assert_eq!(format_number(1.0 / 3.0), "0.3333");
        assert_eq!(format_number(0.00001234), "1.234e-5");
        assert_eq!(format_number(12345678901.5), "1.235e+10");
        assert_eq!(format_cell(&json!(null), "r"), ("NA".into(), true));
        assert_eq!(format_cell(&json!(null), "python"), ("NaN".into(), true));
        assert_eq!(format_cell(&json!("ACTB"), "r"), ("ACTB".into(), false));
    }

    #[test]
    fn filters_read_and_check() {
        let columns = vec![
            column("percent.mt", "number", "dbl"),
            column("cell_type", "category", "fct"),
        ];
        assert_eq!(
            filter_text(&filter(0, "<", &["5"]), &columns),
            "percent.mt < 5"
        );
        assert_eq!(
            filter_text(&filter(1, "in", &["B", "NK"]), &columns),
            "cell_type is one of B, NK"
        );
        assert_eq!(
            filter_text(&filter(0, "between", &["1", "2"]), &columns),
            "percent.mt between 1 and 2"
        );
        assert_eq!(
            filter_error(&filter(0, "<", &["x"]), Some(&columns[0])),
            Some("Give a number.")
        );
        assert_eq!(
            filter_error(&filter(0, "<", &[""]), Some(&columns[0])),
            Some("Give a value.")
        );
        assert_eq!(
            filter_error(&filter(0, "missing", &[]), Some(&columns[0])),
            None
        );
        assert_eq!(
            filter_error(&filter(1, "<", &["1"]), Some(&columns[1])),
            Some("Pick a condition.")
        );
        assert_eq!(ops_for("category")[0].op, "in");
        assert_eq!(ops_for("number").len(), 9);
    }

    #[test]
    fn sorting_cycles_and_adds() {
        assert_eq!(next_sort(&[], 2, false), vec![sort(2, false)]);
        assert_eq!(next_sort(&[sort(2, false)], 2, false), vec![sort(2, true)]);
        assert!(next_sort(&[sort(2, true)], 2, false).is_empty());
        assert_eq!(
            next_sort(&[sort(1, false)], 2, true),
            vec![sort(1, false), sort(2, false)]
        );
        assert_eq!(
            next_sort(&[sort(1, false), sort(2, true)], 2, true),
            vec![sort(1, false)]
        );
        let columns = vec![column("a", "number", "dbl"), column("b", "text", "chr")];
        assert_eq!(
            sort_text(&[sort(0, false)], &columns),
            "Sorted by a, ascending"
        );
        assert_eq!(
            sort_text(&[sort(0, true), sort(1, false)], &columns),
            "Sorted by a, descending, then b"
        );
    }

    #[test]
    fn writes_code_for_the_view() {
        let columns = vec![
            column("percent.mt", "number", "dbl"),
            column("cell type", "category", "fct"),
        ];
        let filters = vec![filter(0, "<", &["5"]), filter(1, "in", &["B"])];
        assert_eq!(
            table_code(
                "r",
                "seu@meta.data",
                false,
                &columns,
                &[sort(0, true)],
                &filters
            ),
            "x <- seu@meta.data\nx <- x[which(x$percent.mt < 5 & x[[\"cell type\"]] %in% c(\"B\")), , drop = FALSE]\nx <- x[order(-xtfrm(x$percent.mt)), , drop = FALSE]\n"
        );
        assert_eq!(
            table_code(
                "python",
                "adata.obs",
                false,
                &columns,
                &[sort(0, false)],
                &filters[..1]
            ),
            "x = adata.obs\nx = x[x[\"percent.mt\"] < 5]\nx = x.sort_values(\"percent.mt\", ascending=True)\n"
        );
    }

    #[test]
    fn notes_and_names() {
        let summary = ColumnSummary {
            distinct: Some(9),
            missing: 2,
            ..Default::default()
        };
        assert_eq!(
            column_note(&column("c", "category", "fct"), Some(&summary)),
            "fct · 9 values · 2 missing"
        );
        assert_eq!(
            export_name(&["seu".into(), "meta.data".into()]),
            "meta.data.csv"
        );
        assert_eq!(export_name(&["my table!".into()]), "my-table.csv");
    }
}
