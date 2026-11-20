//! Adding IRkernel to an R that has none. The R installations on the
//! machine (on `PATH`, CRAN's framework and rig's versions, conda
//! environments, a project's pixi environment) that no installed IRkernel
//! runs are offered it; Ark doesn't count, as it is a different kernel.
//! Installing runs as a job: plain R installs the package from its own
//! mirror (else CRAN's cloud mirror) and registers the kernel by R's full
//! path in the user's Jupyter folder; an environment gets `r-irkernel` from
//! its own package manager, which registers the kernel inside it.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sandhill_proto::{JobSpec, RLibrary, RWithoutIrkernel};

use crate::agents::shell_quote;
use crate::kernelspecs::{self, Search, canonical};
use crate::terminal::Launch;

/// How long `R --version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// The mirror used when R names none.
const CRAN: &str = "https://cloud.r-project.org";

/// An R without IRkernel, and how it gets it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub r: PathBuf,
    pub version: String,
    pub how: How,
}

#[derive(Debug, Clone, PartialEq)]
pub enum How {
    /// Not an environment: install the package and register the kernel.
    Plain,
    /// A conda environment: `conda install r-irkernel` into it.
    Conda { prefix: PathBuf, label: String },
    /// A project's default pixi environment: `pixi add r-irkernel`.
    Pixi { root: PathBuf },
}

/// The R installations IRkernel could be added to.
pub fn find(search: &Search) -> Vec<Candidate> {
    let have = irkernel_rs(search);
    let mut seen = HashSet::new();
    let mut found: Vec<(PathBuf, How)> = Vec::new();
    let mut add = |r: PathBuf, how: How| {
        if r.is_file() {
            let real = canonical(&r);
            if !have.contains(&real) && seen.insert(real) {
                found.push((r, how));
            }
        }
    };
    if let Some(root) = &search.project {
        for env in kernelspecs::project_envs(root) {
            let r = env.prefix.join("bin/R");
            if env.folder == ".pixi/envs/default" {
                add(r, How::Pixi { root: root.clone() });
            } else if env.conda && !env.folder.starts_with(".pixi/") {
                add(
                    r,
                    How::Conda {
                        label: env.label.clone(),
                        prefix: env.prefix,
                    },
                );
            }
        }
    }
    for prefix in kernelspecs::conda_envs(search) {
        add(
            prefix.join("bin/R"),
            How::Conda {
                label: format!("conda env {}", kernelspecs::conda_name(&prefix)),
                prefix,
            },
        );
    }
    if let Some(path) = &search.path {
        for dir in std::env::split_paths(path) {
            add(dir.join("R"), How::Plain);
        }
    }
    for r in installed_versions() {
        add(r, How::Plain);
    }
    found
        .into_iter()
        .map(|(r, how)| Candidate {
            version: version(&r).unwrap_or_default(),
            r,
            how,
        })
        .collect()
}

/// R installed outside `PATH`: CRAN's framework and its versions (rig
/// installs several) on a Mac, rig's `/opt/R` on Linux.
fn installed_versions() -> Vec<PathBuf> {
    let mut found = Vec::new();
    let versions = |dir: &str, bin: &str| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut rs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join(bin))
            .filter(|r| r.is_file())
            .collect();
        rs.sort();
        rs
    };
    if cfg!(target_os = "macos") {
        found.push(PathBuf::from(
            "/Library/Frameworks/R.framework/Resources/bin/R",
        ));
        found.extend(versions(
            "/Library/Frameworks/R.framework/Versions",
            "Resources/bin/R",
        ));
    } else {
        found.extend(versions("/opt/R", "bin/R"));
    }
    found
}

/// The R programs installed IRkernels run, resolved to their real paths.
fn irkernel_rs(search: &Search) -> HashSet<PathBuf> {
    kernelspecs::discover(search)
        .into_iter()
        .map(|found| found.dir.kernelspec)
        .filter(|spec| spec.argv.iter().any(|arg| arg.contains("IRkernel::main")))
        .filter_map(|spec| spec_r(&spec, search).map(|p| canonical(&p)))
        .collect()
}

/// The R an R kernel runs: IRkernel's program, else (Ark) the `R` on the
/// kernel's `PATH` or the machine's.
pub fn spec_r(spec: &jupyter_protocol::JupyterKernelspec, search: &Search) -> Option<PathBuf> {
    let irkernel = spec.argv.iter().any(|arg| arg.contains("IRkernel::main"));
    let program = if irkernel {
        PathBuf::from(spec.argv.first()?)
    } else {
        PathBuf::from("R")
    };
    if program.is_absolute() {
        return program.is_file().then_some(program);
    }
    let path = spec
        .env
        .as_ref()
        .and_then(|env| env.get("PATH"))
        .map(std::ffi::OsString::from)
        .or_else(|| search.path.clone())?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&program))
        .find(|p| p.is_file())
}

/// R's version, like `4.6.2`, as `R --version` says it.
pub fn r_version(r: &Path) -> Option<String> {
    version(r)
}

/// R's version, from `R --version`: "R version 4.6.2 (2026-10-29) …".
fn version(r: &Path) -> Option<String> {
    let mut child = Command::new(r)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() > VERSION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version(&out)
}

fn parse_version(said: &str) -> Option<String> {
    let rest = said.lines().next()?.strip_prefix("R version ")?;
    let number = rest.split_whitespace().next()?;
    number
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.')
        .then(|| number.to_string())
}

impl Candidate {
    fn named(&self) -> String {
        if self.version.is_empty() {
            "R".into()
        } else {
            format!("R {}", self.version)
        }
    }

    /// What it is for the app: the R, its version and what installing does.
    pub fn info(&self, search: &Search) -> RWithoutIrkernel {
        let plan = match &self.how {
            How::Plain => {
                let (name, dir) = kernel_dir(self, search);
                format!(
                    "Installs the R package IRkernel into the library you choose, from its package mirror (else {CRAN}), and registers the kernel \"{}\" as {name} in {}.",
                    display_name(self),
                    dir.parent()
                        .map(Path::display)
                        .map(|d| d.to_string())
                        .unwrap_or_default(),
                )
            }
            How::Conda { prefix, label } => format!(
                "Runs `{} install -y -p {} -c conda-forge r-irkernel`, adding IRkernel and its kernel to {label}.",
                conda_tool(prefix),
                prefix.display()
            ),
            How::Pixi { .. } => format!(
                "Runs `{} add r-irkernel` in the project's folder, which adds it to the project's pixi manifest and installs it.",
                pixi_tool(search)
            ),
        };
        RWithoutIrkernel {
            r: self.r.to_string_lossy().into_owned(),
            version: self.version.clone(),
            env: match &self.how {
                How::Plain => None,
                How::Conda { label, .. } => Some(label.clone()),
                How::Pixi { .. } => Some("pixi default".into()),
            },
            plan,
            needs_library: matches!(self.how, How::Plain),
        }
    }
}

fn display_name(candidate: &Candidate) -> String {
    format!("{} (IRkernel)", candidate.named())
}

/// The kernel's name and folder in the user's Jupyter folder: `ir`, or
/// `ir-<version>` when another kernel has that name.
fn kernel_dir(candidate: &Candidate, search: &Search) -> (String, PathBuf) {
    let taken: HashSet<String> = kernelspecs::discover(search)
        .into_iter()
        .map(|found| found.dir.kernel_name)
        .collect();
    let kernels = search
        .user_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("kernels");
    let free = |name: &str| !taken.contains(name) && !kernels.join(name).exists();
    let versioned = if candidate.version.is_empty() {
        "ir-r".to_string()
    } else {
        format!("ir-{}", candidate.version)
    };
    let name = std::iter::once("ir".to_string())
        .chain(std::iter::once(versioned.clone()))
        .chain((2..).map(|n| format!("{versioned}-{n}")))
        .find(|name| free(name))
        .unwrap_or(versioned);
    (name.clone(), kernels.join(name))
}

/// conda's own program for an environment: mamba, then conda, from its
/// installation, else whichever is on `PATH`.
pub(crate) fn conda_tool(prefix: &Path) -> String {
    let root = kernelspecs::conda_root(prefix);
    ["bin/mamba", "bin/conda", "condabin/conda"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "conda".into())
}

fn pixi_tool(search: &Search) -> String {
    search
        .path
        .as_ref()
        .and_then(|path| {
            std::env::split_paths(path)
                .map(|dir| dir.join("pixi"))
                .find(|p| p.is_file())
        })
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME")?);
            Some(home.join(".pixi/bin/pixi")).filter(|p| p.is_file())
        })
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pixi".into())
}

/// A string as R writes one.
fn r_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The R code that installs IRkernel, if need be, into `library` (one of
/// R's libraries the user chose; Sandhill never creates one, Huan,
/// 2026-11-19) and registers the kernel in `dir` with `r` as its program.
/// What it adds, and the kernel folder, go in Sandhill's `record`, so it
/// can be removed later.
fn install_code(r: &Path, name: &str, dir: &Path, library: &Path, record: &Path) -> String {
    format!(
        r#"local({{
  {append}
  repos <- getOption("repos")
  if (!length(repos) || any(is.na(repos) | !nzchar(repos) | repos == "@CRAN@")) repos <- c(CRAN = {cran})
  lib <- {library}
  {check_library}
  if (!requireNamespace("IRkernel", quietly = TRUE)) {{
    before <- rownames(utils::installed.packages(lib.loc = lib))
    message("Installing IRkernel into ", lib, " from ", paste(repos, collapse = ", "))
    install.packages("IRkernel", lib = lib, repos = repos)
    sandhill_record(lib, before)
  }}
  if (!requireNamespace("IRkernel", quietly = TRUE)) stop("IRkernel could not be installed; see the messages above.")
  src <- system.file("kernelspec", package = "IRkernel")
  spec <- jsonlite::fromJSON(file.path(src, "kernel.json"))
  spec$argv[[1]] <- {r}
  spec$display_name <- {name}
  dir <- {dir}
  dir.create(dir, recursive = TRUE, showWarnings = FALSE)
  for (logo in list.files(src, pattern = "^logo")) file.copy(file.path(src, logo), file.path(dir, logo), overwrite = TRUE)
  jsonlite::write_json(spec, file.path(dir, "kernel.json"), auto_unbox = TRUE, pretty = TRUE)
  {append_kernel}
  message("Registered the kernel ", basename(dir), " (", spec$display_name, ") in ", dirname(dir))
}})"#,
        append = crate::quick::record::r_append(record),
        append_kernel = crate::quick::record::r_append_kernel(record, "dir"),
        library = r_string(&library.to_string_lossy()),
        check_library = check_library_code(),
        cran = r_string(CRAN),
        r = r_string(&r.to_string_lossy()),
        name = r_string(name),
        dir = r_string(&dir.to_string_lossy()),
    )
}

/// R code that stops unless `lib` is an existing library the user can
/// write to, and puts it first so what is installed there is found.
pub(crate) fn check_library_code() -> &'static str {
    r#"if (!dir.exists(lib)) stop("The library ", lib, " doesn't exist. Sandhill doesn't create libraries: choose another one, or make it yourself.", call. = FALSE)
  if (file.access(lib, 2) != 0) stop("You can't write to the library ", lib, ". Choose another one.", call. = FALSE)
  .libPaths(unique(c(lib, .libPaths())))"#
}

/// R code printing each of R's libraries as `SANDHILL_LIBPATH`, its path,
/// whether the user can write to it and how many packages it holds.
pub(crate) const LIBRARIES_CODE: &str = "for (l in .libPaths()) { cat('SANDHILL_LIBPATH', l, if (file.access(l, 2) == 0) 'yes' else 'no', length(list.files(l)), sep = '\\t'); cat('\\n') }";

/// R's libraries, from what [`LIBRARIES_CODE`] printed.
pub(crate) fn parse_libraries(out: &str) -> Vec<RLibrary> {
    out.lines()
        .filter_map(|line| match line.split('\t').collect::<Vec<_>>()[..] {
            ["SANDHILL_LIBPATH", path, writable, packages, ..] => Some(RLibrary {
                path: path.to_string(),
                writable: writable == "yes",
                packages: packages.trim().parse().unwrap_or(0),
            }),
            _ => None,
        })
        .collect()
}

/// The package libraries of the R at `r`, as R reads them with the user's
/// own `.Renviron` and `.Rprofile`, as the install job and the kernel do.
pub fn libraries(r: &Path) -> Result<Vec<RLibrary>> {
    let out = Command::new(r)
        .args(["--no-save", "--no-restore", "-s", "-e", LIBRARIES_CODE])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("couldn't run {}", r.display()))?;
    Ok(parse_libraries(&String::from_utf8_lossy(&out.stdout)))
}

/// The job that adds IRkernel to the R at `r`, into `library` when R
/// itself installs it (the offer `needs_library`), and the folder it acts
/// on that must be trusted (a pixi project's), if any.
pub fn launch(
    exe: &Path,
    r: &str,
    library: Option<&str>,
    search: &Search,
    record: &Path,
) -> Result<(Launch, Option<PathBuf>)> {
    let wanted = canonical(Path::new(r));
    let Some(candidate) = find(search)
        .into_iter()
        .find(|c| c.r == Path::new(r) || canonical(&c.r) == wanted)
    else {
        bail!("{r} isn't an R without IRkernel that Sandhill found");
    };
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME isn't set")?);
    let (line, cwd, trust) = match &candidate.how {
        How::Plain => {
            let rscript = candidate
                .r
                .parent()
                .map(|bin| bin.join("Rscript"))
                .filter(|p| p.is_file())
                .with_context(|| format!("there is no Rscript beside {r}"))?;
            let (_, dir) = kernel_dir(&candidate, search);
            let library = library.context("choose which of R's libraries IRkernel goes in")?;
            let code = install_code(
                &candidate.r,
                &display_name(&candidate),
                &dir,
                Path::new(library),
                record,
            );
            (
                crate::quick::install::worker_command(
                    exe,
                    record,
                    &crate::quick::install::Job::Irkernel {
                        command: format!(
                            "{} -e {}",
                            shell_quote(&rscript.to_string_lossy()),
                            shell_quote(&code)
                        ),
                        prefix: None,
                    },
                )?,
                home,
                None,
            )
        }
        How::Conda { prefix, .. } => (
            crate::quick::install::worker_command(
                exe,
                record,
                &crate::quick::install::Job::Irkernel {
                    command: format!(
                        "{} install -y -p {} -c conda-forge r-irkernel",
                        shell_quote(&conda_tool(prefix)),
                        shell_quote(&prefix.to_string_lossy())
                    ),
                    prefix: Some(prefix.to_string_lossy().into_owned()),
                },
            )?,
            home,
            None,
        ),
        How::Pixi { root } => (
            format!("{} add r-irkernel", shell_quote(&pixi_tool(search))),
            root.clone(),
            Some(root.clone()),
        ),
    };
    let job = JobSpec {
        path: None,
        command: Some(line),
    };
    let mut launch = crate::jobs::launch(&job, Some(&cwd), None)?;
    launch.title = format!("Install IRkernel for {}", candidate.named());
    Ok((launch, trust))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_r_s_first_line() {
        assert_eq!(
            parse_version("R version 4.6.2 (2026-10-29) -- \"Some Name\"\nCopyright"),
            Some("4.6.2".into())
        );
        assert_eq!(parse_version("R Under development (unstable)"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn r_strings_escape_quotes_and_backslashes() {
        assert_eq!(r_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }

    #[test]
    fn the_kernel_is_ir_unless_that_name_is_taken() {
        let tmp = tempfile::tempdir().unwrap();
        let search = Search {
            user_dir: Some(tmp.path().to_path_buf()),
            ..Search::default()
        };
        let candidate = Candidate {
            r: "/usr/bin/R".into(),
            version: "4.6.2".into(),
            how: How::Plain,
        };
        assert_eq!(kernel_dir(&candidate, &search).0, "ir");
        std::fs::create_dir_all(tmp.path().join("kernels/ir")).unwrap();
        assert_eq!(kernel_dir(&candidate, &search).0, "ir-4.6.2");
        std::fs::create_dir_all(tmp.path().join("kernels/ir-4.6.2")).unwrap();
        assert_eq!(kernel_dir(&candidate, &search).0, "ir-4.6.2-2");
    }
}
