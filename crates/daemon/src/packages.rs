//! The packages installed for a session's language, for the Packages pane:
//! R's libraries (`installed.packages()`), or the Python environment's
//! distributions (`importlib.metadata`), with which are loaded. Listed by
//! running a short piece of code in the session, quietly. Changes are
//! planned and run separately through Getting Ready's install jobs.
//! Updates are asked of the repositories the same way: CRAN and
//! Bioconductor for R, PyPI and conda's channels for Python.

use std::time::Duration;

use sandhill_proto::{Package, PackageUpdate};

use crate::tables::unescape;

const R: &str = r#"local({
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
  .ip <- utils::installed.packages(fields = "Title")
  .ip <- .ip[!duplicated(.ip[, "Package"]), , drop = FALSE]
  .repos <- getOption("repos")
  .cran <- if ("CRAN" %in% names(.repos)) .repos[["CRAN"]] else ""
  .line("SANDHILL_PKGS", as.integer(!is.na(.cran) && nzchar(.cran) && .cran != "@CRAN@"),
    as.integer("BiocManager" %in% .ip[, "Package"]), 0L)
  .line("SANDHILL_PROGRAM", .esc(file.path(R.home("bin"), "R")))
  writeLines(paste("SANDHILL_LIB", .esc(.libPaths()), sep = "\t"))
  .writable <- Filter(function(lib) file.access(lib, 2) == 0, .libPaths())
  if (length(.writable)) writeLines(paste("SANDHILL_WRITABLE", .esc(.writable), sep = "\t"))
  if (nrow(.ip)) {
    .name <- .ip[, "Package"]
    .title <- .ip[, "Title"]
    .title[is.na(.title)] <- ""
    .base <- !is.na(.ip[, "Priority"]) & .ip[, "Priority"] == "base"
    writeLines(paste("SANDHILL_PKG", .name, .ip[, "Version"], .esc(gsub("\\s+", " ", .title)),
      .esc(.ip[, "LibPath"]), as.integer(.name %in% .packages()), as.integer(.name %in% loadedNamespaces()),
      as.integer(.base), "", sep = "\t"))
  }
  invisible(NULL)
})
"#;

const PYTHON: &str = r#"def _sandhill_packages():
    import importlib.metadata as md, inspect, os, re, sys, types
    ip = globals().get("get_ipython", lambda: None)()
    ns = ip.user_ns if ip is not None else globals()

    def esc(s):
        return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")

    def line(*fields):
        print(*fields, sep="\t")

    def norm(name):
        return re.sub(r"[-_.]+", "-", name).lower()

    def owner(v):
        try:
            name = v.__name__ if isinstance(v, types.ModuleType) else getattr(v, "__module__", None)
        except Exception:
            name = None
        return name if isinstance(name, str) else None

    try:
        tops = md.packages_distributions()
    except Exception:
        tops = {}
    provides = {}
    for top, dists in tops.items():
        if not top.startswith("_"):
            for d in dists:
                provides.setdefault(norm(d), set()).add(top)
    # IPython's own names (In, Out, get_ipython) aren't the user's imports.
    hidden = set(getattr(ip, "user_ns_hidden", None) or ())
    used = set()
    for k, v in list(ns.items()):
        if k in hidden or k.startswith("_"):
            continue
        name = owner(v) if isinstance(v, (types.ModuleType, type)) or inspect.isroutine(v) else owner(type(v))
        if name:
            used.add(name.split(".")[0])
    loaded = {name.split(".")[0] for name in list(sys.modules)}
    line("SANDHILL_PKGS", 0, 0, int(os.path.isdir(os.path.join(sys.prefix, "conda-meta"))))
    line("SANDHILL_PROGRAM", esc(sys.executable))
    line("SANDHILL_PREFIX", esc(sys.prefix))
    line("SANDHILL_LIB", esc(sys.prefix))
    seen = set()
    for d in md.distributions():
        try:
            meta = d.metadata
            name = meta["Name"]
            version = d.version or ""
        except Exception:
            continue
        if not name or norm(name) in seen:
            continue
        seen.add(norm(name))
        modules = sorted(provides.get(norm(name), ()))
        try:
            where = str(d.locate_file(""))
            installer = (d.read_text("INSTALLER") or "").strip()
        except Exception:
            where, installer = "", ""
        line("SANDHILL_PKG", esc(name), esc(version), esc(" ".join((meta["Summary"] or "").split())), esc(where),
             int(any(m in used for m in modules)), int(any(m in loaded for m in modules)), 0, esc(",".join(modules)),
             esc(installer))

_sandhill_packages()
del _sandhill_packages
"#;

/// Asking the repositories can take a while: they send their indexes.
pub const UPDATES_TIMEOUT: Duration = Duration::from_secs(180);

/// R packages with newer versions: on CRAN (cloud.r-project.org when no
/// mirror is set) and, with BiocManager, the Bioconductor release that goes
/// with this R.
const R_UPDATES: &str = r#"local({
  .esc <- function(s) {
    s <- gsub("\\", "\\\\", s, fixed = TRUE)
    s <- gsub("\t", "\\t", s, fixed = TRUE)
    s <- gsub("\n", "\\n", s, fixed = TRUE)
    gsub("\r", "\\r", s, fixed = TRUE)
  }
  .unset <- function(r) is.null(r) || is.na(r) || !nzchar(r) || r == "@CRAN@"
  .repos <- getOption("repos")
  if (.unset(.repos["CRAN"])) .repos["CRAN"] <- "https://cloud.r-project.org"
  if (requireNamespace("BiocManager", quietly = TRUE)) {
    .bioc <- tryCatch(suppressMessages(suppressWarnings(BiocManager::repositories())), error = function(e) NULL)
    if (length(.bioc)) {
      .repos <- c(.bioc, .repos[setdiff(names(.repos), names(.bioc))])
      if (.unset(.repos["CRAN"])) .repos["CRAN"] <- "https://cloud.r-project.org"
    }
  }
  # A repository that can't be reached lists nothing, which isn't "up to date".
  .avail <- tryCatch(suppressWarnings(utils::available.packages(repos = .repos)), error = function(e) NULL)
  .reached <- if (length(.avail)) unique(.avail[, "Repository"]) else character(0)
  .missed <- unname(.repos[!utils::contrib.url(.repos) %in% .reached])
  if (!length(.reached)) stop("couldn't reach ", paste(unname(.repos), collapse = " or "), call. = FALSE)
  .old <- suppressWarnings(utils::old.packages(repos = .repos, available = .avail))
  if (length(.old) && nrow(.old)) {
    .from_bioc <- grepl("bioconductor", .old[, "Repository"], ignore.case = TRUE)
    writeLines(paste("SANDHILL_UPDATE", .old[, "Package"], .old[, "Installed"], .old[, "ReposVer"],
      .esc(.old[, "LibPath"]), as.integer(.from_bioc), sep = "\t"))
  }
  writeLines(paste("SANDHILL_REPO", .esc(unname(.repos)), as.integer(unname(.repos) %in% .missed), sep = "\t"))
  invisible(NULL)
})
"#;

/// Python packages with newer versions. In a conda environment, those
/// conda installed are asked of its channels with a dry run of `conda
/// update --all` (so only versions that fit the rest of the environment
/// count); the rest are asked of PyPI, one at a time but in parallel.
/// Packages installed from a path or a URL have no index to ask.
/// `SANDHILL_PYPI_URL` names another index with PyPI's JSON API.
const PYTHON_UPDATES: &str = r#"def _sandhill_updates():
    import importlib.metadata as md, json, os, re, shutil, subprocess, sys, urllib.error, urllib.request
    from concurrent.futures import ThreadPoolExecutor

    def esc(s):
        return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")

    def line(*fields):
        print(*fields, sep="\t")

    def norm(name):
        return re.sub(r"[-_.]+", "-", name).lower()

    def key(v):
        try:
            from packaging.version import Version
            return (1, Version(v))
        except Exception:
            return (0, tuple(int(n) for n in re.findall(r"\d+", v)))

    def newer(a, b):
        try:
            ka, kb = key(a), key(b)
            if ka[0] == kb[0]:
                return ka[1] > kb[1]
        except Exception:
            pass
        return a != b and tuple(int(n) for n in re.findall(r"\d+", a)) > tuple(int(n) for n in re.findall(r"\d+", b))

    dists = {}
    for d in md.distributions():
        try:
            name = d.metadata["Name"]
            installer = (d.read_text("INSTALLER") or "").strip()
            direct = d.read_text("direct_url.json") is not None
            where = str(d.locate_file(""))
        except Exception:
            continue
        if name and norm(name) not in dists:
            dists[norm(name)] = (name, d.version or "", installer, direct, where)
    found, reached, missed = {}, [], []

    if os.path.isdir(os.path.join(sys.prefix, "conda-meta")) and any(i == "conda" for _, _, i, _, _ in dists.values()):
        envs = os.path.dirname(sys.prefix)
        base = os.path.dirname(envs) if os.path.basename(envs) == "envs" else sys.prefix
        tools = [os.environ.get("CONDA_EXE"), os.path.join(base, "bin", "conda"), shutil.which("conda"),
                 os.environ.get("MAMBA_EXE"), os.path.join(base, "bin", "mamba"), shutil.which("mamba")]
        tool = next((t for t in tools if t and os.path.isfile(t)), None)
        data = None
        if tool:
            try:
                done = subprocess.run([tool, "update", "--all", "--dry-run", "--json", "--prefix", sys.prefix],
                                      capture_output=True, text=True, timeout=150)
                data = json.loads(done.stdout or "null")
            except Exception:
                data = None
        if isinstance(data, dict) and not data.get("error"):
            reached.append("conda's channels")
            actions = data.get("actions") or {}
            if isinstance(actions, list):
                actions = actions[0] if actions else {}
            def versions(kind):
                return {p.get("name"): str(p.get("version", "")) for p in actions.get(kind) or [] if isinstance(p, dict)}
            old = versions("UNLINK")
            for name, version in versions("LINK").items():
                dist = dists.get(norm(name or ""))
                if dist and name in old and newer(version, old[name]):
                    found[norm(name)] = (dist[0], dist[1], version, dist[4])
        elif tool:
            missed.append("conda's channels")

    index = os.environ.get("SANDHILL_PYPI_URL") or "https://pypi.org/pypi"
    asked = [d for n, d in dists.items() if d[2] != "conda" and not d[3] and n not in found]

    def latest(dist):
        try:
            with urllib.request.urlopen(index.rstrip("/") + "/" + dist[0] + "/json", timeout=15) as r:
                return dist, str(json.load(r)["info"]["version"]), True
        except urllib.error.HTTPError:
            return dist, None, True
        except Exception:
            return dist, None, False

    if asked:
        with ThreadPoolExecutor(16) as pool:
            answers = list(pool.map(latest, asked))
        if any(ok for _, _, ok in answers):
            reached.append(index)
            for dist, version, _ in answers:
                if version and newer(version, dist[1]):
                    found[norm(dist[0])] = (dist[0], dist[1], version, dist[4])
        else:
            missed.append(index)
    if missed and not reached:
        raise RuntimeError("couldn't reach " + " or ".join(missed))
    for name, installed, available, where in found.values():
        line("SANDHILL_UPDATE", esc(name), esc(installed), esc(available), esc(where), 0)
    for repo in reached + missed:
        line("SANDHILL_REPO", esc(repo), int(repo in missed))

_sandhill_updates()
del _sandhill_updates
"#;

/// The code that asks for updates in a session of `language`.
pub fn updates_code(language: &str) -> Option<String> {
    match language.to_ascii_lowercase().as_str() {
        "r" => Some(R_UPDATES.to_string()),
        "python" | "python3" => Some(PYTHON_UPDATES.to_string()),
        _ => None,
    }
}

/// What asking for updates printed.
#[derive(Debug, Default, PartialEq)]
pub struct Updates {
    pub updates: Vec<PackageUpdate>,
    pub repos: Vec<String>,
    /// The repositories that couldn't be reached.
    pub unreached: Vec<String>,
}

pub fn parse_updates(stdout: &str) -> Updates {
    let mut parsed = Updates::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_UPDATE") => {
                let mut next = || unescape(fields.next().unwrap_or(""));
                let update = PackageUpdate {
                    name: next(),
                    installed: next(),
                    available: next(),
                    library: next(),
                    bioconductor: next() == "1",
                };
                if !update.name.is_empty() {
                    parsed.updates.push(update);
                }
            }
            Some("SANDHILL_REPO") => {
                let repo = unescape(fields.next().unwrap_or(""));
                if fields.next() == Some("1") {
                    parsed.unreached.push(repo.clone());
                }
                parsed.repos.push(repo);
            }
            _ => {}
        }
    }
    parsed
        .updates
        .sort_by_key(|u| (u.name.to_lowercase(), u.library.clone()));
    parsed
}

/// The code that lists the packages in a session of `language`.
pub fn code(language: &str) -> Option<String> {
    match language.to_ascii_lowercase().as_str() {
        "r" => Some(R.to_string()),
        "python" | "python3" => Some(PYTHON.to_string()),
        _ => None,
    }
}

/// What the listing printed.
#[derive(Debug, Default, PartialEq)]
pub struct Listed {
    pub packages: Vec<Package>,
    pub libraries: Vec<String>,
    pub cran: bool,
    pub bioconductor: bool,
    pub conda: bool,
    pub program: String,
    pub prefix: String,
    pub writable_libraries: Vec<String>,
}

pub fn parse(stdout: &str) -> Listed {
    let mut parsed = Listed::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("SANDHILL_PKGS") => {
                parsed.cran = flag(fields.next());
                parsed.bioconductor = flag(fields.next());
                parsed.conda = flag(fields.next());
            }
            Some("SANDHILL_LIB") => parsed.libraries.push(unescape(fields.next().unwrap_or(""))),
            Some("SANDHILL_WRITABLE") => parsed
                .writable_libraries
                .push(unescape(fields.next().unwrap_or(""))),
            Some("SANDHILL_PROGRAM") => parsed.program = unescape(fields.next().unwrap_or("")),
            Some("SANDHILL_PREFIX") => parsed.prefix = unescape(fields.next().unwrap_or("")),
            Some("SANDHILL_PKG") => {
                let name = unescape(fields.next().unwrap_or(""));
                let version = unescape(fields.next().unwrap_or(""));
                let title = unescape(fields.next().unwrap_or(""));
                let library = unescape(fields.next().unwrap_or(""));
                let attached = flag(fields.next());
                let loaded = flag(fields.next());
                let base = flag(fields.next());
                let modules = unescape(fields.next().unwrap_or(""))
                    .split(',')
                    .filter(|m| !m.is_empty())
                    .map(String::from)
                    .collect();
                let installer = unescape(fields.next().unwrap_or(""));
                if !name.is_empty() {
                    parsed.packages.push(Package {
                        name,
                        version,
                        title,
                        library,
                        attached,
                        loaded,
                        base,
                        modules,
                        installer,
                    });
                }
            }
            _ => {}
        }
    }
    parsed
        .packages
        .sort_by_key(|p| (p.name.to_lowercase(), p.name.clone()));
    parsed
}

fn flag(field: Option<&str>) -> bool {
    field == Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_listing() {
        let out = "SANDHILL_PKGS\t1\t0\t0\nSANDHILL_PROGRAM\t/usr/lib/R/bin/R\nSANDHILL_LIB\t/home/me/R\nSANDHILL_WRITABLE\t/home/me/R\nSANDHILL_LIB\t/usr/lib/R/library\nSANDHILL_PKG\tstats\t4.3.3\tThe R Stats Package\t/usr/lib/R/library\t1\t1\t1\t\nSANDHILL_PKG\tSeurat\t5.1.0\tTools for \\tSingle Cell\t/home/me/R\t0\t1\t0\t\nSANDHILL_PKG\tscikit-learn\t1.5\tA set of tools\t/env\t0\t0\t0\tsklearn\tconda\n";
        let listed = parse(out);
        assert!(listed.cran && !listed.bioconductor && !listed.conda);
        assert_eq!(listed.libraries, vec!["/home/me/R", "/usr/lib/R/library"]);
        assert_eq!(listed.writable_libraries, vec!["/home/me/R"]);
        assert_eq!(listed.program, "/usr/lib/R/bin/R");
        assert!(listed.prefix.is_empty());
        let names: Vec<&str> = listed.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["scikit-learn", "Seurat", "stats"]);
        assert_eq!(
            listed.packages[1],
            Package {
                name: "Seurat".into(),
                version: "5.1.0".into(),
                title: "Tools for \tSingle Cell".into(),
                library: "/home/me/R".into(),
                attached: false,
                loaded: true,
                base: false,
                modules: vec![],
                installer: String::new(),
            }
        );
        assert!(listed.packages[2].base && listed.packages[2].attached);
        assert_eq!(listed.packages[0].modules, vec!["sklearn"]);
        assert_eq!(listed.packages[0].installer, "conda");
        assert!(code("R").is_some() && code("python3").is_some() && code("julia").is_none());
    }

    #[test]
    fn parses_updates() {
        let out = "noise\nSANDHILL_UPDATE\tharmony\t1.2.1\t1.2.3\t/home/me/R\t0\nSANDHILL_UPDATE\tDESeq2\t1.44.0\t1.44.1\t/home/me/R\t1\nSANDHILL_REPO\thttps://bioconductor.org/packages/3.19/bioc\t1\nSANDHILL_REPO\thttps://cloud.r-project.org\t0\n";
        let Updates {
            updates,
            repos,
            unreached,
        } = parse_updates(out);
        assert_eq!(unreached, ["https://bioconductor.org/packages/3.19/bioc"]);
        let names: Vec<&str> = updates.iter().map(|u| u.name.as_str()).collect();
        assert_eq!(names, ["DESeq2", "harmony"]);
        assert_eq!(
            updates[1],
            PackageUpdate {
                name: "harmony".into(),
                installed: "1.2.1".into(),
                available: "1.2.3".into(),
                library: "/home/me/R".into(),
                bioconductor: false,
            }
        );
        assert!(updates[0].bioconductor);
        assert_eq!(repos.len(), 2);
        assert!(updates_code("R").is_some() && updates_code("python").is_some());
    }
}
