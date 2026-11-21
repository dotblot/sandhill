//! Plan a copy from package records, then install only into Sandhill's folder.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use sandhill_proto::{
    CopyReport, CopyRow, EnvProblem, FailureFix, InstallTarget, InstallTool, OwnSetupPlan,
    PackageAction, PackageChange, SetupStep,
};
use serde::{Deserialize, Serialize};

use super::{
    environments, install, install_failure, own, package_inventory as inventory, package_names,
    package_target, record,
};
use crate::{kernelspecs::Search, own_env};
use inventory::Source;

#[derive(Deserialize, Serialize, Debug)]
pub(super) struct Task {
    row: usize,
    spec: String,
    route: Route,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Conda,
    Cran,
    Bioc,
    GithubR,
    GithubPython,
    Pip,
}

impl Route {
    fn tool(self) -> InstallTool {
        match self {
            Self::Conda => InstallTool::Conda,
            Self::Cran | Self::Bioc | Self::GithubR => InstallTool::R,
            Self::GithubPython | Self::Pip => InstallTool::Pip,
        }
    }
}

struct Candidate {
    language: &'static str,
    name: String,
    version: String,
    source: Source,
    conda: String,
}

#[derive(Deserialize)]
struct Hit {
    name: String,
    version: String,
    channel: String,
    #[serde(default)]
    size: u64,
}

type Hits = BTreeMap<String, Vec<Hit>>;

fn candidates(
    record: &Path,
    plan: &OwnSetupPlan,
    core: &HashSet<(&str, String)>,
) -> Result<Vec<Candidate>> {
    let mut result = BTreeMap::new();
    for env in environments::copy_sources(&Search::from_env(), record, &plan.setup.copy_from)? {
        if env.r {
            ensure!(
                plan.setup.r,
                "Choose R in Sandhill's folder before copying R packages."
            );
            let runtime = inventory::r_runtime(&env.program, None, None, None)?;
            for (name, pkg) in inventory::r_packages(&runtime.libraries) {
                if pkg.base || pkg.recommended || core.contains(&("r", name.clone())) {
                    continue;
                }
                let conda = own::r_package(&name, pkg.source == Source::Bioc);
                result.entry(("r", name.clone())).or_insert(Candidate {
                    language: "r",
                    name,
                    version: pkg.version,
                    source: pkg.source,
                    conda,
                });
            }
        } else {
            ensure!(
                plan.setup.python,
                "Choose Python in Sandhill's folder before copying Python packages."
            );
            let prefix = env.prefix.context("Python's folder wasn't found.")?;
            let mut sites = inventory::prefix_sites(&prefix);
            if !prefix.join("pyvenv.cfg").is_file()
                && !prefix.join("conda-meta").is_dir()
                && let Some(info) = package_target::python_info(&env.program, false)
            {
                sites.extend(info.sites.iter().cloned());
                if std::env::var_os("PYTHONNOUSERSITE").is_none() {
                    sites.insert(0, info.user_site.clone());
                }
            }
            for (key, pkg) in inventory::python_packages(&prefix, sites, false) {
                if matches!(key.as_str(), "pip" | "setuptools" | "wheel")
                    || core.contains(&("python", key.clone()))
                {
                    continue;
                }
                let conda = pkg.conda.map_or_else(
                    || {
                        record::python_name(package_names::distribution(
                            &pkg.name,
                            InstallTool::Conda,
                        ))
                    },
                    |(name, _)| name,
                );
                result.entry(("python", key)).or_insert(Candidate {
                    language: "python",
                    name: pkg.name,
                    version: pkg.version,
                    source: pkg.source,
                    conda,
                });
            }
        }
    }
    Ok(result.into_values().collect())
}

fn index(record: &Path, platform: &str, candidates: &[Candidate]) -> Result<Hits> {
    let names = candidates
        .iter()
        .filter(|pkg| pkg.source != Source::Local)
        .map(|pkg| pkg.conda.as_str())
        .filter(|name| super::package_plan::python_spec(name) == Some(*name))
        .collect::<std::collections::BTreeSet<_>>();
    if names.is_empty() {
        return Ok(Hits::new());
    }
    let cache = record.with_file_name("copy-package-index");
    fs::create_dir_all(&cache)?;
    let _lock = install::lock(&cache.join(".lock"), true)?;
    let bin = cache.join("micromamba");
    if !install::executable(&bin) {
        // Publish the cached tool only after a complete download.
        let tmp = tempfile::NamedTempFile::new_in(&cache)?;
        let status = Command::new("/bin/bash")
            .env_remove("BASH_ENV")
            .env("HOME", &cache)
            .current_dir(&cache)
            .arg("-c")
            .arg(format!(
                "set -e -o pipefail\n{}",
                own::download_mamba(tmp.path(), platform)
            ))
            .status()?;
        ensure!(
            status.success(),
            "Sandhill couldn't download micromamba to check the conda packages."
        );
        tmp.persist(&bin)?;
    }
    let cfg = cache.join("condarc");
    if !cfg.is_file() {
        crate::files::write_atomic(&cfg, b"local_repodata_ttl: 86400\n")?;
    }
    let fingerprint = names.iter().copied().collect::<Vec<_>>().join("\n");
    let cached = cache.join("last-query.json");
    let fresh = cached
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|m| m.elapsed().is_ok_and(|age| age.as_secs() < 86_400));
    let parse = |text: &str| -> Result<Hits> {
        #[derive(Deserialize)]
        struct Query {
            result: QueryResult,
        }
        #[derive(Deserialize)]
        struct QueryResult {
            pkgs: Vec<Hit>,
        }
        let mut index = Hits::new();
        for hit in serde_json::from_str::<Query>(text)?.result.pkgs {
            if inventory::version(&hit.version)
                && matches!(inventory::channel(&hit.channel), "conda-forge" | "bioconda")
            {
                index.entry(hit.name.clone()).or_default().push(hit);
            }
        }
        Ok(index)
    };
    if fresh
        && let Ok(bytes) = fs::read(&cached)
        && let Ok((saved, text)) = serde_json::from_slice::<(String, String)>(&bytes)
        && saved == fingerprint
        && let Ok(index) = parse(&text)
    {
        return Ok(index);
    }
    let mut command = Command::new(&bin);
    command
        .current_dir(&cache)
        .env("HOME", &cache)
        .env("XDG_CACHE_HOME", cache.join("cache"))
        .env("TMPDIR", &cache)
        .env_remove("MAMBARC")
        .env_remove("CONDA_ENVS_PATH")
        .env_remove("CONDA_PKGS_DIRS")
        .env_remove("CONDA_PREFIX")
        .env_remove("CONDA_DEFAULT_ENV");
    let root = cache.join("conda");
    let mut args = vec![
        "--rc-file",
        cfg.to_str()
            .context("Sandhill's state folder couldn't be read.")?,
        "repoquery",
        "search",
        "--json",
        "--root-prefix",
        root.to_str()
            .context("Sandhill's state folder couldn't be read.")?,
        "--override-channels",
        "-c",
        "conda-forge",
        "-c",
        "bioconda",
        "--platform",
        platform,
    ];
    args.extend(names);
    if let Some(text) = install::probe_command_with(command, &args, Duration::from_secs(150), false)
        && let Ok(index) = parse(&text)
    {
        // Keep one answer for the last selection as well as micromamba's day-long channel cache.
        crate::files::write_atomic(&cached, &serde_json::to_vec(&(&fingerprint, &text))?)?;
        return Ok(index);
    }
    anyhow::bail!(
        "Sandhill couldn't check the conda package lists. Check your internet connection and try again."
    )
}

fn decide(pkg: &Candidate, hits: &Hits) -> (CopyRow, Option<(Route, String, u64)>) {
    let mut row = CopyRow {
        language: pkg.language.into(),
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        from: pkg.source.label().into(),
        source: None,
        new_version: None,
        tool: None,
        cant: None,
    };
    let known = hits.get(&pkg.conda).into_iter().flatten().filter(|hit| {
        pkg.language != "r"
            || inventory::channel(&hit.channel)
                == if pkg.source == Source::Bioc {
                    "bioconda"
                } else {
                    "conda-forge"
                }
    });
    let selected = known
        .clone()
        .find(|p| p.version == pkg.version)
        .or_else(|| {
            known.max_by(|a, b| {
                version_key(&a.version)
                    .cmp(&version_key(&b.version))
                    .then_with(|| a.version.cmp(&b.version))
            })
        });
    let choice = if matches!(&pkg.source, Source::Local) {
        row.cant = Some(
            "Built from a folder on this computer, so there is nowhere to get it from.".into(),
        );
        None
    } else if let Some(hit) = selected {
        let source = inventory::channel(&hit.channel);
        row.source = Some(source.into());
        row.new_version = Some(hit.version.clone());
        Some((
            Route::Conda,
            format!("{source}::{}=={}", hit.name, hit.version),
            hit.size.div_ceil(1_000_000).max(1),
        ))
    } else {
        match &pkg.source {
            Source::Cran | Source::Bioc => {
                row.source = Some(pkg.source.label().into());
                Some((
                    if pkg.source == Source::Cran {
                        Route::Cran
                    } else {
                        Route::Bioc
                    },
                    pkg.name.clone(),
                    3,
                ))
            }
            Source::Github(repo) => {
                row.source = Some("GitHub".into());
                row.new_version = Some(pkg.version.clone());
                if pkg.language == "r" {
                    Some((Route::GithubR, repo.spec(), 3))
                } else {
                    let subdir = repo
                        .subdir
                        .as_ref()
                        .map_or_else(String::new, |s| format!("#subdirectory={s}"));
                    Some((
                        Route::GithubPython,
                        format!(
                            "{} @ git+https://github.com/{}/{}.git@{}{subdir}",
                            pkg.name, repo.owner, repo.repo, repo.sha
                        ),
                        3,
                    ))
                }
            }
            Source::Pypi | Source::Conda(_) if pkg.language == "python" => {
                row.source = Some("PyPI".into());
                row.new_version = Some(pkg.version.clone());
                Some((
                    Route::Pip,
                    format!(
                        "{}=={}",
                        package_names::distribution(&pkg.name, InstallTool::Pip),
                        pkg.version
                    ),
                    4,
                ))
            }
            _ => {
                row.cant = Some(
                    "No source for this package was recorded, and it wasn't found in conda.".into(),
                );
                None
            }
        }
    };
    row.tool = choice.as_ref().map(|(route, ..)| route.tool());
    (row, choice)
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split(['.', '-', '_'])
        .map_while(|part| part.parse().ok())
        .collect()
}

pub(super) fn prepare(
    record: &Path,
    plan: &mut OwnSetupPlan,
    platform: &str,
    core: &HashSet<(&str, String)>,
) -> Result<Vec<Task>> {
    if plan.setup.copy_from.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = candidates(record, plan, core)?;
    let (hits, checked) = match index(record, platform, &candidates) {
        Ok(hits) => (hits, true),
        Err(error) => {
            plan.notes.push(error.to_string());
            (Hits::new(), false)
        }
    };
    let mut tasks = Vec::new();
    for pkg in &candidates {
        let (mut row, choice) = decide(pkg, &hits);
        if !checked && pkg.source == Source::Unknown && row.cant.is_some() {
            row.cant = Some("There was no recorded source, and Sandhill couldn't check the conda package lists.".into());
        }
        if let Some((route, spec, size)) = choice {
            plan.download_mb += size;
            tasks.push(Task {
                row: plan.copy.len(),
                spec,
                route,
            });
        }
        plan.copy.push(row);
    }
    let folder = Path::new(&plan.setup.folder);
    for batch in batches(&tasks, &plan.copy) {
        let route = batch[0].route;
        plan.steps.push(SetupStep {
            what: match route {
                Route::Conda => format!(
                    "Copy packages from conda into Sandhill's {}",
                    if plan.copy[batch[0].row].language == "r" {
                        "R"
                    } else {
                        "Python"
                    }
                ),
                Route::Cran => "Build the copy from CRAN".into(),
                Route::Bioc => "Build the copy from Bioconductor".into(),
                Route::GithubR | Route::GithubPython => "Copy a package from GitHub".into(),
                Route::Pip => "Copy packages from PyPI".into(),
            },
            command: command(folder, &plan.copy, &batch),
        });
    }
    if tasks
        .iter()
        .any(|task| matches!(task.route, Route::Cran | Route::Bioc | Route::GithubR))
    {
        plan.notes.push("R packages copied from CRAN, Bioconductor or GitHub are built here. That can take a few minutes and may require developer tools.".into());
        plan.minutes += 5;
    }
    Ok(tasks)
}

fn batches<'a>(tasks: &'a [Task], rows: &[CopyRow]) -> Vec<Vec<&'a Task>> {
    let mut all = Vec::new();
    for language in ["r", "python"] {
        let conda: Vec<_> = tasks
            .iter()
            .filter(|t| t.route == Route::Conda && rows[t.row].language == language)
            .collect();
        if !conda.is_empty() {
            all.push(conda);
        }
    }
    for route in [
        Route::Cran,
        Route::Bioc,
        Route::GithubR,
        Route::GithubPython,
    ] {
        all.extend(tasks.iter().filter(|t| t.route == route).map(|t| vec![t]));
    }
    let pip: Vec<_> = tasks.iter().filter(|t| t.route == Route::Pip).collect();
    if !pip.is_empty() {
        all.push(pip);
    }
    all
}

fn command(folder: &Path, rows: &[CopyRow], batch: &[&Task]) -> String {
    let task = batch[0];
    let row = &rows[task.row];
    let prefix = own_env::prefix(folder, &row.language);
    use install::quote;
    let isolated = |line| {
        own_env::command(
            folder,
            &format!(
                "/usr/bin/env {} {line}",
                quote([format!("HOME={}", folder.join("home").display())])
            ),
        )
    };
    match task.route {
        Route::Conda => quote(
            [
                folder.join("bin/micromamba").to_string_lossy().as_ref(),
                "install",
                "--freeze-installed",
                "-y",
                "-p",
                &prefix.to_string_lossy(),
                "--override-channels",
                "-c",
                "conda-forge",
                "-c",
                "bioconda",
            ]
            .into_iter()
            .chain(batch.iter().map(|task| task.spec.as_str())),
        ),
        Route::Pip | Route::GithubPython => isolated(quote(
            [
                prefix.join("bin/python").to_string_lossy().as_ref(),
                "-I",
                "-m",
                "pip",
                "install",
                "--no-deps",
                "--",
            ]
            .into_iter()
            .chain(batch.iter().map(|task| task.spec.as_str())),
        )),
        Route::Cran | Route::Bioc | Route::GithubR => {
            let name = serde_json::to_string(&row.name).expect("string");
            let spec = serde_json::to_string(&task.spec).expect("string");
            let code = match task.route {
                Route::Cran => format!("utils::install.packages({name}, type='source')"),
                Route::Bioc => format!(
                    "if (!requireNamespace('BiocManager', quietly=TRUE)) utils::install.packages('BiocManager'); BiocManager::install({name}, type='source', ask=FALSE, update=FALSE)"
                ),
                _ => format!(
                    "if (!requireNamespace('remotes', quietly=TRUE)) utils::install.packages('remotes'); remotes::install_github({spec}, upgrade='never')"
                ),
            };
            let code = format!(
                "options(repos=c(CRAN='https://cloud.r-project.org')); withCallingHandlers({{{code}}}, warning=function(w) {{ if (grepl('non-zero exit status|not available|not supported', conditionMessage(w))) stop(conditionMessage(w), call.=FALSE) }}); stopifnot(nzchar(system.file(package={name})))"
            );
            let line = isolated(quote([
                prefix.join("bin/Rscript").to_string_lossy().as_ref(),
                "--vanilla",
                "-e",
                &code,
            ]));
            if cfg!(target_os = "macos") {
                format!(
                    "if SDKROOT=$(/usr/bin/xcrun --show-sdk-path 2>/dev/null); then export SDKROOT; fi\n{line}"
                )
            } else {
                line
            }
        }
    }
}

fn capture(command: &str, log: &mut fs::File) -> Result<bool> {
    log.set_len(0)?;
    use std::io::{Seek as _, SeekFrom};
    log.seek(SeekFrom::Start(0))?;
    let status = Command::new("/bin/bash")
        .arg("-c")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?)
        .status()?;
    log.seek(SeekFrom::Start(0))?;
    std::io::copy(log, &mut std::io::stdout().lock())?;
    Ok(status.success())
}

fn report_path(record_path: &Path, folder: &Path) -> PathBuf {
    record_path.with_file_name(format!(
        "copy-report-{}.json",
        record::digest(folder.as_os_str().as_encoded_bytes())
    ))
}

pub(super) fn report(record_path: &Path, folder: &Path) -> Option<CopyReport> {
    serde_json::from_slice(&fs::read(report_path(record_path, folder)).ok()?).ok()
}

pub(super) fn forget(record: &Path, folder: &Path) {
    let _ = fs::remove_file(report_path(record, folder));
}

pub(super) fn run(record_path: &Path, plan: &OwnSetupPlan, tasks: &[Task]) -> Result<()> {
    let folder = Path::new(&plan.setup.folder);
    let mut report = CopyReport {
        date: record::today(),
        copied: 0,
        failed: Vec::new(),
    };
    for row in &plan.copy {
        if let Some(reason) = &row.cant {
            report.failed.push(EnvProblem {
                package: row.name.clone(),
                reason: reason.clone(),
                fix: None,
            });
        }
    }
    let mut log = tempfile::tempfile_in(
        record_path
            .parent()
            .context("Sandhill's state folder wasn't found.")?,
    )?;
    let before = versions(folder);
    for batch in batches(tasks, &plan.copy) {
        let mut pending = vec![batch.as_slice()];
        while let Some(batch) = pending.pop() {
            let names = batch
                .iter()
                .map(|task| plan.copy[task.row].name.as_str())
                .collect::<Vec<_>>();
            println!("Copying {}.", names.join(", "));
            let succeeded = capture(&command(folder, &plan.copy, batch), &mut log)?;
            if succeeded {
                report.copied += batch.len() as u32;
                continue;
            }
            let log_text = {
                use std::io::{Read as _, Seek as _, SeekFrom};
                let len = log.metadata()?.len();
                log.seek(SeekFrom::Start(len.saturating_sub(1024 * 1024)))?;
                let mut bytes = Vec::new();
                (&mut log).take(1024 * 1024).read_to_end(&mut bytes)?;
                String::from_utf8_lossy(&bytes).into_owned()
            };
            let failure = install_failure::copy_failure(
                &log_text,
                folder,
                names[0],
                alternative(folder, plan, batch[0]),
            );
            if batch.len() > 1
                && failure.as_ref().is_none_or(|f| {
                    !matches!(
                        f.kind,
                        sandhill_proto::InstallFailureKind::Network
                            | sandhill_proto::InstallFailureKind::Space
                            | sandhill_proto::InstallFailureKind::Permission
                    )
                })
            {
                let (left, right) = batch.split_at(batch.len() / 2);
                pending.push(right);
                pending.push(left);
                continue;
            }
            for name in names {
                let fix = match failure.as_ref().and_then(|f| f.fix.as_ref()) {
                    Some(FailureFix::OtherTool { change, .. }) if batch.len() == 1 => {
                        change.clone()
                    }
                    _ => None,
                };
                report.failed.push(EnvProblem {
                    package: name.into(),
                    reason: failure.as_ref().map_or_else(
                        || "The installer didn't finish. See the log for details.".into(),
                        |f| f.message.clone(),
                    ),
                    fix,
                });
            }
        }
    }
    for ((kind, place), packages) in versions(folder) {
        let old = before.get(&(kind, place.clone()));
        let tool = match kind {
            record::CONDA_PACKAGE => "conda",
            record::R_PACKAGE => "r",
            _ => "pip",
        };
        record::add(
            record_path,
            packages
                .into_iter()
                .filter(|(name, ver)| old.and_then(|p| p.get(name)) != Some(ver))
                .map(|(name, version)| {
                    record::entry(
                        kind,
                        name,
                        version,
                        &place.to_string_lossy(),
                        tool,
                        "Copied into Sandhill's own folder",
                    )
                }),
        )?;
    }
    crate::files::write_atomic(
        &report_path(record_path, folder),
        &serde_json::to_vec(&report)?,
    )?;
    println!(
        "Copied {} packages. {} couldn't be copied.",
        report.copied,
        report.failed.len()
    );
    Ok(())
}

fn versions(folder: &Path) -> BTreeMap<(&'static str, PathBuf), record::Versions> {
    let mut versions = BTreeMap::new();
    for lang in ["r", "python"] {
        let prefix = own_env::prefix(folder, lang);
        if let Ok(conda) = record::conda_packages(&prefix) {
            versions.insert((record::CONDA_PACKAGE, prefix.clone()), conda);
        }
        if lang == "r" {
            let lib = prefix.join("lib/R/library");
            if let Ok(packages) = record::r_packages(&lib) {
                versions.insert((record::R_PACKAGE, lib), packages);
            }
        } else {
            let pip = inventory::python_packages(&prefix, inventory::prefix_sites(&prefix), false)
                .into_iter()
                .filter(|(_, p)| p.conda.is_none())
                .map(|(name, p)| (name, p.version))
                .collect();
            versions.insert((record::PYTHON_PACKAGE, prefix), pip);
        }
    }
    versions
}

fn alternative(folder: &Path, plan: &OwnSetupPlan, task: &Task) -> Option<PackageChange> {
    let row = &plan.copy[task.row];
    if task.route != Route::Conda {
        return None;
    }
    let prefix = own_env::prefix(folder, &row.language);
    let (tool, target) = if row.language == "python" {
        (
            InstallTool::Pip,
            InstallTarget::Python {
                python: prefix.join("bin/python").to_string_lossy().into_owned(),
                prefix: prefix.to_string_lossy().into_owned(),
            },
        )
    } else if matches!(row.from.as_str(), "CRAN" | "Bioconductor") {
        (
            InstallTool::R,
            InstallTarget::R {
                r: prefix.join("bin/R").to_string_lossy().into_owned(),
                library: prefix.join("lib/R/library").to_string_lossy().into_owned(),
            },
        )
    } else {
        return None;
    };
    Some(PackageChange {
        action: PackageAction::Install,
        target,
        packages: vec![row.name.clone()],
        tool: Some(tool),
        reason: "Try copying from another source".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_decisions_use_provenance_and_exact_conda_versions() {
        let hits = BTreeMap::from([(
            "r-test".into(),
            vec![
                Hit {
                    name: "r-test".into(),
                    version: "1.1".into(),
                    channel: "conda-forge".into(),
                    size: 0,
                },
                Hit {
                    name: "r-test".into(),
                    version: "2.0".into(),
                    channel: "conda-forge".into(),
                    size: 0,
                },
            ],
        )]);
        let mut pkg = Candidate {
            language: "r",
            name: "test".into(),
            version: "1.1".into(),
            source: Source::Cran,
            conda: "r-test".into(),
        };
        let (same, task) = decide(&pkg, &hits);
        assert_eq!(same.new_version.as_deref(), Some("1.1"));
        assert_eq!(task.unwrap().1, "conda-forge::r-test==1.1");
        pkg.version = "1.0".into();
        let (other, _) = decide(&pkg, &hits);
        assert_eq!(other.new_version.as_deref(), Some("2.0"));
        assert_eq!(other.from, "CRAN");
        assert_eq!(decide(&pkg, &Hits::new()).0.tool, Some(InstallTool::R));
        pkg.source = Source::Github(inventory::Github {
            owner: "someone".into(),
            repo: "repo".into(),
            sha: "012345abcdef".into(),
            subdir: None,
        });
        let (github, task) = decide(&pkg, &Hits::new());
        assert_eq!(github.source.as_deref(), Some("GitHub"));
        assert_eq!(task.unwrap().1, "someone/repo@012345abcdef");
        pkg.source = Source::Local;
        let (local, task) = decide(&pkg, &hits);
        assert!(local.cant.unwrap().contains("folder on this computer"));
        assert!(task.is_none());
    }
}

#[cfg(test)]
#[test]
fn failed_conda_batch_does_not_block_the_other_copies() {
    use crate::quick::test_common::tempdir;
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let folder = root.join("own");
    let prefix = own_env::prefix(&folder, "python");
    fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    fs::create_dir_all(folder.join("bin")).unwrap();
    let mamba = folder.join("bin/micromamba");
    fs::write(
        &mamba,
        format!(
            r#"#!/bin/sh
for word in "$@"; do
  case "$word" in
    *::bad=*) echo 'Could not solve for environment specs' >&2; exit 1 ;;
  esac
done
printf '{{}}' > '{}'
"#,
            prefix.join("conda-meta/good-1.0-build.json").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&mamba, fs::Permissions::from_mode(0o700)).unwrap();
    let record = root.join(record::FILE);
    let setup = sandhill_proto::OwnSetup {
        folder: folder.display().to_string(),
        r: false,
        python: true,
        kits: Vec::new(),
        copy_from: vec!["fake".into()],
    };
    let row = |name: &str| CopyRow {
        language: "python".into(),
        name: name.into(),
        version: "1.0".into(),
        from: "PyPI".into(),
        source: Some("conda-forge".into()),
        new_version: Some("1.0".into()),
        tool: Some(InstallTool::Conda),
        cant: None,
    };
    let plan = OwnSetupPlan {
        setup,
        steps: Vec::new(),
        r_packages: Vec::new(),
        python_packages: Vec::new(),
        download_mb: 0,
        free_mb: 0,
        minutes: 0,
        notes: Vec::new(),
        needs_developer_tools: false,
        copy: vec![row("bad"), row("good")],
        refused: None,
    };
    let tasks = [
        Task {
            row: 0,
            spec: "conda-forge::bad=1.0".into(),
            route: Route::Conda,
        },
        Task {
            row: 1,
            spec: "conda-forge::good=1.0".into(),
            route: Route::Conda,
        },
    ];
    run(&record, &plan, &tasks).unwrap();
    let report = report(&record, &folder).unwrap();
    assert_eq!(report.copied, 1);
    assert_eq!(report.failed[0].package, "bad");
    assert_eq!(record::load(&record).unwrap()[0].name, "good");
}
