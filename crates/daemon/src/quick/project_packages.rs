//! Read, save and restore the package files a project carries.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use jupyter_protocol::JupyterKernelspec;
use sandhill_proto::{
    InstallTarget, InstallTool, PackageAction, PackageChange, PackagePlan, PlannedPackage,
    ProjectFilePlan, ProjectPackage, ProjectPackageFile, ProjectSetupPlan, SetupStep,
};
use serde::{Deserialize, Serialize};

use super::{
    env_actions, install, package_inventory as inventory, package_names, package_plan,
    package_target, python, record,
};
use crate::{kernelspecs::Search, own_env};
use inventory::Source;

const CRAN: &str = "https://cloud.r-project.org";

mod setup;
pub(crate) use setup::{Work, list, prepare, run};
const FILES: [&str; 4] = [
    "renv.lock",
    "environment.yml",
    "environment.yaml",
    "requirements.txt",
];
const MAX_PACKAGE_FILE: u64 = 8 * 1024 * 1024;

pub(crate) struct Runtime {
    pub program: PathBuf,
    pub spec: JupyterKernelspec,
}

fn add_step(plan: &mut ProjectSetupPlan, what: &str, command: String) {
    plan.steps.push(SetupStep {
        what: what.into(),
        command,
    });
}

#[derive(Deserialize, Serialize)]
struct Lock {
    #[serde(rename = "R", default)]
    r: LockR,
    #[serde(
        rename = "Bioconductor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    bioconductor: Option<LockBioc>,
    #[serde(rename = "Packages", default)]
    packages: BTreeMap<String, LockPackage>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LockR {
    #[serde(default)]
    version: String,
    #[serde(default)]
    repositories: Vec<Repository>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Repository {
    name: String,
    #[serde(rename = "URL")]
    url: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LockBioc {
    version: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LockPackage {
    #[serde(default)]
    package: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_subdir: Option<String>,
}

fn lock_file(
    version: &str,
    packages: &BTreeMap<String, inventory::RPackage>,
    names: BTreeSet<String>,
    plan: &mut ProjectFilePlan,
) -> Result<String> {
    let mut lock = Lock {
        r: LockR {
            version: version.into(),
            repositories: vec![Repository {
                name: "CRAN".into(),
                url: CRAN.into(),
            }],
        },
        bioconductor: None,
        packages: BTreeMap::new(),
    };
    let mut seen = HashSet::new();
    let mut pending: Vec<_> = names.iter().map(String::as_str).collect();
    while let Some(name) = pending.pop() {
        if !seen.insert(name) || package_target::R_BASE.contains(&name) {
            continue;
        }
        let Some(pkg) = packages.get(name) else {
            plan.notes
                .push(format!("{name} isn't installed in this project's R."));
            continue;
        };
        if pkg.base {
            continue;
        }
        pending.extend(pkg.dependencies.iter().map(String::as_str));
        let mut locked = LockPackage {
            package: name.into(),
            version: pkg.version.clone(),
            ..Default::default()
        };
        match &pkg.source {
            Source::Cran => {
                locked.source = "Repository".into();
                locked.repository = Some("CRAN".into());
            }
            Source::Bioc => {
                locked.source = "Bioconductor".into();
                if lock.bioconductor.is_none() {
                    let version = packages
                        .get("BiocVersion")
                        .and_then(|p| major_minor(&p.version))
                        .or_else(|| bioc_version(version));
                    if let Some(version) = version {
                        lock.bioconductor = Some(LockBioc {
                            version: version.into(),
                        });
                    } else {
                        plan.notes.push("The Bioconductor release couldn't be found. Check it before sharing this file.".into());
                    }
                }
            }
            Source::Github(repo) => {
                locked.source = "GitHub".into();
                locked.remote_type = Some("github".into());
                locked.remote_host = Some("api.github.com".into());
                locked.remote_username = Some(repo.owner.clone());
                locked.remote_repo = Some(repo.repo.clone());
                locked.remote_sha = Some(repo.sha.clone());
                locked.remote_subdir.clone_from(&repo.subdir);
            }
            _ => {
                plan.notes.push(format!(
                    "{name} wasn't saved because R did not record where to get it."
                ));
                continue;
            }
        }
        lock.packages.insert(name.into(), locked);
        plan.packages.push(ProjectPackage {
            name: name.into(),
            version: pkg.version.clone(),
            source: pkg.source.label().into(),
        });
    }
    plan.packages.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    plan.notes.sort_unstable();
    Ok(serde_json::to_string_pretty(&lock)? + "\n")
}

fn major_minor(version: &str) -> Option<&str> {
    let (major, rest) = version.split_once('.')?;
    let minor = rest.split('.').next()?;
    (!major.is_empty()
        && !minor.is_empty()
        && major
            .bytes()
            .chain(minor.bytes())
            .all(|c| c.is_ascii_digit()))
    .then(|| &version[..major.len() + 1 + minor.len()])
}

fn bioc_version(r: &str) -> Option<&'static str> {
    Some(match major_minor(r)? {
        "4.0" => "3.12",
        "4.1" => "3.14",
        "4.2" => "3.16",
        "4.3" => "3.18",
        "4.4" => "3.20",
        "4.5" => "3.22",
        "4.6" => "3.24",
        _ => return None,
    })
}

#[derive(Default, Deserialize, Serialize)]
struct CondaFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    dependencies: Vec<CondaDependency>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum CondaDependency {
    Conda(String),
    Pip { pip: Vec<String> },
}

pub(crate) fn project_name(project: &Path) -> String {
    let name = project
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project");
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches(['.', '-', '_']);
    if name.is_empty() {
        "project".into()
    } else {
        name.into()
    }
}

fn python_file(
    project: &Path,
    env: &PythonEnv,
    imports: BTreeSet<String>,
    plan: &mut ProjectFilePlan,
) -> Result<String> {
    let installed = inventory::python_packages(&env.prefix, env.sites.clone(), true);
    let mut selected = BTreeSet::new();
    for module in imports {
        if env.info.as_ref().is_some_and(|info| {
            info.stdlib.contains(&module)
                || info.stdlib_path.join(format!("{module}.py")).is_file()
                || info.stdlib_path.join(&module).is_dir()
        }) || project.join(format!("{module}.py")).is_file()
            || project.join(&module).is_dir()
        {
            continue;
        }
        let dist = record::python_name(package_names::module_distribution(&module));
        let keys: Vec<_> = installed
            .iter()
            .filter(|(_, package)| package.imports.contains(&module))
            .map(|(key, _)| key)
            .collect();
        if !keys.is_empty() {
            selected.extend(keys);
        } else if let Some((key, _)) = installed.get_key_value(&dist) {
            selected.insert(key);
        } else {
            plan.notes.push(format!(
                "{module} isn't installed in this project's Python."
            ));
        }
    }
    let mut conda = Vec::new();
    let mut pip = Vec::new();
    if env.conda {
        let version = env
            .info
            .as_ref()
            .and_then(|i| i.version.strip_prefix("python"))
            .or_else(|| {
                env.sites
                    .iter()
                    .find_map(|s| s.parent()?.file_name()?.to_str()?.strip_prefix("python"))
            })
            .context("Python's version couldn't be found.")?;
        conda.push(CondaDependency::Conda(format!("python={version}")));
        plan.packages.push(ProjectPackage {
            name: "python".into(),
            version: version.into(),
            source: "conda-forge".into(),
        });
    }
    for key in selected {
        let package = &installed[key];
        if matches!(package.source, Source::Local | Source::Unknown) {
            plan.notes.push(format!(
                "{} wasn't saved because Python did not record a source it can be installed from.",
                package.name
            ));
            continue;
        }
        let (name, version, source) = match (&package.conda, env.conda) {
            (Some((name, version)), true) => {
                conda.push(CondaDependency::Conda(format!("{name}={version}")));
                (name.as_str(), version.as_str(), package.source.label())
            }
            _ => {
                let name = package_names::distribution(&package.name, InstallTool::Pip);
                pip.push(format!("{name}=={}", package.version));
                if matches!(package.source, Source::Github(_)) {
                    plan.notes.push(format!("{} was installed from GitHub. Check that this version is also on PyPI before sharing this file.", package.name));
                }
                (name, package.version.as_str(), "PyPI")
            }
        };
        plan.packages.push(ProjectPackage {
            name: name.into(),
            version: version.into(),
            source: source.into(),
        });
    }
    if !env.conda {
        return Ok(if pip.is_empty() {
            String::new()
        } else {
            pip.join("\n") + "\n"
        });
    }
    if !pip.is_empty() {
        conda.push(CondaDependency::Conda("pip".into()));
        conda.push(CondaDependency::Pip { pip });
    }
    Ok(serde_yaml_ng::to_string(&CondaFile {
        name: Some(project_name(project)),
        channels: vec!["conda-forge".into(), "bioconda".into()],
        dependencies: conda,
    })?)
}

struct PythonEnv {
    prefix: PathBuf,
    conda: bool,
    sites: Vec<PathBuf>,
    info: Option<std::sync::Arc<package_target::PythonInfo>>,
}

fn python_env(runtime: &Runtime, own: Option<&Path>) -> Result<PythonEnv> {
    let program = &runtime.program;
    let location = program
        .parent()
        .and_then(Path::parent)
        .context("Python's folder couldn't be found.")?;
    let info = package_target::python_info(program, false);
    let venv = fs::read_to_string(location.join("pyvenv.cfg")).ok();
    let conda = location.join("conda-meta").is_dir() || own.is_some();
    let prefix = if conda || venv.is_some() {
        location
    } else {
        info.as_ref().map_or(location, |i| i.prefix.as_path())
    };
    let mut sites = inventory::prefix_sites(prefix);
    if let Some(info) = &info
        && (venv.is_none()
            || venv.as_deref().is_some_and(|text| {
                text.lines().any(|line| {
                    line.split_once('=').is_some_and(|(key, value)| {
                        key.trim() == "include-system-site-packages"
                            && value.trim().eq_ignore_ascii_case("true")
                    })
                })
            }))
    {
        if own.is_none()
            && !conda
            && !runtime
                .spec
                .argv
                .iter()
                .any(|arg| arg == "-I" || arg == "-s")
            && !runtime
                .spec
                .env
                .as_ref()
                .is_some_and(|vars| vars.contains_key("PYTHONNOUSERSITE"))
            && std::env::var_os("PYTHONNOUSERSITE").is_none()
        {
            if venv.is_some() {
                sites.push(info.user_site.clone());
            } else {
                sites.insert(0, info.user_site.clone());
            }
        }
        sites.extend(info.sites.iter().cloned());
    }
    if let Some(own) = own {
        sites.retain(|p| own_env::inside(own, p));
    }
    Ok(PythonEnv {
        prefix: prefix.into(),
        conda,
        sites,
        info,
    })
}

pub(crate) fn file_plan(
    project: &Path,
    search: &Search,
    language: &str,
    trusted: bool,
    runtime: impl FnOnce() -> Result<Runtime>,
    imports: impl FnOnce() -> BTreeSet<String>,
) -> ProjectFilePlan {
    let mut plan = ProjectFilePlan {
        project: project.to_string_lossy().into_owned(),
        language: language.into(),
        path: String::new(),
        replaces: false,
        packages: Vec::new(),
        contents: String::new(),
        notes: Vec::new(),
        refused: None,
    };
    let result = (|| -> Result<()> {
        ensure!(trusted, "Trust this project before saving its packages.");
        let runtime = runtime()?;
        let own = search.own.as_deref();
        let (basename, contents) = match language {
            "r" => {
                let mut r = inventory::r_runtime(
                    &runtime.program,
                    Some(&runtime.spec),
                    Some(project),
                    own,
                )?;
                let library = renv_root(project, own);
                let lib = r_library(&library, &r);
                if lib.is_dir() {
                    r.libraries.insert(
                        0,
                        sandhill_proto::RLibrary {
                            path: lib.to_string_lossy().into_owned(),
                            writable: false,
                            packages: 0,
                        },
                    );
                }
                (
                    "renv.lock",
                    lock_file(
                        &r.version,
                        &inventory::r_packages(&r.libraries),
                        imports(),
                        &mut plan,
                    )?,
                )
            }
            "python" => {
                let env = python_env(&runtime, own)?;
                let yaml = if project.join("environment.yaml").is_file()
                    && !project.join("environment.yml").exists()
                {
                    "environment.yaml"
                } else {
                    "environment.yml"
                };
                (
                    if env.conda { yaml } else { "requirements.txt" },
                    python_file(project, &env, imports(), &mut plan)?,
                )
            }
            _ => bail!("Choose R or Python."),
        };
        let path = project.join(basename);
        plan.replaces = path.symlink_metadata().is_ok();
        if plan.replaces {
            ensure!(
                path.symlink_metadata().is_ok_and(|meta| meta.is_file()),
                "There is already a folder or shortcut with this file name."
            );
        }
        plan.path = path.to_string_lossy().into_owned();
        plan.contents = contents;
        Ok(())
    })();
    if let Err(error) = result {
        plan.refused = Some(error.to_string());
    }
    plan
}

pub(crate) fn write(plan: &ProjectFilePlan) -> Result<()> {
    let path = Path::new(&plan.path);
    let root = Path::new(&plan.project);
    ensure!(
        plan.refused.is_none() && path.parent() == Some(root),
        "The package file isn't ready to save."
    );
    if plan.replaces {
        ensure!(
            path.symlink_metadata().is_ok_and(|meta| meta.is_file()),
            "The package file changed. Review it again before saving."
        );
    }
    let mut temp = tempfile::NamedTempFile::new_in(root)?;
    if let Ok(meta) = path.symlink_metadata()
        && meta.is_file()
    {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    temp.write_all(plan.contents.as_bytes())?;
    temp.as_file().sync_all()?;
    if plan.replaces {
        temp.persist(path)?;
    } else {
        temp.persist_noclobber(path)
            .context("The package file changed. Review it again before saving.")?;
    }
    Ok(())
}

fn read_file(project: &Path, file: &str) -> Result<String> {
    ensure!(
        FILES.contains(&file),
        "Choose a package file in the project's folder."
    );
    let path = project.join(file);
    use std::os::unix::fs::OpenOptionsExt as _;
    ensure!(
        path.symlink_metadata().is_ok_and(|m| m.is_file()),
        "{file} isn't a regular file in this project."
    );
    let mut text = String::new();
    let input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    ensure!(
        input.metadata()?.is_file(),
        "Choose a regular package file."
    );
    input.take(MAX_PACKAGE_FILE + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= MAX_PACKAGE_FILE,
        "{file} is too large to review."
    );
    Ok(text)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Store {
    R,
    Conda,
    Pip,
}

struct Pin {
    package: PlannedPackage,
    store: Store,
    minor: bool,
    unconditional: bool,
}

fn parse_pip(line: &str) -> Option<Pin> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let spec = line.split_once(" #").map_or(line, |(s, _)| s).trim();
    let name_end = spec
        .find(|c: char| c.is_whitespace() || "[<>=!~;@".contains(c))
        .unwrap_or(spec.len());
    let name = &spec[..name_end];
    let name = if package_plan::python_spec(name) == Some(name) {
        name
    } else {
        spec
    };
    let marker = spec.contains(';');
    let version = spec
        .split([';', ' '])
        .next()?
        .split_once("==")
        .and_then(|(_, version)| inventory::version(version).then(|| version.into()));
    let source = if spec.contains("github.com/") {
        "GitHub"
    } else if spec.contains(" @ ") || spec.starts_with(['.', '/', '-']) {
        "From the package file"
    } else {
        "PyPI"
    };
    Some(Pin {
        package: PlannedPackage {
            name: name.into(),
            spec: spec.into(),
            source: source.into(),
            version,
        },
        store: Store::Pip,
        minor: false,
        unconditional: !marker,
    })
}

fn parse_conda(line: &str, channel: &str) -> Pin {
    let (source, spec) = line.split_once("::").unwrap_or((channel, line));
    let name = spec
        .split(['=', '<', '>', '!', ' ', '[', '~'])
        .next()
        .unwrap_or(spec);
    let first = spec[name.len()..].trim_start();
    let version = first
        .strip_prefix('=')
        .map(|s| s.trim_start_matches('='))
        .and_then(|s| s.split(['=', ' ']).next())
        .filter(|s| inventory::version(s));
    let minor =
        name == "python" && version.is_some_and(|v| v.bytes().filter(|&b| b == b'.').count() == 1);
    Pin {
        package: PlannedPackage {
            name: name.into(),
            spec: line.into(),
            source: source.into(),
            version: version.map(str::to_owned),
        },
        store: Store::Conda,
        minor,
        unconditional: true,
    }
}

struct FilePins {
    pins: Vec<Pin>,
    conda_name: Option<String>,
    r_version: Option<String>,
    notes: Vec<String>,
}

fn parse(file: &str, text: &str) -> Result<FilePins> {
    let mut parsed = FilePins {
        pins: Vec::new(),
        conda_name: None,
        r_version: None,
        notes: Vec::new(),
    };
    match file {
        "renv.lock" => {
            let lock: Lock = serde_json::from_str(text)
                .context("Sandhill couldn't read this renv.lock file.")?;
            parsed.r_version = (!lock.r.version.is_empty()).then_some(lock.r.version);
            for (name, pkg) in lock.packages {
                ensure!(
                    record::r_name(&name) && (pkg.package.is_empty() || pkg.package == name),
                    "renv.lock has an invalid package name."
                );
                let source = match pkg.source.as_str() {
                    "Repository" => pkg.repository.as_deref().unwrap_or("R package repository"),
                    "" => "No source recorded",
                    source => source,
                };
                if !matches!(
                    pkg.source.as_str(),
                    "Repository" | "Bioconductor" | "GitHub" | "GitLab" | "Bitbucket" | "URL"
                ) {
                    parsed.notes.push(format!("{name}: renv may not be able to find this package from the recorded source."));
                }
                if pkg.source == "GitHub" && pkg.remote_sha.is_none() {
                    parsed
                        .notes
                        .push(format!("{name}: no GitHub commit was pinned."));
                }
                parsed.pins.push(Pin {
                    package: PlannedPackage {
                        spec: name.clone(),
                        name,
                        source: source.into(),
                        version: (!pkg.version.is_empty()).then_some(pkg.version),
                    },
                    store: Store::R,
                    minor: false,
                    unconditional: true,
                });
            }
        }
        "requirements.txt" => {
            parsed.pins.extend(text.lines().filter_map(parse_pip));
        }
        _ => {
            let env: CondaFile = serde_yaml_ng::from_str(text)
                .context("Sandhill couldn't read this conda environment file.")?;
            parsed.conda_name = env.name;
            let source = env.channels.first().map_or("conda", String::as_str);
            for dependency in env.dependencies {
                match dependency {
                    CondaDependency::Conda(spec) => parsed.pins.push(parse_conda(&spec, source)),
                    CondaDependency::Pip { pip } => parsed
                        .pins
                        .extend(pip.iter().filter_map(|line| parse_pip(line))),
                }
            }
        }
    }
    for p in &parsed.pins {
        if p.package.version.is_none() {
            parsed.notes.push(format!(
                "{} has no exact version in this file.",
                p.package.name
            ));
        }
        if !p.unconditional {
            parsed.notes.push(format!(
                "{} is installed only when the condition in the file applies.",
                p.package.name
            ));
        }
    }
    Ok(parsed)
}

pub(crate) fn renv_root(project: &Path, own: Option<&Path>) -> PathBuf {
    if let Some(folder) = own {
        let canonical = project.canonicalize().unwrap_or_else(|_| project.into());
        let digest = record::digest(canonical.as_os_str().as_encoded_bytes());
        folder
            .join("renv/library")
            .join(format!("{}-{}", project_name(&canonical), &digest[..16]))
    } else {
        project.join("renv/library")
    }
}

fn r_library(root: &Path, runtime: &inventory::RRuntime) -> PathBuf {
    let minor = major_minor(&runtime.version).unwrap_or(&runtime.version);
    let path = PathBuf::from(format!("R-{minor}")).join(&runtime.platform);
    // Newer renv versions can prefix the platform with the host's distribution.
    let direct = root.join(&path);
    if direct.is_dir() {
        return direct;
    }
    let mut candidates: Vec<_> = package_target::subdirs(root)
        .map(|p| p.join(&path))
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort_unstable();
    candidates.into_iter().next().unwrap_or(direct)
}

fn conda_tool(search: &Search) -> Result<(PathBuf, PathBuf)> {
    python::find_tool(search)
        .or_else(|| {
            if search.own.is_some() {
                return None;
            }
            ["mamba", "conda", "micromamba"]
                .into_iter()
                .find_map(|name| {
                    let program = install::on_path(search, name)?;
                    let root = env_actions::conda_root(program.to_str()?)?;
                    Some((program, root))
                })
        })
        .context("No mamba or conda tool was found. Set one up first on the R and Python page.")
}

fn conda_destination(project: &Path, file: &str, env: &FilePins, root: &Path) -> Result<PathBuf> {
    let name = match file {
        "requirements.txt" => format!(
            "{}-{}-python",
            project_name(project),
            &record::digest(project.as_os_str().as_encoded_bytes())[..8]
        ),
        _ => env
            .conda_name
            .clone()
            .unwrap_or_else(|| project_name(project)),
    };
    ensure!(
        !matches!(name.as_str(), "base" | "sandhill-r" | "sandhill-python")
            && name.starts_with(|c: char| c.is_ascii_alphanumeric())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "Choose a different conda environment name in {file}."
    );
    Ok(root.join("envs").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::test_common::tempdir;

    fn plan(root: &Path, language: &str) -> ProjectFilePlan {
        ProjectFilePlan {
            project: root.display().to_string(),
            language: language.into(),
            path: String::new(),
            replaces: false,
            packages: Vec::new(),
            contents: String::new(),
            notes: Vec::new(),
            refused: None,
        }
    }

    #[test]
    fn writes_exact_renv_lock_with_recursive_dependencies() {
        let tmp = tempdir().unwrap();
        let library = tmp.path().join("library");
        for (name, version, extra) in [
            ("A", "1.0", "Repository: CRAN\nImports: B (>= 1), base\n"),
            (
                "B",
                "2.1",
                "biocViews: Data\nLinkingTo: C\nDepends: R (>= 4.0)\n",
            ),
            (
                "C",
                "3.2",
                "RemoteType: github\nRemoteUsername: someone\nRemoteRepo: c-package\nRemoteSha: 123456789abcdef\nDepends: A, methods\n",
            ),
            ("base", "4.5.2", "Priority: base\n"),
        ] {
            let package = library.join(name);
            fs::create_dir_all(&package).unwrap();
            fs::write(
                package.join("DESCRIPTION"),
                format!("Package: {name}\nVersion: {version}\n{extra}"),
            )
            .unwrap();
        }
        let packages = inventory::r_packages(&[sandhill_proto::RLibrary {
            path: library.display().to_string(),
            writable: true,
            packages: 4,
        }]);
        let mut plan = plan(tmp.path(), "r");
        let contents = lock_file(
            "4.5.2",
            &packages,
            BTreeSet::from(["A".into(), "Missing".into()]),
            &mut plan,
        )
        .unwrap();
        assert_eq!(
            contents,
            include_str!("../../tests/fixtures/project-renv.lock")
        );
        assert_eq!(
            plan.packages
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
        assert_eq!(plan.notes, ["Missing isn't installed in this project's R."]);
    }

    #[test]
    fn writes_python_requirements_and_conda_yaml() {
        let tmp = tempdir().unwrap();
        let prefix = tmp.path().join("prefix");
        let site = prefix.join("lib/python3.12/site-packages");
        for (name, installer, module) in [("NumPy", "conda", "numpy"), ("Pillow", "pip", "PIL")] {
            let dist = site.join(format!("{name}-1.0.dist-info"));
            fs::create_dir_all(&dist).unwrap();
            fs::write(
                dist.join("METADATA"),
                format!("Name: {name}\nVersion: 1.0\n"),
            )
            .unwrap();
            fs::write(dist.join("INSTALLER"), installer).unwrap();
            fs::write(dist.join("top_level.txt"), module).unwrap();
        }
        let meta = prefix.join("conda-meta");
        fs::create_dir(&meta).unwrap();
        fs::write(
            meta.join("numpy-1.0-build.json"),
            r#"{"name":"numpy","version":"1.0","channel":"conda-forge"}"#,
        )
        .unwrap();
        let mut env = PythonEnv {
            prefix,
            conda: true,
            sites: vec![site],
            info: None,
        };
        let mut preview = plan(tmp.path(), "python");
        let code = BTreeSet::from(["PIL".into(), "numpy".into(), "notinstalled".into()]);
        let yaml = python_file(tmp.path(), &env, code.clone(), &mut preview).unwrap();
        let contents: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(
            contents["channels"],
            serde_yaml_ng::to_value(["conda-forge", "bioconda"]).unwrap()
        );
        assert_eq!(contents["dependencies"][0], "python=3.12");
        assert_eq!(contents["dependencies"][1], "numpy=1.0");
        assert_eq!(contents["dependencies"][3]["pip"][0], "pillow==1.0");
        assert_eq!(
            preview.notes,
            ["notinstalled isn't installed in this project's Python."]
        );
        env.conda = false;
        let requirements =
            python_file(tmp.path(), &env, code, &mut plan(tmp.path(), "python")).unwrap();
        assert_eq!(requirements, "NumPy==1.0\npillow==1.0\n");
    }
}
