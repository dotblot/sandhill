//! Tables for the Data Explorer: a data frame or matrix in a session, read a
//! page of rows at a time. Filtering, sorting and the column summaries run in
//! the kernel, quietly, as the variable listing does, so only the rows on
//! screen leave it. Objects are found by the same paths as the Variables
//! pane's (`seu`, `meta.data`), with the walk shared with `variables`.

use sandhill_proto::{ColumnSummary, TableColumn, TableFilter, TableSort, ValueCount};
use serde_json::Value;

use crate::variables::{PYTHON_WALK, R_WALK};

/// Rows a page has unless asked otherwise, and at most.
pub const ROWS: u32 = 200;
pub const MAX_ROWS: u32 = 2000;

/// What the code is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Rows,
    Summary,
    Export,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Rows => "rows",
            Mode::Summary => "summary",
            Mode::Export => "export",
        }
    }
}

/// What to read from the table.
pub struct Ask<'a> {
    pub mode: Mode,
    pub path: &'a [String],
    pub sort: &'a [TableSort],
    pub filters: &'a [TableFilter],
    pub offset: u64,
    pub limit: u32,
    /// The CSV file to write, for [`Mode::Export`].
    pub file: &'a str,
}

const R: &str = r#"local({
  .path <- __PATH__
  .mode <- __MODE__
  .sort_col <- __SORT_COL__
  .sort_desc <- __SORT_DESC__
  .filters <- __FILTERS__
  .offset <- __OFFSET__
  .limit <- __LIMIT__
  .file <- __FILE__
__WALK__
  .found <- .find(.path)
  .x <- .found$x
  if (!.is_table(.x)) stop("a ", class(.x)[1], " can't be shown as a table")
  if (inherits(.x, "DataFrame")) .x <- as.data.frame(.x)
  .matrix <- is.matrix(.x)
  .n <- NROW(.x)
  .width <- NCOL(.x)
  .names <- colnames(.x)
  if (is.null(.names)) .names <- paste0("[,", seq_len(.width), "]")
  .col <- function(j) if (.matrix) .x[, j] else .x[[j]]
  .rn <- if (.matrix) rownames(.x) else if (.row_names_info(.x) > 0) rownames(.x) else NULL
  .esc <- function(s) {
    s <- gsub("\\", "\\\\", s, fixed = TRUE)
    s <- gsub("\t", "\\t", s, fixed = TRUE)
    s <- gsub("\n", "\\n", s, fixed = TRUE)
    gsub("\r", "\\r", s, fixed = TRUE)
  }
  .line <- function(...) {
    cat(..., sep = "\t")
    cat("\n")
  }
  .kind <- function(v) {
    if (is.factor(v)) return("category")
    if (is.logical(v)) return("logical")
    if (inherits(v, c("Date", "POSIXt"))) return("date")
    if (is.numeric(v)) return("number")
    if (is.character(v)) return("text")
    "other"
  }
  .type <- function(v) {
    if (is.factor(v)) return(if (is.ordered(v)) "ord" else "fct")
    if (inherits(v, "Date")) return("date")
    if (inherits(v, "POSIXt")) return("dttm")
    switch(typeof(v), double = "dbl", integer = "int", character = "chr", logical = "lgl",
      complex = "cplx", list = "list", typeof(v))
  }
  .cells <- function(v) {
    out <- if (is.list(v)) vapply(v, function(e) paste(format(e), collapse = ", "), "") else as.character(v)
    out <- .esc(substr(out, 1, 500))
    out[is.na(v)] <- "\\N"
    out
  }
  .mask <- rep(TRUE, .n)
  for (.f in .filters) {
    .v <- .col(.f$col)
    .a <- .f$values
    .num <- is.numeric(.v) && !is.factor(.v)
    .val <- function(i) if (.num) as.numeric(.a[[i]]) else .a[[i]]
    .s <- as.character(.v)
    .m <- switch(.f$op,
      "<" = .v < .val(1), "<=" = .v <= .val(1), ">" = .v > .val(1), ">=" = .v >= .val(1),
      "=" = if (.num) .v == .val(1) else .s == .a[[1]],
      "!=" = if (.num) .v != .val(1) else .s != .a[[1]],
      "between" = .v >= .val(1) & .v <= .val(2),
      "in" = .s %in% .a,
      "not in" = !(.s %in% .a) & !is.na(.v),
      "contains" = grepl(tolower(.a[[1]]), tolower(.s), fixed = TRUE),
      "missing" = is.na(.v),
      "present" = !is.na(.v),
      stop("unknown filter: ", .f$op))
    .m[is.na(.m)] <- FALSE
    .mask <- .mask & .m
  }
  .idx <- which(.mask)
  if (length(.sort_col)) {
    .keys <- lapply(seq_along(.sort_col), function(i) {
      k <- xtfrm(.col(.sort_col[[i]])[.idx])
      if (.sort_desc[[i]]) -k else k
    })
    .idx <- .idx[do.call(order, c(.keys, list(na.last = TRUE)))]
  }
  .cols <- seq_len(min(.width, 500))
  if (.mode == "rows") {
    .line("SANDHILL_TABLE", .n, length(.idx), .width, .esc(.found$code), as.integer(.matrix))
    for (.j in .cols) {
      .v <- .col(.j)
      .line("SANDHILL_COL", .esc(.names[[.j]]), .kind(.v), .type(.v))
    }
    .page <- .idx[seq_len(max(0, min(.limit, length(.idx) - .offset))) + .offset]
    if (length(.page)) {
      .labels <- if (is.null(.rn)) as.character(.page) else .esc(.rn[.page])
      .out <- lapply(.cols, function(j) .cells(.col(j)[.page]))
      writeLines(do.call(paste, c(list("SANDHILL_ROW", .labels), .out, list(sep = "\t"))))
    }
  }
  if (.mode == "summary") {
    .line("SANDHILL_TABLE", .n, length(.idx), .width)
    for (.j in .cols) {
      .v <- .col(.j)[.idx]
      .k <- .kind(.v)
      .w <- .v[!is.na(.v)]
      .d <- tryCatch(length(unique(.w)), error = function(e) NA)
      .stats <- rep("", 4)
      .hist <- ""
      if (.k == "number" && length(.w)) {
        .w <- as.numeric(.w)
        .w <- .w[is.finite(.w)]
        if (length(.w)) {
          .lo <- min(.w)
          .hi <- max(.w)
          .stats <- as.character(c(.lo, .hi, mean(.w), stats::median(.w)))
          .bins <- if (.hi > .lo) 20 else 1
          .at <- if (.hi > .lo) pmin(floor((.w - .lo) / (.hi - .lo) * 20) + 1, 20) else rep(1, length(.w))
          .hist <- paste(tabulate(.at, .bins), collapse = ",")
        }
      }
      .line("SANDHILL_SUM", .j - 1, sum(is.na(.v)), if (is.na(.d)) "" else .d, .stats, .hist)
      if (.k != "number" && .k != "other" && !is.na(.d) && .d <= 1000 && length(.w)) {
        .tab <- table(if (is.factor(.w)) droplevels(.w) else as.character(.w))
        if (!is.factor(.w) || length(.tab) > 10) .tab <- sort(.tab, decreasing = TRUE)
        .tab <- utils::head(.tab, 10)
        for (.i in seq_along(.tab)) .line("SANDHILL_TOP", .j - 1, .tab[[.i]], .esc(names(.tab)[[.i]]))
      }
    }
  }
  if (.mode == "export") {
    .all <- lapply(seq_len(.width), function(j) .col(j)[.idx])
    names(.all) <- .names
    dir.create(dirname(.file), recursive = TRUE, showWarnings = FALSE)
    utils::write.csv(as.data.frame(.all, optional = TRUE, stringsAsFactors = FALSE), .file,
      row.names = if (is.null(.rn)) FALSE else .rn[.idx], fileEncoding = "UTF-8")
    .line("SANDHILL_WROTE", length(.idx))
  }
  invisible(NULL)
})
"#;

const PYTHON: &str = r#"def _sandhill_table(path, mode, sort, filters, offset, limit, file):
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()
__WALK__
    v, code = walk(path)
    if not is_table(v):
        raise TypeError(f"a {type(v).__name__} can't be shown as a table")
    import numpy as np
    import pandas as pd
    t = v if type(v).__name__ == "DataFrame" else pd.DataFrame(v)
    n, width = t.shape
    cols = range(min(width, 500))
    types = pd.api.types

    def esc(s):
        return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")

    def line(*fields):
        print(*fields, sep="\t")

    def kind(s):
        d = s.dtype
        if isinstance(d, pd.CategoricalDtype):
            return "category"
        if types.is_bool_dtype(d):
            return "logical"
        if types.is_datetime64_any_dtype(d) or types.is_timedelta64_dtype(d):
            return "date"
        if types.is_numeric_dtype(d) and not types.is_complex_dtype(d):
            return "number"
        if d == object or types.is_string_dtype(d):
            return "text"
        return "other"

    def cells(s):
        out = []
        for x, missing in zip(s.tolist(), s.isna().to_numpy()):
            if missing:
                out.append("\\N")
            elif isinstance(x, float):
                out.append(repr(x))
            else:
                out.append(esc(str(x)[:500]))
        return out

    mask = np.ones(n, dtype=bool)
    for col, op, values in filters:
        s = t.iloc[:, col]
        k = kind(s)

        def val(i):
            if k == "number":
                return float(values[i])
            return pd.Timestamp(values[i]) if k == "date" else values[i]

        text = s.astype(str)
        present = s.notna()
        if op == "<":
            m = s < val(0)
        elif op == "<=":
            m = s <= val(0)
        elif op == ">":
            m = s > val(0)
        elif op == ">=":
            m = s >= val(0)
        elif op == "=":
            m = (s == val(0)) if k == "number" else (text == values[0]) & present
        elif op == "!=":
            m = ((s != val(0)) if k == "number" else (text != values[0])) & present
        elif op == "between":
            m = (s >= val(0)) & (s <= val(1))
        elif op == "in":
            m = text.isin(values) & present
        elif op == "not in":
            m = ~text.isin(values) & present
        elif op == "contains":
            m = text.str.lower().str.contains(values[0].lower(), regex=False) & present
        elif op == "missing":
            m = ~present
        elif op == "present":
            m = present
        else:
            raise ValueError(f"unknown filter: {op}")
        mask &= m.fillna(False).to_numpy(dtype=bool)
    idx = np.flatnonzero(mask)
    if sort:
        keys = pd.DataFrame({i: t.iloc[idx, c].reset_index(drop=True) for i, (c, _) in enumerate(sort)})
        ascending = [not d for _, d in sort]
        try:
            order = keys.sort_values(list(keys.columns), ascending=ascending, na_position="last", kind="stable")
        except TypeError:
            order = keys.astype(str).sort_values(list(keys.columns), ascending=ascending, kind="stable")
        idx = idx[order.index.to_numpy()]

    if mode == "rows":
        line("SANDHILL_TABLE", n, len(idx), width, esc(code), int(t is not v))
        for c in cols:
            s = t.iloc[:, c]
            line("SANDHILL_COL", esc(str(t.columns[c])), kind(s), str(s.dtype))
        page = idx[offset:offset + limit]
        if len(page):
            labels = [esc(str(x)) for x in t.index[page]]
            columns = [cells(t.iloc[page, c]) for c in cols]
            print("\n".join("\t".join(["SANDHILL_ROW", label] + [col[r] for col in columns]) for r, label in enumerate(labels)))
    if mode == "summary":
        line("SANDHILL_TABLE", n, len(idx), width)
        for c in cols:
            s = t.iloc[idx, c]
            k = kind(s)
            present = s.dropna()
            try:
                distinct = int(present.nunique())
            except TypeError:
                distinct = ""
            stats, hist = ["", "", "", ""], ""
            if k == "number" and len(present):
                w = pd.to_numeric(present, errors="coerce").to_numpy(dtype=float)
                w = w[np.isfinite(w)]
                if len(w):
                    lo, hi = float(w.min()), float(w.max())
                    stats = [repr(lo), repr(hi), repr(float(w.mean())), repr(float(np.median(w)))]
                    counts = np.histogram(w, bins=20, range=(lo, hi))[0] if hi > lo else [len(w)]
                    hist = ",".join(str(int(x)) for x in counts)
            line("SANDHILL_SUM", c, len(s) - len(present), distinct, *stats, hist)
            if k not in ("number", "other") and distinct != "" and distinct <= 1000 and len(present):
                if k == "category":
                    counts = present.value_counts(sort=False)
                    counts = counts[counts > 0]
                    if len(counts) > 10:
                        counts = counts.sort_values(ascending=False, kind="stable")
                else:
                    counts = present.astype(str).value_counts()
                for value, count in list(counts.items())[:10]:
                    line("SANDHILL_TOP", c, int(count), esc(str(value)))
    if mode == "export":
        import os
        os.makedirs(os.path.dirname(file) or ".", exist_ok=True)
        t.iloc[idx].to_csv(file, index=not isinstance(t.index, pd.RangeIndex))
        line("SANDHILL_WROTE", len(idx))

_sandhill_table(__PATH__, __MODE__, __SORT__, __FILTERS__, __OFFSET__, __LIMIT__, __FILE__)
del _sandhill_table
"#;

/// The code that reads `ask` from the table at its path in a session of
/// `language`.
pub fn code(language: &str, ask: &Ask) -> Option<String> {
    match language.to_ascii_lowercase().as_str() {
        "r" => Some(r_code(ask)),
        "python" | "python3" => Some(python_code(ask)),
        _ => None,
    }
}

fn r_code(ask: &Ask) -> String {
    let sort_col: Vec<String> = ask
        .sort
        .iter()
        .map(|s| format!("{}L", s.column + 1))
        .collect();
    let sort_desc: Vec<&str> = ask
        .sort
        .iter()
        .map(|s| if s.descending { "TRUE" } else { "FALSE" })
        .collect();
    let filters: Vec<String> = ask
        .filters
        .iter()
        .map(|f| {
            format!(
                "list(col = {}L, op = {}, values = {})",
                f.column + 1,
                literal(&f.op),
                r_strings(&f.values)
            )
        })
        .collect();
    R.replace("__WALK__", R_WALK)
        .replace("__PATH__", &r_strings(ask.path))
        .replace("__MODE__", &literal(ask.mode.name()))
        .replace("__SORT_COL__", &format!("c({})", sort_col.join(", ")))
        .replace("__SORT_DESC__", &format!("c({})", sort_desc.join(", ")))
        .replace("__FILTERS__", &format!("list({})", filters.join(", ")))
        .replace("__OFFSET__", &ask.offset.to_string())
        .replace("__LIMIT__", &ask.limit.to_string())
        .replace("__FILE__", &literal(ask.file))
}

fn python_code(ask: &Ask) -> String {
    let sort: Vec<String> = ask
        .sort
        .iter()
        .map(|s| {
            format!(
                "({}, {})",
                s.column,
                if s.descending { "True" } else { "False" }
            )
        })
        .collect();
    let filters: Vec<String> = ask
        .filters
        .iter()
        .map(|f| {
            format!(
                "({}, {}, {})",
                f.column,
                literal(&f.op),
                python_strings(&f.values)
            )
        })
        .collect();
    PYTHON
        .replace("__WALK__", PYTHON_WALK)
        .replace("__PATH__", &python_strings(ask.path))
        .replace("__MODE__", &literal(ask.mode.name()))
        .replace("__SORT__", &format!("[{}]", sort.join(", ")))
        .replace("__FILTERS__", &format!("[{}]", filters.join(", ")))
        .replace("__OFFSET__", &ask.offset.to_string())
        .replace("__LIMIT__", &ask.limit.to_string())
        .replace("__FILE__", &literal(ask.file))
}

/// A JSON string is also an R string literal and a Python one.
fn literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn r_strings(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| literal(s)).collect();
    format!("as.character(c({}))", items.join(", "))
}

fn python_strings(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| literal(s)).collect();
    format!("[{}]", items.join(", "))
}

/// What the code printed for [`Mode::Rows`].
#[derive(Debug, Default, PartialEq)]
pub struct Rows {
    pub total: u64,
    pub rows: u64,
    pub width: u64,
    pub code: String,
    pub matrix: bool,
    pub columns: Vec<TableColumn>,
    pub labels: Vec<String>,
    pub values: Vec<Vec<Value>>,
}

/// What the code printed for [`Mode::Summary`]: rows that pass, and each
/// column's summary.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub rows: u64,
    pub columns: Vec<ColumnSummary>,
}

pub fn parse_rows(stdout: &str) -> Rows {
    let mut parsed = Rows::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_TABLE") => {
                parsed.total = number(fields.next());
                parsed.rows = number(fields.next());
                parsed.width = number(fields.next());
                parsed.code = unescape(fields.next().unwrap_or(""));
                parsed.matrix = fields.next() == Some("1");
            }
            Some("SANDHILL_COL") => parsed.columns.push(TableColumn {
                name: unescape(fields.next().unwrap_or("")),
                kind: fields.next().unwrap_or("other").to_string(),
                kind_detail: fields.next().unwrap_or("").to_string(),
            }),
            Some("SANDHILL_ROW") => {
                parsed.labels.push(unescape(fields.next().unwrap_or("")));
                let row = fields
                    .enumerate()
                    .map(|(i, text)| {
                        let kind = parsed.columns.get(i).map_or("other", |c| c.kind.as_str());
                        cell(text, kind)
                    })
                    .collect();
                parsed.values.push(row);
            }
            _ => {}
        }
    }
    parsed
}

pub fn parse_summary(stdout: &str) -> Summary {
    let mut parsed = Summary::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_TABLE") => {
                fields.next();
                parsed.rows = number(fields.next());
            }
            Some("SANDHILL_SUM") => {
                let column = number(fields.next()) as u32;
                let missing = number(fields.next());
                let distinct = fields.next().and_then(|d| d.trim().parse().ok());
                let mut stat = || {
                    fields
                        .next()
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .filter(|x| x.is_finite())
                };
                let (min, max, mean, median) = (stat(), stat(), stat(), stat());
                let histogram = fields
                    .next()
                    .unwrap_or("")
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                parsed.columns.push(ColumnSummary {
                    column,
                    missing,
                    distinct,
                    min,
                    max,
                    mean,
                    median,
                    histogram,
                    top: Vec::new(),
                });
            }
            Some("SANDHILL_TOP") => {
                let column = number(fields.next()) as u32;
                let count = number(fields.next());
                let value = unescape(fields.next().unwrap_or(""));
                if let Some(summary) = parsed.columns.iter_mut().rfind(|c| c.column == column) {
                    summary.top.push(ValueCount { value, count });
                }
            }
            _ => {}
        }
    }
    parsed
}

/// How many rows an export wrote, if it got that far.
pub fn parse_written(stdout: &str) -> Option<u64> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("SANDHILL_WROTE\t"))
        .and_then(|n| n.trim().parse().ok())
}

fn number(field: Option<&str>) -> u64 {
    field
        .and_then(|n| n.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map_or(0, |n| n as u64)
}

/// A cell as JSON: null where missing, a number in a number column when it
/// is one (not Inf or NaN), otherwise its text.
fn cell(text: &str, kind: &str) -> Value {
    if text == "\\N" {
        return Value::Null;
    }
    let text = unescape(text);
    if kind == "number"
        && let Ok(n) = text.trim().parse::<f64>()
        && n.is_finite()
    {
        if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
            return Value::from(n as i64);
        }
        return Value::from(n);
    }
    Value::String(text)
}

/// Undoes the kernel's escaping of backslashes, tabs and line ends.
fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rows_with_numbers_text_and_missing_cells() {
        let out = "noise\nSANDHILL_TABLE\t2638\t618\t7\tseu@meta.data\t0\n\
                   SANDHILL_COL\tnCount_RNA\tnumber\tdbl\n\
                   SANDHILL_COL\tcell type\tcategory\tfct\n\
                   SANDHILL_COL\tnote\ttext\tchr\n\
                   SANDHILL_ROW\tAAAC-1\t2197\tCD14+ Mono\ta\\tb\\\\c\n\
                   SANDHILL_ROW\tAAAG-1\t0.25\t\\N\tInf\n";
        let rows = parse_rows(out);
        assert_eq!((rows.total, rows.rows, rows.width), (2638, 618, 7));
        assert_eq!(rows.code, "seu@meta.data");
        assert!(!rows.matrix);
        assert_eq!(rows.columns[1].name, "cell type");
        assert_eq!(rows.columns[1].kind_detail, "fct");
        assert_eq!(rows.labels, vec!["AAAC-1", "AAAG-1"]);
        assert_eq!(
            rows.values[0],
            vec![json!(2197), json!("CD14+ Mono"), json!("a\tb\\c")]
        );
        assert_eq!(rows.values[1], vec![json!(0.25), Value::Null, json!("Inf")]);
    }

    #[test]
    fn parses_summaries_with_histograms_and_top_values() {
        let out = "SANDHILL_TABLE\t2638\t618\t3\n\
                   SANDHILL_SUM\t0\t2\t600\t548\t15818\t2366.5\t2197\t3,10,0,1\n\
                   SANDHILL_SUM\t1\t0\t2\t\t\t\t\t\n\
                   SANDHILL_TOP\t1\t400\tCD14+ Mono\n\
                   SANDHILL_TOP\t1\t218\tFCGR3A+ Mono\n\
                   SANDHILL_SUM\t2\t0\t\t\t\t\t\t\n";
        let summary = parse_summary(out);
        assert_eq!(summary.rows, 618);
        let numbers = &summary.columns[0];
        assert_eq!((numbers.missing, numbers.distinct), (2, Some(600)));
        assert_eq!(
            (numbers.min, numbers.max, numbers.median),
            (Some(548.0), Some(15818.0), Some(2197.0))
        );
        assert_eq!(numbers.histogram, vec![3, 10, 0, 1]);
        let category = &summary.columns[1];
        assert_eq!(category.min, None);
        assert!(category.histogram.is_empty());
        assert_eq!(
            category
                .top
                .iter()
                .map(|t| (t.value.as_str(), t.count))
                .collect::<Vec<_>>(),
            vec![("CD14+ Mono", 400), ("FCGR3A+ Mono", 218)]
        );
        assert_eq!(summary.columns[2].distinct, None);
        assert_eq!(parse_written("x\nSANDHILL_WROTE\t618\n"), Some(618));
    }

    #[test]
    fn asks_go_in_as_literals() {
        let path = vec!["seu".to_string(), "meta \"data\"".to_string()];
        let sort = vec![TableSort {
            column: 2,
            descending: true,
        }];
        let filters = vec![
            TableFilter {
                column: 0,
                op: "<".into(),
                values: vec!["5".into()],
            },
            TableFilter {
                column: 4,
                op: "missing".into(),
                values: vec![],
            },
        ];
        let ask = Ask {
            mode: Mode::Rows,
            path: &path,
            sort: &sort,
            filters: &filters,
            offset: 200,
            limit: 100,
            file: "",
        };
        let r = code("R", &ask).unwrap();
        assert!(r.contains(r#".path <- as.character(c("seu", "meta \"data\""))"#));
        assert!(r.contains(".sort_col <- c(3L)"));
        assert!(r.contains(".sort_desc <- c(TRUE)"));
        assert!(r.contains(r#".filters <- list(list(col = 1L, op = "<", values = as.character(c("5"))), list(col = 5L, op = "missing", values = as.character(c())))"#));
        assert!(r.contains(".offset <- 200") && r.contains(".limit <- 100"));
        assert!(r.contains(".find <- function(path)"));
        let py = code("python", &ask).unwrap();
        assert!(py.contains(r#"_sandhill_table(["seu", "meta \"data\""], "rows", [(2, True)], [(0, "<", ["5"]), (4, "missing", [])], 200, 100, "")"#));
        assert!(py.contains("    def walk(path):"));
        assert!(code("julia", &ask).is_none());
    }
}
