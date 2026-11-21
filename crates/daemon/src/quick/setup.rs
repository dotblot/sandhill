//! What Explore a Table needs on this computer: R, an R kernel and the R
//! packages its templates use, Claude Code and Codex for its AI helper,
//! the install tools found, and the job that installs an agent, recorded
//! (`record`) so Sandhill can later remove exactly what it installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use sandhill_proto::{
    KernelspecInfo, QUICK_R_MIN, QuickAgent, QuickPackage, QuickR, QuickReadiness, QuickTool,
    RLibrary, RSource,
};

use super::record;
use crate::kernelspecs::{self, Search};
use crate::terminal::Launch;

/// The packages Quick Tools uses: `(name, from Bioconductor, what for)`.
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
    ("DESeq2", true, QuickTool::Differential),
    ("edgeR", true, QuickTool::Differential),
    ("limma", true, QuickTool::Differential),
    ("apeglm", true, QuickTool::Differential),
    ("sva", true, QuickTool::Differential),
    ("RUVSeq", true, QuickTool::Differential),
    ("AnnotationDbi", true, QuickTool::Symbols),
    ("org.Hs.eg.db", true, QuickTool::Symbols),
    ("org.Mm.eg.db", true, QuickTool::Symbols),
];

/// One command for both the Agents page and the install job it starts.
pub(crate) fn agent_install(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "claude" => Some(("native", "curl -fsSL https://claude.ai/install.sh | bash")),
        "codex" if super::install::brew(&Search::from_env()).is_some() => {
            Some(("brew", "brew install --cask codex"))
        }
        "codex" => Some(("npm", "npm install -g @openai/codex")),
        _ => None,
    }
}

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
    let installed_by_sandhill: Vec<_> = installed_by_sandhill
        .into_iter()
        .filter(|item| item.kind != record::AGENT_FOLDER)
        .collect();
    let prefs = super::install::prefs(record_path);
    let own = super::own::readiness(search, record_path, Path::new(&prefs.own_folder));
    let scoped = search.clone().in_own(
        (prefs.mode == Some(sandhill_proto::EnvMode::Own))
            .then(|| PathBuf::from(&prefs.own_folder)),
    );
    let search = &scoped;
    let developer_tools = super::own::developer_tools(search);
    let own_r = own.as_ref().and_then(|env| env.r.as_ref());
    let kernel = if let Some(folder) = &search.own {
        own_r.filter(|r| !r.kernel.is_empty()).map(|r| {
            (
                r.kernel.clone(),
                crate::own_env::prefix(folder, "r").join("bin/R"),
            )
        })
    } else {
        r_kernels(search).into_iter().next()
    };
    let r = match &kernel {
        Some((_, program)) => Some(program.clone()),
        None if search.own.is_some() => own_r.map(|r| Path::new(&r.prefix).join("bin/R")),
        None => newest_r(search),
    };
    let r = r.map(|path| {
        let version = if search.own.is_some() {
            own_r.map(|r| r.version.clone())
        } else {
            crate::irkernel::r_version(&path)
        }
        .unwrap_or_default();
        let listed = installed(&path, search.own.as_deref());
        (
            QuickR {
                new_enough: new_enough(&version),
                source: source(&path),
                path: path.to_string_lossy().into_owned(),
                version,
                binaries: listed.binaries,
                can_build: developer_tools,
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
    let agents = ["claude", "codex"]
        .into_iter()
        .filter_map(|kind| {
            let (how, _) = agent_install(kind)?;
            let info = agents.iter().find(|a| a.kind == kind)?;
            Some(QuickAgent {
                kind: kind.into(),
                name: info.name.clone(),
                path: info.path.clone(),
                version: info.version.clone(),
                install: info.install.clone(),
                installable: how != "npm" || npm,
                by_sandhill: installed_by_sandhill
                    .iter()
                    .any(|i| i.kind == record::AGENT && i.name == kind && i.ours),
            })
        })
        .collect();
    let python = super::python::readiness(search, &super::sources::chosen(record_path));
    let (tools, brew, brew_version) = super::install::readiness_tools(search, python.tool.as_ref());
    QuickReadiness {
        r,
        kernel: kernel.map(|(name, _)| name),
        packages,
        agents,
        installed: installed_by_sandhill,
        python,
        tools,
        brew,
        brew_version,
        prefs,
        own,
        developer_tools,
        interrupted: super::interrupted::list(record_path),
    }
}

/// Where an R came from, by where it is installed.
pub(super) fn source(r: &Path) -> RSource {
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

pub(super) fn version_key(version: &Option<String>) -> Vec<u32> {
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
fn installed(r: &Path, own: Option<&Path>) -> Listed {
    let names = PACKAGES
        .iter()
        .map(|(n, ..)| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let code = format!(
        "cat('SANDHILL_PKGTYPE\\t', .Platform$pkgType, '\\n', sep = ''); {libraries}; for (p in c({names})) {{ d <- tryCatch(find.package(p), error = function(e) NA); if (!is.na(d)) cat('SANDHILL_PKG', p, as.character(utils::packageVersion(p, lib.loc = dirname(d))), dirname(d), sep = '\\t'); if (!is.na(d)) cat('\\n') }}",
        libraries = crate::irkernel::LIBRARIES_CODE,
    );
    let Some(out) =
        super::install::probe_in(r, &["--no-save", "--no-restore", "-s", "-e", &code], own)
    else {
        return Listed::default();
    };
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
pub(super) fn can_build() -> bool {
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

/// The job that installs Claude Code or Codex with its own command, its
/// output also in `log`, and records it.
pub fn install_agent(
    exe: &Path,
    kind: &str,
    reviewed: Option<&str>,
    log: &Path,
    record_path: &Path,
) -> Result<Launch> {
    let Some((how, command)) = agent_install(kind) else {
        bail!("{kind} isn't an agent Sandhill installs");
    };
    let name = if kind == "claude" {
        "Claude Code"
    } else {
        "Codex"
    };
    if reviewed.is_some_and(|reviewed| reviewed != command) {
        bail!(
            "The way to install {name} changed since you looked (now: {command}). Check it and try again."
        );
    }
    if crate::agents::find(kind).is_some() {
        bail!("it is already installed");
    }
    if how == "npm" && crate::agents::find("npm").is_none() {
        bail!("Codex needs Node.js, which isn't installed");
    }
    let job = super::install::Job::Agent {
        command: command.into(),
        name: kind.into(),
        method: how.into(),
    };
    super::install::launch(exe, record_path, job, log, &format!("Install {name}"))
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
    fn only_supported_agents_can_be_installed() {
        let record = Path::new("/tmp/record.tsv");
        let err = install_agent(
            Path::new("/sandhilld"),
            "pi",
            None,
            Path::new("/tmp/x.log"),
            record,
        )
        .unwrap_err();
        assert!(err.to_string().contains("isn't an agent Sandhill installs"));
    }

    #[test]
    fn agent_install_requires_review_when_the_command_changed() {
        let err = install_agent(
            Path::new("/sandhilld"),
            "claude",
            Some("old command"),
            Path::new("/not-run/install.log"),
            Path::new("/not-run/record.tsv"),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "The way to install Claude Code changed since you looked (now: curl -fsSL https://claude.ai/install.sh | bash). Check it and try again."
        );
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
