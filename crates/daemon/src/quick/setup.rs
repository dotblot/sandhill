//! What Quick Tools needs on this computer: R, an R kernel and the R
//! packages its templates use, and the job that installs missing packages
//! into the user's own R library, from binaries only.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    JobSpec, KernelspecInfo, QUICK_R_MIN, QuickPackage, QuickR, QuickReadiness, QuickTool,
};

use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search};
use crate::terminal::Launch;

/// How long reading the installed packages may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const CRAN: &str = "https://cloud.r-project.org";

/// The packages Quick Tools uses: `(name, from Bioconductor, what for)`.
/// Phase 1 is Quick Plot; Quick Analysis adds its packages when it comes.
pub const PACKAGES: &[(&str, bool, QuickTool)] = &[
    ("jsonlite", false, QuickTool::Plot),
    ("ggplot2", false, QuickTool::Plot),
    ("ggrepel", false, QuickTool::Plot),
    ("pheatmap", false, QuickTool::Plot),
    ("readxl", false, QuickTool::Plot),
    ("writexl", false, QuickTool::Plot),
    ("svglite", false, QuickTool::Plot),
];

/// What is there: the R an R kernel runs (IRkernel before Ark, the newest
/// first), else the newest R found, and which packages it has.
pub fn readiness(search: &Search) -> QuickReadiness {
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
                path: path.to_string_lossy().into_owned(),
                version,
                binaries: listed.binaries,
                can_build: can_build(),
            },
            listed.packages,
        )
    });
    let installed = r.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    let r = r.map(|(r, _)| r);
    let packages = PACKAGES
        .iter()
        .map(|&(name, bioconductor, tool)| QuickPackage {
            name: name.to_string(),
            bioconductor,
            tool,
            version: installed
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone()),
        })
        .collect();
    QuickReadiness {
        r,
        kernel: kernel.map(|(name, _)| name),
        packages,
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
    /// The packages of [`PACKAGES`] it has, with their versions.
    packages: Vec<(String, String)>,
    /// It installs ready-built packages (`.Platform$pkgType` isn't
    /// `source`).
    binaries: bool,
}

/// The packages of [`PACKAGES`] the R at `r` has, read from their
/// descriptions without loading them, and whether it takes ready-built
/// packages.
fn installed(r: &Path) -> Listed {
    let names = PACKAGES
        .iter()
        .map(|(n, ..)| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(
        "cat('SANDHILL_PKGTYPE', .Platform$pkgType, '\\n'); for (p in c({names})) {{ v <- tryCatch(as.character(utils::packageVersion(p)), error = function(e) NA); if (!is.na(v)) cat('SANDHILL_PKG', p, v, '\\n') }}"
    );
    let Ok(mut child) = Command::new(r)
        .args(["--no-save", "--no-restore", "--no-init-file", "-s", "-e"])
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
    let mut listed = Listed::default();
    for line in out.lines() {
        if let Some(kind) = line.strip_prefix("SANDHILL_PKGTYPE ") {
            listed.binaries = !matches!(kind.trim(), "" | "source");
        } else if let Some(rest) = line.strip_prefix("SANDHILL_PKG ") {
            let mut words = rest.split_whitespace();
            if let (Some(name), Some(version)) = (words.next(), words.next()) {
                listed
                    .packages
                    .push((name.to_string(), version.to_string()));
            }
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

/// The R code that installs `cran` and `bioconductor` packages into the
/// user's own library, never updating what is there: ready-built packages
/// when this R takes them (never compiling then), else built from source,
/// as Homebrew's and conda's R need.
fn install_code(cran: &[&str], bioconductor: &[&str]) -> String {
    format!(
        r#"local({{
  binary <- .Platform$pkgType != "source"
  type <- if (binary) "binary" else "source"
  if (binary) options(install.packages.compile.from.source = "never")
  else message("This R builds packages from their source code on this computer. It can take a few minutes.")
  repos <- c(CRAN = {mirror})
  lib <- Sys.getenv("R_LIBS_USER")
  dir.create(lib, recursive = TRUE, showWarnings = FALSE)
  .libPaths(c(lib, .libPaths()))
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
        mirror = r_string(CRAN),
        cran = r_vector(cran),
        bioc = r_vector(bioconductor),
    )
}

/// The job that installs `packages` (names from [`PACKAGES`]) into the R at
/// `r`, with its output also written to `log`.
pub fn install(r: &str, packages: &[String], log: &Path) -> Result<Launch> {
    let r = PathBuf::from(r);
    if !r.is_file() {
        bail!("{} isn't an R program", r.display());
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
            shell_quote(&install_code(&cran, &bioconductor)),
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&job, Some(&home), None)?;
    launch.title = "Install packages for exploring tables".into();
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
    fn installed_packages_and_package_type_come_from_the_marked_lines() {
        let out = "Loading profile\nSANDHILL_PKGTYPE mac.binary.big-sur-arm64 \nSANDHILL_PKG ggplot2 4.0.3 \nSANDHILL_PKG readxl 1.4.5 \n";
        assert_eq!(
            parse_installed(out),
            Listed {
                packages: vec![
                    ("ggplot2".to_string(), "4.0.3".to_string()),
                    ("readxl".to_string(), "1.4.5".to_string())
                ],
                binaries: true,
            }
        );
        // Homebrew's R builds every package from source.
        assert!(!parse_installed("SANDHILL_PKGTYPE source \n").binaries);
    }

    #[test]
    fn only_quick_tools_packages_can_be_installed() {
        let err = install("/bin/sh", &["Seurat".to_string()], Path::new("/tmp/x.log")).unwrap_err();
        assert!(
            err.to_string()
                .contains("isn't one of the packages for exploring tables")
        );
    }

    #[test]
    fn install_code_never_compiles_ready_built_packages_or_updates() {
        let code = install_code(&["ggplot2"], &["DESeq2"]);
        assert!(
            code.contains("if (binary) options(install.packages.compile.from.source = \"never\")")
        );
        assert!(code.contains("type <- if (binary) \"binary\" else \"source\""));
        assert!(code.contains("update = FALSE"));
        assert!(code.contains("R_LIBS_USER"));
    }
}
