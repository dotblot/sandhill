//! Jobs: long scripts and pipelines run in the background on the daemon's
//! machine. Each runs in a terminal of kind [`TERMINAL_JOB`], so it goes on
//! when the app disconnects or the daemon restarts, and its output is its
//! log. It runs in the user's login shell, as in their Terminal, so their
//! `PATH`, modules and conda setup apply. A script runs with the program for
//! its kind; R and Python with those of the project's kernel for the
//! language, with that kernel's environment first on `PATH`, so a job finds
//! the packages the project's sessions do.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use sandhill_proto::{AgentOptions, JobSpec, TERMINAL_JOB};

use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search};
use crate::projects;
use crate::terminal::Launch;

/// How long a command line can be as a job's title.
const TITLE_CHARS: usize = 60;

/// The terminal to run `job` in. Without `cwd` it runs in the project
/// folder, else the script's folder.
pub fn launch(job: &JobSpec, cwd: Option<&Path>, project: Option<&str>) -> Result<Launch> {
    let root = project.map(Path::new);
    let args = job
        .command
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let script = job.path.as_deref().map(crate::files::resolve).transpose()?;
    let (line, title) = match &script {
        Some(script) => {
            if !script.is_file() {
                bail!("{} is not a file", script.display());
            }
            let kind = Kind::of(script);
            let (program, bin) = match kind {
                Kind::R | Kind::Python => interpreter(kind, root),
                _ => (None, None),
            };
            let mut words: Vec<String> = match (kind, program) {
                (_, Some(program)) => vec![shell_quote(&program.to_string_lossy())],
                (Kind::R, None) => vec!["Rscript".into()],
                (Kind::Python, None) => vec!["python3".into()],
                (Kind::Shell, _) => vec!["bash".into()],
                (Kind::Nextflow, _) => vec!["nextflow".into(), "run".into()],
                (Kind::Snakemake, _) => vec!["snakemake".into(), "--snakefile".into()],
                (Kind::Program, _) => {
                    if !is_executable(script) {
                        bail!(
                            "{} is not a script Sandhill knows how to run; make it executable, or run a command",
                            script.display()
                        );
                    }
                    Vec::new()
                }
            };
            words.push(shell_quote(&script.to_string_lossy()));
            match args {
                Some(args) => words.push(args.to_string()),
                // Snakemake won't start without being told how many cores to use.
                None if kind == Kind::Snakemake => words.push("--cores all".into()),
                None => {}
            }
            let mut line = words.join(" ");
            if let Some(bin) = bin {
                // After the shell's own setup, which may activate another environment.
                line = format!(
                    "export PATH={}:\"$PATH\"; {line}",
                    shell_quote(&bin.to_string_lossy())
                );
            }
            let name = script
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (line, name)
        }
        None => {
            let Some(command) = args else {
                bail!("a job needs a script or a command to run");
            };
            (command.to_string(), title_of(command))
        }
    };
    let cwd = match cwd {
        Some(cwd) => cwd.to_path_buf(),
        None => root
            .map(Path::to_path_buf)
            .or_else(|| {
                script
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into()))),
    };
    if !cwd.is_dir() {
        bail!("{} is not a folder", cwd.display());
    }
    Ok(Launch {
        kind: TERMINAL_JOB.into(),
        title,
        program: shell(),
        // Interactive too: most shells set up conda and modules only then.
        args: vec!["-l".into(), "-i".into(), "-c".into(), line],
        env: root.map(projects::kernel_env).unwrap_or_default(),
        cwd,
        project: project.map(String::from),
        options: AgentOptions::default(),
        size: None,
        job: Some(job.clone()),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    R,
    Python,
    Shell,
    Nextflow,
    Snakemake,
    Program,
}

impl Kind {
    fn of(script: &Path) -> Kind {
        let name = script.file_name().unwrap_or_default().to_string_lossy();
        let ext = script
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("r") => Kind::R,
            Some("py") => Kind::Python,
            Some("sh" | "bash") => Kind::Shell,
            Some("nf") => Kind::Nextflow,
            Some("smk") => Kind::Snakemake,
            _ if name == "Snakefile" => Kind::Snakemake,
            _ => Kind::Program,
        }
    }
}

/// `Rscript` or `python` from the environment of the project's kernel for
/// the language, and that environment's `bin` folder, when it has one.
fn interpreter(kind: Kind, root: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
    let Some(root) = root else {
        return (None, None);
    };
    let setup = projects::setup(root);
    let name = if kind == Kind::R {
        setup.r
    } else {
        setup.python
    };
    let search = Search::from_env().in_project(Some(root));
    let Some(spec) = name.and_then(|name| kernelspecs::find(&search, &name)) else {
        return (None, None);
    };
    let spec = spec.kernelspec;
    // A kernel in an environment has the environment's bin first on its PATH.
    let path = spec.env.as_ref().and_then(|env| env.get("PATH")).cloned();
    let bin = path
        .as_deref()
        .and_then(|path| std::env::split_paths(path).next());
    let first = spec.argv.first().map(PathBuf::from);
    let program = match kind {
        Kind::Python => first.filter(|p| p.is_absolute() && file_name(p).starts_with("python")),
        // IRkernel runs R, with Rscript beside it; Ark runs on the R on its PATH.
        _ => first
            .filter(|p| p.is_absolute() && file_name(p) == "R")
            .and_then(|r| Some(r.parent()?.join("Rscript")))
            .filter(|p| p.is_file())
            .or_else(|| {
                std::env::split_paths(path.as_deref()?)
                    .map(|dir| dir.join("Rscript"))
                    .find(|p| p.is_file())
            }),
    };
    (program, bin)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// The user's shell, when it takes `sh` syntax; else bash.
fn shell() -> PathBuf {
    let shell = crate::agents::shell();
    match file_name(&shell).as_str() {
        "bash" | "zsh" | "sh" | "ksh" | "dash" => shell,
        _ => PathBuf::from("/bin/bash"),
    }
}

/// A command line's first line, shortened, as a job's title.
fn title_of(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= TITLE_CHARS {
        return line.to_string();
    }
    let short: String = line.chars().take(TITLE_CHARS - 1).collect();
    format!("{}…", short.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(path: Option<&Path>, command: Option<&str>) -> JobSpec {
        JobSpec {
            path: path.map(|p| p.to_string_lossy().into_owned()),
            command: command.map(String::from),
        }
    }

    /// The command line a launch runs, after the shell's options.
    fn line(launch: &Launch) -> &str {
        assert_eq!(launch.args[..3], ["-l", "-i", "-c"]);
        &launch.args[3]
    }

    #[test]
    fn scripts_run_with_the_program_for_their_kind() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path();
        let file = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, "").unwrap();
            path
        };
        let quoted = |path: &Path| shell_quote(&path.to_string_lossy());

        let sh = file("align it.sh");
        let job = launch(&spec(Some(&sh), Some("--threads 8")), None, None).unwrap();
        assert_eq!(line(&job), format!("bash {} --threads 8", quoted(&sh)));
        assert_eq!(job.title, "align it.sh");
        assert_eq!(job.kind, TERMINAL_JOB);
        assert_eq!(job.cwd, dir);
        assert_eq!(job.job, Some(spec(Some(&sh), Some("--threads 8"))));

        // Without a project there is no kernel to follow.
        let r = file("de.R");
        let job = launch(&spec(Some(&r), None), None, None).unwrap();
        assert_eq!(line(&job), format!("Rscript {}", quoted(&r)));
        let py = file("qc.py");
        let job = launch(&spec(Some(&py), None), None, None).unwrap();
        assert_eq!(line(&job), format!("python3 {}", quoted(&py)));

        let nf = file("main.nf");
        let job = launch(&spec(Some(&nf), Some("-profile docker")), None, None).unwrap();
        assert_eq!(
            line(&job),
            format!("nextflow run {} -profile docker", quoted(&nf))
        );
        let smk = file("Snakefile");
        let job = launch(&spec(Some(&smk), None), None, None).unwrap();
        assert_eq!(
            line(&job),
            format!("snakemake --snakefile {} --cores all", quoted(&smk))
        );

        let other = file("tool");
        let err = launch(&spec(Some(&other), None), None, None).unwrap_err();
        assert!(err.to_string().contains("make it executable"), "{err}");
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let job = launch(&spec(Some(&other), None), None, None).unwrap();
        assert_eq!(line(&job), quoted(&other));
    }

    #[test]
    fn command_lines_run_as_written() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().to_string_lossy().into_owned();
        let command =
            "snakemake --cores 16 --use-conda all_samples_with_a_very_long_target_name_here";
        let job = launch(&spec(None, Some(command)), None, Some(&project)).unwrap();
        assert_eq!(line(&job), command);
        assert_eq!(job.cwd, dir.path());
        assert_eq!(job.title.chars().count(), TITLE_CHARS);
        assert!(job.title.ends_with('…'), "{}", job.title);

        assert!(launch(&spec(None, Some("  ")), None, None).is_err());
        let missing = dir.path().join("missing.R");
        assert!(launch(&spec(Some(&missing), None), None, None).is_err());
    }
}
