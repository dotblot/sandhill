//! Plan a package change without touching its target, then run that command
//! and record the before/after difference.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    InstallTarget, InstallTool, PackageAction, PackageChange, PackagePlan, PlannedPackage,
};
use serde_json::Value;

use super::install::{self, executable, info, probe, quote};
use super::{record, setup};
use crate::kernelspecs::Search;

const CRAN: &str = "https://cloud.r-project.org";
const CHANNELS: &[&str] = &["-c", "conda-forge", "-c", "bioconda", "--override-channels"];

pub(crate) fn plan(search: &Search, change: PackageChange) -> PackagePlan {
    let mut plan = PackagePlan {
        change,
        tools: Vec::new(),
        folder: String::new(),
        packages: Vec::new(),
        command: String::new(),
        notes: Vec::new(),
        refused: None,
    };
    match build(search, &mut plan) {
        Ok(command) => plan.command = command,
        Err(error) => plan.refused = Some(error.to_string()),
    }
    plan
}

fn build(search: &Search, plan: &mut PackagePlan) -> Result<String> {
    use InstallTool::{Conda, Pip, R, Uv};
    let change = plan.change.clone();
    let PackageChange {
        action,
        target,
        packages,
        tool,
        ..
    } = &change;
    let program;
    let prefix: Option<std::path::PathBuf>;
    let mut python_probe = None;
    match target {
        InstallTarget::R { r, library } => {
            program = Path::new(r);
            plan.folder.clone_from(library);
            if !executable(program) {
                bail!("R wasn't found at {r}.");
            }
            // R.home("bin") may be <prefix>/lib/R/bin, not <prefix>/bin.
            let canonical = fs::canonicalize(program).ok();
            prefix = program
                .ancestors()
                .chain(canonical.iter().flat_map(|p| p.ancestors()))
                .find(|p| p.join("conda-meta").is_dir())
                .map(Path::to_path_buf);
            if let Some(conda) = prefix.as_deref().and_then(|p| install::conda(search, p)) {
                plan.tools.push(info(Conda, &conda));
            }
            plan.tools.push(info(R, program));
        }
        InstallTarget::Python {
            python,
            prefix: folder,
        } => {
            program = Path::new(python);
            let root = Path::new(folder);
            plan.folder.clone_from(folder);
            if !executable(program) || !root.is_absolute() || !root.is_dir() {
                bail!("This Python or its environment folder couldn't be found.");
            }
            let actual = inspect_python(program)
                .context("This Python couldn't tell Sandhill where its packages go.")?;
            if !same_folder(Path::new(&actual.prefix), root) {
                bail!("This Python doesn't use the folder {folder}. Choose its own environment.");
            }
            prefix = Some(root.to_path_buf());
            if root.join("conda-meta").is_dir() {
                if let Some(conda) = install::conda(search, root) {
                    plan.tools.push(info(Conda, &conda));
                }
            } else if root.join("pyvenv.cfg").is_file() {
                if let Some(uv) = install::uv(search) {
                    plan.tools.push(info(Uv, &uv));
                }
            } else if actual.managed {
                bail!(
                    "This Python is managed by Homebrew or the system and refuses pip. Make an environment for it in Getting Ready first."
                );
            }
            let pip = info(Pip, program);
            if !pip.version.is_empty() {
                plan.tools.push(pip);
            }
            python_probe = Some(actual);
        }
    }
    let selected = match *tool {
        Some(choice) => plan.tools.iter().find(|info| info.tool == choice)
            .with_context(|| format!("{} isn't available for this folder. Choose another tool.", choice.label()))?,
        None => plan.tools.first().context("No tool can change packages in this Python. Make an environment in Getting Ready first.")?,
    }
    .clone();
    plan.change.tool = Some(selected.tool);
    if packages.is_empty() {
        bail!("Enter at least one package name.");
    }
    let is_r = matches!(target, InstallTarget::R { .. });
    let requested: Vec<_> = packages
        .iter()
        .map(|spec| {
            let name = if is_r {
                if record::r_name(spec) || (*action != PackageAction::Remove && github_repo(spec)) {
                    Some(spec.as_str())
                } else {
                    None
                }
            } else {
                python_spec(spec).filter(|name| *action != PackageAction::Remove || *name == spec)
            };
            name.with_context(|| {
                format!("{spec:?} isn't a package name or version that Sandhill can use.")
            })
        })
        .collect::<Result<_>>()?;
    let tool_path = Path::new(&selected.path);
    let command = match selected.tool {
        R => {
            let library = Path::new(&plan.folder);
            if !library.is_absolute() || !library.is_dir() {
                bail!(
                    "The library {} doesn't exist. Sandhill doesn't create libraries. Choose another one.",
                    library.display()
                );
            }
            if !writable(library) {
                bail!(
                    "You can't write to the library {}. Choose another one.",
                    library.display()
                );
            }
            let rscript = program.with_file_name("Rscript");
            if !executable(&rscript) {
                bail!("There is no Rscript beside {}.", program.display());
            }
            let r = inspect_r(&rscript, library)
                .context("R couldn't open this library. Choose another R or library.")?;
            if *action == PackageAction::Remove {
                guard_r_remove(library, &r.standard, &requested)?;
            }
            let use_bioc = r.bioc || requested.iter().any(|name| bioconductor(name));
            for &name in &requested {
                let source = if github_repo(name) {
                    "GitHub"
                } else if bioconductor(name) {
                    "Bioconductor"
                } else if use_bioc && !setup::PACKAGES.iter().any(|(n, ..)| n == &name) {
                    "CRAN or Bioconductor"
                } else {
                    "CRAN"
                };
                let version = (*action == PackageAction::Remove)
                    .then(|| record::package_version(library, name))
                    .flatten();
                plan.packages.push(package(name, name, source, version));
            }
            if *action != PackageAction::Remove && !r.binary {
                plan.notes.push(
                    "This R builds packages from their source code. That can take a few minutes."
                        .into(),
                );
                if !setup::can_build() {
                    plan.notes.push(
                        "Packages that need to be built may fail without developer tools.".into(),
                    );
                    if known_to_need_compiler(&rscript, &requested, use_bioc) {
                        bail!(
                            "None of these packages can be built here without developer tools. Install those tools or choose another R."
                        );
                    }
                }
            }
            quote([
                rscript.to_string_lossy().as_ref(),
                "-e",
                &r_code(*action, library, &requested, use_bioc),
            ])
        }
        Conda => {
            let prefix = prefix
                .as_deref()
                .context("This isn't a conda environment.")?;
            plan.folder = prefix.to_string_lossy().into_owned();
            let installed = record::conda_packages(prefix)?;
            if let InstallTarget::R { library, .. } = target
                && *action == PackageAction::Remove
            {
                guard_r_remove(
                    Path::new(library),
                    &prefix.join("lib/R/library"),
                    &requested,
                )?;
            }
            let python_conda_names = (!is_r && *action != PackageAction::Install)
                .then(|| conda_distribution_names(prefix));
            for (&name, spec) in requested.iter().zip(packages.iter()) {
                let (spec, source, version) = if is_r {
                    conda_r_package(tool_path, name, *action, &installed)?
                } else {
                    let basename = record::python_name(name);
                    let conda_name = if installed.contains_key(name) {
                        name
                    } else {
                        python_conda_names
                            .as_ref()
                            .and_then(|mapping| mapping.get(&basename))
                            .map(String::as_str)
                            .unwrap_or(name)
                    };
                    if spec.contains("~=") || spec.contains("===") {
                        bail!(
                            "mamba and conda read versions written with ==, >=, <=, > or < only."
                        );
                    }
                    let version = if *action == PackageAction::Remove {
                        installed.get(conda_name).cloned()
                    } else {
                        pinned(spec, name)
                    };
                    if *action != PackageAction::Install && !installed.contains_key(conda_name) {
                        bail!(
                            "{name} wasn't installed by mamba or conda in this folder. Choose the tool that installed it."
                        );
                    }
                    (
                        format!("{conda_name}{}", &spec[name.len()..]),
                        "conda-forge or bioconda",
                        version,
                    )
                };
                plan.packages.push(package(name, &spec, source, version));
            }
            if *action == PackageAction::Remove {
                plan.notes.push(
                    "This tool may also remove packages that need the ones you chose.".into(),
                );
            } else {
                plan.notes
                    .push("This tool may add other packages that these need.".into());
            }
            conda_command(&selected.path, prefix, *action, &plan.packages)
        }
        Pip | Uv => {
            let prefix = prefix
                .as_deref()
                .context("The Python environment folder wasn't found.")?;
            let actual = python_probe.context("This Python couldn't be checked.")?;
            let installed = python_inventory(program, prefix)?;
            let conda_distributions = (*action != PackageAction::Install
                && prefix.join("conda-meta").is_dir())
            .then(|| conda_distribution_names(prefix));
            for (&name, spec) in requested.iter().zip(packages.iter()) {
                let normalized = record::python_name(name);
                if conda_distributions
                    .as_ref()
                    .is_some_and(|mapping| mapping.contains_key(&normalized))
                {
                    bail!("{name} was installed by mamba or conda. Use that tool to change it.");
                }
                let version = match *action {
                    PackageAction::Remove => {
                        Some(installed.get(&normalized).cloned().with_context(|| {
                            format!("{name} isn't installed in this Python folder.")
                        })?)
                    }
                    _ => pinned(spec, name),
                };
                plan.packages.push(package(name, spec, "PyPI", version));
            }
            if !writable(&actual.package_folder) {
                bail!(
                    "You can't write to {}. Make an environment in Getting Ready first.",
                    actual.package_folder.display()
                );
            }
            if *action != PackageAction::Remove {
                if !actual.package_folder.starts_with(prefix)
                    && !fs::canonicalize(&actual.package_folder).is_ok_and(|p| {
                        fs::canonicalize(prefix).is_ok_and(|root| p.starts_with(root))
                    })
                {
                    bail!(
                        "This Python would install packages outside the folder shown. Make an environment in Getting Ready first."
                    );
                }
                plan.notes
                    .push("Other packages that these need may be installed too.".into());
            }
            python_command(
                selected.tool,
                &selected.path,
                program.to_string_lossy().as_ref(),
                *action,
                packages,
            )
        }
    };
    Ok(command)
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b
        || fs::canonicalize(a)
            .ok()
            .zip(fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

fn package(name: &str, spec: &str, source: &str, version: Option<String>) -> PlannedPackage {
    PlannedPackage {
        name: name.into(),
        spec: spec.into(),
        source: source.into(),
        version,
    }
}

fn writable(path: &Path) -> bool {
    CString::new(path.as_os_str().as_bytes()).is_ok_and(|dir| {
        // SAFETY: a valid, NUL-terminated path; this doesn't change the folder.
        unsafe { libc::access(dir.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
    })
}

fn github_repo(name: &str) -> bool {
    let part = |s: &str| {
        s.starts_with(|c: char| c.is_ascii_alphanumeric())
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    };
    name.split_once('/')
        .is_some_and(|(owner, repo)| part(owner) && part(repo))
}

/// A PyPI name with optional comma-separated version comparisons; not a
/// path, direct URL, environment marker, or option.
fn python_spec(spec: &str) -> Option<&str> {
    let end = spec.find(['<', '>', '=', '!', '~']).unwrap_or(spec.len());
    let name = &spec[..end];
    if !name.starts_with(|c: char| c.is_ascii_alphanumeric())
        || !name.ends_with(|c: char| c.is_ascii_alphanumeric())
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return None;
    }
    let versions = &spec[end..];
    if !versions.is_empty()
        && !versions.split(',').all(|part| {
            ["===", "==", "!=", "<=", ">=", "~=", "<", ">"]
                .into_iter()
                .find_map(|operator| part.strip_prefix(operator).map(|v| (operator, v)))
                .is_some_and(|(op, version)| {
                    let version = if matches!(op, "==" | "!=") {
                        version.strip_suffix(".*").unwrap_or(version)
                    } else {
                        version
                    };
                    version.starts_with(|c: char| c.is_ascii_digit() || c == 'v' || c == 'V')
                        && version.bytes().all(|c| {
                            c.is_ascii_alphanumeric()
                                || matches!(c, b'-' | b'_' | b'.' | b'+' | b'!')
                        })
                })
        })
    {
        return None;
    }
    Some(name)
}

fn pinned(spec: &str, name: &str) -> Option<String> {
    let version = spec[name.len()..].strip_prefix("==")?;
    (!version.contains([',', '*'])).then(|| version.into())
}

fn bioconductor(name: &str) -> bool {
    setup::PACKAGES
        .iter()
        .any(|&(n, bioc, _)| n == name && bioc)
}

fn conda_command(
    tool: &str,
    prefix: &Path,
    action: PackageAction,
    packages: &[PlannedPackage],
) -> String {
    let verb = match action {
        PackageAction::Install => "install",
        PackageAction::Update => "update",
        PackageAction::Remove => "remove",
    };
    let folder = prefix.to_string_lossy();
    let mut args = vec![tool, verb, "-y", "-p", folder.as_ref()];
    if action != PackageAction::Remove {
        args.extend(CHANNELS);
    }
    if action == PackageAction::Install {
        args.push("--freeze-installed");
    }
    args.extend(packages.iter().map(|p| p.spec.as_str()));
    quote(args)
}

fn python_command(
    tool: InstallTool,
    path: &str,
    python: &str,
    action: PackageAction,
    packages: &[String],
) -> String {
    let verb = if action == PackageAction::Remove {
        "uninstall"
    } else {
        "install"
    };
    let mut args = if tool == InstallTool::Uv {
        vec![path, "pip", verb, "--python", python]
    } else {
        vec![python, "-m", "pip", verb]
    };
    match action {
        PackageAction::Update => args.push(if tool == InstallTool::Uv {
            "--upgrade"
        } else {
            "-U"
        }),
        PackageAction::Remove if tool == InstallTool::Pip => args.push("-y"),
        _ => {}
    }
    args.extend(packages.iter().map(String::as_str));
    quote(args)
}

fn conda_r_package(
    tool: &Path,
    name: &str,
    action: PackageAction,
    installed: &record::Versions,
) -> Result<(String, &'static str, Option<String>)> {
    if github_repo(name) {
        bail!("Use R's own installer for GitHub packages like {name}.");
    }
    let lower = name.to_ascii_lowercase();
    let r = format!("r-{lower}");
    let bioc = format!("bioconductor-{lower}");
    let candidates = if bioconductor(name) {
        [(&bioc, "bioconda"), (&r, "conda-forge")]
    } else {
        [(&r, "conda-forge"), (&bioc, "bioconda")]
    };
    for (spec, source) in candidates {
        if let Some(version) = installed.get(spec) {
            return Ok((
                spec.clone(),
                source,
                (action == PackageAction::Remove).then(|| version.clone()),
            ));
        }
        if action != PackageAction::Install {
            continue;
        }
        // These names are already known, and don't need a network request to plan.
        if setup::PACKAGES
            .iter()
            .any(|&(n, bioc, _)| name == n && bioc == (source == "bioconda"))
        {
            return Ok((spec.clone(), source, None));
        }
        if let Some(version) = conda_search(tool, spec) {
            return Ok((spec.clone(), source, Some(version)));
        }
    }
    if action == PackageAction::Install {
        bail!("Mamba or conda couldn't find {name}. Try R's own installer instead.");
    }
    bail!(
        "{name} wasn't installed by mamba or conda in this folder. Try R's own installer instead."
    )
}

fn conda_search(tool: &Path, name: &str) -> Option<String> {
    let mut args = vec!["search", "--json"];
    args.extend(CHANNELS);
    args.push(name);
    let out = probe(tool, &args)?;
    let json: Value = serde_json::from_str(&out).ok()?;
    let builds = json
        .get(name)
        .or_else(|| json.pointer("/result/pkgs"))?
        .as_array()?;
    builds.iter().rev().find_map(|pkg| {
        (pkg["name"].as_str().unwrap_or(name) == name).then_some(())?;
        Some(pkg["version"].as_str()?.into())
    })
}

/// Python distributions can have a different name in conda (`igraph` is
/// `python-igraph`). The conda record names the files it installed.
fn conda_distribution_names(prefix: &Path) -> std::collections::BTreeMap<String, String> {
    fs::read_dir(prefix.join("conda-meta"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            if entry.path().extension()?.to_str()? != "json" {
                return None;
            }
            let json: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let name = json["name"].as_str()?;
            let dists = json["files"]
                .as_array()?
                .iter()
                .filter_map(|file| {
                    let (stem, _) = file.as_str()?.rsplit_once(".dist-info/")?;
                    let (dist, _) = stem.rsplit('/').next()?.rsplit_once('-')?;
                    Some((record::python_name(dist), name.to_string()))
                })
                .collect::<std::collections::BTreeSet<_>>();
            Some(dists)
        })
        .flatten()
        .collect()
}

#[derive(serde::Deserialize)]
struct Python {
    prefix: String,
    managed: bool,
    package_folder: std::path::PathBuf,
}

fn inspect_python(program: &Path) -> Option<Python> {
    let code = r#"import json, os, sys, sysconfig
print(json.dumps(dict(prefix=sys.prefix, managed=os.path.isfile(os.path.join(sysconfig.get_path('stdlib'), 'EXTERNALLY-MANAGED')), package_folder=sysconfig.get_path('purelib'))))"#;
    serde_json::from_str(&probe(program, &["-I", "-c", code])?).ok()
}

fn python_inventory(program: &Path, folder: &Path) -> Result<record::Versions> {
    let code = r#"import importlib.metadata as md, json, os, re, sys, sysconfig
prefix = os.path.realpath(sys.argv[1])
if os.path.realpath(sys.prefix) != prefix:
    sys.exit('This Python no longer uses the selected folder')
paths = {os.path.realpath(sysconfig.get_path(key)) for key in ('purelib', 'platlib')}
paths = [path for path in paths if os.path.commonpath((path, prefix)) == prefix]
print(json.dumps({re.sub(r'[-_.]+', '-', d.metadata['Name']).lower(): d.version for d in md.distributions(path=paths) if d.metadata['Name'] and d.version}))"#;
    let out = probe(
        program,
        &["-I", "-c", code, folder.to_string_lossy().as_ref()],
    )
    .context("This Python couldn't list the packages in this folder.")?;
    Ok(serde_json::from_str(&out)?)
}

struct RInfo {
    binary: bool,
    bioc: bool,
    standard: std::path::PathBuf,
}

fn inspect_r(rscript: &Path, library: &Path) -> Option<RInfo> {
    let code = format!(
        r#".libPaths(c({}, .libPaths())); cat("\nSANDHILL\t", .Platform$pkgType, "\t", nzchar(system.file(package = "BiocManager")), "\t", .Library, "\n", sep = "")"#,
        r_string(library.to_string_lossy().as_ref())
    );
    let result = probe(rscript, &["-e", &code])?;
    let fields = result
        .lines()
        .find_map(|line| line.strip_prefix("SANDHILL\t"))?;
    let mut fields = fields.split('\t');
    Some(RInfo {
        binary: fields.next()? != "source",
        bioc: fields.next()? == "TRUE",
        standard: fields.next()?.into(),
    })
}

fn guard_r_remove(library: &Path, standard: &Path, names: &[&str]) -> Result<()> {
    for name in names {
        let built_in = [library, standard].into_iter().any(|lib| {
            fs::read_to_string(lib.join(name).join("DESCRIPTION")).is_ok_and(|desc| {
                matches!(
                    record::r_field(&desc, "Priority"),
                    Some("base" | "recommended")
                )
            })
        });
        if built_in {
            bail!("{name} comes with R and can't be removed here.");
        }
    }
    Ok(())
}

fn known_to_need_compiler(rscript: &Path, names: &[&str], use_bioc: bool) -> bool {
    if names.iter().any(|name| github_repo(name)) {
        return false;
    }
    let repos = if use_bioc {
        "if (requireNamespace('BiocManager', quietly = TRUE)) BiocManager::repositories() else repos"
    } else {
        "repos"
    };
    let code = format!(
        r#"options(timeout = 10L); repos <- c(CRAN = "{CRAN}"); a <- suppressWarnings(utils::available.packages(repos = {repos}, type = "source", fields = "NeedsCompilation")); n <- {}; cat("\nSANDHILL:", all(n %in% rownames(a)) && isTRUE(all(a[n, "NeedsCompilation"] == "yes")), "\n", sep = "")"#,
        r_vector(names)
    );
    probe(rscript, &["-e", &code])
        .is_some_and(|text| text.lines().any(|line| line == "SANDHILL:TRUE"))
}

pub(super) fn r_string(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn r_vector(names: &[&str]) -> String {
    format!(
        "c({})",
        names
            .iter()
            .map(|name| r_string(name))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn r_code(action: PackageAction, library: &Path, names: &[&str], use_bioc: bool) -> String {
    let library = r_string(library.to_string_lossy().as_ref());
    let check = crate::irkernel::check_library_code();
    if action == PackageAction::Remove {
        return format!(
            "local({{\n  lib <- {library}\n  {check}\n  utils::remove.packages({}, lib = lib)\n}})",
            r_vector(names)
        );
    }
    let (github, other): (Vec<_>, Vec<_>) =
        names.iter().copied().partition(|name| github_repo(name));
    let other = r_vector(&other);
    let github = r_vector(&github);
    // A copy in another of R's libraries doesn't count: the user chose this one.
    let install = if use_bioc {
        r#"if (!requireNamespace("BiocManager", quietly = TRUE)) utils::install.packages("BiocManager", lib = lib, repos = repos, type = type)
    BiocManager::install(wanted, lib = lib, ask = FALSE, update = FALSE, force = TRUE, type = type)"#
    } else {
        "utils::install.packages(wanted, lib = lib, repos = repos, type = type)"
    };
    let only_missing = if action == PackageAction::Install {
        "\n  wanted <- setdiff(wanted, rownames(utils::installed.packages(lib.loc = lib, noCache = TRUE)))"
    } else {
        ""
    };
    let rverb = if action == PackageAction::Update {
        "Updating"
    } else {
        "Installing"
    };
    format!(
        r#"local({{
  lib <- {library}
  {check}
  binary <- .Platform$pkgType != "source"
  type <- if (binary) "binary" else "source"
  if (binary) options(install.packages.compile.from.source = "never")
  else message("This R builds packages from their source code. That can take a few minutes.")
  repos <- c(CRAN = "{CRAN}")
  wanted <- {other}{only_missing}
  if (length(wanted)) {{
    message("{rverb} ", paste(wanted, collapse = ", "), " in ", lib)
    {install}
    missing <- setdiff(wanted, rownames(utils::installed.packages(lib.loc = lib, noCache = TRUE)))
    if (length(missing)) stop("These packages couldn't be installed: ", paste(missing, collapse = ", "), call. = FALSE)
  }}
  github <- {github}
  if (length(github)) {{
    if (!requireNamespace("remotes", quietly = TRUE)) utils::install.packages("remotes", lib = lib, repos = repos, type = type)
    for (repo in github) remotes::install_github(repo, lib = lib, upgrade = "never", repos = repos)
  }}
}})"#
    )
}

pub(super) fn run(record_path: &Path, plan: &PackagePlan) -> Result<()> {
    let folder = Path::new(&plan.folder);
    let tool = plan.change.tool.context("No tool was chosen.")?;
    let action = plan.change.action;
    let snapshot = || match tool {
        InstallTool::R => record::r_packages(folder),
        InstallTool::Conda => record::conda_packages(folder),
        InstallTool::Pip | InstallTool::Uv => match &plan.change.target {
            InstallTarget::Python { python, .. } => python_inventory(Path::new(python), folder),
            _ => bail!("Python wasn't selected."),
        },
    };
    let kind = match tool {
        InstallTool::R => record::R_PACKAGE,
        InstallTool::Conda => record::CONDA_PACKAGE,
        InstallTool::Pip | InstallTool::Uv => record::PYTHON_PACKAGE,
    };
    let before = snapshot()?;
    let owned: std::collections::BTreeMap<_, _> = if action != PackageAction::Remove {
        record::owned_in(record_path, kind, folder, &before)?
    } else {
        Default::default()
    };
    let result = install::run_command(&plan.command);
    if action != PackageAction::Remove {
        let after = snapshot()?;
        let fresh = after.into_iter().filter_map(|(name, version)| {
            let old_version = before.get(&name);
            let old_record = owned.get(&name);
            if old_version.is_some_and(|v| v == &version)
                || (old_version.is_some() && old_record.is_none())
            {
                return None;
            }
            let requested = plan.packages.iter().any(|p| match tool {
                InstallTool::R => p.name == name || github_match(folder, &name, &p.name),
                InstallTool::Conda => {
                    p.spec.split(['=', '<', '>', '!', '~']).next() == Some(name.as_str())
                }
                _ => record::python_name(&p.name) == name,
            });
            let reason = if requested && !plan.change.reason.is_empty() {
                &plan.change.reason
            } else {
                old_record.map(String::as_str).unwrap_or("")
            };
            Some(record::entry(
                kind,
                name,
                version,
                &plan.folder,
                tool.id(),
                reason,
            ))
        });
        record::add(record_path, fresh)?;
    }
    record::prune(record_path)?;
    result?;
    println!("Done.");
    Ok(())
}

fn github_match(folder: &Path, name: &str, wanted: &str) -> bool {
    let Some((owner, repo)) = wanted.split_once('/') else {
        return false;
    };
    fs::read_to_string(folder.join(name).join("DESCRIPTION")).is_ok_and(|description| {
        record::r_field(&description, "RemoteUsername")
            .is_some_and(|v| v.eq_ignore_ascii_case(owner))
            && record::r_field(&description, "RemoteRepo")
                .is_some_and(|v| v.eq_ignore_ascii_case(repo))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use PackageAction::{Install, Remove, Update};

    #[test]
    fn validates_names_without_admitting_urls_paths_or_options() {
        for name in [
            "numpy",
            "scikit_learn",
            "scanpy>=1.10,<2",
            "numpy==1.5.*",
            "package==1!2.0+local",
        ] {
            assert!(python_spec(name).is_some(), "{name}");
        }
        for name in [
            "--user",
            "name @ https://example.test",
            "file.whl/",
            "./name",
            "-r",
            "name>=",
            "name,foo",
            "name==1;evil",
            "name==1, --target/tmp",
            "numpy[*]",
        ] {
            assert!(python_spec(name).is_none(), "{name}");
        }
        assert!(record::r_name("org.Hs.eg.db"));
        assert!(!record::r_name("../lib"));
        assert!(github_repo("some-owner/a.repo"));
        assert!(!github_repo("../repo"));
    }

    #[test]
    fn package_commands_quote_arguments_and_match_each_tool() {
        let packages = ["science>=2".to_string()];
        let platform = "/a b/bin/python";
        assert_eq!(
            python_command(InstallTool::Pip, platform, platform, Install, &packages),
            "'/a b/bin/python' '-m' 'pip' 'install' 'science>=2'"
        );
        assert_eq!(
            python_command(InstallTool::Pip, platform, platform, Update, &packages),
            "'/a b/bin/python' '-m' 'pip' 'install' '-U' 'science>=2'"
        );
        assert_eq!(
            python_command(
                InstallTool::Pip,
                platform,
                platform,
                Remove,
                &["science".into()]
            ),
            "'/a b/bin/python' '-m' 'pip' 'uninstall' '-y' 'science'"
        );
        assert_eq!(
            python_command(InstallTool::Uv, "/uv", platform, Update, &packages),
            "'/uv' 'pip' 'install' '--python' '/a b/bin/python' '--upgrade' 'science>=2'"
        );
        assert_eq!(
            python_command(
                InstallTool::Uv,
                "/uv",
                platform,
                Remove,
                &["science".into()]
            ),
            "'/uv' 'pip' 'uninstall' '--python' '/a b/bin/python' 'science'"
        );
        let known = plan_r_conda();
        assert_eq!(
            conda_command("/mamba", Path::new("/env"), Install, &known),
            "'/mamba' 'install' '-y' '-p' '/env' '-c' 'conda-forge' '-c' 'bioconda' '--override-channels' '--freeze-installed' 'bioconductor-deseq2'"
        );
        assert_eq!(
            conda_command("/mamba", Path::new("/env"), Remove, &known),
            "'/mamba' 'remove' '-y' '-p' '/env' 'bioconductor-deseq2'"
        );
        assert!(
            !conda_command("/mamba", Path::new("/env"), Update, &known)
                .contains("--freeze-installed")
        );
        let r = r_code(
            Install,
            Path::new("/R's lib"),
            &["ggplot2", "DESeq2", "org/repo"],
            true,
        );
        assert!(r.contains("BiocManager::install(wanted, lib = lib, ask = FALSE, update = FALSE"));
        assert!(r.contains(
            "remotes::install_github(repo, lib = lib, upgrade = \"never\", repos = repos)"
        ));
        assert!(r.contains(
            "wanted <- setdiff(wanted, rownames(utils::installed.packages(lib.loc = lib"
        ));
        assert!(
            r_code(Remove, Path::new("/lib"), &["x"], false)
                .contains("utils::remove.packages(c(\"x\"), lib = lib)")
        );
    }

    fn plan_r_conda() -> Vec<PlannedPackage> {
        let (spec, source, version) =
            conda_r_package(Path::new("/mamba"), "DESeq2", Install, &Default::default()).unwrap();
        vec![package("DESeq2", &spec, source, version)]
    }

    #[test]
    fn refuses_to_create_a_library_or_remove_r_shipped_packages() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        let plan = plan(
            &Search::default(),
            PackageChange {
                action: Install,
                target: InstallTarget::R {
                    r: "/bin/sh".into(),
                    library: missing.to_string_lossy().into_owned(),
                },
                packages: vec!["ggplot2".into()],
                tool: Some(InstallTool::R),
                reason: String::new(),
            },
        );
        assert!(plan.refused.unwrap().contains("doesn't create libraries"));
        assert!(!missing.exists() && plan.command.is_empty());
        fs::create_dir(temp.path().join("boot")).unwrap();
        fs::write(
            temp.path().join("boot/DESCRIPTION"),
            "Package: boot\nPriority: recommended\n",
        )
        .unwrap();
        assert!(guard_r_remove(temp.path(), temp.path(), &["boot"]).is_err());
    }
}
