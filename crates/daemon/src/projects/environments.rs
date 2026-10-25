//! What a project's files say it runs in: `renv.lock` (R with the
//! project's own library), the environments inside its folder (`.venv`,
//! pixi's), `pixi.toml` and `uv.lock` before they are installed, and the
//! conda environment `environment.yml` names. Each is matched to the
//! installed kernel that runs in it; one without a kernel says what to do.

use std::path::Path;

use sandhill_proto::{FoundEnvironment, KernelspecInfo};

use crate::kernelspecs::{self, ProjectEnv, Search};

const R: &str = "r";
const PYTHON: &str = "python";

/// The environments `root`'s files point to, best first for each language.
/// `kernels` are the kernels installed, the project's own included.
pub fn find(root: &Path, kernels: &[KernelspecInfo], search: &Search) -> Vec<FoundEnvironment> {
    let mut found = Vec::new();
    let has = |name: &str| root.join(name).exists();
    let named = |name: &str, language: &str| {
        kernels
            .iter()
            .find(|k| k.name == name && language_of(k) == language)
            .map(|k| k.name.clone())
    };

    if has("renv.lock") {
        // renv isn't another R: R loads the project's library through its
        // `.Rprofile`, which it reads when it starts in the project's folder.
        let kernel = ["ark", "ir"]
            .iter()
            .find_map(|name| named(name, R))
            .or_else(|| {
                kernels
                    .iter()
                    .find(|k| language_of(k) == R && k.env.is_none())
                    .map(|k| k.name.clone())
            });
        found.push(FoundEnvironment {
            language: R.into(),
            file: "renv.lock".into(),
            label: "renv".into(),
            hint: kernel
                .is_none()
                .then(|| "No R kernel is installed: install Ark, or IRkernel in R.".into()),
            kernel,
        });
    }

    let uv = has("uv.lock") || mentions(&root.join("pyproject.toml"), "[tool.uv");
    let pixi = has("pixi.toml") || mentions(&root.join("pyproject.toml"), "[tool.pixi");
    let local = kernelspecs::project_envs(root);
    for env in &local {
        let python = named(&format!("{}-py", env.names), PYTHON);
        if python.is_some() || has_program(env, "python") {
            found.push(FoundEnvironment {
                language: PYTHON.into(),
                file: env.folder.clone(),
                label: env.label.clone(),
                hint: python
                    .is_none()
                    .then(|| install_hint(env, PYTHON, "ipykernel", uv)),
                kernel: python,
            });
        }
        let r = named(&format!("{}-r", env.names), R);
        if r.is_some() || has_program(env, "R") {
            found.push(FoundEnvironment {
                language: R.into(),
                file: env.folder.clone(),
                label: env.label.clone(),
                hint: r.is_none().then(|| install_hint(env, R, "r-irkernel", uv)),
                kernel: r,
            });
        }
    }
    let made = |folder: &str| local.iter().any(|env| env.folder.starts_with(folder));
    if pixi && !made(".pixi/") {
        found.push(FoundEnvironment {
            language: PYTHON.into(),
            file: if has("pixi.toml") { "pixi.toml" } else { "pyproject.toml" }.into(),
            label: "pixi default".into(),
            kernel: None,
            hint: Some("It isn't installed yet: run `pixi install` in the project's folder, with ipykernel among its dependencies.".into()),
        });
    }
    if uv && !made(".venv") {
        found.push(FoundEnvironment {
            language: PYTHON.into(),
            file: if has("uv.lock") { "uv.lock" } else { "pyproject.toml" }.into(),
            label: ".venv".into(),
            kernel: None,
            hint: Some("It isn't made yet: run `uv sync` in the project's folder, with ipykernel among its dependencies (`uv add --dev ipykernel`).".into()),
        });
    }

    for file in ["environment.yml", "environment.yaml"] {
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let Some(name) = conda_name(&text) else {
            continue;
        };
        let installed = kernels
            .iter()
            .any(|k| k.env.as_deref() == Some(name.as_str()))
            || kernelspecs::conda_env_exists(search, &name);
        let in_env = |language: &str| {
            kernels
                .iter()
                .find(|k| k.env.as_deref() == Some(name.as_str()) && language_of(k) == language)
                .map(|k| k.name.clone())
        };
        let hint = |language: &str, package: &str| {
            Some(if installed {
                format!(
                    "It has no {} kernel: `conda install -n {name} {package}`.",
                    shown(language)
                )
            } else {
                format!(
                    "It isn't installed: `conda env create -f {file}`, with {package} among its dependencies."
                )
            })
        };
        let python = in_env(PYTHON);
        found.push(FoundEnvironment {
            language: PYTHON.into(),
            file: file.into(),
            label: format!("conda env {name}"),
            hint: if python.is_some() {
                None
            } else {
                hint(PYTHON, "ipykernel")
            },
            kernel: python,
        });
        let r = in_env(R);
        if r.is_some()
            || ["r-base", "r-essentials", "r-irkernel"]
                .iter()
                .any(|p| lists(&text, p))
        {
            found.push(FoundEnvironment {
                language: R.into(),
                file: file.into(),
                label: format!("conda env {name}"),
                hint: if r.is_some() {
                    None
                } else {
                    hint(R, "r-irkernel")
                },
                kernel: r,
            });
        }
        break;
    }
    found
}

fn language_of(kernel: &KernelspecInfo) -> String {
    crate::cells::normalize_language(&kernel.language)
}

fn has_program(env: &ProjectEnv, program: &str) -> bool {
    env.prefix.join("bin").join(program).is_file()
}

fn shown(language: &str) -> &'static str {
    if language == R { "R" } else { "Python" }
}

fn install_hint(env: &ProjectEnv, language: &str, package: &str, uv: bool) -> String {
    let folder = &env.folder;
    let command = if folder.starts_with(".pixi/") {
        format!("pixi add {package}")
    } else if env.conda {
        format!("conda install -p {folder} {package}")
    } else if uv {
        format!("uv add --dev {package}")
    } else {
        format!("{folder}/bin/python -m pip install {package}")
    };
    format!("It has no {} kernel: `{command}`.", shown(language))
}

/// Whether a file has this text in it.
fn mentions(path: &Path, text: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains(text))
}

/// The `name:` an `environment.yml` gives its environment.
fn conda_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let name = line.strip_prefix("name:")?.trim();
        let name = name.split('#').next()?.trim().trim_matches(['"', '\'']);
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// Whether an `environment.yml` lists a package among its dependencies,
/// as `- r-base` or `- r-base=4.4`.
fn lists(text: &str, package: &str) -> bool {
    text.lines().any(|line| {
        let Some(item) = line.trim_start().strip_prefix('-') else {
            return false;
        };
        let item = item.trim().trim_start_matches("conda-forge::");
        item.strip_prefix(package)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['=', '<', '>', ' ', '!']))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel(name: &str, language: &str, env: Option<&str>) -> KernelspecInfo {
        KernelspecInfo {
            name: name.into(),
            display_name: name.into(),
            language: language.into(),
            env: env.map(String::from),
        }
    }

    fn write(root: &Path, file: &str, text: &str) {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn summary(found: &[FoundEnvironment]) -> Vec<String> {
        found
            .iter()
            .map(|f| {
                format!(
                    "{} {} {} -> {}",
                    f.language,
                    f.file,
                    f.label,
                    f.kernel.as_deref().or(f.hint.as_deref()).unwrap_or("?")
                )
            })
            .collect()
    }

    #[test]
    fn a_plain_folder_points_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            find(
                dir.path(),
                &[kernel("python3", "python", None)],
                &Search::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn renv_runs_in_the_usual_r_and_environment_yml_names_a_conda_env() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "renv.lock", "{}");
        write(
            dir.path(),
            "environment.yml",
            "name: omicverse  # scRNA-seq\nchannels:\n  - conda-forge\ndependencies:\n  - python=3.11\n  - r-base=4.4\n",
        );
        let kernels = [
            kernel("ir", "R", None),
            kernel("ark", "R", None),
            kernel("python3", "python", None),
            kernel("conda-env-omicverse-py", "python", Some("omicverse")),
        ];
        assert_eq!(
            summary(&find(dir.path(), &kernels, &Search::default())),
            [
                "r renv.lock renv -> ark",
                "python environment.yml conda env omicverse -> conda-env-omicverse-py",
                "r environment.yml conda env omicverse -> It has no R kernel: `conda install -n omicverse r-irkernel`.",
            ]
        );
    }

    #[test]
    fn environments_inside_the_folder_and_ones_still_to_make() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".venv/pyvenv.cfg", "home = /usr/bin\n");
        write(dir.path(), ".venv/bin/python", "");
        write(dir.path(), "uv.lock", "");
        write(dir.path(), "pixi.toml", "[workspace]\n");
        let found = find(dir.path(), &[], &Search::default());
        assert_eq!(
            summary(&found),
            [
                "python .venv .venv -> It has no Python kernel: `uv add --dev ipykernel`.",
                "python pixi.toml pixi default -> It isn't installed yet: run `pixi install` in the project's folder, with ipykernel among its dependencies.",
            ]
        );

        write(dir.path(), ".pixi/envs/default/conda-meta/history", "");
        write(dir.path(), ".pixi/envs/default/bin/R", "");
        let kernels = [
            kernel("project-venv-py", "python", Some(".venv")),
            kernel("project-pixi-default-py", "python", Some("pixi default")),
        ];
        assert_eq!(
            summary(&find(dir.path(), &kernels, &Search::default())),
            [
                "python .venv .venv -> project-venv-py",
                "python .pixi/envs/default pixi default -> project-pixi-default-py",
                "r .pixi/envs/default pixi default -> It has no R kernel: `pixi add r-irkernel`.",
            ]
        );
    }

    #[test]
    fn environment_files_are_read_loosely() {
        assert_eq!(
            conda_name("channels: []\nname: \"scvi\"\n").as_deref(),
            Some("scvi")
        );
        assert_eq!(conda_name("dependencies: []\n"), None);
        assert!(lists(
            "dependencies:\n  - conda-forge::r-base>=4.3\n",
            "r-base"
        ));
        assert!(!lists("dependencies:\n  - r-basejump\n", "r-base"));
    }
}
