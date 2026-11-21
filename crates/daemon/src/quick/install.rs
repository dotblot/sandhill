//! Confirmed package and runtime changes, run in independent jobs.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    InstallChoice, InstallPrefs, InstallTool, InstallToolInfo, InstalledKey, JobSpec, NewRuntime,
    PackageAction, PackagePlan, QuickCondaTool, RuntimePlan,
};
use serde::{Deserialize, Serialize};

use super::{python, record, runtime_plan};
use crate::agents::shell_quote;
use crate::kernelspecs::Search;
use crate::terminal::Launch;

pub(crate) use super::package_plan::plan as plan_packages;
pub(crate) use super::runtime_plan::plan as plan_runtime;

pub(super) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(super) fn quote(args: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    args.into_iter()
        .map(|arg| shell_quote(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn executable(path: &Path) -> bool {
    path.is_absolute()
        && fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub(super) fn on_path(search: &Search, name: &str) -> Option<PathBuf> {
    std::env::split_paths(search.path.as_ref()?)
        .map(|dir| dir.join(name))
        .find(|p| executable(p))
}

pub(super) fn uv(search: &Search) -> Option<PathBuf> {
    on_path(search, "uv").or_else(|| {
        let home = home()?;
        [".local/bin/uv", ".cargo/bin/uv"]
            .map(|relative| home.join(relative))
            .into_iter()
            .find(|p| executable(p))
    })
}

pub(super) fn brew(search: &Search) -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| executable(p))
        .or_else(|| on_path(search, "brew"))
}

pub(super) fn conda(search: &Search, prefix: &Path) -> Option<PathBuf> {
    let tool = PathBuf::from(crate::irkernel::conda_tool(prefix));
    if executable(&tool) {
        Some(tool)
    } else if !tool.is_absolute() {
        python::find_tool(search).map(|(tool, _)| tool)
    } else {
        None
    }
}

pub(super) fn info(tool: InstallTool, path: &Path) -> InstallToolInfo {
    InstallToolInfo {
        tool,
        path: path.to_string_lossy().into_owned(),
        version: if tool == InstallTool::Pip {
            probe(path, &["-I", "-m", "pip", "--version"])
                .and_then(|out| sandhill_proto::version_in(out.lines().next()?))
                .unwrap_or_default()
        } else {
            python::tool_version(path)
        },
    }
}

pub(super) fn readiness_tools(
    search: &Search,
    conda: Option<&QuickCondaTool>,
) -> (Vec<InstallToolInfo>, Option<String>, String) {
    let mut tools = Vec::new();
    if let Some(conda) = conda {
        tools.push(InstallToolInfo {
            tool: InstallTool::Conda,
            path: conda.path.clone(),
            version: conda.version.clone(),
        });
    }
    if search.own.is_some() {
        return (tools, None, String::new());
    }
    if let Some(uv) = uv(search) {
        tools.push(info(InstallTool::Uv, &uv));
    }
    let brew = brew(search);
    let version = brew
        .as_deref()
        .and_then(|brew| {
            sandhill_proto::version_in(probe_brew(brew, &["--version"])?.lines().next()?)
        })
        .unwrap_or_default();
    (
        tools,
        brew.map(|p| p.to_string_lossy().into_owned()),
        version,
    )
}

pub(super) fn probe_brew(program: &Path, args: &[&str]) -> Option<String> {
    // Even `brew --version` writes a tool-selection cache on some versions.
    let scratch = tempfile::Builder::new()
        .prefix("sandhill-brew-")
        .tempdir()
        .ok()?;
    let mut command = Command::new(program);
    command
        .env("HOMEBREW_CACHE", scratch.path().join("cache"))
        .env("HOMEBREW_LOGS", scratch.path().join("logs"))
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1");
    probe_command(command, args)
}

pub(super) fn probe_conda_search(program: &Path, args: &[&str]) -> Option<String> {
    probe_conda_search_timeout(program, args, Duration::from_secs(25))
}

pub(super) fn probe_conda_search_timeout(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    if crate::own_env::containing(program).is_some() {
        // Sandhill's shim already sends every file to its own folder.
        return probe_timeout(program, args, None, timeout);
    }
    let scratch = tempfile::Builder::new()
        .prefix("sandhill-conda-")
        .tempdir()
        .ok()?;
    let mut command = Command::new(program);
    command
        .env("HOME", scratch.path())
        .env("XDG_CACHE_HOME", scratch.path().join("cache"))
        .env("CONDA_PKGS_DIRS", scratch.path().join("pkgs"));
    probe_command_with(command, args, timeout, false)
}

/// Run a bounded probe. Read stdout as it arrives: a conda search can fill the pipe.
pub(super) fn probe(program: &Path, args: &[&str]) -> Option<String> {
    probe_in(program, args, None)
}

pub(super) fn probe_in(program: &Path, args: &[&str], own: Option<&Path>) -> Option<String> {
    probe_timeout(program, args, own, Duration::from_secs(25))
}

pub(super) fn probe_timeout(
    program: &Path,
    args: &[&str],
    own: Option<&Path>,
    timeout: Duration,
) -> Option<String> {
    scoped_probe(program, args, own, timeout, false)
}

pub(super) fn probe_allow_failure(
    program: &Path,
    args: &[&str],
    own: Option<&Path>,
) -> Option<String> {
    scoped_probe(program, args, own, Duration::from_secs(25), true)
}

fn scoped_probe(
    program: &Path,
    args: &[&str],
    own: Option<&Path>,
    timeout: Duration,
    allow_failure: bool,
) -> Option<String> {
    let mut command = Command::new(program);
    if let Some(folder) = own {
        crate::own_env::apply(&mut command, folder);
        command.current_dir(folder);
    }
    probe_command_with(command, args, timeout, allow_failure)
}

fn probe_command(command: Command, args: &[&str]) -> Option<String> {
    probe_command_with(command, args, Duration::from_secs(25), false)
}

pub(super) fn probe_command_with(
    mut command: Command,
    args: &[&str],
    timeout: Duration,
    allow_failure: bool,
) -> Option<String> {
    let mut process = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = process.stdout.take()?;
    std::thread::scope(|scope| {
        let read = scope.spawn(move || {
            let mut text = String::new();
            stdout.read_to_string(&mut text).ok()?;
            Some(text)
        });
        let started = Instant::now();
        let ok = loop {
            match process.try_wait() {
                Ok(Some(status)) => break status.success() || allow_failure,
                Ok(None) if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(15))
                }
                _ => {
                    let _ = process.kill();
                    let _ = process.wait();
                    break false;
                }
            }
        };
        let text = read.join().ok().flatten();
        text.filter(|_| ok)
    })
}

pub(super) fn lock(path: &Path, wait: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    if wait {
        file.lock()?
    } else {
        file.try_lock().map_err(io::Error::from)?
    }
    Ok(file)
}

fn prefs_path(record: &Path) -> PathBuf {
    record.with_file_name("install-prefs.json")
}

pub(crate) fn prefs(record: &Path) -> InstallPrefs {
    fs::read(prefs_path(record))
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

pub(crate) fn update_prefs(
    record: &Path,
    update: impl FnOnce(&mut InstallPrefs),
) -> Result<InstallPrefs> {
    let _lock = lock(&record.with_file_name(".install-prefs.lock"), true)?;
    let mut state = prefs(record);
    update(&mut state);
    crate::files::write_atomic(&prefs_path(record), &serde_json::to_vec_pretty(&state)?)?;
    Ok(state)
}

pub(crate) fn remember(record: &Path, plan: &PackagePlan) -> Result<()> {
    if plan.change.action == PackageAction::Install
        && let Some(tool) = plan.change.tool
    {
        let target = &plan.change.target;
        let language = target.language();
        update_prefs(record, |prefs| {
            prefs.last.retain(|choice| choice.language != language);
            prefs.last.push(InstallChoice {
                language: language.into(),
                target: target.clone(),
                tool,
            });
        })?;
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Job {
    Packages(PackagePlan),
    Runtime(RuntimePlan),
    OwnSetup(super::own::Work),
    Environment(super::env_actions::Work),
    ProjectSetup(super::project_packages::Work),
    RemoveOwn(Vec<PathBuf>, Vec<InstalledKey>),
    Remove(Vec<InstalledKey>),
    Agent {
        command: String,
        name: String,
        method: String,
    },
    /// The existing IRkernel job writes its own record while holding the same lock.
    Irkernel {
        command: String,
        prefix: Option<String>,
    },
}

pub(crate) fn worker_command(exe: &Path, record: &Path, job: &Job) -> Result<String> {
    Ok(format!(
        "{} install-worker {} {}",
        shell_quote(&exe.to_string_lossy()),
        shell_quote(&record.to_string_lossy()),
        shell_quote(&serde_json::to_string(job)?)
    ))
}

fn job_payload(log: &Path, job: &Job) -> Result<String> {
    use std::io::Write as _;
    let path = log.with_extension("work.json");
    let mut file = tempfile::NamedTempFile::new_in(
        path.parent().context("The job log folder wasn't found.")?,
    )?;
    serde_json::to_writer(&mut file, job)?;
    file.flush()?;
    file.persist_noclobber(&path)?;
    Ok(format!("@{}", path.display()))
}

pub(crate) fn launch(
    exe: &Path,
    record: &Path,
    job: Job,
    log: &Path,
    title: &str,
) -> Result<Launch> {
    let own_install = matches!(
        &job,
        Job::OwnSetup(_) | Job::RemoveOwn(..) | Job::Environment(_) | Job::ProjectSetup(_)
    );
    let worker = if matches!(&job, Job::OwnSetup(_) | Job::ProjectSetup(_)) {
        format!(
            "{} install-worker {} {}",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&record.to_string_lossy()),
            shell_quote(&job_payload(log, &job)?)
        )
    } else {
        worker_command(exe, record, &job)?
    };
    let spec = JobSpec {
        path: None,
        command: Some(format!(
            "set -o pipefail; {worker} 2>&1 | tee {}",
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&spec, Some(&home().context("HOME isn't set")?), None)?;
    if own_install {
        // Setup/removal don't depend on any personal shell settings.
        launch.program = "/bin/bash".into();
        launch.args = vec![
            "--noprofile".into(),
            "--norc".into(),
            "-c".into(),
            spec.command.unwrap(),
        ];
        launch.env.push(("BASH_ENV".into(), "/dev/null".into()));
        // A removal must go through the daemon again to stop live kernels.
        launch.job = None;
    }
    launch.title = title.into();
    super::install_failure::remember(log, &job)?;
    Ok(launch)
}

/// "Install DESeq2 and edgeR", "Remove 4 packages".
pub(crate) fn package_title(plan: &PackagePlan) -> String {
    let verb = match plan.change.action {
        PackageAction::Install => "Install",
        PackageAction::Update => "Update",
        PackageAction::Remove => "Remove",
    };
    match plan.packages.as_slice() {
        [one] => format!("{verb} {}", one.name),
        [one, two] => format!("{verb} {} and {}", one.name, two.name),
        many => format!("{verb} {} packages", many.len()),
    }
}

pub(crate) fn run_command(command: &str) -> Result<()> {
    run_command_with_path(command, None)
}

fn run_command_with_path(command: &str, path: Option<&str>) -> Result<()> {
    let mut shell = Command::new("/bin/bash");
    if let Some(path) = path {
        shell.env("PATH", path);
    }
    let status = shell
        .env_remove("BASH_ENV")
        .args(["--noprofile", "--norc", "-o", "pipefail", "-c", command])
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        bail!("The command didn't finish. The messages above say why.");
    }
    Ok(())
}

/// Entry point for the terminal job, so recording continues if the daemon restarts.
#[doc(hidden)]
pub async fn worker(record_path: &Path, payload: &str) -> Result<()> {
    let job = if let Some(path) = payload.strip_prefix('@') {
        let path = Path::new(path);
        let job = serde_json::from_reader(File::open(path)?)?;
        fs::remove_file(path)?;
        job
    } else {
        serde_json::from_str(payload)?
    };
    let _lock = match record::lock(record_path, false) {
        Ok(lock) => lock,
        Err(_) => {
            println!("Waiting for another install or removal to finish.");
            record::lock(record_path, true)?
        }
    };
    super::undo::prune_locked(record_path);
    match job {
        Job::Packages(plan) => super::package_plan::run(record_path, &plan),
        Job::Runtime(plan) => runtime_plan::run(record_path, &plan),
        Job::OwnSetup(plan) => super::own::run(record_path, &plan),
        Job::Environment(work) => super::env_actions::run(record_path, work).await,
        Job::ProjectSetup(work) => super::project_packages::run(record_path, work).await,
        Job::RemoveOwn(folders, keys) => {
            for folder in folders {
                super::own::remove(record_path, &folder)?;
            }
            if keys.is_empty() {
                Ok(())
            } else {
                record::remove(record_path, &keys)
            }
        }
        Job::Remove(keys) => record::remove(record_path, &keys),
        Job::Agent {
            command,
            name,
            method,
        } => {
            let program = match method.as_str() {
                "brew" => brew(&Search::from_env()).context("Homebrew wasn't found.")?,
                "npm" => crate::agents::find("npm").context("Node.js wasn't found.")?,
                "native" if name == "claude" => PathBuf::from("/bin/bash"),
                _ => bail!("Sandhill doesn't know how to install this agent."),
            };
            if crate::agents::find(&name).is_some()
                || record::agent_version(&name, &method).is_some()
            {
                bail!("This agent is already installed.");
            }
            let path = crate::agents::path_for(&program)?;
            run_command_with_path(&command, path.as_deref())?;
            let version = record::agent_version(&name, &method).unwrap_or_default();
            record::add(
                record_path,
                [record::entry(
                    record::AGENT,
                    &name,
                    version,
                    &method,
                    &method,
                    "Installed from Manage, under Agents",
                )],
            )
        }
        Job::Irkernel {
            command,
            prefix: None,
        } => run_command(&command),
        Job::Irkernel {
            command,
            prefix: Some(prefix),
        } => {
            let before = record::conda_packages(Path::new(&prefix))?;
            let result = run_command(&command);
            let after = record::conda_packages(Path::new(&prefix))?;
            record::add(
                record_path,
                after
                    .into_iter()
                    .filter(|(name, _)| !before.contains_key(name))
                    .map(|(name, version)| {
                        let reason = if name == "r-irkernel" {
                            "R needs it to run in Sandhill"
                        } else {
                            ""
                        };
                        record::entry(
                            record::CONDA_PACKAGE,
                            name,
                            version,
                            &prefix,
                            "conda",
                            reason,
                        )
                    }),
            )?;
            result
        }
    }
}

pub(crate) fn runtime_title(runtime: &NewRuntime) -> &'static str {
    match runtime {
        NewRuntime::RFromCran | NewRuntime::RFromHomebrew => "Install R",
        NewRuntime::RConda { .. } => "Create an R environment",
        NewRuntime::PythonConda { .. } | NewRuntime::PythonUv { .. } => {
            "Create a Python environment"
        }
        NewRuntime::Miniforge => "Install Miniforge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sandhill_proto::InstallTarget;

    #[test]
    fn own_readiness_brew_probe_uses_and_removes_a_temporary_cache() {
        let temp = tempfile::tempdir().unwrap();
        let brew = temp.path().join("brew");
        fs::write(&brew, "#!/bin/sh\n[ \"$HOMEBREW_NO_AUTO_UPDATE:$HOMEBREW_NO_ANALYTICS\" = 1:1 ] || exit 1\nmkdir -p \"$HOMEBREW_CACHE\" && touch \"$HOMEBREW_CACHE/file\" && printf '%s\\n' \"$HOMEBREW_CACHE\"\n").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o700)).unwrap();
        let out = probe_brew(&brew, &["--version"]).unwrap();
        let cache = Path::new(out.trim());
        assert_eq!(cache.file_name().unwrap(), "cache");
        assert!(!cache.parent().unwrap().exists());
    }

    #[test]
    fn prefs_keep_the_last_choice_per_language_and_the_switch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(record::FILE);
        assert_eq!(prefs(&path), InstallPrefs::default());
        let choice = InstallChoice {
            language: "r".into(),
            target: InstallTarget::R {
                r: "/R/bin/R".into(),
                library: "/R/library".into(),
            },
            tool: InstallTool::R,
        };
        update_prefs(&path, |prefs| prefs.last.push(choice.clone())).unwrap();
        let updated = update_prefs(&path, |prefs| prefs.without_asking = true).unwrap();
        assert_eq!(prefs(&path), updated);
        assert_eq!(updated.last_for("r"), Some(&choice));
        assert!(updated.without_asking);
    }
}
