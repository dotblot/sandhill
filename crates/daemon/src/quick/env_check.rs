//! Load packages outside the user's sessions and start a short-lived kernel.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use jupyter_protocol::{JupyterMessage, ShutdownRequest};
use sandhill_proto::{EnvCheck, EnvProblem, InstallTool, PackageAction, PackageChange};

use super::{
    environments::{self, Environment, RPackage},
    install, package_plan, record,
};

pub(super) const REASON: &str = "Check and Repair";

pub(super) async fn run(record_path: &Path, env: &Environment) -> Result<()> {
    println!("Checking packages in {}.", env.card.name);
    let (checked, problems) = if env.is_r() {
        check_r(record_path, env)
    } else {
        check_python(env)
    };
    let kernel = if env.card.kernel.is_some() {
        println!("Starting the kernel to make sure it answers.");
        match check_kernel(record_path, env).await {
            Ok(()) => None,
            Err(err) => {
                println!("{}", err);
                Some(
                    "The kernel didn't start or answer. See the job's messages for details.".into(),
                )
            }
        }
    } else {
        None
    };
    let path = environments::state_path(record_path, "checks", &env.card.id);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let count = problems.len();
    let report = EnvCheck {
        date: record::today(),
        checked,
        kernel,
        problems,
    };
    crate::files::write_atomic(&path, &serde_json::to_vec(&report)?)?;
    println!("Checked {checked} packages. Found {count} package problems.");
    Ok(())
}

fn r_tool(env: &Environment, conda: &record::Versions, package: &str) -> Option<InstallTool> {
    let lower = package.to_ascii_lowercase();
    if env.tool(InstallTool::Conda).is_some()
        && [format!("r-{lower}"), format!("bioconductor-{lower}")]
            .iter()
            .any(|name| conda.contains_key(name))
    {
        Some(InstallTool::Conda)
    } else {
        env.tool(InstallTool::R).map(|_| InstallTool::R)
    }
}

fn fix(
    env: &Environment,
    library: Option<&str>,
    name: &str,
    action: PackageAction,
    tool: Option<InstallTool>,
) -> Option<PackageChange> {
    Some(PackageChange {
        action,
        target: env.target(library).ok()?,
        packages: vec![name.into()],
        tool: Some(tool?),
        reason: REASON.into(),
    })
}

fn quoted_name(text: &str) -> Option<&str> {
    for (open, close) in [('‘', '’'), ('\'', '\''), ('"', '"')] {
        if let Some((_, tail)) = text.split_once(open)
            && let Some((name, _)) = tail.split_once(close)
        {
            return Some(name);
        }
    }
    None
}

fn r_problem(
    record_path: &Path,
    env: &Environment,
    p: &RPackage,
    error: Option<&str>,
    conda: &record::Versions,
) -> Option<EnvProblem> {
    let wrong_build = environments::wrong_r_build(&p.description, &env.version);
    if wrong_build.is_none() && error.is_none() {
        return None;
    }
    let writable = env
        .card
        .libraries
        .iter()
        .any(|l| l.path == p.library && l.writable);
    let library = writable.then_some(p.library.as_str());
    let tool = r_tool(env, conda, &p.name);
    let origin = if tool == Some(InstallTool::R) {
        package_plan::r_archive_for(record_path, Path::new(&p.library), &p.name, &p.version)
            .or_else(|| {
                if record::r_field(&p.description, "RemoteType")? != "github" {
                    return None;
                }
                let repo = format!(
                    "{}/{}",
                    record::r_field(&p.description, "RemoteUsername")?,
                    record::r_field(&p.description, "RemoteRepo")?
                );
                package_plan::github_repo(&repo).then_some(repo)
            })
    } else {
        None
    };
    let mut repair = fix(
        env,
        library,
        origin.as_deref().unwrap_or(&p.name),
        if writable || tool == Some(InstallTool::Conda) {
            PackageAction::Update
        } else {
            PackageAction::Install
        },
        tool,
    );
    let reason = if let Some(old) = wrong_build {
        format!("Built for R {old}; this is R {}.", env.version)
    } else {
        let error = error?;
        let dependency = error
            .split_once("there is no package called")
            .and_then(|(_, tail)| quoted_name(tail));
        if let Some(dep) = dependency.filter(|dep| *dep != p.name && record::r_name(dep)) {
            repair = fix(
                env,
                library,
                dep,
                PackageAction::Install,
                env.tool(InstallTool::R).map(|_| InstallTool::R),
            );
            format!("Needs {dep}, which isn't installed.")
        } else if error.contains("unable to load shared object")
            || error.contains("symbol not found")
            || error.contains("dyn.load")
        {
            "Its compiled part can't load.".into()
        } else if error.is_empty() {
            "R stopped before this package could finish loading.".into()
        } else {
            format!("R couldn't load this package: {error}")
        }
    };
    Some(EnvProblem {
        package: p.name.clone(),
        reason,
        fix: repair,
    })
}

fn check_r(record_path: &Path, env: &Environment) -> (u32, Vec<EnvProblem>) {
    let packages = environments::r_packages(&env.card.libraries);
    let conda = env
        .conda
        .as_deref()
        .and_then(|p| record::conda_packages(p).ok())
        .unwrap_or_default();
    let mut problems = Vec::new();
    let mut tested = 0usize;
    for library in &env.card.libraries {
        let subset: Vec<_> = packages
            .iter()
            .filter(|p| p.library == library.path)
            .collect();
        for batch in subset.chunks(25) {
            let out = r_batch(env, &library.path, batch);
            for (i, p) in batch.iter().enumerate() {
                // A crashing namespace can take its batch down. Try each
                // unanswered package on its own rather than blaming its neighbors.
                let answer = out.as_ref().and_then(|answers| answers.get(i)?.as_deref());
                let retry;
                let answer = if answer.is_none() && batch.len() > 1 {
                    retry = r_batch(env, &library.path, &[*p]);
                    retry.as_ref().and_then(|a| a.first()?.as_deref())
                } else {
                    answer
                };
                let error = match answer {
                    Some("OK") => None,
                    Some(reason) => Some(reason),
                    None => Some(""),
                };
                if let Some(problem) = r_problem(record_path, env, p, error, &conda) {
                    problems.push(problem);
                }
            }
            tested += batch.len();
            println!("Checked {tested} of {} R packages.", packages.len());
        }
    }
    (tested.try_into().unwrap_or(u32::MAX), problems)
}

fn r_batch(env: &Environment, library: &str, batch: &[&RPackage]) -> Option<Vec<Option<String>>> {
    let names = batch
        .iter()
        .map(|p| package_plan::r_string(&p.name))
        .collect::<Vec<_>>()
        .join(",");
    let library = package_plan::r_string(library);
    let code = format!(
        r#"local({{
      lib <- {library}
      .libPaths(c(lib, .libPaths()))
      names <- c({names})
      for (i in seq_along(names)) {{
        reason <- tryCatch({{ suppressPackageStartupMessages(loadNamespace(names[i], lib.loc = .libPaths())); 'OK' }}, error = conditionMessage)
        cat('SANDHILL_CHECK', i, gsub('[[:space:]]+', ' ', reason), sep = '\t')
        cat('\n')
        flush.console()
      }}
    }})"#
    );
    let stdout = environments::r_probe(Path::new(&env.card.program), &code)?;
    let mut answers = vec![None; batch.len()];
    for line in stdout.lines() {
        if let Some((number, reason)) = line
            .strip_prefix("SANDHILL_CHECK\t")
            .and_then(|line| line.split_once('\t'))
            && let Ok(n) = number.parse::<usize>()
            && let Some(answer) = n.checked_sub(1).and_then(|i| answers.get_mut(i))
        {
            *answer = Some(reason.into());
        }
    }
    Some(answers)
}

fn python_tool(
    env: &Environment,
    managed: &std::collections::BTreeMap<String, String>,
    name: &str,
) -> Option<InstallTool> {
    if managed.contains_key(&record::python_name(name)) && env.tool(InstallTool::Conda).is_some() {
        Some(InstallTool::Conda)
    } else {
        env.card
            .tools
            .iter()
            .map(|t| t.tool)
            .find(|t| matches!(t, InstallTool::Uv | InstallTool::Pip))
    }
}

fn check_python(env: &Environment) -> (u32, Vec<EnvProblem>) {
    let program = Path::new(&env.card.program);
    let managed = env
        .conda
        .as_deref()
        .map(package_plan::conda_distribution_names)
        .unwrap_or_default();
    let mut problems = Vec::new();
    println!("Checking which Python packages need other packages.");
    if let Some(out) =
        install::probe_allow_failure(program, &["-I", "-m", "pip", "check"], env.own.as_deref())
    {
        for line in out.lines() {
            if let Some((owner, tail)) = line.split_once(" requires ")
                && let Some(dep) = tail.strip_suffix(", which is not installed.")
                && package_plan::python_spec(dep) == Some(dep)
            {
                let name = owner.split_whitespace().next().unwrap_or(owner);
                problems.push(EnvProblem {
                    package: name.into(),
                    reason: format!("Needs {dep}, which isn't installed."),
                    fix: fix(
                        env,
                        None,
                        dep,
                        PackageAction::Install,
                        python_tool(env, &managed, name),
                    ),
                });
            } else if let Some((owner, detail)) = line.split_once(" has requirement ") {
                let name = owner.split_whitespace().next().unwrap_or(owner);
                problems.push(EnvProblem {
                    package: name.into(),
                    reason: format!("Needs a different package version: {detail}"),
                    fix: fix(
                        env,
                        None,
                        name,
                        PackageAction::Update,
                        python_tool(env, &managed, name),
                    ),
                });
            }
        }
    }
    println!("Loading Python packages from the kits.");
    let installed =
        package_plan::python_inventory(program, Path::new(&env.card.folder)).unwrap_or_default();
    let names: BTreeSet<_> = package_plan::kits()
        .flat_map(|kit| kit.python.iter().map(|&(_, p)| record::python_name(p)))
        .chain(["ipykernel".into()])
        .filter(|p| installed.contains_key(p))
        .collect();
    let code = r#"import contextlib, importlib, importlib.metadata, re, sys
norm = lambda n: re.sub(r'[-_.]+', '-', n).lower()
projects = {n: [] for n in sys.argv[1:]}
for mod, owners in importlib.metadata.packages_distributions().items():
    if mod.isidentifier() and not mod.startswith('_'):
        for owner in owners:
            if norm(owner) in projects:
                projects[norm(owner)].append(mod)
for package, modules in projects.items():
    for mod in modules or [package.replace('-', '_')]:
        try:
            with contextlib.redirect_stdout(sys.stderr):
                importlib.import_module(mod)
        except Exception as error:
            dependency = getattr(error, 'name', '') if isinstance(error, ModuleNotFoundError) else ''
            print('SANDHILL_CHECK', package, mod, dependency or '', ' '.join(str(error).split()), sep='\t', flush=True)
            break
"#;
    let args = ["-I", "-c", code]
        .into_iter()
        .chain(names.iter().map(String::as_str))
        .collect::<Vec<_>>();
    if !names.is_empty() {
        if let Some(out) = install::probe_in(program, &args, env.own.as_deref()) {
            for line in out
                .lines()
                .filter_map(|line| line.strip_prefix("SANDHILL_CHECK\t"))
            {
                let fields: Vec<_> = line.splitn(4, '\t').collect();
                let [name, module, missing, error] = fields.as_slice() else {
                    continue;
                };
                let dependency = missing.split('.').next().unwrap_or_default();
                let missing = !dependency.is_empty() && dependency != *module;
                let name_for_install = match dependency {
                    "sklearn" => "scikit-learn",
                    "cv2" => "opencv-python",
                    "PIL" => "Pillow",
                    _ => dependency,
                };
                let (reason, action, install_name) = if missing {
                    (
                        format!("Needs {dependency}, which couldn't be loaded."),
                        PackageAction::Install,
                        name_for_install,
                    )
                } else {
                    (
                        format!("Python couldn't load {module}: {error}"),
                        PackageAction::Update,
                        *name,
                    )
                };
                problems.push(EnvProblem {
                    package: (*name).into(),
                    reason,
                    fix: fix(
                        env,
                        None,
                        install_name,
                        action,
                        python_tool(env, &managed, name),
                    ),
                });
            }
        } else {
            problems.push(EnvProblem {
                package: "Python packages".into(),
                reason: "Python stopped before it finished loading the packages.".into(),
                fix: None,
            });
        }
    }
    (names.len().try_into().unwrap_or(u32::MAX), problems)
}

async fn check_kernel(record_path: &Path, env: &Environment) -> Result<()> {
    let name = env
        .card
        .kernel
        .as_deref()
        .context("No kernel is installed.")?;
    let search = crate::kernelspecs::Search::from_env().in_own(env.own.clone());
    let spec =
        crate::kernelspecs::find(&search, name).context("The kernel could no longer be found.")?;
    let scratch = tempfile::Builder::new()
        .prefix("kernel-check-")
        .tempdir_in(record_path.parent().context("No Sandhill state folder.")?)?;
    let mut launch = crate::kernel::launch_scoped(
        spec,
        scratch.path(),
        &[],
        scratch.path(),
        "check",
        env.own.as_deref(),
        false,
    )
    .await?;
    let identity = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let handshake = async {
        let mut conn = crate::kernel::connect(&launch.connection_info, &identity, tx).await?;
        crate::session::wait_until_ready(&mut conn, &mut rx).await?;
        Ok::<_, anyhow::Error>(conn)
    };
    let result = match tokio::time::timeout(Duration::from_secs(20), handshake).await {
        Ok(Ok(mut conn)) => {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                conn.control.send(JupyterMessage::new(
                    ShutdownRequest { restart: false },
                    None,
                )),
            )
            .await;
            let _ = tokio::time::timeout(Duration::from_secs(3), launch.child.wait()).await;
            Ok(())
        }
        Ok(Err(error)) => Err(error),
        Err(_) => Err(anyhow::anyhow!(
            "The kernel didn't answer within 20 seconds."
        )),
    };
    if launch.child.try_wait()?.is_none() {
        launch.child.kill().await?;
    }
    if result.is_err()
        && let Ok(log) = std::fs::read_to_string(scratch.path().join("check.log"))
        && !log.trim().is_empty()
    {
        println!("{log}");
    }
    result
}
