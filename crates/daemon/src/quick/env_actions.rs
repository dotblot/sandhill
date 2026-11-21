//! Reviewed environment actions share the install worker, log and record.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use sandhill_proto::{
    EnvAction, EnvActionPlan, InstallTarget, InstallTool, PackageAction, PackageChange,
    PackagePlan, PlannedPackage,
};
use serde::{Deserialize, Serialize};

use super::{
    env_check,
    environments::{self, Environment},
    install::{self, quote},
    package_plan, record, undo,
};
use crate::{
    kernelspecs::{self, Search},
    own_env,
};

#[derive(Serialize, Deserialize)]
pub(crate) enum Work {
    Check(Box<Environment>),
    Undo { id: String, command: String },
    Packages(PackagePlan),
    Shell { command: String, effect: Effect },
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Effect {
    None,
    Add {
        folder: PathBuf,
        conda: bool,
        method: String,
        reason: String,
        kernel: Option<PathBuf>,
    },
    Delete {
        folder: PathBuf,
        id: String,
    },
}

pub(crate) struct Prepared {
    pub plan: EnvActionPlan,
    pub work: Option<Work>,
    pub stop: Option<PathBuf>,
}

pub(crate) fn prepare(
    search: &Search,
    record_path: &Path,
    exe: &Path,
    action: EnvAction,
) -> Prepared {
    let mut prepared = Prepared {
        plan: EnvActionPlan {
            action,
            what: String::new(),
            folder: String::new(),
            command: String::new(),
            notes: Vec::new(),
            refused: None,
        },
        work: None,
        stop: None,
    };
    match build(search, record_path, exe, &mut prepared) {
        Ok(work) => prepared.work = Some(work),
        Err(error) => {
            prepared.plan.refused = Some(error.to_string());
            prepared.plan.command.clear();
        }
    }
    prepared
}

fn build(search: &Search, record_path: &Path, exe: &Path, prepared: &mut Prepared) -> Result<Work> {
    let plan = &mut prepared.plan;
    let id = match &plan.action {
        EnvAction::Check { env }
        | EnvAction::Undo { env }
        | EnvAction::Duplicate { env, .. }
        | EnvAction::Export { env, .. }
        | EnvAction::Delete { env } => Some(env.as_str()),
        EnvAction::Import { file, env, .. } if !is_yaml(file) => env.as_deref(),
        EnvAction::Import { .. } => None,
    };
    let env = id
        .map(|id| environments::get(search, record_path, id))
        .transpose()?;
    if let Some(env) = &env {
        plan.folder.clone_from(&env.card.folder);
    }
    let effect = match (&plan.action, env) {
        (EnvAction::Check { .. }, Some(env)) => {
            plan.what = format!("Check packages in {} and start its kernel", env.card.name);
            if env.card.kernel.is_none() {
                plan.what = format!("Check packages in {}", env.card.name);
                plan.notes
                    .push("There's no kernel in this environment to start.".into());
            }
            let job = install::Job::Environment(Work::Check(Box::new(env)));
            plan.command = install::worker_command(exe, record_path, &job)?;
            let install::Job::Environment(work) = job else {
                unreachable!()
            };
            return Ok(work);
        }
        (EnvAction::Undo { .. }, Some(env)) => {
            let (what, command, notes) = undo::plan(record_path, &env.card.id)?;
            plan.what = format!("Undo {what}");
            plan.command = command.clone();
            plan.notes = notes;
            return Ok(Work::Undo {
                id: env.card.id,
                command,
            });
        }
        (EnvAction::Duplicate { prefix, .. }, Some(env)) => {
            let destination = new_path(prefix, "the new environment")?;
            let is_conda = env.conda.is_some();
            let method;
            let mut kernel = None;
            plan.command = if let Some(source) = &env.conda {
                let tool = conda_tool(&env)?;
                method = "conda";
                duplicate_conda(tool, source, &destination)
            } else if !env.is_r() && is_venv(&env) {
                let (tool, path) = python_tool(&env)?;
                method = tool.id();
                let mut command = duplicate_venv(tool, path, &env.card.program, &destination);
                if env.packages.contains_key("ipykernel") {
                    let (name, at, register) = register_venv(search, &destination)?;
                    plan.notes.push(format!("Register its kernel as {name}."));
                    command.push('\n');
                    command.push_str(&register);
                    kernel = Some(at);
                }
                command
            } else {
                bail!(
                    "Sandhill can copy a conda environment or a Python virtual environment, but not this installation."
                );
            };
            plan.folder = destination.to_string_lossy().into_owned();
            plan.what = format!("Copy {} to a new environment", env.card.name);
            Effect::Add {
                folder: destination,
                conda: is_conda,
                method: method.into(),
                reason: "Copied from the R and Python page".into(),
                kernel,
            }
        }
        (EnvAction::Export { path, .. }, Some(env)) => {
            let destination = new_path(path, "the package list")?;
            let shell = if env.is_r() {
                ensure!(
                    path.ends_with(".csv"),
                    "Save the R package list as a .csv file."
                );
                export_r(&env)
            } else if let Some(prefix) = &env.conda {
                ensure!(
                    is_yaml(path),
                    "Save the conda environment as a .yml or .yaml file."
                );
                quote([
                    conda_tool(&env)?,
                    "env",
                    "export",
                    "--no-builds",
                    "-p",
                    &prefix.to_string_lossy(),
                ])
            } else {
                ensure!(
                    is_venv(&env),
                    "Sandhill can export a conda environment or a Python virtual environment, but not this installation."
                );
                ensure!(
                    path.ends_with(".txt"),
                    "Save the Python package list as a .txt file."
                );
                let (tool, path) = python_tool(&env)?;
                freeze(tool, path, &env.card.program)
            };
            plan.what = format!("Save the package list for {}", env.card.name);
            plan.folder = destination.to_string_lossy().into_owned();
            let template = destination.with_file_name(".sandhill-export.XXXXXXXX");
            plan.command = format!(
                "set -e -o pipefail\nout=$(mktemp {})\ntrap 'rm -f -- \"$out\"' EXIT\n{shell} > \"$out\"\nln -- \"$out\" {}",
                quote([template.to_string_lossy().as_ref()]),
                quote([plan.folder.as_str()])
            );
            Effect::None
        }
        (EnvAction::Import { file, prefix, .. }, _) if is_yaml(file) => {
            let source = existing_file(file)?;
            let destination = new_path(
                prefix
                    .as_deref()
                    .context("Choose a folder for the new conda environment.")?,
                "the new environment",
            )?;
            let search = search.clone().in_own(own_env::chosen(record_path));
            let (tool, _) = super::python::find_tool(&search)
                .or_else(|| install::on_path(&search, "micromamba").map(|p| (p, PathBuf::new())))
                .context(
                    "No mamba or conda tool was found. Set one up first on the R and Python page.",
                )?;
            plan.what = "Make a new conda environment from this file".into();
            plan.folder = destination.to_string_lossy().into_owned();
            plan.command = conda_import_command(&tool, &destination, &source);
            Effect::Add {
                folder: destination,
                conda: true,
                method: "conda".into(),
                reason: "Imported from an environment file".into(),
                kernel: None,
            }
        }
        (EnvAction::Import { file, .. }, Some(env)) => {
            let file = existing_file(file)?;
            let packages = import_packages(&env, &file)?;
            plan.what = format!("Install packages from this file into {}", env.card.name);
            if env.is_r() {
                plan.notes.push(format!(
                    "Install: {}.",
                    packages
                        .packages
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                plan.notes.push(
                    "R installs the versions available from the listed sources today.".into(),
                );
            }
            plan.folder.clone_from(&packages.folder);
            plan.command.clone_from(&packages.command);
            return Ok(Work::Packages(packages));
        }
        (EnvAction::Import { .. }, None) => {
            bail!("Choose an environment to install these packages into.")
        }
        (EnvAction::Delete { .. }, Some(env)) => {
            let folder = deletable(&env, search)?;
            plan.what = format!("Delete {} and all its packages", env.card.name);
            plan.folder = folder.to_string_lossy().into_owned();
            plan.notes
                .push("Any kernels still running from this folder will be stopped first.".into());
            plan.command = if env.conda.is_some() {
                quote([conda_tool(&env)?, "env", "remove", "-y", "-p", &plan.folder])
            } else {
                quote(["/bin/rm", "-rf", "--", &plan.folder])
            };
            if matches!(env.card.kind.as_str(), "venv" | "uv")
                && let Some(kernel) = copied_kernel(record_path, &folder)
            {
                plan.notes
                    .push("Remove the kernel Sandhill registered for this copy.".into());
                plan.command.push_str(" && ");
                plan.command
                    .push_str(&quote(["/bin/rm", "-rf", "--", &kernel.to_string_lossy()]));
            }
            prepared.stop = Some(folder.clone());
            Effect::Delete {
                folder,
                id: env.card.id,
            }
        }
        _ => bail!("Choose an environment first."),
    };
    Ok(Work::Shell {
        command: plan.command.clone(),
        effect,
    })
}

pub(crate) async fn run(record_path: &Path, work: Work) -> Result<()> {
    match work {
        Work::Check(env) => env_check::run(record_path, &env).await,
        Work::Undo { id, command } => undo::run(record_path, &id, &command),
        Work::Packages(plan) => package_plan::run(record_path, &plan),
        Work::Shell { command, effect } => {
            match &effect {
                Effect::Add { folder, kernel, .. } => {
                    ensure!(
                        folder.symlink_metadata().is_err(),
                        "The new folder already exists. Review the plan again."
                    );
                    ensure!(
                        kernel
                            .as_ref()
                            .is_none_or(|p| p.symlink_metadata().is_err()),
                        "The new kernel name is taken. Review the plan again."
                    );
                }
                Effect::Delete { folder, .. } => ensure!(
                    folder.symlink_metadata().is_ok_and(|m| m.is_dir())
                        && (folder.join("conda-meta").is_dir()
                            || folder.join("pyvenv.cfg").is_file())
                        && own_env::containing(folder).is_none(),
                    "The environment changed after Sandhill checked it. It won't be deleted."
                ),
                Effect::None => {}
            }
            install::run_command(&command)?;
            match effect {
                Effect::None => {}
                Effect::Add {
                    folder,
                    conda,
                    method,
                    reason,
                    kernel,
                } => {
                    let kind = if conda {
                        record::CONDA_ENV
                    } else {
                        record::PYTHON_ENV
                    };
                    let packages = if conda {
                        record::conda_packages(&folder)?
                    } else {
                        record::Versions::new()
                    };
                    let name = folder.file_name().unwrap_or_default().to_string_lossy();
                    record::add(
                        record_path,
                        [record::entry(
                            kind,
                            name,
                            packages.len().to_string(),
                            &folder.to_string_lossy(),
                            &method,
                            &reason,
                        )],
                    )?;
                    if let Some(kernel) = kernel {
                        use sha2::{Digest, Sha256};
                        let spec = fs::read(kernel.join("kernel.json"))?;
                        let version = sandhill_proto::hex(&Sha256::digest(spec));
                        record::add(
                            record_path,
                            [record::entry(
                                record::KERNEL,
                                kernel.file_name().unwrap_or_default().to_string_lossy(),
                                version,
                                &kernel.to_string_lossy(),
                                &method,
                                &reason,
                            )],
                        )?;
                    }
                }
                Effect::Delete { folder, id } => {
                    record::forget_own(record_path, &folder)?;
                    record::prune(record_path)?;
                    for (area, directory) in [("checks", false), ("undo", true)] {
                        let path = environments::state_path(record_path, area, &id);
                        if directory {
                            let _ = fs::remove_dir_all(path);
                        } else {
                            let _ = fs::remove_file(path);
                        }
                    }
                }
            }
            println!("Done.");
            Ok(())
        }
    }
}

fn conda_tool(env: &Environment) -> Result<&str> {
    env.tool(InstallTool::Conda)
        .map(|t| t.path.as_str())
        .context("The mamba or conda tool for this environment wasn't found.")
}

fn python_tool(env: &Environment) -> Result<(InstallTool, &str)> {
    env.card
        .tools
        .iter()
        .find(|t| matches!(t.tool, InstallTool::Pip | InstallTool::Uv))
        .map(|t| (t.tool, t.path.as_str()))
        .context("Neither pip nor uv can change packages in this environment.")
}

fn is_venv(env: &Environment) -> bool {
    matches!(env.card.kind.as_str(), "venv" | "uv")
}

fn is_yaml(path: &str) -> bool {
    path.ends_with(".yml") || path.ends_with(".yaml")
}

fn existing_file(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    ensure!(
        path.is_absolute() && path.is_file(),
        "Choose an existing file."
    );
    Ok(path.canonicalize()?)
}

fn new_path(path: &str, name: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    ensure!(path.is_absolute(), "Choose an absolute path for {name}.");
    let leaf = path
        .file_name()
        .context("Choose a name for the new file or folder.")?;
    let parent = path
        .parent()
        .context("Choose a folder for the new file or folder.")?
        .canonicalize()
        .context("The folder around the new file or environment doesn't exist.")?;
    ensure!(
        package_plan::writable(&parent),
        "You can't write to {}.",
        parent.display()
    );
    let path = parent.join(leaf);
    ensure!(
        path.symlink_metadata().is_err(),
        "{} already exists. Choose another name or folder.",
        path.display()
    );
    Ok(path)
}

fn deletable(env: &Environment, search: &Search) -> Result<PathBuf> {
    ensure!(
        env.own.is_none(),
        "Use Remove Sandhill's Own R and Python to remove Sandhill's folder."
    );
    let folder = if let Some(prefix) = &env.conda {
        let tool = conda_tool(env)?;
        let info = conda_root(tool);
        let base = info
            .as_deref()
            .map(|base| package_plan::same_folder(base, prefix));
        let known = search
            .conda_roots
            .iter()
            .any(|root| package_plan::same_folder(kernelspecs::conda_root(root), prefix));
        ensure!(
            base == Some(false)
                || (base.is_none() && !known && kernelspecs::conda_root(prefix) != prefix),
            "Sandhill doesn't delete the main conda environment."
        );
        prefix.clone()
    } else {
        ensure!(
            !env.is_r() && is_venv(env),
            "Sandhill can delete a conda environment or a Python virtual environment, but not this installation."
        );
        PathBuf::from(&env.card.folder)
    };
    ensure!(
        folder.is_absolute()
            && folder.parent().is_some()
            && folder.symlink_metadata().is_ok_and(|meta| meta.is_dir()),
        "This environment's folder couldn't be found."
    );
    if let Some(home) = install::home() {
        ensure!(
            !kernelspecs::canonical(&home).starts_with(&folder),
            "Sandhill won't delete your home folder or a folder containing it."
        );
    }
    Ok(folder)
}

pub(super) fn conda_root(tool: &str) -> Option<PathBuf> {
    let out = install::probe(Path::new(tool), &["info", "--json"])?;
    let json: serde_json::Value = serde_json::from_str(&out).ok()?;
    ["root_prefix", "base environment", "base_environment"]
        .into_iter()
        .find_map(|key| {
            let path = json[key].as_str()?.split("  ").next()?;
            let path = PathBuf::from(path);
            path.is_absolute().then_some(path)
        })
}

pub(super) fn conda_import_command(tool: &Path, destination: &Path, source: &Path) -> String {
    quote([
        tool.to_string_lossy().as_ref(),
        "env",
        "create",
        "-y",
        "-p",
        &destination.to_string_lossy(),
        "--file",
        &source.to_string_lossy(),
    ])
}

fn duplicate_conda(tool: &str, source: &Path, destination: &Path) -> String {
    let src = source.to_string_lossy();
    let dest = destination.to_string_lossy();
    if !tool.ends_with("/micromamba") {
        return quote([tool, "create", "-y", "-p", &dest, "--clone", &src]);
    }
    let export = quote([tool, "env", "export", "--explicit", "-p", &src]);
    let create = quote([tool, "create", "-y", "-p", &dest, "--file"]);
    with_freeze(&export, &format!("{create} \"$list\""))
}

fn with_freeze(export: &str, rest: &str) -> String {
    format!("set -e\nlist=$(mktemp)\ntrap 'rm -f -- \"$list\"' EXIT\n{export} > \"$list\"\n{rest}")
}

fn freeze(tool: InstallTool, path: &str, program: &str) -> String {
    if tool == InstallTool::Uv {
        quote([path, "pip", "freeze", "--python", program])
    } else {
        quote([program, "-m", "pip", "freeze", "--local"])
    }
}

fn duplicate_venv(tool: InstallTool, path: &str, program: &str, destination: &Path) -> String {
    let target = destination.to_string_lossy();
    let new_python = destination.join("bin/python");
    let new_python = new_python.to_string_lossy();
    let (create, install) = if tool == InstallTool::Uv {
        (
            quote([path, "venv", "--python", program, &target]),
            quote([path, "pip", "install", "--python", &new_python, "-r"]),
        )
    } else {
        (
            quote([program, "-m", "venv", &target]),
            quote([new_python.as_ref(), "-m", "pip", "install", "-r"]),
        )
    };
    with_freeze(
        &freeze(tool, path, program),
        &format!("{create}\n{install} \"$list\""),
    )
}

fn register_venv(search: &Search, destination: &Path) -> Result<(String, PathBuf, String)> {
    use sha2::{Digest, Sha256};
    let data = search
        .user_dir
        .as_deref()
        .context("Jupyter's folder for kernels wasn't found.")?;
    let label = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let suffix = sandhill_proto::hex(&Sha256::digest(destination.as_os_str().as_encoded_bytes()));
    let name = format!("sandhill-{}", &suffix[..16]);
    let registration = data.join("kernels").join(&name);
    ensure!(
        registration.symlink_metadata().is_err(),
        "The kernel name {name} is already taken. Choose another folder."
    );
    let python = destination.join("bin/python");
    let command = quote([
        "/usr/bin/env",
        &format!("JUPYTER_DATA_DIR={}", data.display()),
        &python.to_string_lossy(),
        "-m",
        "ipykernel",
        "install",
        "--user",
        "--name",
        &name,
        "--display-name",
        &format!("Python ({label})"),
    ]);
    Ok((name, registration, command))
}

fn copied_kernel(record_path: &Path, folder: &Path) -> Option<PathBuf> {
    record::read(record_path).into_iter().find_map(|row| {
        if !row.ours
            || row.kind != record::KERNEL
            || row.reason != "Copied from the R and Python page"
        {
            return None;
        }
        let path = PathBuf::from(row.place);
        let spec: jupyter_protocol::JupyterKernelspec =
            serde_json::from_slice(&fs::read(path.join("kernel.json")).ok()?).ok()?;
        Path::new(spec.argv.first()?)
            .starts_with(folder)
            .then_some(path)
    })
}

fn export_r(env: &Environment) -> String {
    let code = r#"local({
      result <- data.frame(name = character(), version = character(), source = character(), library = character())
      for (lib in .libPaths()) {
        p <- utils::installed.packages(lib.loc = lib, fields = c('Repository', 'RemoteType', 'RemoteUsername', 'RemoteRepo'), noCache = TRUE)
        if (nrow(p) == 0L) next
        source <- rep('other', nrow(p))
        source[grep('CRAN', p[, 'Repository'], ignore.case = TRUE)] <- 'CRAN'
        source[grep('Bioconductor', p[, 'Repository'], ignore.case = TRUE)] <- 'Bioconductor'
        github <- which(p[, 'RemoteType'] == 'github' & !is.na(p[, 'RemoteUsername']) & !is.na(p[, 'RemoteRepo']))
        source[github] <- paste0('GitHub: ', p[github, 'RemoteUsername'], '/', p[github, 'RemoteRepo'])
        result <- rbind(result, data.frame(name = p[, 'Package'], version = p[, 'Version'], source = source, library = lib))
      }
      utils::write.csv(result, stdout(), row.names = FALSE)
    })"#;
    let command = quote([
        env.card.program.as_str(),
        "--no-save",
        "--no-restore",
        "-s",
        "-e",
        code,
    ]);
    if let Some(folder) = &env.own {
        own_env::command(folder, &command)
    } else {
        command
    }
}

fn import_packages(env: &Environment, file: &Path) -> Result<PackagePlan> {
    let target = env.target(None)?;
    let (tool, folder, command, packages) = if let InstallTarget::R { r, library } = &target {
        ensure!(
            file.extension().is_some_and(|s| s == "csv"),
            "Choose the R package list that Sandhill saved as a .csv file."
        );
        let rows = csv(&fs::read_to_string(file)?)?;
        ensure!(
            rows.first().is_some_and(|row| row
                .iter()
                .map(String::as_str)
                .eq(["name", "version", "source", "library"])),
            "This isn't an R package list saved by Sandhill."
        );
        let mut use_bioc = false;
        let mut unknown = Vec::new();
        let mut packages = Vec::new();
        for row in rows.into_iter().skip(1) {
            let Ok([name, version, source, _]) = <[String; 4]>::try_from(row) else {
                bail!("A line in the R package list doesn't have four values.");
            };
            ensure!(record::r_name(&name), "{name:?} isn't an R package name.");
            let spec = if let Some(repo) = source.strip_prefix("GitHub: ") {
                ensure!(
                    package_plan::github_repo(repo),
                    "{repo:?} isn't a GitHub package name."
                );
                repo.into()
            } else if source == "CRAN" || source == "Bioconductor" {
                use_bioc |= source == "Bioconductor";
                name.clone()
            } else {
                if !env.packages.contains_key(&name) {
                    unknown.push(name);
                }
                continue;
            };
            packages.push(PlannedPackage {
                name,
                spec,
                version: Some(version),
                source,
            });
        }
        if !unknown.is_empty() {
            bail!(
                "The file doesn't say where to find these packages: {}. Install them separately first.",
                unknown.join(", ")
            );
        }
        ensure!(
            !packages.is_empty(),
            "There are no packages from CRAN, Bioconductor or GitHub in this file."
        );
        let names = packages.iter().map(|p| p.spec.as_str()).collect::<Vec<_>>();
        let rscript = Path::new(r).with_file_name("Rscript");
        ensure!(
            install::executable(&rscript),
            "There is no Rscript beside {r}."
        );
        let command = quote([
            rscript.to_string_lossy().as_ref(),
            "-e",
            &package_plan::r_code(PackageAction::Install, Path::new(library), &names, use_bioc),
        ]);
        (InstallTool::R, library.clone(), command, packages)
    } else {
        ensure!(
            file.extension().is_some_and(|s| s == "txt"),
            "Choose a Python requirements .txt file."
        );
        let (tool, path) = python_tool(env)?;
        let program = &env.card.program;
        let actual = package_plan::inspect_python(Path::new(program))
            .context("Python couldn't tell Sandhill where its packages go.")?;
        ensure!(
            own_env::inside(Path::new(&env.card.folder), &actual.package_folder)
                && package_plan::writable(&actual.package_folder),
            "Python can't install packages into the folder shown."
        );
        let file = file.to_string_lossy();
        let command = if tool == InstallTool::Uv {
            quote([path, "pip", "install", "--python", program, "-r", &file])
        } else {
            quote([program, "-m", "pip", "install", "-r", &file])
        };
        (tool, env.card.folder.clone(), command, Vec::new())
    };
    let command = if let Some(own) = &env.own {
        own_env::command(own, &command)
    } else {
        command
    };
    Ok(PackagePlan {
        change: PackageChange {
            action: PackageAction::Install,
            target,
            packages: packages.iter().map(|p| p.spec.clone()).collect(),
            tool: Some(tool),
            reason: "Imported from a package list".into(),
        },
        tools: env.card.tools.clone(),
        folder,
        packages,
        command,
        notes: Vec::new(),
        refused: None,
    })
}

fn csv(text: &str) -> Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push(c);
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => row.push(std::mem::take(&mut field)),
            '\r' if !quoted && chars.peek() == Some(&'\n') => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut field));
                if row.len() > 1 || !row[0].is_empty() {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            _ => field.push(c),
        }
    }
    ensure!(!quoted, "This package list has an unclosed quote.");
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_and_copy_commands() {
        assert_eq!(
            csv("\"name\",\"source\",\"library\"\r\n\"test\",\"CRAN\",\"/a,b\"\r\n").unwrap()[1],
            ["test", "CRAN", "/a,b"]
        );
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("new env");
        assert!(new_path(target.to_str().unwrap(), "the new folder").is_ok());
        assert!(new_path(temp.path().to_str().unwrap(), "the new folder").is_err());
        assert!(new_path("relative", "the new folder").is_err());
        assert!(duplicate_conda("/bin/conda", Path::new("/old"), &target).contains("--clone"));
        assert!(
            duplicate_conda("/bin/micromamba", Path::new("/old"), &target).contains("--explicit")
        );
        assert!(
            duplicate_venv(
                InstallTool::Pip,
                "/old/bin/python",
                "/old/bin/python",
                &target
            )
            .contains("'pip' 'install' '-r'")
        );
    }

    #[test]
    fn r_export_and_import_agree_and_real_installations_cannot_be_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path();
        let library = path.join("library");
        fs::create_dir(&library).unwrap();
        let rscript = path.join("Rscript");
        fs::write(&rscript, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&rscript, fs::Permissions::from_mode(0o700)).unwrap();
        let card: sandhill_proto::EnvCard = serde_json::from_value(serde_json::json!({
            "id": "r:fake", "language": "r", "name": "R 4.5.2", "kind": "cran",
            "program": path.join("R"), "folder": path, "status": "",
            "libraries": [{"path": library, "writable": true, "packages": 0}],
            "tools": [{"path": path.join("R"), "tool": "r"}]
        }))
        .unwrap();
        let env = Environment {
            card,
            version: "4.5.2".into(),
            own: None,
            conda: None,
            packages: Default::default(),
        };
        assert!(export_r(&env).contains("utils::write.csv"));
        assert!(
            deletable(&env, &Search::default())
                .unwrap_err()
                .to_string()
                .contains("not this installation")
        );
        let csv = path.join("package-list.csv");
        fs::write(&csv, "name,version,source,library\npraise,1.0,CRAN,/old\nGenomicRanges,2.0,Bioconductor,/old\nother,3.0,GitHub: org/repo,/old\n").unwrap();
        let plan = import_packages(&env, &csv).unwrap();
        assert!(plan.command.contains("BiocManager::install") && plan.command.contains("org/repo"));
        assert_eq!(plan.folder, library.to_string_lossy());
        fs::write(&csv, "wrong,file\n").unwrap();
        assert!(import_packages(&env, &csv).is_err());
        let mut own = env;
        own.own = Some(path.to_path_buf());
        assert!(
            deletable(&own, &Search::default())
                .unwrap_err()
                .to_string()
                .contains("Sandhill's folder")
        );
    }
}
