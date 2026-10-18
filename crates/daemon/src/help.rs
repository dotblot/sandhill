//! Help for the Help pane: searching a session's help topics, reading a
//! page, and a function's source, by running a short piece of code in the
//! session quietly, as the variable listing does.
//!
//! In R, topics are every installed package's help aliases, read from each
//! package's `Meta/Rd.rds` and kept in the session (in `options()`) until a
//! library folder changes; a page is the package's own HTML from
//! `tools::Rd2HTML`. In Python, topics are the public functions, classes
//! and modules of the modules already imported; a page is the object's
//! signature, docstring and public members, found the way `pydoc` finds
//! them (importing a module when asked for it by name).

use sandhill_proto::{HelpMember, HelpTopic};

use crate::tables::{literal, unescape};

/// Topics returned unless asked otherwise, and at most.
pub const TOPICS: u32 = 50;
pub const MAX_TOPICS: u32 = 500;

/// What the code is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Topics,
    Page,
    Source,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Topics => "topics",
            Mode::Page => "page",
            Mode::Source => "source",
        }
    }
}

/// What to look up.
pub struct Ask<'a> {
    pub mode: Mode,
    /// For [`Mode::Topics`]: what to search for; empty lists a package's
    /// topics.
    pub query: &'a str,
    /// A package to search or look in; empty for all of them.
    pub package: &'a str,
    /// For [`Mode::Page`] and [`Mode::Source`].
    pub topic: &'a str,
    pub limit: u32,
}

const R: &str = r#"local({
  .mode <- __MODE__
  .query <- __QUERY__
  .package <- __PACKAGE__
  .topic <- __TOPIC__
  .limit <- __LIMIT__
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
  # Every installed package's help aliases and titles, read again when a
  # library folder changes (a package was installed or removed).
  .index <- function() {
    .libs <- .libPaths()
    .stamp <- paste(.libs, file.info(.libs)$mtime, collapse = "\n")
    .cache <- getOption("sandhill.help")
    if (is.environment(.cache) && identical(.cache$stamp, .stamp)) return(.cache)
    .dirs <- list.files(.libs, full.names = TRUE)
    .dirs <- .dirs[!duplicated(basename(.dirs))]
    .alias <- list()
    .pkg <- list()
    .title <- list()
    for (.d in .dirs) {
      .f <- file.path(.d, "Meta", "Rd.rds")
      if (!file.exists(.f)) next
      .x <- tryCatch(readRDS(.f), error = function(e) NULL)
      if (!is.data.frame(.x) || !nrow(.x) || is.null(.x$Aliases)) next
      if (!is.null(.x$Keywords)) .x <- .x[!vapply(.x$Keywords, function(k) "internal" %in% k, NA), , drop = FALSE]
      .n <- lengths(.x$Aliases)
      .b <- basename(.d)
      .alias[[.b]] <- unlist(.x$Aliases, use.names = FALSE)
      .pkg[[.b]] <- rep(.b, sum(.n))
      .title[[.b]] <- rep(as.character(.x$Title), .n)
    }
    .a <- as.character(unlist(.alias, use.names = FALSE))
    # S4 methods' aliases ("show,Seurat-method") would crowd the results.
    .keep <- !grepl(",", .a, fixed = TRUE)
    .cache <- new.env()
    .cache$stamp <- .stamp
    .cache$alias <- .a[.keep]
    .cache$lower <- tolower(.cache$alias)
    .cache$package <- unlist(.pkg, use.names = FALSE)[.keep]
    .cache$title <- gsub("\\s+", " ", unlist(.title, use.names = FALSE)[.keep])
    .cache$ltitle <- tolower(.cache$title)
    options(sandhill.help = .cache)
    .cache
  }
  # Attached packages first, in search order, then loaded ones.
  .rank_pkgs <- function(p) {
    if (!length(p)) return(integer(0))
    .at <- match(paste0("package:", p), search())
    ifelse(!is.na(.at), .at, ifelse(p %in% loadedNamespaces(), 1000L, 2000L))
  }
  # The packages a package name means: itself, or those it starts.
  .wanted <- function(all) {
    .u <- unique(all)
    .p <- tolower(.package)
    .exact <- .u[tolower(.u) == .p]
    if (length(.exact)) .exact else .u[startsWith(tolower(.u), .p)]
  }
  if (.mode == "topics") {
    .ix <- .index()
    .in <- if (nzchar(.package)) .ix$package %in% .wanted(.ix$package) else rep(TRUE, length(.ix$alias))
    .q <- tolower(.query)
    .rank <- rep(NA_integer_, length(.ix$alias))
    if (!nzchar(.q)) {
      # Names before operators and the like ("$.multiPhylo").
      .rank[.in] <- ifelse(grepl("^[A-Za-z]", .ix$alias[.in]), 2L, 3L)
    } else {
      .rank[.in & grepl(.q, .ix$ltitle, fixed = TRUE)] <- 4L
      .rank[.in & grepl(.q, .ix$lower, fixed = TRUE)] <- 3L
      .rank[.in & startsWith(.ix$lower, .q)] <- 2L
      .rank[.in & .ix$lower == .q] <- 1L
      .rank[.in & .ix$alias == .query] <- 0L
    }
    .hit <- which(!is.na(.rank))
    .hit <- .hit[order(.rank[.hit], .rank_pkgs(.ix$package[.hit]),
      if (nzchar(.q)) nchar(.ix$alias[.hit]) else rep(0L, length(.hit)), .ix$lower[.hit])]
    .line("SANDHILL_HELP", length(.hit))
    .top <- utils::head(.hit, .limit)
    if (length(.top)) writeLines(paste("SANDHILL_TOPIC", .esc(.ix$alias[.top]), .ix$package[.top], .esc(.ix$title[.top]), sep = "\t"))
  }
  if (.mode != "topics") {
    .ix <- .index()
    .has <- unique(.ix$package[.ix$alias == .topic])
    if (!length(.has)) .has <- unique(.ix$package[.ix$lower == tolower(.topic)])
    .has <- .has[order(.rank_pkgs(.has))]
    .cands <- if (nzchar(.package)) .package else .has
  }
  if (.mode == "page") {
    .path <- character(0)
    for (.p in .cands) {
      .path <- tryCatch(as.character(do.call(utils::help, list(.topic, package = .p))), error = function(e) character(0))
      if (length(.path)) break
    }
    # Topics left out of the index (internal ones) are found in loaded packages.
    if (!length(.path) && !nzchar(.package)) .path <- as.character(do.call(utils::help, list(.topic)))
    if (!length(.path)) stop("no help for ", .topic, if (nzchar(.package)) paste(" in", .package))
    .path <- .path[[1]]
    .pkg <- basename(dirname(dirname(.path)))
    .rd <- utils:::.getHelpFile(.path)
    .args <- list(.rd, out = "", package = .pkg, dynamic = TRUE)
    if ("standalone" %in% names(formals(tools::Rd2HTML))) .args$standalone <- FALSE
    .html <- paste(suppressWarnings(utils::capture.output(do.call(tools::Rd2HTML, .args))), collapse = "\n")
    .title <- tryCatch(paste(tools:::.Rd_get_title(.rd), collapse = " "), error = function(e) "")
    .version <- tryCatch(as.character(utils::packageVersion(.pkg)), error = function(e) "")
    .line("SANDHILL_PAGE", .esc(.topic), .pkg, .esc(gsub("\\s+", " ", .title)), .version)
    for (.p in setdiff(.has, .pkg)) {
      .t <- .ix$title[.ix$package == .p & .ix$alias == .topic]
      .line("SANDHILL_ALT", .esc(.topic), .p, .esc(if (length(.t)) .t[[1]] else ""))
    }
    .line("SANDHILL_HTML", .esc(.html))
  }
  if (.mode == "source") {
    .f <- NULL
    for (.p in .cands) {
      .f <- tryCatch(get(.topic, envir = asNamespace(.p)), error = function(e) NULL)
      if (is.function(.f)) break
    }
    if (!is.function(.f) && !nzchar(.package)) .f <- tryCatch(get(.topic, envir = globalenv(), mode = "function"), error = function(e) NULL)
    if (!is.function(.f)) stop(.topic, " isn't a function, so it has no source")
    .pkg <- if (is.primitive(.f)) "base" else {
      .e <- environment(.f)
      if (is.null(.e)) "base" else if (isNamespace(.e)) getNamespaceName(.e)[[1]] else ""
    }
    .line("SANDHILL_SOURCE", .esc(.topic), .pkg, "", 0)
    .line("SANDHILL_TEXT", .esc(paste(deparse(.f, width.cutoff = 80L), collapse = "\n")))
  }
  invisible(NULL)
})
"#;

const PYTHON: &str = r#"def _sandhill_help(mode, query, package, topic, limit):
    import builtins, importlib, inspect, sys, types
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()

    def esc(s):
        return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")

    def line(*fields):
        print(*fields, sep="\t")

    def owner(obj):
        try:
            name = obj.__name__ if isinstance(obj, types.ModuleType) else getattr(obj, "__module__", None)
        except Exception:
            name = None
        return name if isinstance(name, str) else None

    def title(obj):
        try:
            doc = obj.__doc__ if isinstance(obj, (type, types.ModuleType, property)) or callable(obj) else None
        except Exception:
            doc = None
        if not isinstance(doc, str):
            return ""
        paragraphs = [p.strip() for p in inspect.cleandoc(doc).split("\n\n") if p.strip()]
        # C functions start with their signature: "add(x1, x2, /, ...)".
        if len(paragraphs) > 1 and "(" in paragraphs[0] and paragraphs[0].split("(")[0].strip().isidentifier():
            paragraphs = paragraphs[1:]
        return " ".join(paragraphs[0].split())[:300] if paragraphs else ""

    def mine():
        # The packages the user's own code imported, which come first.
        hidden = set(getattr(ip, "user_ns_hidden", None) or ())
        tops = set()
        for k, v in list(ns.items()):
            if k in hidden or k.startswith("_"):
                continue
            if isinstance(v, (types.ModuleType, type)) or inspect.isroutine(v):
                name = owner(v)
            else:
                name = owner(type(v))
            if name and not name.startswith("_"):
                tops.add(name.split(".")[0])
        return tops

    def index():
        # Public functions, classes and modules of the imported modules, each
        # once, by its shortest name. Kept until more modules are imported.
        cached = getattr(sys, "_sandhill_help_index", None)
        if cached is not None and cached[0] == len(sys.modules):
            return cached[1]
        found = {}
        for modname, mod in list(sys.modules.items()):
            if not isinstance(mod, types.ModuleType):
                continue
            parts = modname.split(".")
            if any(p.startswith("_") for p in parts):
                continue
            top = parts[0]
            found.setdefault(id(mod), (modname, top))
            try:
                items = list(vars(mod).items())
            except TypeError:
                continue
            for name, obj in items:
                if name.startswith("_") or isinstance(obj, types.ModuleType):
                    continue
                if not (isinstance(obj, type) or callable(obj)):
                    continue
                home = owner(obj)
                if home is not None and home.split(".")[0] != top:
                    continue
                path = modname + "." + name
                seen = found.get(id(obj))
                if seen is None or (seen[0].count("."), len(seen[0])) > (path.count("."), len(path)):
                    found[id(obj)] = (path, top)
        entries = list(found.values())
        sys._sandhill_help_index = (len(sys.modules), entries)
        return entries

    def lookup(path):
        if path in sys.modules:
            return sys.modules[path]
        modname, _, name = path.rpartition(".")
        return getattr(sys.modules.get(modname), name, None)

    def topics():
        entries = index()
        wanted = None
        if package:
            p = package.lower()
            tops = {top for _, top in entries}
            wanted = {t for t in tops if t.lower() == p} or {t for t in tops if t.lower().startswith(p)}
        tops = mine()
        q = query.lower()
        hits = []
        for path, top in entries:
            if wanted is not None and top not in wanted:
                continue
            name = path.rpartition(".")[2]
            low = name.lower()
            if not q:
                rank = 2
            elif path.lower() == q:
                rank = -1
            elif name == query:
                rank = 0
            elif low == q:
                rank = 1
            elif low.startswith(q):
                rank = 2
            elif q in low or ("." in q and q in path.lower()):
                rank = 3
            else:
                continue
            hits.append((rank, top not in tops, len(path) if q else 0, path.lower(), path, top))
        hits.sort()
        line("SANDHILL_HELP", len(hits))
        for hit in hits[:limit]:
            line("SANDHILL_TOPIC", esc(hit[4]), esc(hit[5]), esc(title(lookup(hit[4]))))

    def resolve(topic):
        parts = [p for p in topic.strip().split(".") if p]
        if not parts:
            raise LookupError("give a topic")
        if parts[0] in ns:
            obj, rest = ns[parts[0]], parts[1:]
            name = obj.__name__ if isinstance(obj, types.ModuleType) else parts[0]
        else:
            obj = None
            for i in range(len(parts), 0, -1):
                modname = ".".join(parts[:i])
                try:
                    obj = sys.modules.get(modname) or importlib.import_module(modname)
                except Exception:
                    continue
                name, rest = modname, parts[i:]
                break
            if obj is None:
                if not hasattr(builtins, parts[0]):
                    raise LookupError(f"no help for {topic}")
                obj, name, rest = getattr(builtins, parts[0]), parts[0], parts[1:]
        for part in rest:
            try:
                obj = getattr(obj, part)
            except AttributeError:
                if not isinstance(obj, types.ModuleType):
                    raise LookupError(f"no help for {topic}: {name} has no {part}") from None
                try:
                    obj = importlib.import_module(f"{obj.__name__}.{part}")
                except ImportError:
                    raise LookupError(f"no help for {topic}: {name} has no {part}") from None
            name = f"{name}.{part}"
        return obj, name

    def kind(obj):
        if isinstance(obj, types.ModuleType):
            return "module"
        if isinstance(obj, type):
            return "class"
        if isinstance(obj, property):
            return "property"
        if inspect.ismethod(obj) or inspect.ismethoddescriptor(obj):
            return "method"
        if callable(obj):
            return "function"
        return "value"

    def top_of(obj, name):
        home = owner(type(obj)) if kind(obj) in ("value", "property") else owner(obj)
        return (home or name).split(".")[0]

    def version(top):
        if top == "builtins":
            import platform
            return "Python " + platform.python_version()
        try:
            import importlib.metadata as md
            return md.version(top)
        except Exception:
            v = getattr(sys.modules.get(top), "__version__", "")
            return v if isinstance(v, str) else ""

    def members(obj):
        out = []
        if isinstance(obj, types.ModuleType):
            listed = getattr(obj, "__all__", None)
            listed = [n for n in listed if isinstance(n, str)] if isinstance(listed, (list, tuple)) else None
            top = obj.__name__.split(".")[0]
            for n in listed if listed is not None else sorted(vars(obj)):
                if n.startswith("_"):
                    continue
                try:
                    v = getattr(obj, n)
                except Exception:
                    continue
                if listed is None:
                    home = owner(v)
                    if not (isinstance(v, (type, types.ModuleType)) or callable(v)):
                        continue
                    if home is not None and home.split(".")[0] != top:
                        continue
                out.append((n, kind(v), v))
        elif isinstance(obj, type):
            seen = set()
            for cls in obj.__mro__:
                if cls is object:
                    continue
                for n, v in vars(cls).items():
                    if n.startswith("_") or n in seen:
                        continue
                    seen.add(n)
                    if isinstance(v, (staticmethod, classmethod)):
                        v = v.__func__
                    out.append((n, kind(v), v))
            out.sort(key=lambda m: m[0].lower())
        return out[:500]

    def page():
        obj, name = resolve(topic)
        k = kind(obj)
        top = top_of(obj, name)
        line("SANDHILL_PAGE", esc(name), esc(top), esc(title(obj if k != "value" else type(obj))), esc(version(top)), k)
        if callable(obj):
            try:
                line("SANDHILL_SIGNATURE", esc(str(inspect.signature(obj))))
            except (TypeError, ValueError):
                pass
        doc = inspect.getdoc(obj if k != "value" else type(obj))
        line("SANDHILL_TEXT", esc(doc or ""))
        for n, mk, v in members(obj):
            line("SANDHILL_MEMBER", esc(n), mk, esc(title(v)))

    def source():
        obj, name = resolve(topic)
        target = obj.fget if isinstance(obj, property) else obj
        try:
            target = inspect.unwrap(target)
            text = inspect.getsource(target)
            file, first = inspect.getsourcefile(target) or "", inspect.getsourcelines(target)[1]
        except (TypeError, OSError, ValueError):
            raise LookupError(f"{name} has no Python source; it may be built in or compiled") from None
        line("SANDHILL_SOURCE", esc(name), esc(top_of(obj, name)), esc(file), first)
        line("SANDHILL_TEXT", esc(text))

    {"topics": topics, "page": page, "source": source}[mode]()

_sandhill_help(__MODE__, __QUERY__, __PACKAGE__, __TOPIC__, __LIMIT__)
del _sandhill_help
"#;

/// The code that does `ask` in a session of `language`.
pub fn code(language: &str, ask: &Ask) -> Option<String> {
    let template = match language.to_ascii_lowercase().as_str() {
        "r" => R,
        "python" | "python3" => PYTHON,
        _ => return None,
    };
    Some(
        template
            .replace("__MODE__", &literal(ask.mode.name()))
            .replace("__QUERY__", &literal(ask.query))
            .replace("__PACKAGE__", &literal(ask.package))
            .replace("__TOPIC__", &literal(ask.topic))
            .replace("__LIMIT__", &ask.limit.to_string()),
    )
}

/// What a search printed: the topics, and how many matched in all.
#[derive(Debug, Default, PartialEq)]
pub struct Topics {
    pub matched: u64,
    pub topics: Vec<HelpTopic>,
}

pub fn parse_topics(stdout: &str) -> Topics {
    let mut parsed = Topics::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_HELP") => {
                parsed.matched = fields
                    .next()
                    .and_then(|n| n.trim().parse().ok())
                    .unwrap_or(0)
            }
            Some("SANDHILL_TOPIC") => parsed.topics.push(topic(&mut fields)),
            _ => {}
        }
    }
    parsed
}

/// What a page printed. R's has HTML; Python's has a kind, a signature,
/// the docstring and members.
#[derive(Debug, Default, PartialEq)]
pub struct Page {
    pub topic: String,
    pub package: String,
    pub title: String,
    pub version: String,
    pub kind: Option<String>,
    pub html: Option<String>,
    pub signature: Option<String>,
    pub text: Option<String>,
    pub members: Vec<HelpMember>,
    pub alternatives: Vec<HelpTopic>,
}

pub fn parse_page(stdout: &str) -> Option<Page> {
    let mut parsed: Option<Page> = None;
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        let tag = fields.next();
        if tag == Some("SANDHILL_PAGE") {
            let mut next = || unescape(fields.next().unwrap_or(""));
            parsed = Some(Page {
                topic: next(),
                package: next(),
                title: next(),
                version: next(),
                kind: Some(next()).filter(|k| !k.is_empty()),
                ..Page::default()
            });
            continue;
        }
        let Some(page) = parsed.as_mut() else {
            continue;
        };
        let rest = || unescape(line.split_once('\t').map_or("", |(_, rest)| rest));
        match tag {
            Some("SANDHILL_HTML") => page.html = Some(body(&rest())),
            Some("SANDHILL_SIGNATURE") => page.signature = Some(rest()),
            Some("SANDHILL_TEXT") => page.text = Some(rest()),
            Some("SANDHILL_ALT") => page.alternatives.push(topic(&mut fields)),
            Some("SANDHILL_MEMBER") => page.members.push(HelpMember {
                name: unescape(fields.next().unwrap_or("")),
                kind: fields.next().unwrap_or("").to_string(),
                title: unescape(fields.next().unwrap_or("")),
            }),
            _ => {}
        }
    }
    parsed
}

/// What asking for the source printed.
#[derive(Debug, Default, PartialEq)]
pub struct Source {
    pub topic: String,
    pub package: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub text: String,
}

pub fn parse_source(stdout: &str) -> Option<Source> {
    let mut parsed: Option<Source> = None;
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_SOURCE") => {
                let topic = unescape(fields.next().unwrap_or(""));
                let package = unescape(fields.next().unwrap_or(""));
                let file = Some(unescape(fields.next().unwrap_or(""))).filter(|f| !f.is_empty());
                let line = fields
                    .next()
                    .and_then(|n| n.trim().parse().ok())
                    .filter(|n| *n > 0);
                parsed = Some(Source {
                    topic,
                    package,
                    file,
                    line,
                    text: String::new(),
                });
            }
            Some("SANDHILL_TEXT") => {
                if let Some(source) = parsed.as_mut() {
                    source.text = unescape(line.split_once('\t').map_or("", |(_, rest)| rest));
                }
            }
            _ => {}
        }
    }
    parsed
}

fn topic<'a>(fields: &mut impl Iterator<Item = &'a str>) -> HelpTopic {
    HelpTopic {
        topic: unescape(fields.next().unwrap_or("")),
        package: unescape(fields.next().unwrap_or("")),
        title: unescape(fields.next().unwrap_or("")),
    }
}

/// A whole HTML page's body, for R versions whose `Rd2HTML` can't leave
/// out the rest; a fragment as it is.
fn body(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let Some(open) = lower.find("<body") else {
        return html.to_string();
    };
    let start = lower[open..].find('>').map_or(open, |end| open + end + 1);
    let end = lower
        .rfind("</body>")
        .filter(|end| *end >= start)
        .unwrap_or(html.len());
    html[start..end].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_go_in_as_literals_for_the_language() {
        let ask = Ask {
            mode: Mode::Topics,
            query: "read\"csv\n",
            package: "",
            topic: "",
            limit: 7,
        };
        let r = code("R", &ask).unwrap();
        assert!(r.contains(r#".query <- "read\"csv\n""#));
        assert!(r.contains(r#".mode <- "topics""#));
        assert!(r.contains(".limit <- 7\n"));
        let py = code("python3", &ask).unwrap();
        assert!(py.contains(r#"_sandhill_help("topics", "read\"csv\n", "", "", 7)"#));
        assert!(code("julia", &ask).is_none());
    }

    #[test]
    fn parses_topics() {
        let out = "noise\nSANDHILL_HELP\t3\nSANDHILL_TOPIC\tfilter\tstats\tLinear Filtering on a Time Series\nSANDHILL_TOPIC\tfilter\tdplyr\tKeep rows that \\tmatch\n";
        let parsed = parse_topics(out);
        assert_eq!(parsed.matched, 3);
        assert_eq!(
            parsed.topics,
            vec![
                HelpTopic {
                    topic: "filter".into(),
                    package: "stats".into(),
                    title: "Linear Filtering on a Time Series".into()
                },
                HelpTopic {
                    topic: "filter".into(),
                    package: "dplyr".into(),
                    title: "Keep rows that \tmatch".into()
                },
            ]
        );
    }

    #[test]
    fn parses_an_r_page() {
        let out = "SANDHILL_PAGE\tfilter\tstats\tLinear Filtering\t4.3.3\nSANDHILL_ALT\tfilter\tdplyr\tKeep rows\nSANDHILL_HTML\t<h3>Description</h3>\\n<p>Applies\\tfilters.</p>\n";
        let page = parse_page(out).unwrap();
        assert_eq!(page.topic, "filter");
        assert_eq!(page.package, "stats");
        assert_eq!(page.version, "4.3.3");
        assert_eq!(page.kind, None);
        assert_eq!(
            page.html.as_deref(),
            Some("<h3>Description</h3>\n<p>Applies\tfilters.</p>")
        );
        assert_eq!(page.alternatives[0].package, "dplyr");
        assert!(parse_page("nothing").is_none());
    }

    #[test]
    fn parses_a_python_page() {
        let out = "SANDHILL_PAGE\tpandas.concat\tpandas\tConcatenate pandas objects.\t3.0.1\tfunction\nSANDHILL_SIGNATURE\t(objs, *, axis=0)\nSANDHILL_TEXT\tConcatenate.\\n\\nParameters\\n----------\nSANDHILL_MEMBER\tDataFrame\tclass\tTwo-dimensional data.\n";
        let page = parse_page(out).unwrap();
        assert_eq!(page.kind.as_deref(), Some("function"));
        assert_eq!(page.signature.as_deref(), Some("(objs, *, axis=0)"));
        assert_eq!(
            page.text.as_deref(),
            Some("Concatenate.\n\nParameters\n----------")
        );
        assert_eq!(
            page.members,
            vec![HelpMember {
                name: "DataFrame".into(),
                kind: "class".into(),
                title: "Two-dimensional data.".into()
            }]
        );
    }

    #[test]
    fn keeps_a_whole_pages_body() {
        assert_eq!(
            body(
                "<html><head><title>x</title></head><BODY class=\"a\">\n<h2>lm</h2>\n</body></html>"
            ),
            "<h2>lm</h2>"
        );
        assert_eq!(body("<h2>lm</h2>"), "<h2>lm</h2>");
    }

    #[test]
    fn parses_source() {
        let source = parse_source(
            "SANDHILL_SOURCE\tnumpy.mean\tnumpy\t/x/fromnumeric.py\t3700\nSANDHILL_TEXT\tdef mean(a):\\n    pass\n",
        )
        .unwrap();
        assert_eq!(source.file.as_deref(), Some("/x/fromnumeric.py"));
        assert_eq!(source.line, Some(3700));
        assert_eq!(source.text, "def mean(a):\n    pass");
        let r =
            parse_source("SANDHILL_SOURCE\tFilter\tbase\t\t0\nSANDHILL_TEXT\tfunction (f, x)\n")
                .unwrap();
        assert_eq!((r.file, r.line), (None, None));
    }
}
