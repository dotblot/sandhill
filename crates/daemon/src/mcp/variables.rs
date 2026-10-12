//! Listing a session's variables by running a short piece of code in it,
//! quietly (no history, no execution count), that prints one line per
//! variable. Summaries know the usual bioinformatics objects: Seurat,
//! SingleCellExperiment and other SummarizedExperiments, AnnData, data
//! frames and arrays.

/// Each variable's line starts with this, then name, class, size in bytes
/// (-1 if unknown) and a summary, separated by tabs.
pub const MARK: &str = "SANDHILL_VAR";

pub const R: &str = r#"local({
  .clean <- function(s) gsub("[\t\r\n]+", " ", paste(s, collapse = " "))
  .n <- function(n) format(n, big.mark = ",", scientific = FALSE, trim = TRUE)
  .by <- if (isTRUE(l10n_info()[["UTF-8"]])) "\u00d7" else "x"
  .names <- function(label, v) if (length(v)) paste0("; ", label, ": ", paste(utils::head(v, 8), collapse = ", "), if (length(v) > 8) ", …" else "") else ""
  .describe <- function(x) {
    if (inherits(x, "Seurat")) {
      id <- tryCatch(x@active.ident, error = function(e) NULL)
      return(paste0(.n(ncol(x)), " cells ", .by, " ", .n(nrow(x)), " features",
        if (length(id)) paste0(", ", nlevels(id), " identities") else "",
        .names("assays", tryCatch(names(x@assays), error = function(e) NULL)),
        .names("reductions", tryCatch(names(x@reductions), error = function(e) NULL)),
        .names("meta.data", tryCatch(colnames(x@meta.data), error = function(e) NULL))))
    }
    if (inherits(x, "SummarizedExperiment")) {
      return(paste0(.n(ncol(x)), " columns ", .by, " ", .n(nrow(x)), " rows",
        .names("assays", tryCatch(SummarizedExperiment::assayNames(x), error = function(e) NULL)),
        .names("reducedDims", tryCatch(SingleCellExperiment::reducedDimNames(x), error = function(e) NULL)),
        .names("colData", tryCatch(colnames(SummarizedExperiment::colData(x)), error = function(e) NULL))))
    }
    if (is.data.frame(x) || inherits(x, "DataFrame")) {
      return(paste0(.n(nrow(x)), " ", .by, " ", .n(ncol(x)), .names("columns", names(x))))
    }
    d <- dim(x)
    if (!is.null(d)) return(paste0(paste(.n(d), collapse = paste0(" ", .by, " ")), " ", typeof(x)))
    if (is.factor(x)) return(paste0(.n(length(x)), " values, ", nlevels(x), " levels"))
    if (is.list(x)) return(paste0(.n(length(x)), " items", .names("names", names(x))))
    if (is.atomic(x)) {
      if (length(x) == 1) return(substr(paste(format(x), collapse = " "), 1, 100))
      return(paste0(.n(length(x)), " ", typeof(x), " values"))
    }
    ""
  }
  for (.v in ls(envir = globalenv())) {
    .x <- get(.v, envir = globalenv())
    if (is.function(.x)) next
    .size <- tryCatch(as.numeric(utils::object.size(.x)), error = function(e) -1)
    .d <- tryCatch(.describe(.x), error = function(e) "")
    cat("SANDHILL_VAR", .v, class(.x)[1], format(.size, scientific = FALSE), .clean(.d), sep = "\t")
    cat("\n")
  }
  invisible(NULL)
})
"#;

pub const PYTHON: &str = r#"def _sandhill_variables():
    import sys, types
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()
    hidden = set(getattr(ip, "user_ns_hidden", None) or ())
    skip = (types.ModuleType, types.FunctionType, types.BuiltinFunctionType, types.MethodType, type)
    by = "×"

    def names(label, items):
        items = [str(i) for i in items]
        if not items:
            return ""
        return f"; {label}: " + ", ".join(items[:8]) + (", …" if len(items) > 8 else "")

    def describe(v):
        name = type(v).__name__
        shape = getattr(v, "shape", None)
        if name == "AnnData":
            return (f"{v.n_obs:,} obs {by} {v.n_vars:,} vars" + names("obs", v.obs.columns)
                    + names("layers", v.layers.keys()) + names("obsm", v.obsm.keys()))
        if name == "DataFrame" and isinstance(shape, tuple):
            return f"{shape[0]:,} {by} {shape[1]:,}" + names("columns", v.columns)
        if name == "Series" and isinstance(shape, tuple):
            return f"{shape[0]:,} values, {v.dtype}"
        if isinstance(shape, tuple):
            dtype = getattr(v, "dtype", None)
            return f" {by} ".join(f"{n:,}" for n in shape) + (f" {dtype}" if dtype is not None else "")
        if isinstance(v, dict):
            return f"{len(v):,} items" + names("keys", list(v)[:9])
        if isinstance(v, (list, tuple, set, frozenset)):
            return f"{len(v):,} items"
        return repr(v)[:100]

    def size(v):
        if hasattr(v, "memory_usage") and hasattr(v, "columns"):
            return int(v.memory_usage(index=True, deep=False).sum())
        if hasattr(v, "nbytes"):
            return int(v.nbytes)
        return sys.getsizeof(v)

    for key, value in list(ns.items()):
        if key.startswith("_") or key in hidden or key in ("In", "Out", "exit", "quit", "get_ipython"):
            continue
        if isinstance(value, skip):
            continue
        try:
            text = " ".join(describe(value).split())
        except Exception:
            text = ""
        try:
            nbytes = size(value)
        except Exception:
            nbytes = -1
        print("SANDHILL_VAR", key, type(value).__name__, nbytes, text, sep="\t")

_sandhill_variables()
del _sandhill_variables
"#;

/// The snippet for a session's language.
pub fn code_for(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "r" => Some(R),
        "python" => Some(PYTHON),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    pub class: String,
    pub size: Option<u64>,
    pub summary: String,
}

/// The variables in what the snippet printed.
pub fn parse(stdout: &str) -> Vec<Variable> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.strip_prefix(MARK)?.strip_prefix('\t')?.splitn(4, '\t');
            let name = fields.next()?.to_string();
            let class = fields.next()?.to_string();
            let size = fields
                .next()?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|s| *s >= 0.0)
                .map(|s| s as u64);
            let summary = fields.next().unwrap_or("").trim().to_string();
            Some(Variable {
                name,
                class,
                size,
                summary,
            })
        })
        .collect()
}

/// One line per variable, aligned, for an agent to read.
pub fn format(variables: &[Variable]) -> String {
    let width = |f: fn(&Variable) -> usize| variables.iter().map(f).max().unwrap_or(0);
    let name_width = width(|v| v.name.chars().count());
    let class_width = width(|v| v.class.chars().count());
    variables
        .iter()
        .map(|v| {
            let size = v.size.map(super::text::bytes).unwrap_or_default();
            format!(
                "{:name_width$}  {:class_width$}  {:>8}  {}",
                v.name, v.class, size, v.summary
            )
            .trim_end()
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_snippet_lines_and_skips_the_rest() {
        let out = "noise\nSANDHILL_VAR\tseu\tSeurat\t3.12e+08\t2,638 cells × 13,714 features\nSANDHILL_VAR\tx\tint\t28\t3\n";
        assert_eq!(
            parse(out),
            vec![
                Variable {
                    name: "seu".into(),
                    class: "Seurat".into(),
                    size: Some(312_000_000),
                    summary: "2,638 cells × 13,714 features".into()
                },
                Variable {
                    name: "x".into(),
                    class: "int".into(),
                    size: Some(28),
                    summary: "3".into()
                },
            ]
        );
        assert_eq!(
            format(&parse(out)),
            "seu  Seurat  297.5 MB  2,638 cells × 13,714 features\nx    int         28 B  3"
        );
    }
}
