//! What Explore a Table needs on this computer: R, an R kernel and the R
//! packages its templates use, Claude Code and Codex for its AI helper, and
//! the jobs that install what is missing, each recorded (`record`) so
//! Sandhill can later remove exactly what it installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    JobSpec, KernelspecInfo, QUICK_R_MIN, QuickAgent, QuickPackage, QuickR, QuickReadiness,
    QuickTool, RLibrary, RSource,
};

use super::record;
use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search};
use crate::terminal::Launch;

/// How long reading the installed packages may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const CRAN: &str = "https://cloud.r-project.org";

/// The packages Quick Tools uses: `(name, from Bioconductor, what for)`.
/// Phase 1 is Quick Plot; Quick Analysis adds its packages when it comes.
/// The organism packages for gene symbols are large and installed only
/// when wanted (Huan, 2026-11-19; `docs/design/gene-symbols-plan.md`).
pub const PACKAGES: &[(&str, bool, QuickTool)] = &[
    ("jsonlite", false, QuickTool::Plot),
    ("ggplot2", false, QuickTool::Plot),
    ("ggrepel", false, QuickTool::Plot),
    ("pheatmap", false, QuickTool::Plot),
    ("readxl", false, QuickTool::Plot),
    ("writexl", false, QuickTool::Plot),
    ("svglite", false, QuickTool::Plot),
    ("AnnotationDbi", true, QuickTool::Symbols),
    ("org.Hs.eg.db", true, QuickTool::Symbols),
    ("org.Mm.eg.db", true, QuickTool::Symbols),
];

/// The agents Getting Ready offers: `(kind, how Sandhill installs it, the
/// command)`.
const AGENTS: &[(&str, &str, &str)] = &[
    (
        "claude",
        "native",
        "curl -fsSL https://claude.ai/install.sh | bash",
    ),
    ("codex", "npm", "npm install -g @openai/codex"),
];

/// What is there: the R an R kernel runs (IRkernel before Ark, the newest
/// first), else the newest R found, its packages, the agents (from
/// `ListAgents`' descriptions) and what Sandhill's `record` says it
/// installed.
pub fn readiness(
    search: &Search,
    record_path: &Path,
    agents: Vec<sandhill_proto::AgentInfo>,
) -> QuickReadiness {
    let installed_by_sandhill = record::read(record_path);
    let kernel = r_kernels(search).into_iter().next();
    let r = match &kernel {
        Some((_, program)) => Some(program.clone()),
        None => newest_r(search),
    };
    let r = r.map(|path| {
        let version = crate::irkernel::r_version(&path).unwrap_or_default();
        let listed = installed(&path);
        (
            QuickR {
                new_enough: new_enough(&version),
                source: source(&path),
                path: path.to_string_lossy().into_owned(),
                version,
                binaries: listed.binaries,
                can_build: can_build(),
                libraries: listed.libraries,
            },
            listed.packages,
        )
    });
    let installed = r.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    let r = r.map(|(r, _)| r);
    let packages = PACKAGES
        .iter()
        .map(|&(name, bioconductor, tool)| {
            let found = installed.iter().find(|(n, ..)| n == name);
            let version = found.map(|(_, v, _)| v.clone());
            let library = found.map(|(.., l)| l.clone());
            QuickPackage {
                name: name.to_string(),
                bioconductor,
                tool,
                by_sandhill: installed_by_sandhill.iter().any(|i| {
                    i.kind == record::R_PACKAGE
                        && i.ours
                        && i.name == name
                        && Some(&i.version) == version.as_ref()
                        && Some(&i.place) == library.as_ref()
                }),
                version,
                library,
            }
        })
        .collect();
    let npm = crate::agents::find("npm").is_some();
    let agents = AGENTS
        .iter()
        .filter_map(|&(kind, how, install)| {
            let info = agents.iter().find(|a| a.kind == kind)?;
            Some(QuickAgent {
                kind: kind.into(),
                name: info.name.clone(),
                path: info.path.clone(),
                version: info.version.clone(),
                install: install.into(),
                installable: how != "npm" || npm,
                by_sandhill: installed_by_sandhill
                    .iter()
                    .any(|i| i.kind == record::AGENT && i.name == kind && i.ours),
            })
        })
        .collect();
    QuickReadiness {
        r,
        kernel: kernel.map(|(name, _)| name),
        packages,
        agents,
        installed: installed_by_sandhill,
        python: super::python::readiness(search),
    }
}

/// Where an R came from, by where it is installed.
fn source(r: &Path) -> RSource {
    let real = std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf());
    let text = real.to_string_lossy();
    if text.starts_with("/Library/Frameworks/R.framework") {
        RSource::Cran
    } else if text.starts_with("/opt/homebrew/")
        || text.starts_with("/usr/local/Cellar/")
        || text.starts_with("/home/linuxbrew/")
    {
        RSource::Homebrew
    } else if real.ancestors().any(|dir| dir.join("conda-meta").is_dir()) {
        RSource::Conda
    } else {
        RSource::Other
    }
}

/// `4.4.0` and later.
fn new_enough(version: &str) -> bool {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    (major, minor) >= QUICK_R_MIN
}

/// The R kernels on the machine with the R each runs: IRkernel before Ark,
/// newer R first.
fn r_kernels(search: &Search) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(KernelspecInfo, PathBuf)> = kernelspecs::list(search)
        .into_iter()
        .filter(|info| info.language.eq_ignore_ascii_case("r"))
        .filter_map(|info| {
            let dir = kernelspecs::find(search, &info.name)?;
            let program = crate::irkernel::spec_r(&dir.kernelspec, search)?;
            Some((info, program))
        })
        .collect();
    found.sort_by(|(a, _), (b, _)| {
        let irkernel = |i: &KernelspecInfo| i.engine.as_deref() == Some("IRkernel");
        irkernel(b)
            .cmp(&irkernel(a))
            .then_with(|| version_key(&b.version).cmp(&version_key(&a.version)))
    });
    found
        .into_iter()
        .map(|(info, program)| (info.name, program))
        .collect()
}

fn version_key(version: &Option<String>) -> Vec<u32> {
    version
        .as_deref()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// The newest R installed, with or without IRkernel.
fn newest_r(search: &Search) -> Option<PathBuf> {
    crate::irkernel::find(search)
        .into_iter()
        .filter(|c| c.how == crate::irkernel::How::Plain)
        .max_by_key(|c| version_key(&Some(c.version.clone())))
        .map(|c| c.r)
}

/// What R said about its packages.
#[derive(Debug, Default, PartialEq)]
struct Listed {
    /// The packages of [`PACKAGES`] it has: name, version, library.
    packages: Vec<(String, String, String)>,
    /// It installs ready-built packages (`.Platform$pkgType` isn't
    /// `source`).
    binaries: bool,
    /// R's libraries, in R's order.
    libraries: Vec<RLibrary>,
}

/// The packages of [`PACKAGES`] the R at `r` has and where, read from their
/// descriptions without loading them, whether it takes ready-built
/// packages, and its libraries. R reads the user's `.Renviron` and
/// `.Rprofile`, as the install job and the kernel do, so libraries set
/// there count.
fn installed(r: &Path) -> Listed {
    let names = PACKAGES
        .iter()
        .map(|(n, ..)| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(
        "cat('SANDHILL_PKGTYPE\\t', .Platform$pkgType, '\\n', sep = ''); {libraries}; for (p in c({names})) {{ d <- tryCatch(find.package(p), error = function(e) NA); if (!is.na(d)) cat('SANDHILL_PKG', p, as.character(utils::packageVersion(p, lib.loc = dirname(d))), dirname(d), sep = '\\t'); if (!is.na(d)) cat('\\n') }}",
        libraries = crate::irkernel::LIBRARIES_CODE,
    );
    let Ok(mut child) = Command::new(r)
        .args(["--no-save", "--no-restore", "-s", "-e"])
        .arg(&code)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Listed::default();
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < LIST_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Listed::default();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut stdout, &mut out);
    }
    parse_installed(&out)
}

fn parse_installed(out: &str) -> Listed {
    let mut listed = Listed {
        libraries: crate::irkernel::parse_libraries(out),
        ..Listed::default()
    };
    for line in out.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields[..] {
            ["SANDHILL_PKGTYPE", kind, ..] => {
                listed.binaries = !matches!(kind.trim(), "" | "source")
            }
            ["SANDHILL_PKG", name, version, library, ..] => listed.packages.push((
                name.to_string(),
                version.to_string(),
                library.trim().to_string(),
            )),
            _ => {}
        }
    }
    listed
}

/// Whether packages can be built from source here: Apple's developer tools
/// on a Mac (`xcode-select -p` names them), a C compiler elsewhere.
fn can_build() -> bool {
    if cfg!(target_os = "macos") {
        Command::new("xcode-select")
            .arg("-p")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    } else {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path)
                .any(|dir| dir.join("cc").is_file() || dir.join("gcc").is_file())
        })
    }
}

/// A string as R writes one.
fn r_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn r_vector(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| r_string(n)).collect();
    format!("c({})", quoted.join(", "))
}

/// The R code that installs `cran` and `bioconductor` packages into
/// `library`, the one of R's libraries the user chose (never created
/// here), never updating what is there: ready-built packages when this R
/// takes them (never compiling then), else built from source, as
/// Homebrew's and conda's R need. What it adds goes in `record`.
fn install_code(cran: &[&str], bioconductor: &[&str], library: &str, record_path: &Path) -> String {
    format!(
        r#"local({{
  {append}
  binary <- .Platform$pkgType != "source"
  type <- if (binary) "binary" else "source"
  if (binary) options(install.packages.compile.from.source = "never")
  else message("This R builds packages from their source code on this computer. It can take a few minutes.")
  repos <- c(CRAN = {mirror})
  lib <- {library}
  {check_library}
  before <- rownames(utils::installed.packages(lib.loc = lib))
  on.exit(sandhill_record(lib, before), add = TRUE)
  cran <- {cran}
  bioc <- {bioc}
  if (length(cran)) {{
    message("Installing ", paste(cran, collapse = ", "), " into ", lib)
    utils::install.packages(cran, lib = lib, repos = repos, type = type, Ncpus = max(1L, parallel::detectCores() - 1L))
  }}
  if (length(bioc)) {{
    if (!requireNamespace("BiocManager", quietly = TRUE)) utils::install.packages("BiocManager", lib = lib, repos = repos, type = type)
    message("Installing ", paste(bioc, collapse = ", "), " from Bioconductor into ", lib)
    BiocManager::install(bioc, lib = lib, ask = FALSE, update = FALSE, type = type)
  }}
  wanted <- c(cran, bioc)
  missing <- wanted[!vapply(wanted, requireNamespace, logical(1), quietly = TRUE)]
  if (length(missing)) stop("These packages couldn't be installed: ", paste(missing, collapse = ", "), if (binary) ". A ready-built copy may not exist for this R yet." else ". Building them failed; the messages above say why.", call. = FALSE)
  message("Done: every package for exploring tables is installed.")
}})"#,
        append = record::r_append(record_path),
        mirror = r_string(CRAN),
        library = r_string(library),
        check_library = crate::irkernel::check_library_code(),
        cran = r_vector(cran),
        bioc = r_vector(bioconductor),
    )
}

/// The job that installs `packages` (names from [`PACKAGES`]) into
/// `library` of the R at `r`, with its output also written to `log`.
pub fn install(
    r: &str,
    packages: &[String],
    library: &str,
    log: &Path,
    record_path: &Path,
) -> Result<Launch> {
    let r = PathBuf::from(r);
    if !r.is_file() {
        bail!("{} isn't an R program", r.display());
    }
    if !Path::new(library).is_dir() {
        bail!("the library {library} doesn't exist; Sandhill doesn't create libraries");
    }
    let mut cran = Vec::new();
    let mut bioconductor = Vec::new();
    for name in packages {
        let Some(&(known, bioc, _)) = PACKAGES.iter().find(|(n, ..)| n == name) else {
            bail!("{name} isn't one of the packages for exploring tables");
        };
        if bioc {
            bioconductor.push(known);
        } else {
            cran.push(known);
        }
    }
    if cran.is_empty() && bioconductor.is_empty() {
        bail!("no packages to install");
    }
    let rscript = r
        .parent()
        .map(|bin| bin.join("Rscript"))
        .filter(|p| p.is_file())
        .with_context(|| format!("there is no Rscript beside {}", r.display()))?;
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    let job = JobSpec {
        path: None,
        command: Some(format!(
            "set -o pipefail; {} -e {} 2>&1 | tee {}",
            shell_quote(&rscript.to_string_lossy()),
            shell_quote(&install_code(&cran, &bioconductor, library, record_path)),
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&job, Some(&home), None)?;
    launch.title = "Install packages for exploring tables".into();
    Ok(launch)
}

/// The job that installs Claude Code or Codex with its own command, its
/// output also in `log`, and records it.
pub fn install_agent(kind: &str, log: &Path, record_path: &Path) -> Result<Launch> {
    let Some(&(kind, how, command)) = AGENTS.iter().find(|(k, ..)| *k == kind) else {
        bail!("{kind} isn't an agent Sandhill installs");
    };
    if crate::agents::find(if kind == "claude" { "claude" } else { "codex" }).is_some() {
        bail!("it is already installed");
    }
    if how == "npm" && crate::agents::find("npm").is_none() {
        bail!("Codex needs Node.js, which isn't installed");
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    let job = JobSpec {
        path: None,
        command: Some(format!(
            "set -o pipefail; ( {command} && {} ) 2>&1 | tee {}",
            record::sh_append_agent(record_path, kind, how),
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&job, Some(&home), None)?;
    launch.title = format!(
        "Install {}",
        if kind == "claude" {
            "Claude Code"
        } else {
            "Codex"
        }
    );
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_versions_from_4_4_are_new_enough() {
        assert!(new_enough("4.4.0"));
        assert!(new_enough("4.6.2"));
        assert!(new_enough("5.0.1"));
        assert!(!new_enough("4.3.3"));
        assert!(!new_enough(""));
    }

    #[test]
    fn packages_libraries_and_package_type_come_from_the_marked_lines() {
        let out = "Loading profile\nSANDHILL_PKGTYPE\tmac.binary.big-sur-arm64\nSANDHILL_LIBPATH\t/opt/homebrew/lib/R/4.6/site-library\tyes\t741\nSANDHILL_LIBPATH\t/opt/homebrew/Cellar/r/4.6.2/lib/R/library\tno\t30\nSANDHILL_PKG\tggplot2\t4.0.3\t/opt/homebrew/lib/R/4.6/site-library\nSANDHILL_PKG\treadxl\t1.4.5\t/Users/a/Library/R/arm64/4.6/library\n";
        assert_eq!(
            parse_installed(out),
            Listed {
                packages: vec![
                    (
                        "ggplot2".to_string(),
                        "4.0.3".to_string(),
                        "/opt/homebrew/lib/R/4.6/site-library".to_string()
                    ),
                    (
                        "readxl".to_string(),
                        "1.4.5".to_string(),
                        "/Users/a/Library/R/arm64/4.6/library".to_string()
                    )
                ],
                binaries: true,
                libraries: vec![
                    RLibrary {
                        path: "/opt/homebrew/lib/R/4.6/site-library".into(),
                        writable: true,
                        packages: 741,
                    },
                    RLibrary {
                        path: "/opt/homebrew/Cellar/r/4.6.2/lib/R/library".into(),
                        writable: false,
                        packages: 30,
                    },
                ],
            }
        );
        // Homebrew's R builds every package from source.
        assert!(!parse_installed("SANDHILL_PKGTYPE\tsource\n").binaries);
    }

    #[test]
    fn only_quick_tools_packages_and_agents_can_be_installed() {
        let record = Path::new("/tmp/record.tsv");
        let err = install(
            "/bin/sh",
            &["Seurat".to_string()],
            "/tmp",
            Path::new("/tmp/x.log"),
            record,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("isn't one of the packages for exploring tables")
        );
        let err = install(
            "/bin/sh",
            &["ggplot2".to_string()],
            "/nonexistent/sandhill-library",
            Path::new("/tmp/x.log"),
            record,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Sandhill doesn't create libraries")
        );
        let err = install_agent("pi", Path::new("/tmp/x.log"), record).unwrap_err();
        assert!(err.to_string().contains("isn't an agent Sandhill installs"));
    }

    #[test]
    fn install_code_records_never_compiles_ready_built_packages_or_updates() {
        let code = install_code(
            &["ggplot2"],
            &["DESeq2"],
            "/opt/homebrew/lib/R/4.6/site-library",
            Path::new("/tmp/record.tsv"),
        );
        assert!(
            code.contains("if (binary) options(install.packages.compile.from.source = \"never\")")
        );
        assert!(code.contains("type <- if (binary) \"binary\" else \"source\""));
        assert!(code.contains("update = FALSE"));
        assert!(code.contains("lib <- \"/opt/homebrew/lib/R/4.6/site-library\""));
        assert!(!code.contains("dir.create(lib"));
        assert!(code.contains("on.exit(sandhill_record(lib, before)"));
        assert!(code.contains("/tmp/record.tsv"));
    }

    #[test]
    fn r_sources_are_read_from_where_r_is() {
        assert_eq!(
            source(Path::new("/Library/Frameworks/R.framework/Resources/bin/R")),
            RSource::Cran
        );
        assert_eq!(source(Path::new("/opt/homebrew/bin/R")), RSource::Homebrew);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("conda-meta")).unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        assert_eq!(source(&dir.path().join("bin/R")), RSource::Conda);
        assert_eq!(source(Path::new("/usr/bin/R")), RSource::Other);
    }
}
