//! Bounded object search, sharing listing's summaries and accessor generation.
use crate::variables;
use sandhill_proto::VariableMatch;

pub fn code(language: &str, query: &str, root: &[String]) -> Option<String> {
    let listing = variables::list_code(language, root)?;
    let query = serde_json::to_string(query).ok()?;
    if language.eq_ignore_ascii_case("r") {
        let start = listing.rfind("  if (!length(.path)) {")?;
        Some(format!(
            "{}{}",
            &listing[..start],
            R.replace("__QUERY__", &query)
        ))
    } else {
        let start = listing.rfind("    if not path:")?;
        Some(format!(
            "{}{}",
            &listing[..start],
            PYTHON
                .replace("__QUERY__", &query)
                .replace("__PATH__", &serde_json::to_string(root).ok()?)
        ))
    }
}

const R: &str = r#"  .query <- tolower(__QUERY__)
  .deadline <- proc.time()[["elapsed"]] + 2
  .visited <- 0L
  .complete <- TRUE
  .visit <- function(x, code, path, depth) {
    if (.visited >= 20000L || proc.time()[["elapsed"]] >= .deadline) {
      .complete <<- FALSE
      return(invisible(NULL))
    }
    .visited <<- .visited + 1L
    .name <- tail(path, 1)
    .summary <- tryCatch(.describe(x), error = function(e) "")
    if (grepl(.query, tolower(paste(.name, substr(.summary, 1, 200))), fixed = TRUE))
      .emit(.name, x, code, path)
    if (!.has(x)) return(invisible(NULL))
    if (depth >= 6L) {
      .complete <<- FALSE
      return(invisible(NULL))
    }
    .kk <- .kids(x)
    for (.i in seq_along(.kk)) {
      if (.visited >= 20000L || proc.time()[["elapsed"]] >= .deadline) {
        .complete <<- FALSE
        break
      }
      .k <- names(.kk)[[.i]]
      .visit(.kk[[.i]], .code_part(x, code, .k), c(path, .k), depth + 1L)
    }
  }
  setTimeLimit(elapsed = 2, transient = TRUE)
  tryCatch({
    if (length(.path)) {
      .found <- .find(.path)
      .visit(.found$x, .found$code, .path, 0L)
    } else {
      for (.v in ls(envir = globalenv())) {
        if (.visited >= 20000L || proc.time()[["elapsed"]] >= .deadline) {
          .complete <- FALSE
          break
        }
        .visit(get(.v, envir = globalenv()), .code_name(.v), .v, 0L)
      }
    }
  }, error = function(e) {
    if (grepl("time limit", conditionMessage(e), fixed = TRUE)) .complete <<- FALSE else stop(e)
  }, finally = setTimeLimit(cpu = Inf, elapsed = Inf, transient = FALSE))
  cat("SANDHILL_COMPLETE", as.integer(.complete), "\n", sep = "\t")
  invisible(NULL)
})
"#;

const PYTHON: &str = r#"    import time
    deadline = time.monotonic() + 2
    query = __QUERY__.casefold()
    visited = 0
    complete = True
    limit = 20000

    def visit(value, code, path, depth):
        nonlocal visited, complete
        if visited >= 20000 or time.monotonic() >= deadline:
            complete = False
            return
        visited += 1
        try:
            summary = describe(value)[:200]
        except Exception:
            summary = ""
        if query in (str(path[-1]) + " " + summary).casefold():
            emit(path[-1], value, code, path)
        if not has_parts(value):
            return
        if depth >= 6:
            complete = False
            return
        listed, total = parts(value)
        if total > len(listed):
            complete = False
        for key, child in listed:
            if visited >= 20000 or time.monotonic() >= deadline:
                complete = False
                break
            visit(child, code_part(value, code, key), path + [key], depth + 1)

    if path:
        value, code = walk(path)
        visit(value, code, path, 0)
    else:
        for key, value in ns.items():
            if key.startswith("_") or key in hidden or key in ("In", "Out", "exit", "quit", "get_ipython"):
                continue
            if isinstance(value, (types.ModuleType, type)):
                continue
            if visited >= 20000 or time.monotonic() >= deadline:
                complete = False
                break
            visit(value, key, [key], 0)
    print("SANDHILL_COMPLETE", int(complete), sep="\t")

_sandhill_variables(__PATH__)
del _sandhill_variables
"#;

pub fn parse(stdout: &str) -> (Vec<VariableMatch>, bool) {
    let mut matches = Vec::new();
    for line in stdout.lines().filter(|l| l.starts_with("SANDHILL_VAR\t")) {
        let (variables, _) = variables::parse(line);
        let Some(variable) = variables.into_iter().next() else {
            continue;
        };
        let path = line
            .split('\t')
            .skip(9)
            .filter_map(|hex| {
                let bytes: Option<Vec<u8>> = (0..hex.len())
                    .step_by(2)
                    .map(|i| {
                        hex.get(i..i + 2)
                            .and_then(|s| u8::from_str_radix(s, 16).ok())
                    })
                    .collect();
                String::from_utf8(bytes?).ok()
            })
            .collect();
        matches.push(VariableMatch {
            path,
            name: variable.name,
            class: variable.class,
            summary: variable.summary,
            accessor: variable.accessor.unwrap_or_default(),
            children: variable.children,
            table: variable.table,
        });
    }
    (
        matches,
        stdout
            .lines()
            .any(|l| l.trim_end() == "SANDHILL_COMPLETE\t1"),
    )
}
