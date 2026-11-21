use super::*;
use crate::quick::sources::{self, Sources};
use crate::quick::space;

#[derive(Deserialize, Serialize)]
pub(crate) struct Work {
    plan: ProjectSetupPlan,
    fingerprint: String,
    own: Option<PathBuf>,
    effect: Effect,
}

impl Work {
    pub fn plan(&self) -> &ProjectSetupPlan {
        &self.plan
    }

    /// The new folder this set-up makes (a project's `.venv`, a conda
    /// environment, a new renv library), which undoing an interrupted
    /// set-up removes.
    pub fn made(&self) -> Option<PathBuf> {
        let folder = Path::new(&self.plan.folder);
        let new = match &self.effect {
            Effect::R { .. } => !folder.is_dir(),
            Effect::Conda { .. } => true,
            Effect::Python { new, .. } => *new,
        };
        new.then(|| folder.to_path_buf())
    }
}

#[derive(Deserialize, Serialize)]
enum Effect {
    R {
        install_renv: Option<Box<PackagePlan>>,
        target: InstallTarget,
        library: PathBuf,
    },
    Conda {
        create: Box<env_actions::Work>,
    },
    Python {
        new: bool,
        pip: InstallTool,
        tool: String,
    },
}

pub(crate) fn list(
    project: &Path,
    search: &Search,
    mut runtime: impl FnMut(&str) -> Result<Runtime>,
) -> Vec<ProjectPackageFile> {
    FILES
        .into_iter()
        .filter(|file| project.join(file).symlink_metadata().is_ok())
        .map(|file| {
            let language = if file == "renv.lock" { "r" } else { "python" };
            let mut result = ProjectPackageFile {
                file: file.into(),
                language: language.into(),
                packages: 0,
                set_up: false,
                differences: Vec::new(),
            };
            let parsed = read_file(project, file).and_then(|text| parse(file, &text));
            let pins = match parsed {
                Ok(pins) => pins,
                Err(error) => {
                    result.differences.push(error.to_string());
                    return result;
                }
            };
            result.packages = pins
                .pins
                .iter()
                .filter(|pin| pin.package.version.is_some())
                .count() as u32;
            let own = search.own.as_deref();
            let location = match file {
                "renv.lock" => {
                    let root = renv_root(project, own);
                    if root.is_dir() {
                        runtime("r")
                            .and_then(|r| {
                                inventory::r_runtime(&r.program, Some(&r.spec), Some(project), own)
                            })
                            .map(|r| r_library(&root, &r))
                            .ok()
                    } else {
                        Some(root)
                    }
                }
                "requirements.txt" if own.is_none() => Some(project.join(".venv")),
                _ => conda_tool(search)
                    .and_then(|(_, root)| conda_destination(project, file, &pins, &root))
                    .ok(),
            };
            let installed = location.as_ref().is_some_and(|dir| match file {
                "renv.lock" => dir.is_dir() && project.join("renv/activate.R").is_file(),
                "requirements.txt" if own.is_none() => dir.join("pyvenv.cfg").is_file(),
                _ => dir.join("conda-meta/history").is_file(),
            });
            let details = differences(location.as_deref(), &pins);
            result.set_up = installed && details.is_empty();
            result.differences = details;
            if !installed && result.differences.is_empty() {
                result
                    .differences
                    .push("This project's packages aren't set up here yet.".into());
            }
            result
        })
        .collect()
}

fn differences(location: Option<&Path>, pins: &FilePins) -> Vec<String> {
    let conda = location
        .and_then(|p| record::conda_packages(p).ok())
        .unwrap_or_default();
    let pip: record::Versions = location
        .filter(|_| pins.pins.iter().any(|p| p.store == Store::Pip))
        .map(|p| {
            inventory::python_packages(p, inventory::prefix_sites(p), false)
                .into_iter()
                .map(|(name, pkg)| (name, pkg.version))
                .collect()
        })
        .unwrap_or_default();
    let r = location
        .filter(|_| pins.pins.iter().any(|p| p.store == Store::R))
        .and_then(|p| record::r_packages(p).ok())
        .unwrap_or_default();
    let mut differences = Vec::new();
    for pin in &pins.pins {
        let wanted = &pin.package;
        let have = match pin.store {
            Store::R => r.get(&wanted.name),
            Store::Conda => conda.get(&wanted.name),
            Store::Pip => pip.get(&record::python_name(&wanted.name)),
        };
        let label = wanted.version.as_ref().map_or_else(
            || wanted.name.clone(),
            |ver| format!("{} {ver}", wanted.name),
        );
        match (have, wanted.version.as_deref()) {
            (Some(have), Some(want))
                if have != want && !(pin.minor && major_minor(have) == Some(want)) =>
            {
                differences.push(format!("{label} (have {have})"));
            }
            // We can't know whether a platform marker applies without running its installer.
            (None, _) if pin.unconditional => differences.push(format!("{label} (not installed)")),
            _ => {}
        }
    }
    differences
}

pub(crate) fn prepare(
    project: &Path,
    search: &Search,
    record: &Path,
    file: &str,
    trusted: bool,
    runtime: impl FnOnce(&str) -> Result<Runtime>,
) -> (ProjectSetupPlan, Option<Work>) {
    let mut plan = ProjectSetupPlan {
        project: project.to_string_lossy().into_owned(),
        file: file.into(),
        what: "Set up this project's packages".into(),
        folder: String::new(),
        packages: Vec::new(),
        steps: Vec::new(),
        download_mb: None,
        notes: Vec::new(),
        refused: None,
        refused_fix: None,
    };
    match build(project, search, record, trusted, runtime, &mut plan) {
        Ok((effect, fingerprint)) => {
            let work = Work {
                plan: plan.clone(),
                effect,
                fingerprint,
                own: search.own.clone(),
            };
            (plan, Some(work))
        }
        Err(error) => {
            if plan.refused_fix.is_none() {
                plan.refused_fix = space::fix(&error);
            }
            plan.refused = Some(error.to_string());
            plan.steps.clear();
            (plan, None)
        }
    }
}

fn build(
    project: &Path,
    search: &Search,
    record: &Path,
    trusted: bool,
    runtime: impl FnOnce(&str) -> Result<Runtime>,
    plan: &mut ProjectSetupPlan,
) -> Result<(Effect, String)> {
    ensure!(
        trusted,
        "Trust this project before setting up its packages."
    );
    let text = read_file(project, &plan.file)?;
    let mut pins = parse(&plan.file, &text)?;
    plan.notes.append(&mut pins.notes);
    let own = search.own.as_deref();
    let sources = sources::chosen(record);
    let path = project.join(&plan.file);
    let effect = match plan.file.as_str() {
        "renv.lock" => setup_r(
            project,
            search,
            record,
            runtime("r")?,
            &pins,
            plan,
            &sources,
        )?,
        "requirements.txt" if own.is_none() => {
            setup_venv(project, search, runtime, &path, &pins, plan, &sources)?
        }
        file => {
            let (tool, root) = conda_tool(search)?;
            let dst = conda_destination(project, file, &pins, &root)?;
            ensure!(
                dst.symlink_metadata().is_err(),
                "{} already exists. Choose a different environment name if you need a new copy.",
                dst.display()
            );
            if let Some(own) = own {
                ensure!(
                    dst.parent().is_some_and(|p| own_env::inside(own, p)),
                    "The new environment would be outside Sandhill's folder."
                );
            }
            plan.what = "Make a new conda environment for this project".into();
            plan.folder = dst.to_string_lossy().into_owned();
            let in_project = |command: String| {
                format!(
                    "cd {} && {command}",
                    install::quote([project.to_string_lossy().as_ref()])
                )
            };
            let command = if file == "requirements.txt" {
                let mut args = vec![
                    tool.to_string_lossy().into_owned(),
                    "create".into(),
                    "-y".into(),
                    "-p".into(),
                    plan.folder.clone(),
                ];
                args.extend(sources.conda_args());
                args.extend(["python".into(), "pip".into()]);
                let create = install::quote(args);
                add_step(plan, "Make a conda environment with Python", create.clone());
                let kernel = !pins
                    .pins
                    .iter()
                    .any(|p| p.package.name.eq_ignore_ascii_case("ipykernel") && p.unconditional);
                let pip = pip_command(project, &dst, &path, None, own, kernel, &sources);
                add_step(
                    plan,
                    if kernel {
                        "Install the packages in requirements.txt and add the Python kernel"
                    } else {
                        "Install the packages in requirements.txt"
                    },
                    pip,
                );
                create
            } else {
                let create = in_project(env_actions::conda_import_command(
                    &tool, &dst, &path, &sources,
                ));
                add_step(
                    plan,
                    "Make a conda environment from this file",
                    create.clone(),
                );
                let pkg = |name: &str| pins.pins.iter().any(|p| p.package.name == name);
                let r = pkg("r-base") || pkg("r-essentials");
                let python = !r
                    || pkg("python")
                    || pkg("pip")
                    || pins.pins.iter().any(|p| p.store == Store::Pip);
                let extras = [("ipykernel", python), ("r-irkernel", r)]
                    .into_iter()
                    .filter_map(|(name, need)| (need && !pkg(name)).then_some(name))
                    .collect::<Vec<_>>();
                if !extras.is_empty() {
                    let mut args = vec![
                        tool.to_string_lossy().into_owned(),
                        "install".into(),
                        "--freeze-installed".into(),
                        "-y".into(),
                        "-p".into(),
                        plan.folder.clone(),
                    ];
                    args.extend(sources.conda_args());
                    args.extend(extras.iter().map(|name| name.to_string()));
                    let kernel = install::quote(args);
                    add_step(
                        plan,
                        "Add the kernel so Sandhill can use this environment",
                        kernel,
                    );
                }
                create
            };
            Effect::Conda {
                create: Box::new(env_actions::Work::Shell {
                    command,
                    effect: env_actions::Effect::Add {
                        folder: dst,
                        conda: true,
                        method: "conda".into(),
                        reason: "Set up this project's packages".into(),
                        kernel: None,
                    },
                }),
            }
        }
    };
    plan.packages
        .extend(pins.pins.into_iter().map(|pin| pin.package));
    space::check(Path::new(&plan.folder), space::need_mb(plan.download_mb))?;
    Ok((effect, record::digest(text.as_bytes())))
}

fn ensure_not_symlink(path: &Path) -> Result<()> {
    ensure!(
        !path.symlink_metadata().is_ok_and(|m| m.is_symlink()),
        "{} points to another folder or file. Sandhill won't change it.",
        path.display()
    );
    Ok(())
}

fn checked_root(project: &Path, own: Option<&Path>, root: &Path) -> Result<()> {
    let boundary = own.unwrap_or(project);
    ensure!(
        root.starts_with(boundary),
        "The package library is outside its folder."
    );
    for path in root.ancestors().take_while(|p| *p != boundary) {
        ensure_not_symlink(path)?;
    }
    Ok(())
}

fn setup_r(
    project: &Path,
    search: &Search,
    record: &Path,
    runtime: Runtime,
    pins: &FilePins,
    plan: &mut ProjectSetupPlan,
    sources: &Sources,
) -> Result<Effect> {
    let own = search.own.as_deref();
    let r = inventory::r_runtime(&runtime.program, Some(&runtime.spec), Some(project), own)?;
    let root = renv_root(project, own);
    checked_root(project, own, &root)?;
    let project_renv = project.join("renv");
    for path in [
        project_renv.clone(),
        project_renv.join("activate.R"),
        project.join(".Rprofile"),
    ] {
        ensure_not_symlink(&path)?;
    }
    let project_lib = r_library(&root, &r);
    checked_root(project, own, &project_lib)?;
    let renv = r
        .libraries
        .iter()
        .map(|lib| Path::new(&lib.path))
        .chain(std::iter::once(project_lib.as_path()))
        .find(|lib| record::package_version(lib, "renv").is_some());
    let r_program = runtime.program.to_string_lossy().into_owned();
    let (renv, install_renv) = if let Some(library) = renv {
        (library.to_path_buf(), None)
    } else {
        let library = r.libraries.iter().find(|lib| lib.writable && own.is_none_or(|own| own_env::inside(own, Path::new(&lib.path))))
            .context("R has no library you can write to. Choose another R before setting up this project.")?;
        let change = PackageChange {
            action: PackageAction::Install,
            target: InstallTarget::R {
                r: r_program.clone(),
                library: library.path.clone(),
            },
            packages: vec!["renv".into()],
            tool: Some(InstallTool::R),
            reason: "Set up this project's packages".into(),
        };
        let install_renv = install::plan_packages(search, Some(record), change);
        if let Some(reason) = &install_renv.refused {
            plan.refused_fix.clone_from(&install_renv.refused_fix);
            bail!("{reason}");
        }
        add_step(
            plan,
            &format!(
                "Install renv in {} so R can read the lock file",
                install_renv.folder
            ),
            install_renv.command.clone(),
        );
        (PathBuf::from(&library.path), Some(Box::new(install_renv)))
    };
    let program = runtime.program.with_file_name("Rscript");
    let program = if install::executable(&program) {
        program
    } else {
        runtime.program.clone()
    };
    let use_rscript = program.file_name().is_some_and(|s| s == "Rscript");
    plan.what = "Set up the R packages in renv.lock".into();
    plan.folder = root.to_string_lossy().into_owned();
    let cache = own
        .map(|p| p.join("renv/cache"))
        .or_else(|| renv_cache(&runtime.program, project))
        .unwrap_or_else(|| {
            let home = install::home().unwrap_or_else(|| PathBuf::from("~"));
            let default = if cfg!(target_os = "macos") {
                "Library/Caches/org.R-project.R/R/renv/cache"
            } else {
                ".cache/R/renv/cache"
            };
            std::env::var_os("RENV_PATHS_CACHE")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("RENV_PATHS_ROOT").map(|p| PathBuf::from(p).join("cache"))
                })
                .unwrap_or_else(|| home.join(default))
        });
    plan.notes.push(format!(
        "renv keeps its shared package cache in {}.",
        cache.display()
    ));
    if let Some(want) = &pins.r_version
        && major_minor(want) != major_minor(&r.version)
    {
        plan.notes.push(format!("This file was made with R {want}. This project is using R {}; some versions may not install.", r.version));
    }
    let project_r = r_string(project);
    let root_r = r_string(&root);
    let renv_r = r_string(&renv);
    // renv restores from the lock file's own sources; a chosen mirror
    // stands in for CRAN and Bioconductor there.
    let mut mirrors = String::new();
    if !sources.is_main("cran") {
        mirrors.push_str(&format!(
            " options(renv.config.repos.override = c(CRAN = {}));",
            package_plan::r_string(&sources.cran)
        ));
    }
    if !sources.is_main("bioconductor") {
        mirrors.push_str(&format!(
            " options(BioC_mirror = {});",
            package_plan::r_string(&sources.bioconductor)
        ));
    }
    let prelude = format!(
        ".libPaths(c({renv_r}, .libPaths())); options(renv.consent=TRUE); Sys.setenv(RENV_PATHS_LIBRARY={root_r}, RENV_PATHS_RENV=file.path({project_r}, 'renv'), RENV_PATHS_LOCKFILE=file.path({project_r}, 'renv.lock'), RENV_PROFILE='');{mirrors}"
    );
    let command = |expression: &str| {
        let code = format!("{prelude} {expression}");
        let line = install::quote(
            [
                program.to_string_lossy().as_ref(),
                "--no-site-file",
                "--no-init-file",
            ]
            .into_iter()
            .chain((!use_rscript).then_some("--no-restore"))
            .chain((!use_rscript).then_some("--no-save"))
            .chain(["-e", &code]),
        );
        let line = own.map_or(line.clone(), |own| own_env::command(own, &line));
        format!(
            "cd {} && {line}",
            install::quote([project.to_string_lossy().as_ref()])
        )
    };
    let restore = format!(
        "renv::restore(project={project_r}, library=renv::paths$library(project={project_r}), prompt=FALSE, clean=FALSE)"
    );
    add_step(
        plan,
        "Restore the versions in renv.lock into this project's library",
        command(&restore),
    );
    if !project_renv.join("activate.R").is_file() {
        plan.notes.push("renv adds an entry to .Rprofile and creates renv/activate.R so R uses this library when you open the project.".into());
        add_step(
            plan,
            "Turn on the restored library for this project",
            command(&format!("renv::activate(project={project_r})")),
        );
    }
    let target = InstallTarget::R {
        r: r_program,
        library: project_lib.to_string_lossy().into_owned(),
    };
    Ok(Effect::R {
        install_renv,
        target,
        library: project_lib,
    })
}

fn r_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy()).expect("string")
}

fn renv_cache(program: &Path, project: &Path) -> Option<PathBuf> {
    let mut command = std::process::Command::new(program);
    command.current_dir(project);
    // Read the configured cache without loading renv, which can write to it.
    let code = "v <- Sys.getenv('RENV_PATHS_CACHE'); if (!nzchar(v)) { root <- Sys.getenv('RENV_PATHS_ROOT'); if (!nzchar(root)) root <- tools::R_user_dir('renv', 'cache'); v <- file.path(root, 'cache') }; cat('SANDHILL_RENV_CACHE\\t', path.expand(v), '\\n', sep='')";
    let path = install::probe_command_with(
        command,
        &[
            "--no-init-file",
            "--no-site-file",
            "--no-save",
            "--no-restore",
            "-s",
            "-e",
            code,
        ],
        std::time::Duration::from_secs(5),
        false,
    )?;
    path.lines()
        .find_map(|line| line.strip_prefix("SANDHILL_RENV_CACHE\t"))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

fn pip_command(
    project: &Path,
    dst: &Path,
    requirements: &Path,
    uv: Option<&Path>,
    own: Option<&Path>,
    kernel: bool,
    sources: &Sources,
) -> String {
    let python = dst.join("bin/python");
    let python = python.to_string_lossy();
    let uv = uv.map(|p| p.to_string_lossy());
    let mut args = vec![
        "/usr/bin/env",
        "-u",
        "PIP_TARGET",
        "-u",
        "PIP_PREFIX",
        "-u",
        "PIP_ROOT",
        "-u",
        "PYTHONPATH",
        "-u",
        "PYTHONHOME",
        "PIP_USER=0",
    ];
    if let Some(uv) = &uv {
        args.extend([uv.as_ref(), "pip", "install", "--python", &python]);
    } else {
        args.extend([&python, "-I", "-m", "pip", "--isolated", "install"]);
    }
    args.extend(sources.pip_index());
    let requirements = requirements.to_string_lossy();
    args.extend(["-r", &requirements]);
    if kernel {
        args.push("ipykernel");
    }
    let line = install::quote(args);
    let line = if let Some(own) = own {
        own_env::command(own, &line)
    } else {
        line
    };
    format!(
        "cd {} && {line}",
        install::quote([project.to_string_lossy().as_ref()])
    )
}

fn setup_venv(
    project: &Path,
    search: &Search,
    runtime: impl FnOnce(&str) -> Result<Runtime>,
    path: &Path,
    pins: &FilePins,
    plan: &mut ProjectSetupPlan,
    sources: &Sources,
) -> Result<Effect> {
    let dst = project.join(".venv");
    ensure_not_symlink(&dst)?;
    let python = dst.join("bin/python");
    let new = dst.symlink_metadata().is_err();
    ensure!(
        new || (dst.join("pyvenv.cfg").is_file() && install::executable(&python)),
        "The project's .venv isn't a working Python environment. Sandhill won't replace it."
    );
    let uv = install::uv(search);
    plan.what = "Set up this project's Python packages in .venv".into();
    plan.folder = dst.to_string_lossy().into_owned();
    let tool = if uv.is_some() {
        InstallTool::Uv
    } else {
        InstallTool::Pip
    };
    if new {
        let base = runtime("python")?.program;
        let base = base.to_string_lossy();
        let create = if let Some(uv) = &uv {
            install::quote([
                uv.to_string_lossy().as_ref(),
                "venv",
                "--no-project",
                "--no-python-downloads",
                "--python",
                &base,
                &plan.folder,
            ])
        } else {
            install::quote([&base, "-I", "-m", "venv", &plan.folder])
        };
        add_step(
            plan,
            "Make a Python virtual environment in this project",
            create,
        );
    }
    let kernel = !inventory::python_packages(&dst, inventory::prefix_sites(&dst), false)
        .contains_key("ipykernel")
        && !pins
            .pins
            .iter()
            .any(|p| p.package.name.eq_ignore_ascii_case("ipykernel") && p.unconditional);
    add_step(
        plan,
        if kernel {
            "Install the packages in requirements.txt and add the Python kernel"
        } else {
            "Install the packages in requirements.txt"
        },
        pip_command(project, &dst, path, uv.as_deref(), None, kernel, sources),
    );
    Ok(Effect::Python {
        new,
        pip: tool,
        tool: uv
            .unwrap_or_else(|| python.clone())
            .to_string_lossy()
            .into_owned(),
    })
}

pub(crate) async fn run(record_path: &Path, work: Work) -> Result<()> {
    let plan = &work.plan;
    let project = Path::new(&plan.project);
    let folder = Path::new(&plan.folder);
    ensure!(
        project.canonicalize().is_ok_and(|p| p == project),
        "The project folder changed. Review the setup again."
    );
    ensure!(
        record::digest(read_file(project, &plan.file)?.as_bytes()) == work.fingerprint,
        "The package file changed. Review the setup again."
    );
    ensure!(
        own_env::chosen(record_path) == work.own,
        "The R and Python setup changed. Review this plan again."
    );
    match work.effect {
        Effect::R {
            install_renv,
            target,
            library,
        } => {
            checked_root(project, work.own.as_deref(), folder)?;
            checked_root(project, work.own.as_deref(), &library)?;
            for path in [
                project.join("renv"),
                project.join("renv/activate.R"),
                project.join(".Rprofile"),
            ] {
                ensure_not_symlink(&path)?;
            }
            let existed = folder.is_dir();
            let old_identity = record::directory_identity(folder);
            let owned = old_identity.as_ref().is_some_and(|identity| {
                record::load(record_path).is_ok_and(|rows| {
                    rows.into_iter().any(|row| {
                        row.kind == record::RENV_LIBRARY
                            && row.place == plan.folder
                            && row.version == *identity
                    })
                })
            });
            let undo_id = super::super::environments::id(&target);
            if let Some(install_renv) = &install_renv {
                package_plan::run(record_path, install_renv)?;
            }
            let pending = if library.is_dir() {
                Some(super::super::undo::begin(
                    record_path,
                    "setting up the project's packages".into(),
                    vec![super::super::undo::Track {
                        target,
                        folder: library,
                        tool: InstallTool::R,
                        path: String::new(),
                    }],
                )?)
            } else {
                None
            };
            let start = usize::from(install_renv.is_some());
            let result = run_steps(&plan.steps[start..]);
            if let Some(pending) = pending {
                pending.finish(record_path)?;
            }
            if folder.is_dir() && (!existed || owned) {
                let name = project.file_name().unwrap_or_default().to_string_lossy();
                let version = record::directory_identity(folder)
                    .context("The new R library couldn't be found.")?;
                let installed = record::entry(
                    record::RENV_LIBRARY,
                    name,
                    version,
                    &plan.folder,
                    "renv",
                    "Set up this project's packages",
                );
                record::add(record_path, [installed.clone()])?;
                if !existed {
                    super::super::undo::created(record_path, &undo_id, installed)?;
                }
            }
            result?;
        }
        Effect::Conda { create } => {
            checked_root(
                project,
                work.own.as_deref().or_else(|| folder.parent()),
                folder,
            )?;
            env_actions::run(record_path, *create).await?;
            let result = run_steps(&plan.steps[1..]);
            let python = folder.join("bin/python");
            if install::executable(&python) {
                let target = InstallTarget::Python {
                    python: python.to_string_lossy().into_owned(),
                    prefix: plan.folder.clone(),
                };
                if let Some(installed) = record::load(record_path)?
                    .into_iter()
                    .find(|row| row.kind == record::CONDA_ENV && row.place == plan.folder)
                {
                    super::super::undo::created(
                        record_path,
                        &super::super::environments::id(&target),
                        installed,
                    )?;
                }
            }
            result?;
        }
        Effect::Python { new, pip, tool } => {
            checked_root(project, None, folder)?;
            ensure!(
                folder.symlink_metadata().is_err() == new,
                "The project's .venv changed. Review the setup again."
            );
            let python = folder.join("bin/python").to_string_lossy().into_owned();
            let target = InstallTarget::Python {
                python,
                prefix: plan.folder.clone(),
            };
            if new {
                run_steps(&plan.steps[..1])?;
                let result = run_steps(&plan.steps[1..]);
                let installed = record::entry(
                    record::PYTHON_ENV,
                    ".venv",
                    "",
                    &plan.folder,
                    pip.id(),
                    "Set up this project's packages",
                );
                record::add(record_path, [installed.clone()])?;
                super::super::undo::created(
                    record_path,
                    &super::super::environments::id(&target),
                    installed,
                )?;
                result?;
            } else {
                let old =
                    inventory::python_packages(folder, inventory::prefix_sites(folder), false);
                let undo = super::super::undo::begin(
                    record_path,
                    "setting up the project's packages".into(),
                    vec![super::super::undo::Track {
                        target,
                        folder: folder.into(),
                        tool: pip,
                        path: tool,
                    }],
                )?;
                let result = run_steps(&plan.steps);
                undo.finish(record_path)?;
                let package_versions =
                    inventory::python_packages(folder, inventory::prefix_sites(folder), false);
                record::add(
                    record_path,
                    package_versions
                        .into_iter()
                        .filter(|(key, _)| !old.contains_key(key))
                        .map(|(key, pkg)| {
                            record::entry(
                                record::PYTHON_PACKAGE,
                                key,
                                pkg.version,
                                &plan.folder,
                                pip.id(),
                                "Set up this project's packages",
                            )
                        }),
                )?;
                result?;
            }
        }
    }
    Ok(())
}

fn run_steps(steps: &[SetupStep]) -> Result<()> {
    for step in steps {
        println!("{}", step.what);
        install::run_command(&step.command)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::test_common::tempdir;
    use std::os::unix::fs::PermissionsExt as _;

    fn executable(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn runtime(program: &Path, language: &str) -> Result<Runtime> {
        Ok(Runtime {
            program: program.into(),
            spec: JupyterKernelspec {
                argv: vec![program.display().to_string()],
                language: language.into(),
                display_name: language.into(),
                metadata: None,
                interrupt_mode: None,
                env: None,
            },
        })
    }

    #[test]
    fn plans_renv_inside_project_or_own_folder_and_refuses_untrusted() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project = root.join("example");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("renv.lock"),
            include_str!("../../../tests/fixtures/project-renv.lock"),
        )
        .unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let lib = root.join("R/library");
        fs::create_dir_all(lib.join("renv")).unwrap();
        fs::write(
            lib.join("renv/DESCRIPTION"),
            "Package: renv\nVersion: 1.1\nRepository: CRAN\n",
        )
        .unwrap();
        let r = root.join("R/bin/R");
        executable(
            &r,
            &format!(
                "#!/bin/sh\n[ \"$HOME\" = '{}' ] || exit 10\nprintf '4.5.2\\taarch64-apple-darwin20\\nSANDHILL_LIBPATH\\t{}\\tyes\\t1\\n'\n",
                home.display(),
                lib.display()
            ),
        );
        let wrapper = root.join("R/R");
        executable(
            &wrapper,
            &format!(
                "#!/bin/sh\nexport HOME='{}'\nexec '{}' \"$@\"\n",
                home.display(),
                r.display()
            ),
        );
        let own = root.join("sandhill");
        let record = root.join(record::FILE);
        for search in [
            Search::default(),
            Search::default().in_own(Some(own.clone())),
        ] {
            let (plan, work) = prepare(&project, &search, &record, "renv.lock", true, |_| {
                runtime(&wrapper, "r")
            });
            assert!(plan.refused.is_none(), "{plan:?}");
            assert!(work.is_some());
            assert_eq!(
                plan.folder,
                renv_root(&project, search.own.as_deref())
                    .display()
                    .to_string()
            );
            assert!(plan.steps[0].command.contains("renv::restore"));
            assert!(plan.steps[1].command.contains("renv::activate"));
            assert!(plan.notes.iter().any(|note| note.contains(".Rprofile")));
            assert_eq!(
                plan.steps[0].command.contains("RENV_PATHS_LIBRARY_ROOT"),
                search.own.is_some()
            );
        }
        let (plan, work) = prepare(
            &project,
            &Search::default(),
            &record,
            "renv.lock",
            false,
            |_| panic!("R shouldn't run in Restricted Mode"),
        );
        assert!(work.is_none());
        assert!(plan.refused.unwrap().contains("Trust this project"));
    }

    #[test]
    fn plans_python_setups_and_reports_what_differs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project = root.join("example");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("requirements.txt"),
            "pandas==2.3.0\nnumpy==1.0.0\n",
        )
        .unwrap();
        fs::write(project.join("environment.yaml"), "name: experiment\nchannels: [conda-forge, bioconda]\ndependencies: [python=3.12, numpy=1.0.0]\n").unwrap();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let record = root.join(record::FILE);
        let python = root.join("bin/python3");
        executable(&python, "#!/bin/sh\nexit 1\n");
        let own = root.join("sandhill");
        executable(&own.join("bin/micromamba"), "#!/bin/sh\nexit 1\n");
        fs::create_dir_all(own.join("conda/envs")).unwrap();
        let conda = root.join("conda");
        executable(&conda.join("bin/conda"), "#!/bin/sh\nexit 1\n");
        fs::create_dir_all(conda.join("conda-meta")).unwrap();
        fs::create_dir_all(conda.join("envs")).unwrap();
        let existing = Search {
            path: Some(home.as_os_str().into()),
            conda_roots: vec![conda.clone()],
            ..Search::default()
        };
        for (search, location) in [
            (&existing, conda.join("envs/experiment")),
            (
                &existing.clone().in_own(Some(own.clone())),
                own.join("conda/envs/experiment"),
            ),
        ] {
            let (yaml, work) = prepare(&project, search, &record, "environment.yaml", true, |_| {
                panic!("Conda doesn't need the project's Python")
            });
            assert!(yaml.refused.is_none(), "{yaml:?}");
            assert!(work.is_some());
            assert_eq!(Path::new(&yaml.folder), location);
            assert!(yaml.steps[0].command.contains("'env' 'create'"));
            assert_eq!(yaml.packages[0].version.as_deref(), Some("3.12"));
            let (requirements, _) =
                prepare(&project, search, &record, "requirements.txt", true, |_| {
                    runtime(&python, "python")
                });
            assert!(requirements.refused.is_none(), "{requirements:?}");
            if search.own.is_some() {
                assert!(Path::new(&requirements.folder).starts_with(own.join("conda/envs")));
            } else {
                assert_eq!(Path::new(&requirements.folder), project.join(".venv"));
            }
            assert!(
                requirements
                    .steps
                    .iter()
                    .any(|s| s.command.contains("requirements.txt"))
            );
        }
        let venv = project.join(".venv");
        fs::create_dir(&venv).unwrap();
        fs::write(venv.join("pyvenv.cfg"), "").unwrap();
        let package = venv.join("lib/python3.12/site-packages/pandas-2.4.0.dist-info");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("METADATA"), "Name: pandas\nVersion: 2.4.0\n").unwrap();
        let files = list(&project, &existing, |_| unreachable!());
        let requirements = files.iter().find(|f| f.file == "requirements.txt").unwrap();
        assert_eq!(requirements.packages, 2);
        assert!(!requirements.set_up);
        assert_eq!(
            requirements.differences,
            ["pandas 2.3.0 (have 2.4.0)", "numpy 1.0.0 (not installed)"]
        );
        fs::write(project.join("requirements.txt"), "pandas==2.4.0\n").unwrap();
        assert!(
            list(&project, &existing, |_| unreachable!())
                .into_iter()
                .find(|f| f.file == "requirements.txt")
                .unwrap()
                .set_up
        );
    }
}
