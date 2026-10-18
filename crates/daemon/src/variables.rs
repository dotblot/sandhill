//! A session's variables, listed by running a short piece of code in it,
//! quietly (no history, no execution count), that prints one line per
//! variable. Summaries know the usual bioinformatics objects: Seurat,
//! SingleCellExperiment and other SummarizedExperiments, AnnData, data
//! frames and arrays. A variable's parts are listed the same way, by
//! walking a path of names from the variable: a data frame's columns, a
//! list's items, a Seurat object's `meta.data`, an AnnData's `obs` and
//! `var`. The Variables pane and the agents' `list_variables` both use it.

use std::time::Duration;

use anyhow::{Result, bail};
use sandhill_proto::{EVENT_RUN_FINISHED, Variable};
use tokio::sync::broadcast::error::RecvError;

use crate::journal::Journal;
use crate::session::SessionHandle;

/// Each variable's line starts with this, then name, kind, class, size in
/// bytes (-1 if unknown), 1 if it has parts, 1 if it is a table, and a
/// summary, separated by tabs.
pub const MARK: &str = "SANDHILL_VAR";
/// Then how many parts were left out.
pub const MORE: &str = "SANDHILL_MORE";

/// How long listing may take before the session is taken to be busy.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Finds an object by its path in R: `.find(path)` returns the object and
/// R code for it. `.kids(x)` gives an object's parts by name, `.is_table(x)`
/// says whether the Data Explorer shows it.
pub const R_WALK: &str = r#"  .kids <- function(x) {
    .try <- function(e) tryCatch(e, error = function(err) NULL)
    .some <- function(l) Filter(function(v) length(v) > 0, l)
    if (inherits(x, "Seurat")) return(.some(list(meta.data = .try(x@meta.data),
      assays = .try(x@assays), reductions = .try(x@reductions), active.ident = .try(x@active.ident))))
    if (inherits(x, "SummarizedExperiment")) return(.some(list(
      colData = .try(as.data.frame(SummarizedExperiment::colData(x))),
      rowData = .try(as.data.frame(SummarizedExperiment::rowData(x))),
      assays = .try(as.list(SummarizedExperiment::assays(x))),
      reducedDims = .try(as.list(SingleCellExperiment::reducedDims(x))))))
    if (is.data.frame(x) || inherits(x, "DataFrame")) return(.try(as.list(x)))
    if (is.list(x)) {
      y <- unclass(x)
      n <- names(y)
      if (is.null(n)) n <- rep("", length(y))
      blank <- is.na(n) | n == ""
      n[blank] <- paste0("[[", which(blank), "]]")
      names(y) <- n
      return(y)
    }
    NULL
  }
  .is_table <- function(x) {
    is.data.frame(x) || inherits(x, "DataFrame") ||
      (is.matrix(x) && (is.numeric(x) || is.character(x) || is.logical(x)))
  }
  .code_name <- function(k) if (identical(make.names(k), k)) k else paste0("`", gsub("`", "\\`", k), "`")
  .code_part <- function(x, code, k) {
    q <- encodeString(k, quote = '"')
    if (inherits(x, "Seurat")) return(paste0(code, "@", k))
    if (inherits(x, "SummarizedExperiment")) return(switch(k,
      colData = paste0("as.data.frame(SummarizedExperiment::colData(", code, "))"),
      rowData = paste0("as.data.frame(SummarizedExperiment::rowData(", code, "))"),
      assays = paste0("SummarizedExperiment::assays(", code, ")"),
      reducedDims = paste0("SingleCellExperiment::reducedDims(", code, ")")))
    if (grepl("^\\[\\[[0-9]+\\]\\]$", k)) return(paste0(code, k))
    if (identical(make.names(k), k)) paste0(code, "$", k) else paste0(code, "[[", q, "]]")
  }
  .find <- function(path) {
    if (!exists(path[[1]], envir = globalenv(), inherits = FALSE)) stop("no variable named ", path[[1]])
    x <- get(path[[1]], envir = globalenv())
    code <- .code_name(path[[1]])
    for (k in path[-1]) {
      kk <- .kids(x)
      if (!(k %in% names(kk))) stop("no part named ", k)
      code <- .code_part(x, code, k)
      x <- kk[[k]]
    }
    list(x = x, code = code)
  }
"#;

const R: &str = r#"local({
  .path <- __PATH__
  .clean <- function(s) gsub("[\t\r\n]+", " ", paste(s, collapse = " "))
  .n <- function(n) format(n, big.mark = ",", scientific = FALSE, trim = TRUE)
  .by <- if (isTRUE(l10n_info()[["UTF-8"]])) "×" else "x"
  .more <- if (isTRUE(l10n_info()[["UTF-8"]])) "…" else "..."
  .names <- function(label, v) if (length(v)) paste0("; ", label, ": ", paste(utils::head(v, 8), collapse = ", "), if (length(v) > 8) paste0(", ", .more) else "") else ""
  .values <- function(x) {
    v <- utils::head(x, 5)
    v <- if (is.character(v)) encodeString(v, quote = '"') else format(v, trim = TRUE, digits = 4)
    paste0(paste(v, collapse = ", "), if (length(x) > 5) paste0(", ", .more) else "")
  }
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
    if (is.function(x)) return(paste0("function(", paste(names(formals(x)), collapse = ", "), ")"))
    d <- dim(x)
    if (!is.null(d)) return(paste0(paste(.n(d), collapse = paste0(" ", .by, " ")), " ", typeof(x)))
    if (is.factor(x)) return(paste0(.n(length(x)), " values, ", nlevels(x), " levels: ", .values(levels(x))))
    if (is.list(x)) return(paste0(.n(length(x)), if (length(x) == 1) " item" else " items", .names("names", names(x))))
    if (is.atomic(x)) {
      if (length(x) == 0) return(paste0("empty ", typeof(x)))
      if (length(x) == 1) return(substr(.values(x), 1, 100))
      return(substr(paste0(.n(length(x)), " values: ", .values(x)), 1, 100))
    }
    ""
  }
  .kind <- function(x) {
    if (is.function(x)) return("function")
    if (is.atomic(x) && length(x) <= 1 && is.null(dim(x)) && !is.factor(x)) return("value")
    "data"
  }
  .has <- function(x) {
    if (inherits(x, c("Seurat", "SummarizedExperiment"))) return(TRUE)
    if (is.data.frame(x) || inherits(x, "DataFrame")) return(ncol(x) > 0)
    is.list(x) && length(x) > 0
  }
__WALK__
  .emit <- function(name, x) {
    .size <- tryCatch(as.numeric(utils::object.size(x)), error = function(e) -1)
    .d <- tryCatch(.describe(x), error = function(e) "")
    .h <- isTRUE(tryCatch(.has(x), error = function(e) FALSE))
    .tb <- isTRUE(tryCatch(.is_table(x), error = function(e) FALSE))
    cat("SANDHILL_VAR", .clean(name), .kind(x), .clean(class(x)[1]), format(.size, scientific = FALSE),
      if (.h) 1 else 0, if (.tb) 1 else 0, .clean(.d), sep = "\t")
    cat("\n")
  }
  if (!length(.path)) {
    for (.v in ls(envir = globalenv())) .emit(.v, get(.v, envir = globalenv()))
  } else {
    .x <- .find(.path)$x
    .kk <- .kids(.x)
    for (.i in seq_len(min(length(.kk), 200))) .emit(names(.kk)[[.i]], .kk[[.i]])
    if (length(.kk) > 200) cat("SANDHILL_MORE", length(.kk) - 200, "\n", sep = "\t")
  }
  invisible(NULL)
})
"#;

/// Finds an object by its path in Python, inside a function that has set
/// `ns` to the user's namespace: `walk(path)` returns the object and Python
/// code for it. `part(v, key)` gives one of an object's parts,
/// `is_table(v)` says whether the Data Explorer shows it.
pub const PYTHON_WALK: &str = r#"    maps = ("obsm", "varm", "obsp", "varp", "layers")

    def keys(mapping):
        # anndata 0.13 lists X in layers.keys() as None.
        return [k for k in mapping.keys() if k is not None]

    def part(v, key):
        name = type(v).__name__
        if name in ("AnnData", "Raw"):
            x = getattr(v, key)
            if key in maps:
                return {k: x[k] for k in keys(x)}
            return dict(x) if key == "uns" else x
        if name == "DataFrame" and hasattr(v, "columns"):
            return v.iloc[:, [str(c) for c in v.columns].index(key)]
        if isinstance(v, dict):
            return next(x for k, x in v.items() if str(k) == key)
        if isinstance(v, (list, tuple)) and key.startswith("[") and key.endswith("]"):
            return v[int(key[1:-1])]
        raise KeyError(f"no part named {key}")

    def code_part(v, code, key):
        name = type(v).__name__
        if name in ("AnnData", "Raw"):
            return f"{code}.{key}"
        if name == "DataFrame" and hasattr(v, "columns"):
            column = next(c for c in v.columns if str(c) == key)
            return f"{code}[{column!r}]"
        if isinstance(v, dict):
            return f"{code}[{next(k for k in v if str(k) == key)!r}]"
        return f"{code}{key}"

    def walk(path):
        if path[0] not in ns:
            raise NameError(f"no variable named {path[0]}")
        v, code = ns[path[0]], path[0]
        for key in path[1:]:
            v, code = part(v, key), code_part(v, code, key)
        return v, code

    def is_table(v):
        name = type(v).__name__
        if name == "DataFrame" and hasattr(v, "iloc"):
            return True
        return name == "ndarray" and getattr(v, "ndim", 0) == 2

"#;

const PYTHON: &str = r#"def _sandhill_variables(path):
    import sys, types
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()
    hidden = set(getattr(ip, "user_ns_hidden", None) or ())
    functions = (types.FunctionType, types.BuiltinFunctionType, types.MethodType)
    scalars = (int, float, complex, bool, str, bytes, type(None))
    by = "×"
    limit = 200

    def names(label, items):
        items = [str(i) for i in items]
        if not items:
            return ""
        return f"; {label}: " + ", ".join(items[:8]) + (", …" if len(items) > 8 else "")

    def first(values, n):
        shown = ", ".join(repr(v) if isinstance(v, str) else str(v) for v in values)
        return shown + (", …" if n > len(values) else "")

    def describe(v):
        name = type(v).__name__
        shape = getattr(v, "shape", None)
        if name == "AnnData":
            return (f"{v.n_obs:,} obs {by} {v.n_vars:,} vars" + names("obs", v.obs.columns)
                    + names("layers", keys(v.layers)) + names("obsm", keys(v.obsm)))
        if name == "DataFrame" and isinstance(shape, tuple):
            return f"{shape[0]:,} {by} {shape[1]:,}" + names("columns", v.columns)
        if name == "Series" and isinstance(shape, tuple):
            return f"{shape[0]:,} values, {v.dtype}: " + first(list(v.head(5)), shape[0])
        if isinstance(v, functions):
            try:
                import inspect
                return f"{v.__name__}{inspect.signature(v)}"
            except Exception:
                return getattr(v, "__name__", "function")
        if isinstance(shape, tuple) and shape:
            dtype = getattr(v, "dtype", None)
            text = f" {by} ".join(f"{n:,}" for n in shape) + (f" {dtype}" if dtype is not None else "")
            nnz = getattr(v, "nnz", None)
            return text + (f", {nnz:,} stored" if isinstance(nnz, int) else "")
        if isinstance(v, dict):
            return f"{len(v):,} item{'' if len(v) == 1 else 's'}" + names("keys", list(v)[:9])
        if isinstance(v, (list, tuple, set, frozenset)):
            return f"{len(v):,} item{'' if len(v) == 1 else 's'}"
        return repr(v)[:100]

    def kind(v):
        if isinstance(v, functions):
            return "function"
        if isinstance(v, scalars) or getattr(v, "shape", None) == ():
            return "value"
        return "data"

    def size(v):
        name = type(v).__name__
        if name in ("DataFrame", "Series") and hasattr(v, "memory_usage"):
            used = v.memory_usage(index=True, deep=False)
            return int(used.sum() if hasattr(used, "sum") else used)
        if all(hasattr(v, a) for a in ("data", "indices", "indptr")):
            return int(v.data.nbytes + v.indices.nbytes + v.indptr.nbytes)
        if hasattr(v, "nbytes"):
            return int(v.nbytes)
        return sys.getsizeof(v)

    def has_parts(v):
        name = type(v).__name__
        if name in ("AnnData", "Raw"):
            return True
        if name == "DataFrame" and hasattr(v, "columns"):
            return len(v.columns) > 0
        return isinstance(v, (dict, list, tuple)) and len(v) > 0

__WALK__
    def parts(v):
        """The first `limit` parts as (name, value), and how many there are."""
        name = type(v).__name__
        if name == "AnnData":
            shown = ["X", "obs", "var"] + [k for k in maps if getattr(v, k, None) is not None and keys(getattr(v, k))]
            shown += ["uns"] if len(v.uns) else []
            shown += ["raw"] if v.raw is not None else []
            if v.X is None:
                shown.remove("X")
            return [(k, part(v, k)) for k in shown], len(shown)
        if name == "Raw":
            return [("X", v.X), ("var", v.var)], 2
        if name == "DataFrame" and hasattr(v, "columns"):
            n = len(v.columns)
            return [(str(c), v.iloc[:, i]) for i, c in enumerate(v.columns[:limit])], n
        if isinstance(v, dict):
            return [(str(k), x) for k, x in list(v.items())[:limit]], len(v)
        if isinstance(v, (list, tuple)):
            return [(f"[{i}]", x) for i, x in enumerate(v[:limit])], len(v)
        return [], 0

    def emit(key, value):
        try:
            text = " ".join(describe(value).split())
        except Exception:
            text = ""
        try:
            nbytes = size(value)
        except Exception:
            nbytes = -1
        try:
            more = 1 if has_parts(value) else 0
        except Exception:
            more = 0
        try:
            table = 1 if is_table(value) else 0
        except Exception:
            table = 0
        clean = " ".join(str(key).split())
        print("SANDHILL_VAR", clean, kind(value), type(value).__name__, nbytes, more, table, text, sep="\t")

    if not path:
        for key, value in list(ns.items()):
            if key.startswith("_") or key in hidden or key in ("In", "Out", "exit", "quit", "get_ipython"):
                continue
            if isinstance(value, (types.ModuleType, type)):
                continue
            emit(key, value)
        return
    v, _ = walk(path)
    listed, total = parts(v)
    for key, value in listed:
        emit(key, value)
    if total > len(listed):
        print("SANDHILL_MORE", total - len(listed), sep="\t")

_sandhill_variables(__PATH__)
del _sandhill_variables
"#;

const R_DELETE: &str = r#"local({
  .names <- __NAMES__
  rm(list = intersect(.names, ls(envir = globalenv(), all.names = TRUE)), envir = globalenv())
  invisible(gc())
})
"#;

const PYTHON_DELETE: &str = r#"def _sandhill_delete(names):
    import gc, re
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()
    gone = [ns.pop(n) for n in names if n in ns]
    held = lambda v: any(v is g for g in gone)
    # IPython keeps outputs too (Out, _, _12, and copies of _ and _12 in
    # its hidden names), which would hold on to them.
    for space in (ns, getattr(ip, "user_ns_hidden", None)):
        if isinstance(space, dict):
            for key in [k for k, v in space.items() if re.fullmatch(r"_+|_\d+", k) and held(v)]:
                space[key] = None
    out = ns.get("Out")
    if isinstance(out, dict):
        for key in [k for k, v in out.items() if held(v)]:
            del out[key]
    hook = getattr(ip, "displayhook", None)
    for attr in ("_", "__", "___"):
        if held(getattr(hook, attr, None)):
            setattr(hook, attr, "")
    gone.clear()
    gc.collect()

_sandhill_delete(__NAMES__)
del _sandhill_delete
"#;

/// The code that lists the variables at `path` (empty for all of them) in
/// a session of `language`.
pub fn list_code(language: &str, path: &[String]) -> Option<String> {
    match normalize(language).as_str() {
        "r" => Some(
            R.replace("__WALK__", R_WALK)
                .replace("__PATH__", &r_strings(path)),
        ),
        "python" => Some(
            PYTHON
                .replace("__WALK__", PYTHON_WALK)
                .replace("__PATH__", &python_strings(path)),
        ),
        _ => None,
    }
}

/// The code that deletes `names` from a session of `language` and frees
/// their memory.
pub fn delete_code(language: &str, names: &[String]) -> Option<String> {
    match normalize(language).as_str() {
        "r" => Some(R_DELETE.replace("__NAMES__", &r_strings(names))),
        "python" => Some(PYTHON_DELETE.replace("__NAMES__", &python_strings(names))),
        _ => None,
    }
}

fn normalize(language: &str) -> String {
    match language.to_ascii_lowercase().as_str() {
        "python3" => "python".into(),
        other => other.into(),
    }
}

/// A JSON string is also an R string literal and a Python one: both read
/// `\"`, `\\`, `\n` and `\u00XX`.
fn literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn r_strings(items: &[String]) -> String {
    if items.is_empty() {
        return "character(0)".into();
    }
    let items: Vec<String> = items.iter().map(|s| literal(s)).collect();
    format!("c({})", items.join(", "))
}

fn python_strings(items: &[String]) -> String {
    let items: Vec<String> = items.iter().map(|s| literal(s)).collect();
    format!("[{}]", items.join(", "))
}

/// The variables in what the listing printed, and how many were left out.
pub fn parse(stdout: &str) -> (Vec<Variable>, u64) {
    let mut more = 0;
    let variables = stdout
        .lines()
        .filter_map(|line| {
            if let Some(n) = line.strip_prefix(MORE) {
                more = n.trim().parse().unwrap_or(0);
                return None;
            }
            let mut fields = line.strip_prefix(MARK)?.strip_prefix('\t')?.splitn(7, '\t');
            let name = fields.next()?.to_string();
            let kind = fields.next()?.to_string();
            let class = fields.next()?.to_string();
            let size = fields
                .next()?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|s| *s >= 0.0)
                .map(|s| s as u64);
            let children = fields.next()?.trim() == "1";
            let table = fields.next()?.trim() == "1";
            let summary = fields.next().unwrap_or("").trim().to_string();
            Some(Variable {
                name,
                kind,
                class,
                size,
                children,
                table,
                summary,
            })
        })
        .collect();
    (variables, more)
}

/// What a quiet run printed.
pub struct Printed {
    pub stdout: String,
    /// The error it raised, as "Name: message".
    pub error: Option<String>,
}

/// Runs `code` quietly in a session and waits up to `timeout` for it to
/// finish.
pub async fn run_quietly(
    handle: &SessionHandle,
    journal: &Journal,
    code: String,
    timeout: Duration,
) -> Result<Printed> {
    // Subscribe first, so the end of a quick run isn't missed.
    let mut events = handle.subscribe();
    let run = handle.execute(code, None, true, None).await?.run;
    let finished = async {
        loop {
            match events.recv().await {
                Ok(event) if event.run.as_deref() == Some(run.as_str()) => {
                    if event.msg_type == EVENT_RUN_FINISHED {
                        return Ok(());
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => bail!("the session ended"),
            }
        }
    };
    match tokio::time::timeout(timeout, finished).await {
        Ok(result) => result?,
        Err(_) => bail!("it took too long; the session may be busy"),
    }
    // The journal has every event of the run by the time it finishes.
    let mut printed = Printed {
        stdout: String::new(),
        error: None,
    };
    for (msg_type, content) in journal.run_events(&run, i64::MAX)? {
        match msg_type.as_str() {
            "stream" if content["name"] == "stdout" => printed
                .stdout
                .push_str(content["text"].as_str().unwrap_or("")),
            "error" => {
                printed.error = Some(format!(
                    "{}: {}",
                    content["ename"].as_str().unwrap_or("Error"),
                    content["evalue"].as_str().unwrap_or("")
                ))
            }
            _ => {}
        }
    }
    // Read once, it needn't stay: listing after every run would fill the journal.
    journal.forget_run(&run)?;
    Ok(printed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_listing_and_skips_the_rest() {
        let out = "noise\nSANDHILL_VAR\tseu\tdata\tSeurat\t3.12e+08\t1\t0\t2,638 cells × 13,714 features\nSANDHILL_VAR\tx\tvalue\tint\t28\t0\t0\t3\nSANDHILL_VAR\tobs\tdata\tDataFrame\t1200\t1\t1\t3 × 2\nSANDHILL_MORE\t12\t\n";
        let (variables, more) = parse(out);
        assert_eq!(
            variables,
            vec![
                Variable {
                    name: "seu".into(),
                    kind: "data".into(),
                    class: "Seurat".into(),
                    size: Some(312_000_000),
                    children: true,
                    table: false,
                    summary: "2,638 cells × 13,714 features".into()
                },
                Variable {
                    name: "x".into(),
                    kind: "value".into(),
                    class: "int".into(),
                    size: Some(28),
                    children: false,
                    table: false,
                    summary: "3".into()
                },
                Variable {
                    name: "obs".into(),
                    kind: "data".into(),
                    class: "DataFrame".into(),
                    size: Some(1200),
                    children: true,
                    table: true,
                    summary: "3 × 2".into()
                },
            ]
        );
        assert_eq!(more, 12);
    }

    #[test]
    fn paths_and_names_go_in_as_string_literals() {
        let path = vec!["seu".to_string(), "meta \"data\"\\\n".to_string()];
        let r = list_code("R", &path).unwrap();
        assert!(r.contains(r#".path <- c("seu", "meta \"data\"\\\n")"#));
        assert!(
            list_code("r", &[])
                .unwrap()
                .contains(".path <- character(0)")
        );
        let py = list_code("python3", &path).unwrap();
        assert!(py.contains(r#"_sandhill_variables(["seu", "meta \"data\"\\\n"])"#));
        let delete = delete_code("python", &["a".into(), "b".into()]).unwrap();
        assert!(delete.contains(r#"_sandhill_delete(["a", "b"])"#));
        assert!(
            delete_code("r", &["a".into()])
                .unwrap()
                .contains(r#".names <- c("a")"#)
        );
        assert!(list_code("julia", &[]).is_none());
    }
}
