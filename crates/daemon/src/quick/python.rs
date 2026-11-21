//! Python for notebooks, in Getting Ready (Huan, 2026-11-19;
//! `docs/design/python-ready-plan.md`): the Python kernels Sandhill finds,
//! the mamba (else conda) it would use, the conda environments with Python
//! but no ipykernel, and the job that adds ipykernel, recorded (`record`)
//! so Sandhill can later remove exactly what it added. New environments
//! and Miniforge are planned in `runtime_plan`.

use std::path::{Path, PathBuf};

use crate::kernelspecs::{self, Search};
use crate::terminal::Launch;
use anyhow::{Result, bail};
use sandhill_proto::{
    QuickCondaEnv, QuickCondaTool, QuickPython, QuickPythonFound, ipykernel_args,
};

/// What Getting Ready shows about Python.
pub fn readiness(search: &Search) -> QuickPython {
    let listed: Vec<_> = kernelspecs::list(search)
        .into_iter()
        .filter(|k| k.language.to_ascii_lowercase().starts_with("python"))
        .collect();
    let found = listed.first().and_then(|k| {
        let spec = kernelspecs::find(search, &k.name)?;
        let program = kernelspecs::program(&spec.kernelspec, search.path.as_deref())?;
        found(&program, k.version.clone(), search.own.as_deref())
    });
    let mut kernels: Vec<_> = listed.into_iter().map(|k| k.display_name).collect();
    kernels.dedup();
    QuickPython {
        kernels,
        tool: find_tool(search).map(|(path, root)| QuickCondaTool {
            version: tool_version(&path),
            path: path.to_string_lossy().into_owned(),
            root: root.to_string_lossy().into_owned(),
        }),
        bare: bare_envs(search),
        found,
    }
}

pub(super) fn found(
    program: &Path,
    listed_version: Option<String>,
    own: Option<&Path>,
) -> Option<QuickPythonFound> {
    let around = program.parent()?.parent()?;
    let prefix = if around.join("conda-meta").is_dir() || around.join("pyvenv.cfg").is_file() {
        around.to_path_buf()
    } else {
        let result =
            super::install::probe_in(program, &["-I", "-c", "import sys; print(sys.prefix)"], own)?;
        let prefix = PathBuf::from(result.trim());
        if !prefix.is_absolute() || !prefix.is_dir() {
            return None;
        }
        prefix
    };
    let (description, fallback_version) = describe_python(program, &prefix);
    Some(QuickPythonFound {
        program: program.to_string_lossy().into_owned(),
        prefix: prefix.to_string_lossy().into_owned(),
        version: listed_version.or(fallback_version).unwrap_or_default(),
        description,
    })
}

pub(super) fn describe_python(program: &Path, prefix: &Path) -> (String, Option<String>) {
    if prefix.join("conda-meta").is_dir() {
        return (
            format!("conda, {}", kernelspecs::conda_name(prefix)),
            conda_package_version(prefix, "python"),
        );
    }
    if let Ok(config) = std::fs::read_to_string(prefix.join("pyvenv.cfg")) {
        let uv = config.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(key, _)| key.trim() == "uv")
        });
        return (if uv { "uv" } else { "venv" }.into(), None);
    }
    let real = kernelspecs::canonical(program);
    let name = if ["/opt/homebrew", "/usr/local/Cellar", "/home/linuxbrew"]
        .iter()
        .any(|dir| real.starts_with(dir))
    {
        "Homebrew"
    } else if real.starts_with("/Library/Frameworks/Python.framework") {
        "python.org"
    } else {
        "other"
    };
    (name.into(), None)
}

/// The mamba Getting Ready uses, else conda, and the installation it
/// belongs to: from the usual conda installations first, then `PATH`.
pub(super) fn find_tool(search: &Search) -> Option<(PathBuf, PathBuf)> {
    if let Some(folder) = &search.own {
        let shim = folder.join("bin/micromamba");
        return crate::own_env::executable(&shim).then(|| (shim, folder.join("conda")));
    }
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
pub(super) fn tool_version(tool: &Path) -> String {
    super::install::probe(tool, &["--version"])
        .and_then(|out| sandhill_proto::version_in(out.lines().next()?))
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

/// The job that adds ipykernel to an environment Getting Ready listed as
/// having none, and records the version it added.
pub fn add_ipykernel(
    exe: &Path,
    search: &Search,
    prefix: &str,
    log: &Path,
    record_path: &Path,
) -> Result<Launch> {
    let Some(env) = bare_envs(search).into_iter().find(|e| e.prefix == prefix) else {
        bail!("{prefix} isn't an environment without ipykernel that Sandhill found");
    };
    let plan = sandhill_proto::PackagePlan {
        change: sandhill_proto::PackageChange {
            action: sandhill_proto::PackageAction::Install,
            target: sandhill_proto::InstallTarget::Python {
                prefix: prefix.into(),
                python: format!("{prefix}/bin/python"),
            },
            packages: vec!["ipykernel".into()],
            tool: Some(sandhill_proto::InstallTool::Conda),
            reason: "Python needs it to run in Sandhill".into(),
        },
        tools: vec![sandhill_proto::InstallToolInfo {
            tool: sandhill_proto::InstallTool::Conda,
            path: env.tool.clone(),
            version: String::new(),
        }],
        folder: env.prefix,
        packages: vec![sandhill_proto::PlannedPackage {
            name: "ipykernel".into(),
            spec: "ipykernel".into(),
            source: "conda-forge".into(),
            version: None,
        }],
        command: super::install::quote(ipykernel_args(&env.tool, prefix)),
        notes: vec![],
        refused: None,
    };
    super::install::launch(
        exe,
        record_path,
        super::install::Job::Packages(plan),
        log,
        &format!("Add ipykernel to {}", env.name),
    )
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

    #[test]
    fn own_and_existing_python_prefixes_describe_where_the_kernel_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let conda = tmp.path().join("micromamba");
        installation(&conda, &[("omicverse", true)]);
        let prefix = conda.join("envs/omicverse");
        let program = prefix.join("bin/python");
        let env = found(&program, None, None).unwrap();
        assert_eq!(env.description, "conda, omicverse");
        assert_eq!(env.prefix, prefix.to_str().unwrap());
        assert_eq!(env.program, program.to_str().unwrap());
        assert_eq!(env.version, "3.12.4");

        let prefix = tmp.path().join("project/.venv");
        std::fs::create_dir_all(&prefix).unwrap();
        let cfg = prefix.join("pyvenv.cfg");
        std::fs::write(&cfg, "version_info = 3.12.4\nuv = 0.9.0").unwrap();
        let env = found(&prefix.join("bin/python"), Some("3.12.4".into()), None).unwrap();
        assert_eq!(env.description, "uv");
        std::fs::write(&cfg, "version = 3.12.4").unwrap();
        assert_eq!(
            describe_python(&prefix.join("bin/python"), &prefix).0,
            "venv"
        );
        assert_eq!(
            describe_python(Path::new("/opt/homebrew/bin/python3"), &prefix.join("else")).0,
            "Homebrew"
        );
        assert_eq!(
            describe_python(
                Path::new("/Library/Frameworks/Python.framework/bin/python3"),
                &prefix.join("else")
            )
            .0,
            "python.org"
        );
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
}
