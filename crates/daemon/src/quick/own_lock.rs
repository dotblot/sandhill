//! The lock while Sandhill's own R and Python are chosen: what Sandhill
//! starts (kernels, the Terminal's shell, jobs, and the agents that don't
//! run their own sandbox) can't write into the R libraries, Pythons, conda
//! folders and Homebrew folder Sandhill knows are the user's. Reading them
//! still works, and the user's own files stay writable. Apple's sandbox
//! (`sandbox-exec`) on a Mac; bubblewrap on Linux when it works for this
//! user (plan section 3, Huan 2026-11-20).
//!
//! The folders are found once per daemon run and again when the mode
//! changes.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use sandhill_proto::LockStatus;

use super::{environments, install};
use crate::kernelspecs::{self, Search, canonical};
use crate::{irkernel, own_env};

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

const NO_SANDBOX: &str = "Apple's sandbox isn't available on this Mac, so your other R, Python, conda and Homebrew aren't locked.";
const NO_BUBBLEWRAP: &str = "This computer has no bubblewrap, so your other R, Python, conda and Homebrew aren't locked. Installing bubblewrap (the bwrap program) turns the lock on.";
const BUBBLEWRAP_FAILS: &str = "Bubblewrap doesn't work for your account on this computer, so your other R, Python, conda and Homebrew aren't locked.";
const OTHER_SYSTEM: &str = "The lock works on a Mac and on Linux, so your other R, Python, conda and Homebrew aren't locked here.";
const NOTHING: &str = "Sandhill found no other R, Python, conda or Homebrew on this computer, so there is nothing to lock.";
/// Seen on macOS 27.2 (2026-11-21): inside a sandbox with any rule, no
/// other sandbox can be applied, which Codex's commands and Claude Code's
/// sandbox need. So an agent Sandhill starts runs under either its own
/// sandbox or the lock (Huan's choice through Main, 2026-11-21).
const AGENTS_ON_A_MAC: &str = "While the lock is on, Codex or Claude Code with their own sandbox won't work in Sandhill's Terminal. Sandhill's agent conversations are covered by the agent's own sandbox or by the lock.";
/// Set on an agent's terminal host on a Mac while the lock is on: `on`
/// when the host runs under the lock, `agent` when the agent's own
/// sandbox stands in for it.
pub(crate) const HOST_LOCK: &str = "SANDHILL_OWN_LOCK";
pub(crate) const CODEX_SWITCH: &str = "Sandhill's own R and Python are chosen, so on a Mac this conversation can't switch between Full access and the other modes. Start a new conversation for that.";

/// How what Sandhill starts is put under the lock.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wrapper {
    Sandbox,
    Bubblewrap(PathBuf),
}

/// What is started under the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    /// A terminal host for the Terminal's shell, a job, a preview or an
    /// agent without its own sandbox, and everything it starts.
    Host,
    /// A terminal host for an agent that runs its own sandbox: on a Mac
    /// that sandbox can't run inside the lock, so it is left out.
    SandboxedAgent,
    /// A kernel. One that isn't kept ends with what started it.
    Kernel { persistent: bool },
}

/// Whether a Codex conversation may change to `wanted`: on a Mac, one
/// started under the lock (Full access) can't use Codex's own sandbox, and
/// one started with Codex's sandbox would have neither in Full access.
pub(crate) fn codex_mode_allowed(wanted: Option<&str>, host: Option<&str>) -> bool {
    match host {
        Some("on") => wanted == Some("full"),
        Some("agent") => wanted != Some("full"),
        _ => true,
    }
}

/// Whether the agent of `kind` that Sandhill starts in `cwd` runs its own
/// sandbox: Codex unless given full access, Claude Code when its settings
/// switch its sandbox on (read only; `claude_config` is its settings
/// folder, `~/.claude` unless an added account has its own).
pub(crate) fn own_sandbox(
    kind: &str,
    permission: Option<&str>,
    cwd: &Path,
    claude_config: Option<&Path>,
) -> bool {
    match kind {
        crate::agents::CODEX => permission != Some("full"),
        crate::agents::CLAUDE => {
            let managed = if cfg!(target_os = "macos") {
                "/Library/Application Support/ClaudeCode/managed-settings.json"
            } else {
                "/etc/claude-code/managed-settings.json"
            };
            claude_sandbox(cwd, claude_config, Path::new(managed))
        }
        _ => false,
    }
}

/// Later files win: the user's, the project's, its local ones, then the
/// computer's managed settings.
fn claude_sandbox(cwd: &Path, config: Option<&Path>, managed: &Path) -> bool {
    config
        .map(|config| config.join("settings.json"))
        .into_iter()
        .chain([
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
            managed.to_path_buf(),
        ])
        .filter_map(|file| {
            let text = fs::read(file).ok()?;
            serde_json::from_slice::<serde_json::Value>(&text).ok()?["sandbox"]["enabled"].as_bool()
        })
        .next_back()
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lock {
    /// The own folder these were found for.
    own: PathBuf,
    /// Real paths, the outermost of nested ones only.
    folders: Vec<PathBuf>,
    wrapper: Option<Wrapper>,
    /// Why it isn't in force, in plain words.
    note: String,
}

/// By the record of the state folder: the daemon has one.
static CURRENT: Mutex<Vec<(PathBuf, Arc<Lock>)>> = Mutex::new(Vec::new());

/// The lock for what the daemon starts while `record`'s mode is Own.
/// `None` in the other mode, or before Sandhill's folder is set up.
pub(crate) fn current(record: &Path) -> Option<Arc<Lock>> {
    current_with(record, compute)
}

/// [`current`] off the async runtime: finding the folders runs R.
pub(crate) async fn current_async(record: PathBuf) -> Option<Arc<Lock>> {
    tokio::task::spawn_blocking(move || current(&record))
        .await
        .ok()
        .flatten()
}

/// The lock for the folder shown on the R and Python page.
pub(crate) fn status(record: &Path, folder: &Path) -> Option<LockStatus> {
    current(record)
        .filter(|lock| lock.own == folder)
        .map(|lock| lock.status())
}

/// The folders locked now, if the lock was found already, for explaining
/// a refused write without running anything.
pub(crate) fn known() -> Vec<PathBuf> {
    CURRENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(_, lock)| lock.wrapper.is_some())
        .flat_map(|(_, lock)| lock.folders.iter().cloned())
        .collect()
}

/// The folders locked for what the daemon starts while `record`'s mode
/// is Own, found first if need be.
pub(crate) fn locked_now(record: &Path) -> Vec<PathBuf> {
    current(record)
        .filter(|lock| lock.wrapper.is_some())
        .map(|lock| lock.folders.clone())
        .unwrap_or_default()
}

fn current_with(record: &Path, compute: impl FnOnce(&Path, &Path) -> Lock) -> Option<Arc<Lock>> {
    let mut cached = CURRENT.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(own) = own_env::chosen(record).filter(|own| own_env::marked(own)) else {
        // Found again when the mode is Own once more.
        cached.retain(|(path, _)| path != record);
        return None;
    };
    if let Some((_, lock)) = cached
        .iter()
        .find(|(path, lock)| path == record && lock.own == own)
    {
        return Some(lock.clone());
    }
    let lock = Arc::new(compute(record, &own));
    cached.retain(|(path, _)| path != record);
    cached.push((record.to_path_buf(), lock.clone()));
    Some(lock)
}

impl Lock {
    fn active(&self) -> bool {
        self.wrapper.is_some() && !self.folders.is_empty()
    }

    pub(crate) fn status(&self) -> LockStatus {
        let note = if !self.active() && self.wrapper.is_some() {
            NOTHING
        } else if !self.active() {
            &self.note
        } else if self.wrapper == Some(Wrapper::Sandbox) {
            AGENTS_ON_A_MAC
        } else {
            ""
        };
        LockStatus {
            active: self.active(),
            locked: self
                .folders
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            note: note.into(),
        }
    }

    pub(crate) fn uses_bubblewrap(&self) -> bool {
        matches!(self.wrapper, Some(Wrapper::Bubblewrap(_)))
    }

    /// The [`HOST_LOCK`] value for a terminal host started for `run`,
    /// `wrapped` or not: only where an agent's own sandbox and the lock
    /// can't both be in force.
    pub(crate) fn host_lock(&self, run: Run, wrapped: bool) -> Option<&'static str> {
        if !self.active() || self.wrapper != Some(Wrapper::Sandbox) {
            None
        } else if wrapped {
            Some("on")
        } else if run == Run::SandboxedAgent {
            Some("agent")
        } else {
            None
        }
    }

    /// The wrapper and its arguments, up to the program. `keep` (the
    /// project or working folder) stays writable even inside a locked one.
    fn prefix(&self, keep: &[&Path], run: Run) -> Option<(PathBuf, Vec<OsString>)> {
        if !self.active() {
            return None;
        }
        let keep: Vec<PathBuf> = keep.iter().map(|p| real(p)).collect();
        let folders: Vec<&Path> = self
            .folders
            .iter()
            .filter(|folder| !keep.iter().any(|k| k.starts_with(folder)))
            .map(PathBuf::as_path)
            .collect();
        if folders.is_empty() {
            return None;
        }
        match self.wrapper.as_ref()? {
            Wrapper::Sandbox if run == Run::SandboxedAgent => None,
            Wrapper::Sandbox => Some((
                SANDBOX_EXEC.into(),
                vec!["-p".into(), profile(&folders).into(), "--".into()],
            )),
            Wrapper::Bubblewrap(bwrap) => Some((bwrap.clone(), bwrap_args(&folders, run))),
        }
    }

    /// `program` under the lock, or `None` when nothing is locked for it.
    /// Arguments added to the result go to `program`.
    pub(crate) fn command(
        &self,
        program: impl AsRef<OsStr>,
        keep: &[&Path],
        run: Run,
    ) -> Option<Command> {
        let (wrapper, args) = self.prefix(keep, run)?;
        let mut command = Command::new(wrapper);
        command.args(args).arg(program.as_ref());
        Some(command)
    }

    /// `command` rebuilt under the lock: its program, arguments, variables
    /// and folder, which stays writable. Input and output, the process
    /// group and the like are set on the result.
    pub(crate) fn wrap(&self, command: &Command, keep: &[&Path], run: Run) -> Option<Command> {
        let cwd = command.get_current_dir();
        let keep: Vec<&Path> = keep.iter().copied().chain(cwd).collect();
        let mut wrapped = self.command(command.get_program(), &keep, run)?;
        wrapped.args(command.get_args());
        for (key, value) in command.get_envs() {
            match value {
                Some(value) => wrapped.env(key, value),
                None => wrapped.env_remove(key),
            };
        }
        if let Some(cwd) = cwd {
            wrapped.current_dir(cwd);
        }
        Some(wrapped)
    }
}

/// Apple's sandbox profile: everything allowed but writing in `folders`.
fn profile(folders: &[&Path]) -> String {
    let mut profile = String::from("(version 1)(allow default)(deny file-write*");
    for folder in folders {
        profile.push_str(" (subpath \"");
        for c in folder.to_string_lossy().chars() {
            match c {
                '"' => profile.push_str("\\\""),
                '\\' => profile.push_str("\\\\"),
                '\n' => profile.push_str("\\n"),
                c => profile.push(c),
            }
        }
        profile.push_str("\")");
    }
    profile.push(')');
    profile
}

/// Bubblewrap stays as the parent of what it runs and doesn't pass signals
/// on, so a kernel leads its own session for the daemon to signal; one
/// that isn't kept ends with bubblewrap. Kept kernels outlive the daemon.
fn bwrap_args(folders: &[&Path], run: Run) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if let Run::Kernel { persistent } = run {
        args.push("--new-session".into());
        if !persistent {
            args.push("--die-with-parent".into());
        }
    }
    args.extend(["--dev-bind", "/", "/"].map(OsString::from));
    for folder in folders {
        args.push("--ro-bind".into());
        args.push(folder.as_os_str().into());
        args.push(folder.as_os_str().into());
    }
    args.push("--".into());
    args
}

/// The process bubblewrap started for `parent`, once it has started it.
#[cfg(target_os = "linux")]
pub(crate) fn child_of(parent: u32) -> Option<u32> {
    if let Ok(text) = fs::read_to_string(format!("/proc/{parent}/task/{parent}/children")) {
        return text.split_whitespace().next()?.parse().ok();
    }
    fs::read_dir("/proc").ok()?.flatten().find_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
        let ppid: u32 = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        (ppid == parent).then_some(pid)
    })
}

/// A path with its existing part resolved, as the sandbox sees it.
pub(crate) fn real(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(found) = existing.canonicalize() {
            return rest.iter().rev().fold(found, |path, name| path.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn compute(record: &Path, own: &Path) -> Lock {
    let search = Search::from_env();
    let home = install::home();
    let brew = install::brew(&search)
        .and_then(|brew| install::probe_brew(&brew, &["--prefix"]))
        .map(|prefix| PathBuf::from(prefix.trim()))
        .filter(|prefix| prefix.is_absolute());
    let found = found(&search, home.as_deref(), own, brew, r_folders);
    let mut never = vec![own.to_path_buf()];
    never.extend(record.parent().map(Path::to_path_buf));
    never.extend(home.as_ref().map(|home| home.join(".sandhill")));
    let temps = [std::env::temp_dir(), "/tmp".into(), "/var/tmp".into()];
    let folders = locked(found, home.as_deref(), &never, &temps);
    let (wrapper, note) = match wrapper() {
        Ok(wrapper) => (Some(wrapper), String::new()),
        Err(note) => (None, note.to_string()),
    };
    Lock {
        own: own.to_path_buf(),
        folders,
        wrapper,
        note,
    }
}

/// Every folder of the user's R, Python, conda and Homebrew that Sandhill
/// knows, before the folders never locked are taken out.
fn found(
    search: &Search,
    home: Option<&Path>,
    own: &Path,
    brew: Option<PathBuf>,
    r_folders: impl Fn(&Path) -> Vec<PathBuf>,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let envs = kernelspecs::conda_envs(search);
    for env in &envs {
        found.push(env.clone());
        found.push(kernelspecs::conda_root(env).to_path_buf());
    }
    found.extend(brew);
    let kernels = kernelspecs::discover(search);
    let mut rs: Vec<PathBuf> = kernels
        .iter()
        .filter(|k| k.dir.kernelspec.language.eq_ignore_ascii_case("r"))
        .filter_map(|k| irkernel::spec_r(&k.dir.kernelspec, search))
        .collect();
    rs.extend(irkernel::find(search).into_iter().map(|c| c.r));
    let env_folders: Vec<PathBuf> = envs.iter().map(|env| canonical(env)).collect();
    let mut seen = std::collections::HashSet::new();
    for r in rs {
        let real = canonical(&r);
        // An R in a conda environment has everything there, locked with it.
        if own_env::inside(own, &r)
            || env_folders.iter().any(|env| real.starts_with(env))
            || !seen.insert(real)
        {
            continue;
        }
        found.extend(r_folders(&r));
    }
    for kernel in &kernels {
        let spec = &kernel.dir.kernelspec;
        if !spec.language.to_ascii_lowercase().starts_with("python") {
            continue;
        }
        let Some(program) = kernelspecs::program(spec, search.path.as_deref()) else {
            continue;
        };
        let bin = program.parent().filter(|dir| dir.ends_with("bin"));
        if let Some(prefix) = bin.and_then(Path::parent)
            && !own_env::inside(own, &program)
        {
            found.push(prefix.to_path_buf());
        }
    }
    found.extend(
        search
            .version_dirs
            .iter()
            .filter_map(|dir| dir.parent().map(Path::to_path_buf)),
    );
    if let Some(home) = home {
        found.push(home.join(".conda"));
        found.push(home.join("Library/Python"));
        if let Ok(entries) = fs::read_dir(home.join(".local/lib")) {
            found.extend(
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("python"))
                    .map(|e| e.path()),
            );
        }
        for relative in [
            ".local/share/uv",
            ".pyenv",
            "Library/Jupyter",
            ".local/share/jupyter",
        ] {
            found.push(home.join(relative));
        }
    }
    found
}

/// An R's home and libraries, from the probe the R and Python page uses.
fn r_folders(r: &Path) -> Vec<PathBuf> {
    let code = format!(
        "cat('SANDHILL_HOME\\t', R.home(), '\\n', sep=''); {}",
        irkernel::LIBRARIES_CODE
    );
    let Some(out) = environments::r_probe(r, &code) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| line.strip_prefix("SANDHILL_HOME\t"))
        .map(PathBuf::from)
        .chain(
            irkernel::parse_libraries(&out)
                .into_iter()
                .map(|lib| PathBuf::from(lib.path)),
        )
        .collect()
}

/// The existing folders to lock, as real paths: never the own folder or
/// Sandhill's (`never`, nor what is inside them), the home folder, temp
/// folders, or a folder holding any of them, nor a top folder like `/usr`.
fn locked(
    found: Vec<PathBuf>,
    home: Option<&Path>,
    never: &[PathBuf],
    temps: &[PathBuf],
) -> Vec<PathBuf> {
    let home = home.and_then(|home| home.canonicalize().ok());
    let never: Vec<PathBuf> = never.iter().filter_map(|p| p.canonicalize().ok()).collect();
    let temps: Vec<PathBuf> = temps.iter().filter_map(|p| p.canonicalize().ok()).collect();
    let mut folders: Vec<PathBuf> = found
        .iter()
        .filter_map(|p| p.canonicalize().ok())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .count()
                >= 2
        })
        .filter(|p| home.as_ref().is_none_or(|home| !home.starts_with(p)))
        .filter(|p| !temps.iter().any(|temp| temp.starts_with(p)))
        .filter(|p| !never.iter().any(|n| n.starts_with(p) || p.starts_with(n)))
        .collect();
    folders.sort();
    folders.dedup();
    let mut outer: Vec<PathBuf> = Vec::new();
    for folder in folders {
        if !outer.iter().any(|o| folder.starts_with(o)) {
            outer.push(folder);
        }
    }
    outer
}

fn wrapper() -> Result<Wrapper, &'static str> {
    if cfg!(target_os = "macos") {
        return if own_env::executable(Path::new(SANDBOX_EXEC)) {
            Ok(Wrapper::Sandbox)
        } else {
            Err(NO_SANDBOX)
        };
    }
    if !cfg!(target_os = "linux") {
        return Err(OTHER_SYSTEM);
    }
    let path = crate::paths::program_path();
    let bwrap = path
        .iter()
        .flat_map(std::env::split_paths)
        .chain([PathBuf::from("/usr/bin")])
        .map(|dir| dir.join("bwrap"))
        .find(|p| p.is_absolute() && own_env::executable(p))
        .ok_or(NO_BUBBLEWRAP)?;
    install::probe_timeout(
        &bwrap,
        &["--dev-bind", "/", "/", "--ro-bind", "/tmp", "/tmp", "true"],
        None,
        Duration::from_secs(10),
    )
    .map(|_| Wrapper::Bubblewrap(bwrap))
    .ok_or(BUBBLEWRAP_FAILS)
}

/// A lock in force for `folders`, with bubblewrap or Apple's sandbox.
#[cfg(test)]
pub(crate) fn test_lock(folders: Vec<PathBuf>, bubblewrap: bool) -> Lock {
    Lock {
        own: "/own".into(),
        folders,
        wrapper: Some(if bubblewrap {
            Wrapper::Bubblewrap("/usr/bin/bwrap".into())
        } else {
            Wrapper::Sandbox
        }),
        note: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(path: &Path) -> PathBuf {
        fs::create_dir_all(path).unwrap();
        path.canonicalize().unwrap()
    }

    #[test]
    fn locks_each_kind_of_folder_but_never_sandhills_projects_or_home() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let home = dir(&base.join("home"));
        let own = dir(&base.join("home/.sandhill/environment"));
        let state = dir(&base.join("home/.sandhill/state"));
        let project = dir(&base.join("home/work/project"));
        let miniconda = dir(&home.join("miniconda3"));
        dir(&miniconda.join("conda-meta"));
        let conda_env = dir(&miniconda.join("envs/rna"));
        dir(&conda_env.join("conda-meta"));
        let own_env = dir(&own.join("conda/envs/sandhill-python"));
        dir(&own_env.join("conda-meta"));
        let venv = dir(&home.join("venvs/scanpy"));
        dir(&venv.join("bin"));
        fs::write(venv.join("bin/python"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            venv.join("bin/python"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let jupyter = dir(&home.join("Library/Jupyter"));
        let kernel = dir(&jupyter.join("kernels/scanpy"));
        fs::write(
            kernel.join("kernel.json"),
            serde_json::json!({
                "argv": [venv.join("bin/python"), "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                "display_name": "scanpy",
                "language": "python",
            })
            .to_string(),
        )
        .unwrap();
        let own_kernel = dir(&own_env.join("share/jupyter/kernels/python3"));
        dir(&own_env.join("bin"));
        fs::write(own_env.join("bin/python"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            own_env.join("bin/python"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            own_kernel.join("kernel.json"),
            serde_json::json!({
                "argv": [own_env.join("bin/python"), "-m", "ipykernel_launcher"],
                "display_name": "own",
                "language": "python",
            })
            .to_string(),
        )
        .unwrap();
        let mut expected = vec![miniconda.clone(), venv.clone(), jupyter.clone()];
        for relative in [
            ".conda",
            "Library/Python",
            ".local/lib/python3.12",
            ".local/share/uv",
            ".pyenv",
            ".local/share/jupyter",
            "R/library",
            "Rhome",
        ] {
            expected.push(dir(&home.join(relative)));
        }
        let brew = dir(&base.join("homebrew"));
        expected.push(brew.clone());
        // Kinds of folder found that are never locked.
        dir(&home.join(".local/lib/node"));
        let search = Search {
            user_dir: Some(jupyter.clone()),
            conda_roots: vec![miniconda.clone(), own.join("conda")],
            path: Some("/nonexistent".into()),
            ..Default::default()
        };
        let r_home = home.join("Rhome");
        let r_library = home.join("R/library");
        let r_folders = |_: &Path| {
            vec![
                r_home.clone(),
                r_library.clone(),
                home.clone(),
                own.clone(),
                state.clone(),
                std::env::temp_dir(),
                PathBuf::from("/usr"),
            ]
        };
        let fake_r = dir(&home.join("Rbin")).join("R");
        fs::write(&fake_r, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&fake_r, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let search = Search {
            path: Some(fake_r.parent().unwrap().into()),
            ..search
        };
        let found = found(&search, Some(&home), &own, Some(brew), r_folders);
        let never = [own.clone(), state.clone(), home.join(".sandhill")];
        let folders = locked(found, Some(&home), &never, &[std::env::temp_dir()]);
        expected.sort();
        assert_eq!(folders, expected);
        for kept in [&home, &own, &state, &project] {
            assert!(!folders.iter().any(|f| kept.starts_with(f)), "{kept:?}");
        }
        assert!(!folders.iter().any(|f| f.starts_with(&own)));
        // A project inside a locked folder stays writable for what runs there.
        let lock = test_lock(folders, false);
        let (_, args) = lock
            .prefix(&[&miniconda.join("envs/rna/work")], Run::Host)
            .unwrap();
        let profile = args[1].to_string_lossy().into_owned();
        assert!(!profile.contains(miniconda.to_str().unwrap()));
        assert!(profile.contains(jupyter.to_str().unwrap()));
    }

    #[test]
    fn sandbox_profile_quotes_paths() {
        let profile = profile(&[Path::new("/a b/R"), Path::new("/odd\"name\\here")]);
        assert_eq!(
            profile,
            r#"(version 1)(allow default)(deny file-write* (subpath "/a b/R") (subpath "/odd\"name\\here"))"#
        );
    }

    #[test]
    fn bubblewrap_binds_each_folder_read_only() {
        let folders = [Path::new("/opt/homebrew"), Path::new("/home/u/.conda")];
        let text = |run| {
            bwrap_args(&folders, run)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(
            text(Run::Host),
            "--dev-bind / / --ro-bind /opt/homebrew /opt/homebrew --ro-bind /home/u/.conda /home/u/.conda --"
        );
        assert_eq!(text(Run::SandboxedAgent), text(Run::Host));
        assert!(text(Run::Kernel { persistent: true }).starts_with("--new-session --dev-bind"));
        assert!(
            text(Run::Kernel { persistent: false })
                .starts_with("--new-session --die-with-parent --dev-bind")
        );
    }

    #[test]
    fn wrapping_keeps_the_program_arguments_variables_and_folder() {
        let lock = test_lock(vec!["/opt/homebrew".into()], false);
        let mut command = Command::new("python3");
        command
            .args(["-c", "print(1)"])
            .env("PATH", "/own/bin")
            .env_remove("PYTHONPATH")
            .current_dir("/work");
        let wrapped = lock
            .wrap(&command, &[], Run::Kernel { persistent: true })
            .unwrap();
        assert_eq!(wrapped.get_program(), SANDBOX_EXEC);
        let args: Vec<_> = wrapped.get_args().collect();
        assert_eq!(args[0], "-p");
        assert_eq!(
            args[1],
            r#"(version 1)(allow default)(deny file-write* (subpath "/opt/homebrew"))"#
        );
        assert_eq!(args[2..], ["--", "python3", "-c", "print(1)"]);
        let vars: std::collections::HashMap<_, _> = wrapped.get_envs().collect();
        assert_eq!(vars[OsStr::new("PATH")], Some(OsStr::new("/own/bin")));
        assert_eq!(vars[OsStr::new("PYTHONPATH")], None);
        assert_eq!(wrapped.get_current_dir(), Some(Path::new("/work")));

        // An agent's own sandbox can't run inside Apple's; bubblewrap nests.
        assert!(lock.command("claude", &[], Run::SandboxedAgent).is_none());
        assert!(lock.command("claude", &[], Run::Host).is_some());
        let linux = test_lock(vec!["/opt/homebrew".into()], true);
        let agent = linux.command("claude", &[], Run::SandboxedAgent).unwrap();
        assert_eq!(agent.get_program(), "/usr/bin/bwrap");
        assert_eq!(agent.get_args().last(), Some(OsStr::new("claude")));
        // Nothing to lock, or a working folder inside the only one: as is.
        let empty = test_lock(Vec::new(), false);
        assert!(empty.wrap(&command, &[], Run::Host).is_none());
        assert!(
            lock.command("zsh", &[Path::new("/opt/homebrew/x")], Run::Host)
                .is_none()
        );
    }

    #[test]
    fn status_says_why_it_isnt_in_force() {
        let mut lock = test_lock(vec!["/opt/homebrew".into()], true);
        assert_eq!(
            lock.status(),
            LockStatus {
                active: true,
                locked: vec!["/opt/homebrew".into()],
                note: String::new(),
            }
        );
        lock.wrapper = None;
        lock.note = NO_BUBBLEWRAP.into();
        assert!(!lock.status().active);
        assert_eq!(lock.status().note, NO_BUBBLEWRAP);
        lock.wrapper = Some(Wrapper::Sandbox);
        assert_eq!(lock.status().note, AGENTS_ON_A_MAC);
        lock.folders.clear();
        assert_eq!(lock.status().note, NOTHING);
        for note in [
            NO_SANDBOX,
            NO_BUBBLEWRAP,
            BUBBLEWRAP_FAILS,
            OTHER_SYSTEM,
            NOTHING,
            AGENTS_ON_A_MAC,
        ] {
            assert!(!note.contains('\u{2013}') && !note.contains('\u{2014}'));
        }
    }

    #[test]
    fn found_once_and_again_when_the_mode_changes() {
        let temp = tempfile::tempdir().unwrap();
        let state = dir(&temp.path().join("state"));
        let own = dir(&temp.path().join("own"));
        let record = state.join(super::super::record::FILE);
        let found = Lock {
            own: own.clone(),
            folders: vec![own.join("homebrew")],
            wrapper: Some(Wrapper::Sandbox),
            note: String::new(),
        };
        let computed = std::cell::Cell::new(0);
        let compute = |_: &Path, folder: &Path| {
            computed.set(computed.get() + 1);
            assert_eq!(folder, own);
            found.clone()
        };
        assert!(current_with(&record, compute).is_none());
        let own_mode = || {
            install::update_prefs(&record, |prefs| {
                prefs.mode = Some(sandhill_proto::EnvMode::Own);
                prefs.own_folder = own.to_string_lossy().into_owned();
            })
            .unwrap()
        };
        own_mode();
        // Not set up yet: no marker.
        assert!(current_with(&record, compute).is_none());
        fs::write(own.join(own_env::MARKER), "").unwrap();
        assert_eq!(*current_with(&record, compute).unwrap(), found);
        assert_eq!(*current_with(&record, compute).unwrap(), found);
        assert_eq!(computed.get(), 1);
        // Unique folders: other tests share this process.
        let unique = own.join("homebrew");
        assert_eq!(found.folders, std::slice::from_ref(&unique));
        assert!(known().contains(&unique));
        install::update_prefs(&record, |prefs| {
            prefs.mode = Some(sandhill_proto::EnvMode::Existing)
        })
        .unwrap();
        assert!(current_with(&record, compute).is_none());
        assert!(!known().contains(&unique));
        own_mode();
        assert!(current_with(&record, compute).is_some());
        assert_eq!(computed.get(), 2);
        install::update_prefs(&record, |prefs| prefs.mode = None).unwrap();
        assert!(current_with(&record, compute).is_none());
    }

    #[test]
    fn an_agent_runs_under_its_own_sandbox_or_the_lock() {
        let temp = tempfile::tempdir().unwrap();
        let project = dir(&temp.path().join("project"));
        let config = dir(&temp.path().join("claude"));
        let managed = temp.path().join("managed-settings.json");
        use crate::agents::{ANTIGRAVITY, CODEX, OMP, OPENSCIENCE, PI, POSIT};
        // Codex's own sandbox is on in every mode but full access.
        for (permission, sandboxed) in [
            (None, true),
            (Some("supervised"), true),
            (Some("edits"), true),
            (Some("auto"), true),
            (Some("readonly"), true),
            (Some("plan"), true),
            (Some("full"), false),
        ] {
            assert_eq!(
                own_sandbox(CODEX, permission, &project, None),
                sandboxed,
                "{permission:?}"
            );
        }
        for kind in [ANTIGRAVITY, OMP, OPENSCIENCE, PI, POSIT] {
            assert!(!own_sandbox(kind, Some("edits"), &project, None), "{kind}");
        }
        let write = |path: &Path, enabled: bool| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                path,
                serde_json::json!({"sandbox": {"enabled": enabled}}).to_string(),
            )
            .unwrap();
        };
        let claude = || claude_sandbox(&project, Some(&config), &managed);
        assert!(!claude());
        write(&config.join("settings.json"), true);
        assert!(claude());
        // The project's settings, then its local ones, override the user's.
        write(&project.join(".claude/settings.json"), false);
        assert!(!claude());
        write(&project.join(".claude/settings.local.json"), true);
        assert!(claude());
        write(&managed, false);
        assert!(!claude());
        // A file without the setting changes nothing.
        fs::write(&managed, "{\"model\": \"opus\"}").unwrap();
        assert!(claude());
        // Without the user's settings folder, the project's still count.
        assert!(claude_sandbox(&project, None, &temp.path().join("none")));
    }

    #[test]
    fn a_codex_conversation_keeps_to_its_side_of_the_lock() {
        for wanted in [None, Some("edits"), Some("full")] {
            assert!(codex_mode_allowed(wanted, None));
        }
        // Started in Full access, under the lock.
        assert!(codex_mode_allowed(Some("full"), Some("on")));
        for wanted in [None, Some("edits"), Some("readonly"), Some("plan")] {
            assert!(!codex_mode_allowed(wanted, Some("on")), "{wanted:?}");
        }
        // Started with Codex's own sandbox.
        assert!(!codex_mode_allowed(Some("full"), Some("agent")));
        for wanted in [None, Some("edits"), Some("auto"), Some("readonly")] {
            assert!(codex_mode_allowed(wanted, Some("agent")), "{wanted:?}");
        }
        for text in [CODEX_SWITCH, crate::quick::install_failure::LOCKED_MESSAGE] {
            assert!(!text.contains(['\u{2013}', '\u{2014}']));
        }
    }
}
