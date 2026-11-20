//! Jobs: long scripts and pipelines run in the background on the daemon's
//! machine. Each runs in a terminal of kind [`TERMINAL_JOB`], so it goes on
//! when the app disconnects or the daemon restarts, and its output is its
//! log. It runs in the user's login shell, as in their Terminal, so their
//! `PATH`, modules and conda setup apply. A script runs with the program for
//! its kind; R and Python with those of the project's kernel for the
//! language, with that kernel's environment first on `PATH`, so a job finds
//! the packages the project's sessions do. On a Mac, the Mac stays awake
//! while a job runs.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use sandhill_proto::{AgentOptions, JobSpec, TERMINAL_JOB};

use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search};
use crate::projects;
use crate::terminal::Launch;

/// How long a command line can be as a job's title.
const TITLE_CHARS: usize = 60;
/// macOS's tool that keeps the Mac from sleeping while a process runs.
const CAFFEINATE: &str = "/usr/bin/caffeinate";

/// Keeps the Mac from going to sleep while the job whose program is `pid`
/// runs: from sleeping when idle, and on power, from sleeping at all
/// (`caffeinate -i -s -w`), as long as the job runs, whether the daemon
/// does or not. Elsewhere, nothing.
pub fn keep_awake(pid: u32) {
    if !Path::new(CAFFEINATE).exists() {
        return;
    }
    let child = tokio::process::Command::new(CAFFEINATE)
        .args(["-i", "-s", "-w", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Not stopped with the daemon's process group.
        .process_group(0)
        .spawn();
    match child {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(err) => tracing::warn!("cannot keep the Mac awake for a job: {err}"),
    }
}

/// The terminal to run `job` in. Quarto always runs from the document's
/// folder, allowing Quarto to discover the enclosing project itself.
/// Other jobs use `cwd`, then the project folder, then the script's folder.
pub fn launch(job: &JobSpec, cwd: Option<&Path>, project: Option<&str>) -> Result<Launch> {
    launch_scoped(job, cwd, project, None)
}

pub(crate) fn launch_scoped(
    job: &JobSpec,
    cwd: Option<&Path>,
    project: Option<&str>,
    own: Option<&Path>,
) -> Result<Launch> {
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
                Kind::R | Kind::Python if let Some(folder) = own => {
                    let (lang, name) = if kind == Kind::R {
                        ("r", "Rscript")
                    } else {
                        ("python", "python")
                    };
                    let program = crate::own_env::prefix(folder, lang).join("bin").join(name);
                    if !crate::own_env::executable(&program)
                        || !crate::own_env::inside(folder, &program)
                    {
                        bail!(
                            "{name} isn't installed in Sandhill's folder. Add it in Getting Ready or switch to use what's on this computer."
                        );
                    }
                    (Some(program), None)
                }
                Kind::R | Kind::Python => interpreter(kind, root),
                _ => (None, None),
            };
            let mut words: Vec<String> = match (kind, program) {
                (_, Some(program)) => vec![shell_quote(&program.to_string_lossy())],
                (Kind::R, None) => vec!["Rscript".into()],
                (Kind::Python, None) => vec!["python3".into()],
                (Kind::Shell, _) => vec!["bash".into()],
                (Kind::Nextflow, _) => vec!["nextflow".into(), "run".into()],
                (Kind::Quarto, None) => {
                    let search = Search::from_env()
                        .in_project(root)
                        .in_own(own.map(Path::to_path_buf));
                    let Some(quarto) = crate::preview::find_quarto(&search) else {
                        bail!("{}", crate::preview::install_advice());
                    };
                    vec![shell_quote(&quarto.to_string_lossy()), "render".into()]
                }
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
            let title = if kind == Kind::Quarto {
                format!("Render {name}")
            } else {
                name
            };
            (line, title)
        }
        None => {
            let Some(command) = args else {
                bail!("a job needs a script or a command to run");
            };
            (command.to_string(), title_of(command))
        }
    };
    let document_dir = script
        .as_deref()
        .filter(|path| Kind::of(path) == Kind::Quarto)
        .and_then(Path::parent);
    let cwd = if let Some(dir) = document_dir {
        dir.to_path_buf()
    } else {
        match cwd {
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
        }
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
    Quarto,
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
            Some("qmd" | "rmd") => Kind::Quarto,
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

/// Collects Quarto's reported outputs in the independent terminal host, not
/// in a client subscription. Lines and UTF-8/ANSI sequences may span reads.
pub(crate) struct RenderOutputs {
    cwd: PathBuf,
    line: Vec<u8>,
    oversized: bool,
    paths: Vec<PathBuf>,
}

impl RenderOutputs {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            line: Vec::new(),
            oversized: false,
            paths: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if matches!(byte, b'\n' | b'\r') {
                if !self.oversized {
                    self.record();
                }
                self.line.clear();
                self.oversized = false;
            } else if !self.oversized {
                // An unbounded line from user code must not grow host memory.
                if self.line.len() == 64 * 1024 {
                    self.line.clear();
                    self.oversized = true;
                } else {
                    self.line.push(byte);
                }
            }
        }
    }

    fn record(&mut self) {
        let raw = String::from_utf8_lossy(&self.line);
        let text = if raw.contains('\u{1b}') {
            std::borrow::Cow::Owned(crate::mcp::text::strip_ansi(&raw))
        } else {
            raw
        };
        let Some(output) = text.trim().strip_prefix("Output created:") else {
            return;
        };
        let output = output.trim();
        let output = ['"', '\'', '`']
            .into_iter()
            .find_map(|quote| output.strip_prefix(quote)?.strip_suffix(quote))
            .unwrap_or(output);
        if output.is_empty() {
            return;
        }
        let path = self.cwd.join(output);
        if !self.paths.contains(&path) {
            self.paths.push(path);
        }
    }

    /// Called only after a successful exit. Canonical paths handle Quarto's
    /// project output directories, relative paths and symlinks consistently.
    pub(crate) fn finish(mut self) -> Vec<String> {
        if !self.oversized {
            self.record();
        }
        let mut outputs = Vec::new();
        for path in self.paths {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if path.is_file() {
                let path = path.to_string_lossy().into_owned();
                if !outputs.contains(&path) {
                    outputs.push(path);
                }
            }
        }
        outputs
    }
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

    #[test]
    fn only_quarto_documents_are_render_jobs() {
        for name in ["report.qmd", "report.Rmd", "report.QMD"] {
            assert_eq!(Kind::of(Path::new(name)), Kind::Quarto);
        }
        for name in ["report.md", "report.ipynb", "report.html"] {
            assert_eq!(Kind::of(Path::new(name)), Kind::Program);
        }
    }

    #[test]
    fn render_outputs_handle_chunking_ansi_quotes_and_project_paths() {
        let dir = tempfile::tempdir().unwrap();
        let documents = dir.path().join("documents");
        let site = dir.path().join("_site");
        std::fs::create_dir(&documents).unwrap();
        std::fs::create_dir(&site).unwrap();
        let html = site.join("résult report.html");
        let pdf = site.join("report.pdf");
        std::fs::write(&html, "html").unwrap();
        std::fs::write(&pdf, "pdf").unwrap();
        let log = format!(
            "progress\r\u{1b}[32mOutput created: '../_site/résult report.html'\u{1b}[0m\r\n\
             Output created: \u{1b}]8;;file://{}\u{7}{}\u{1b}]8;;\u{1b}\\\n\
             Output created: ../_site/résult report.html\n\
             Output created: missing.html\n\
             Output created: `{}`",
            pdf.display(),
            pdf.display(),
            site.display()
        );
        let mut parser = RenderOutputs::new(documents);
        // Deliberately split multibyte characters and escape sequences.
        for byte in log.as_bytes() {
            parser.push(std::slice::from_ref(byte));
        }
        assert_eq!(
            parser.finish(),
            vec![
                html.canonicalize().unwrap().to_string_lossy().into_owned(),
                pdf.canonicalize().unwrap().to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn render_output_lines_are_bounded_and_final_line_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let html = dir.path().join("report.html");
        std::fs::write(&html, "html").unwrap();
        let mut parser = RenderOutputs::new(dir.path().to_path_buf());
        parser.push(&vec![b'x'; 64 * 1024 + 1]);
        parser.push(b"Output created: report.html\nOutput created: \"report.html\"");
        assert_eq!(
            parser.finish(),
            vec![html.canonicalize().unwrap().to_string_lossy().into_owned()]
        );
    }
}
