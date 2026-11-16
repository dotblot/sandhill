//! Which R or Python a kernel runs, for its name (Huan, 2026-11-15: "R
//! 4.6.2 · Ark", "Python 3.12 (omicverse)"). A kernel that isn't running
//! can't say, so its program is asked once (`python --version`,
//! `R --version`) and the answer kept while that program is unchanged.
//! Nothing in a project runs.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use jupyter_protocol::JupyterKernelspec;

/// How long a program may take to say its version.
const ASK_LIMIT: Duration = Duration::from_secs(5);

/// A program's modification time when asked, and what it said.
type Answer = (Option<SystemTime>, Option<String>);

/// Answers by program.
static ANSWERS: Mutex<Option<HashMap<PathBuf, Answer>>> = Mutex::new(None);

/// For an R kernel, which one it is: Ark, or IRkernel.
pub fn engine(spec: &JupyterKernelspec) -> Option<&'static str> {
    if !spec.language.eq_ignore_ascii_case("r") {
        return None;
    }
    let program = spec.argv.first().map(|p| file_name(p))?;
    if program == "ark" {
        Some("Ark")
    } else if spec.argv.iter().any(|arg| arg.contains("IRkernel")) {
        Some("IRkernel")
    } else {
        None
    }
}

/// The versions of `specs`' languages, asked of their programs at once.
/// `path` is where a bare program name is looked for when the kernel sets
/// no `PATH` of its own.
pub fn versions(specs: &[&JupyterKernelspec], path: Option<&OsStr>) -> Vec<Option<String>> {
    std::thread::scope(|scope| {
        let asks: Vec<_> = specs
            .iter()
            .map(|spec| scope.spawn(move || version(spec, path)))
            .collect();
        asks.into_iter()
            .map(|ask| ask.join().ok().flatten())
            .collect()
    })
}

fn version(spec: &JupyterKernelspec, path: Option<&OsStr>) -> Option<String> {
    let language = spec.language.to_ascii_lowercase();
    let vars = spec.env.as_ref();
    let kernel_path = vars
        .and_then(|vars| vars.get("PATH"))
        .map(|p| p.as_ref())
        .or(path);
    let program = if language.starts_with("python") {
        let program = spec.argv.first()?;
        // Wrappers (`conda run`, `uv run`) aren't asked.
        if !file_name(program).starts_with("python") {
            return None;
        }
        locate(program, kernel_path)?
    } else if language == "r" {
        match engine(spec) {
            // Ark runs the R at R_HOME, else the first on PATH.
            Some("Ark") => match vars.and_then(|vars| vars.get("R_HOME")) {
                Some(home) => Path::new(home).join("bin").join("R"),
                None => locate("R", kernel_path)?,
            },
            _ => {
                let program = spec.argv.first()?;
                if !matches!(file_name(program).as_str(), "R" | "Rscript") {
                    return None;
                }
                locate(program, kernel_path)?
            }
        }
    } else {
        return None;
    };
    let program = std::fs::canonicalize(&program).unwrap_or(program);
    let modified = std::fs::metadata(&program).and_then(|m| m.modified()).ok();
    if let Some((when, said)) = ANSWERS
        .lock()
        .ok()
        .and_then(|answers| answers.as_ref()?.get(&program).cloned())
        && when == modified
    {
        return said;
    }
    let said = ask(&program, vars, kernel_path);
    if let Ok(mut answers) = ANSWERS.lock() {
        answers
            .get_or_insert_with(HashMap::new)
            .insert(program, (modified, said.clone()));
    }
    said
}

/// Runs `<program> --version` and returns the version number it printed.
fn ask(
    program: &Path,
    vars: Option<&HashMap<String, String>>,
    path: Option<&OsStr>,
) -> Option<String> {
    let mut command = Command::new(program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(vars) = vars {
        command.envs(vars);
    } else if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < ASK_LIMIT => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    // Python 2 says it on stderr.
    [output.stdout, output.stderr]
        .iter()
        .find_map(|bytes| sandhill_proto::version_in(&String::from_utf8_lossy(bytes)))
}

/// A program by path, or by name on `path`.
fn locate(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if program.contains('/') {
        let program = PathBuf::from(program);
        return program.is_file().then_some(program);
    }
    std::env::split_paths(path?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn file_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
