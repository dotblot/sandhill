//! Python for notebooks, in Getting Ready (Huan, 2026-11-19;
//! `docs/design/python-ready-plan.md`): the Python kernels Sandhill finds,
//! the mamba (else conda) it would use, the conda environments with Python
//! but no ipykernel, and the jobs that create an environment, add
//! ipykernel or install Miniforge, each from conda-forge and recorded
//! (`record`) so Sandhill can later remove exactly what it made.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    JobSpec, PYTHON_SETS, PYTHON_VERSIONS, QuickCondaEnv, QuickCondaTool, QuickPython,
    ipykernel_args, python_create_args, python_env_name_ok,
};

use super::record;
use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search};
use crate::terminal::Launch;

/// Where Install Miniforge puts it, under the home folder.
const MINIFORGE_DIR: &str = "miniforge3";

/// What Getting Ready shows about Python.
pub fn readiness(search: &Search) -> QuickPython {
    let mut kernels: Vec<String> = kernelspecs::list(search)
        .into_iter()
        .filter(|k| k.language.to_ascii_lowercase().starts_with("python"))
        .map(|k| k.display_name)
        .collect();
    kernels.dedup();
    QuickPython {
        kernels,
        tool: find_tool(search).map(|(path, root)| QuickCondaTool {
            version: tool_version(&path),
            path: path.to_string_lossy().into_owned(),
            root: root.to_string_lossy().into_owned(),
        }),
        bare: bare_envs(search),
    }
}

/// The mamba Getting Ready uses, else conda, and the installation it
/// belongs to: from the usual conda installations first, then `PATH`.
fn find_tool(search: &Search) -> Option<(PathBuf, PathBuf)> {
    let mut roots: Vec<PathBuf> = search
        .conda_roots
        .iter()
        .filter(|root| root.join("conda-meta").is_dir())
        .map(|root| kernelspecs::conda_root(root).to_path_buf())
        .collect();
    for program in ["mamba", "conda"] {
        if let Some(found) = search.path.as_ref().and_then(|path| {
            std::env::split_paths(path)
                .map(|dir| dir.join(program))
                .find(|p| p.is_file())
        }) && let Some(root) = found.parent().and_then(Path::parent)
            && root.join("conda-meta").is_dir()
        {
            roots.push(root.to_path_buf());
        }
    }
    for program in ["bin/mamba", "bin/conda", "condabin/conda"] {
        if let Some(root) = roots.iter().find(|root| root.join(program).is_file()) {
            return Some((root.join(program), root.clone()));
        }
    }
    None
}

/// `2.10.0` from `mamba --version`, or conda's `conda 25.1.1`.
fn tool_version(tool: &Path) -> String {
    Command::new(tool)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|out| {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            sandhill_proto::version_in(text.lines().next()?)
        })
        .unwrap_or_default()
}

/// Whether ipykernel is in the environment, from conda or pip.
fn has_ipykernel(prefix: &Path) -> bool {
    conda_package_version(prefix, "ipykernel").is_some()
        || std::fs::read_dir(prefix.join("lib")).is_ok_and(|dirs| {
            dirs.flatten().any(|d| {
                d.file_name().to_string_lossy().starts_with("python")
                    && d.path().join("site-packages/ipykernel").is_dir()
            })
        })
}

/// The version of a conda package in an environment, from `conda-meta`.
pub(crate) fn conda_package_version(prefix: &Path, package: &str) -> Option<String> {
    let start = format!("{package}-");
    std::fs::read_dir(prefix.join("conda-meta"))
        .ok()?
        .flatten()
        .find_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name.strip_prefix(&start)?.strip_suffix(".json")?;
            // `ipykernel-7.2.0-pyh5552912_0.json`: the version, then the build.
            let (version, _build) = rest.rsplit_once('-')?;
            version
                .starts_with(|c: char| c.is_ascii_digit())
                .then(|| version.to_string())
        })
}

/// Conda environments (not an installation's base) with Python but no
/// ipykernel.
fn bare_envs(search: &Search) -> Vec<QuickCondaEnv> {
    kernelspecs::conda_envs(search)
        .into_iter()
        .filter(|prefix| kernelspecs::conda_root(prefix) != prefix.as_path())
        .filter(|prefix| prefix.join("bin/python").is_file() && !has_ipykernel(prefix))
        .map(|prefix| QuickCondaEnv {
            name: kernelspecs::conda_name(&prefix),
            python: conda_package_version(&prefix, "python").unwrap_or_default(),
            tool: crate::irkernel::conda_tool(&prefix),
            prefix: prefix.to_string_lossy().into_owned(),
        })
        .collect()
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME isn't set")
}

/// A job running `command` with its output also in `log`.
fn job(command: String, log: &Path, title: &str) -> Result<Launch> {
    let job = JobSpec {
        path: None,
        command: Some(format!(
            "set -o pipefail; ( {command} ) 2>&1 | tee {}",
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&job, Some(&home()?), None)?;
    launch.title = title.into();
    Ok(launch)
}

fn quoted(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The job that creates the environment `name` in the installation
/// Getting Ready found, and records it.
pub fn create(
    search: &Search,
    name: &str,
    python: &str,
    sets: &[String],
    log: &Path,
    record_path: &Path,
) -> Result<Launch> {
    if !python_env_name_ok(name) {
        bail!("an environment's name can only have letters, digits, - and _");
    }
    if !PYTHON_VERSIONS.contains(&python) {
        bail!("Python {python} isn't one of the versions offered");
    }
    if let Some(set) = sets
        .iter()
        .find(|s| !PYTHON_SETS.iter().any(|(id, ..)| id == s))
    {
        bail!("{set} isn't a package set Getting Ready offers");
    }
    let (tool, root) = find_tool(search).context("there is no mamba or conda here")?;
    let prefix = root.join("envs").join(name);
    if prefix.exists() || kernelspecs::conda_env_exists(search, name) {
        bail!("an environment called {name} exists already");
    }
    let prefix = prefix.to_string_lossy().into_owned();
    let args = python_create_args(&tool.to_string_lossy(), &prefix, python, sets);
    let command = format!(
        "{} && {}",
        quoted(&args),
        record::sh_append(record_path, record::PYTHON_ENV, name, python, &prefix)
    );
    job(
        command,
        log,
        &format!("Create the Python environment {name}"),
    )
}

/// The job that adds ipykernel to an environment Getting Ready listed as
/// having none, and records the version it added.
pub fn add_ipykernel(
    search: &Search,
    prefix: &str,
    log: &Path,
    record_path: &Path,
) -> Result<Launch> {
    let Some(env) = bare_envs(search).into_iter().find(|e| e.prefix == prefix) else {
        bail!("{prefix} isn't an environment without ipykernel that Sandhill found");
    };
    let args = ipykernel_args(&env.tool, prefix);
    // The version comes from the environment once it's installed.
    let version = format!(
        "\"$(ls {} | sed -n 's/^ipykernel-\\(.*\\)-[^-]*\\.json$/\\1/p' | head -1)\"",
        shell_quote(&format!("{prefix}/conda-meta"))
    );
    let command = format!(
        "{} && {}",
        quoted(&args),
        record::sh_append_raw(
            record_path,
            record::IPYKERNEL,
            "ipykernel",
            &version,
            prefix
        )
    );
    job(command, log, &format!("Add ipykernel to {}", env.name))
}

/// The job that installs Miniforge into `~/miniforge3`, only when there is
/// no conda or mamba, and records it with how many packages its base had,
/// so it is removed later only if nothing was added.
pub fn install_miniforge(search: &Search, log: &Path, record_path: &Path) -> Result<Launch> {
    if let Some((tool, _)) = find_tool(search) {
        bail!("{} is here already", tool.display());
    }
    let root = home()?.join(MINIFORGE_DIR);
    if root.exists() {
        bail!("{} exists already", root.display());
    }
    let root = root.to_string_lossy().into_owned();
    let count = format!(
        "\"$(ls {} | grep -c '\\.json$')\"",
        shell_quote(&format!("{root}/conda-meta"))
    );
    let command = format!(
        "{} && {}",
        sandhill_proto::MINIFORGE_COMMAND,
        record::sh_append_raw(record_path, record::MINIFORGE, "miniforge", &count, &root)
    );
    job(command, log, "Install Miniforge")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conda installation at `root` with `envs`, each `(name, ipykernel)`.
    fn installation(root: &Path, envs: &[(&str, bool)]) {
        std::fs::create_dir_all(root.join("conda-meta")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/mamba"), "").unwrap();
        std::fs::write(root.join("bin/python"), "").unwrap();
        for (name, ipykernel) in envs {
            let prefix = root.join("envs").join(name);
            std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
            std::fs::create_dir_all(prefix.join("bin")).unwrap();
            std::fs::write(prefix.join("bin/python"), "").unwrap();
            std::fs::write(
                prefix.join("conda-meta/python-3.12.4-h99e199e_0_cpython.json"),
                "{}",
            )
            .unwrap();
            if *ipykernel {
                std::fs::write(
                    prefix.join("conda-meta/ipykernel-7.2.0-pyh5552912_0.json"),
                    "{}",
                )
                .unwrap();
            }
        }
    }

    fn search(root: &Path) -> Search {
        Search {
            conda_roots: vec![root.to_path_buf()],
            ..Search::default()
        }
    }

    #[test]
    fn finds_mamba_and_the_environments_without_ipykernel() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("miniforge3");
        installation(&root, &[("omicverse", true), ("plain", false)]);
        let search = search(&root);
        let (tool, found_root) = find_tool(&search).unwrap();
        assert_eq!(tool, root.join("bin/mamba"));
        assert_eq!(found_root, root);
        let bare = bare_envs(&search);
        assert_eq!(bare.len(), 1, "{bare:?}");
        assert_eq!(bare[0].name, "plain");
        assert_eq!(bare[0].python, "3.12.4");
        assert_eq!(
            conda_package_version(&root.join("envs/omicverse"), "ipykernel").as_deref(),
            Some("7.2.0")
        );
    }

    #[test]
    fn refuses_bad_names_versions_sets_and_existing_environments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("miniforge3");
        installation(&root, &[("taken", true)]);
        let search = search(&root);
        let record = dir.path().join("record.tsv");
        let log = dir.path().join("log");
        let err = |result: Result<Launch>| result.err().unwrap().to_string();
        assert!(err(create(&search, "a b", "3.12", &[], &log, &record)).contains("letters"));
        assert!(err(create(&search, "x", "2.7", &[], &log, &record)).contains("versions"));
        assert!(
            err(create(&search, "x", "3.12", &["r".into()], &log, &record)).contains("package set")
        );
        assert!(err(create(&search, "taken", "3.12", &[], &log, &record)).contains("exists"));
        assert!(
            err(add_ipykernel(&search, "/nowhere", &log, &record)).contains("without ipykernel")
        );
        let none = Search::default();
        assert!(err(create(&none, "x", "3.12", &[], &log, &record)).contains("no mamba"));
        assert!(err(install_miniforge(&search, &log, &record)).contains("here already"));
    }

    #[test]
    fn the_create_command_uses_conda_forge_only_and_the_chosen_sets() {
        let args = python_create_args("/m/bin/mamba", "/m/envs/x", "3.12", &["singlecell".into()]);
        let line = args.join(" ");
        assert!(line.starts_with("/m/bin/mamba create -y -p /m/envs/x -c conda-forge --override-channels python=3.12 ipykernel"));
        assert!(line.contains("scanpy") && !line.contains("seaborn"));
        assert!(
            ipykernel_args("mamba", "/p")
                .join(" ")
                .contains("--freeze-installed ipykernel")
        );
    }
}
