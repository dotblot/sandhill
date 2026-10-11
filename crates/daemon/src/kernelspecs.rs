//! Finding the Jupyter kernels installed on this machine.
//!
//! Jupyter looks in a few standard folders and in the environment it runs
//! from. Most people who analyze data in Python or R keep their kernels
//! inside conda environments (or pyenv versions), where Jupyter only sees
//! them when it was started from that environment. The daemon is started from
//! none of them, so besides the standard folders it looks inside every
//! environment it can find:
//!
//! - the environment of the first Python on `PATH` (the one the user's shell
//!   activates), whose kernels keep their usual names such as `python3`;
//! - every conda and mamba environment, named the way nb_conda_kernels does
//!   (`conda-env-<env>-py`, `conda-env-<env>-r`);
//! - pyenv versions (`pyenv-<version>-py`).
//!
//! A kernel found inside an environment runs that environment's own Python or
//! R, with the environment's `bin` first on `PATH`, much as if it had been
//! activated. A kernel that runs the same command as one listed before it
//! (say, an environment registered with `ipykernel install --user` and found
//! again inside the environment) is listed once, but both names still work.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jupyter_protocol::JupyterKernelspec;
use jupyter_zmq_client::KernelspecDir;
use sandhill_proto::KernelspecInfo;

/// Where to look. [`Search::from_env`] fills it in from the daemon's
/// environment; tests build their own.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// `$JUPYTER_PATH`.
    pub jupyter_path: Vec<PathBuf>,
    /// The user's Jupyter data folder, where `--user` installs go.
    pub user_dir: Option<PathBuf>,
    /// System-wide Jupyter data folders.
    pub system_dirs: Vec<PathBuf>,
    /// `$PATH`. The first Python on it is the default environment.
    pub path: Option<OsString>,
    /// Files listing conda environments, one prefix per line
    /// (conda keeps one in `~/.conda/environments.txt`).
    pub conda_lists: Vec<PathBuf>,
    /// Where conda and mamba are usually installed. Each may be an
    /// environment itself (the base) and hold more in `envs/`.
    pub conda_roots: Vec<PathBuf>,
    /// Folders of Python installations, like pyenv's `versions`.
    pub version_dirs: Vec<PathBuf>,
}

impl Search {
    pub fn from_env() -> Search {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let in_home = |rel: &str| home.as_ref().map(|h| h.join(rel));

        let user_dir = var("JUPYTER_DATA_DIR").map(PathBuf::from).or_else(|| {
            if cfg!(target_os = "macos") {
                in_home("Library/Jupyter")
            } else {
                var("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| in_home(".local/share"))
                    .map(|d| d.join("jupyter"))
            }
        });
        let mut system_dirs: Vec<PathBuf> = vec![
            "/usr/local/share/jupyter".into(),
            "/usr/share/jupyter".into(),
        ];
        if cfg!(target_os = "macos") {
            system_dirs.push("/opt/homebrew/share/jupyter".into());
        }

        let mut conda_roots = Vec::new();
        // conda sets these in an activated shell; the root is two levels above `bin/conda`.
        if let Some(exe) = var("CONDA_EXE").map(PathBuf::from) {
            conda_roots.extend(exe.parent().and_then(Path::parent).map(Path::to_path_buf));
        }
        conda_roots.extend(var("MAMBA_ROOT_PREFIX").map(PathBuf::from));
        conda_roots.extend(var("CONDA_PREFIX").map(PathBuf::from));
        for rel in [
            "miniconda3",
            "anaconda3",
            "miniforge3",
            "mambaforge",
            "micromamba",
            "opt/miniconda3",
            "opt/anaconda3",
            ".conda",
            ".local/share/mamba",
        ] {
            conda_roots.extend(in_home(rel));
        }
        for dir in [
            "/opt/homebrew/Caskroom/miniconda/base",
            "/opt/homebrew/Caskroom/miniforge/base",
            "/opt/homebrew/anaconda3",
            "/usr/local/Caskroom/miniconda/base",
            "/usr/local/Caskroom/miniforge/base",
            "/opt/miniconda3",
            "/opt/anaconda3",
            "/opt/miniforge3",
            "/opt/conda",
        ] {
            conda_roots.push(dir.into());
        }

        let pyenv = var("PYENV_ROOT")
            .map(PathBuf::from)
            .or_else(|| in_home(".pyenv"));
        Search {
            jupyter_path: var("JUPYTER_PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
            user_dir,
            system_dirs,
            path: var("PATH"),
            conda_lists: in_home(".conda/environments.txt").into_iter().collect(),
            conda_roots,
            version_dirs: pyenv.map(|p| p.join("versions")).into_iter().collect(),
        }
    }
}

/// A kernel found on this machine.
#[derive(Debug, Clone)]
pub struct Found {
    pub dir: KernelspecDir,
    /// The environment it runs in: a conda env, a venv or a pyenv version.
    pub env: Option<String>,
    /// Runs the same command as a kernel found before it, so it isn't listed.
    pub duplicate: bool,
}

/// The kernels to offer, without duplicates, sorted by name.
pub fn list(search: &Search) -> Vec<KernelspecInfo> {
    let mut specs: Vec<KernelspecInfo> = discover(search)
        .into_iter()
        .filter(|k| !k.duplicate)
        .map(|k| KernelspecInfo {
            name: k.dir.kernel_name,
            display_name: k.dir.kernelspec.display_name,
            language: k.dir.kernelspec.language,
            env: k.env,
        })
        .collect();
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    specs
}

pub fn find(search: &Search, name: &str) -> Option<KernelspecDir> {
    discover(search)
        .into_iter()
        .find(|k| k.dir.kernel_name == name)
        .map(|k| k.dir)
}

/// Every kernel, in the order Jupyter would prefer them. Names are unique:
/// a later kernel with a name already taken is left out, as Jupyter does.
pub fn discover(search: &Search) -> Vec<Found> {
    let mut found = Vec::new();
    let mut names = HashSet::new();
    let mut commands = HashSet::new();
    let mut folders = HashSet::new();
    for source in sources(search) {
        // The shell's environment is usually also a conda environment. Its
        // folder is read again under the conda names: a kernel that lost
        // its plain name to an earlier one (a `python3` installed with
        // `--user`) is listed that way, and the rest are duplicates.
        if !folders.insert((canonical(&source.data_dir), source.names.clone())) {
            continue;
        }
        for (dir_name, path, spec) in read_kernels(&source.data_dir) {
            let name = match &source.names {
                Some(prefix) => sanitize(&format!("{prefix}-{}", short_name(&dir_name))),
                None => dir_name,
            };
            if !names.insert(name.clone()) {
                continue;
            }
            let (kernelspec, env) = resolve(spec, source.env.as_ref(), search);
            let duplicate = !commands.insert(command_key(&kernelspec.argv));
            found.push(Found {
                dir: KernelspecDir {
                    kernel_name: name,
                    path,
                    kernelspec,
                },
                env,
                duplicate,
            });
        }
    }
    found
}

/// A folder with a `kernels/` subfolder.
struct Source {
    data_dir: PathBuf,
    /// The environment the folder belongs to, if any.
    env: Option<Env>,
    /// Kernels get this name prefix; the standard folders and the default
    /// environment keep plain names.
    names: Option<String>,
}

#[derive(Debug, Clone)]
struct Env {
    prefix: PathBuf,
    name: String,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Conda,
    Venv,
    Pyenv,
    /// A plain installation such as `/usr` or Homebrew, not an environment.
    System,
}

fn sources(search: &Search) -> Vec<Source> {
    let plain = |data_dir: &Path| Source {
        data_dir: data_dir.to_path_buf(),
        env: None,
        names: None,
    };
    let mut sources: Vec<Source> = search.jupyter_path.iter().map(|d| plain(d)).collect();
    sources.extend(search.user_dir.as_deref().map(plain));
    // Jupyter also reads the folder of the environment it runs from; the
    // closest thing here is the environment the user's shell activates.
    if let Some(prefix) = default_prefix(search) {
        let env = classify(&prefix, search).unwrap_or(Env {
            prefix: prefix.clone(),
            name: String::new(),
            kind: Kind::System,
        });
        sources.push(Source {
            data_dir: prefix.join("share/jupyter"),
            env: Some(env),
            names: None,
        });
    }
    sources.extend(search.system_dirs.iter().map(|d| plain(d)));

    let mut taken: HashMap<String, PathBuf> = HashMap::new();
    for prefix in conda_envs(search) {
        let mut name = conda_name(&prefix);
        if taken.get(&name).is_some_and(|p| *p != prefix) {
            // A second conda installation: tell its environments apart.
            let root = conda_root(&prefix);
            let root = root.file_name().unwrap_or_default().to_string_lossy();
            name = if name == "base" {
                root.into_owned()
            } else {
                format!("{root}/{name}")
            };
        }
        taken.insert(name.clone(), prefix.clone());
        sources.push(Source {
            data_dir: prefix.join("share/jupyter"),
            names: Some(format!("conda-env-{name}")),
            env: Some(Env {
                prefix,
                name,
                kind: Kind::Conda,
            }),
        });
    }
    for dir in &search.version_dirs {
        for prefix in subfolders(dir) {
            let name = file_name(&prefix);
            let kind = classify(&prefix, search).map_or(Kind::Pyenv, |e| e.kind);
            sources.push(Source {
                data_dir: prefix.join("share/jupyter"),
                names: Some(format!("pyenv-{name}")),
                env: Some(Env { prefix, name, kind }),
            });
        }
    }
    sources
}

/// Reads `<data_dir>/kernels/*/kernel.json`, skipping what doesn't parse.
fn read_kernels(data_dir: &Path) -> Vec<(String, PathBuf, JupyterKernelspec)> {
    let mut kernels = Vec::new();
    for path in subfolders(&data_dir.join("kernels")) {
        let Ok(bytes) = std::fs::read(path.join("kernel.json")) else {
            continue;
        };
        match serde_json::from_slice::<JupyterKernelspec>(&bytes) {
            Ok(spec) => kernels.push((file_name(&path), path, spec)),
            Err(err) => tracing::debug!("skipping {}: {err}", path.display()),
        }
    }
    kernels
}

/// Makes a kernel run inside its environment: the environment's own program
/// (kernel.json files inside environments often just say `python`), with the
/// environment activated. Returns the environment's name.
fn resolve(
    mut spec: JupyterKernelspec,
    source_env: Option<&Env>,
    search: &Search,
) -> (JupyterKernelspec, Option<String>) {
    let Some(env) = source_env
        .cloned()
        .or_else(|| command_env(&spec.argv, search))
    else {
        return (spec, None);
    };
    if let Some(program) = spec.argv.first_mut()
        && !program.contains('/')
    {
        let local = env.prefix.join("bin").join(&*program);
        if local.is_file() {
            *program = local.to_string_lossy().into_owned();
        }
    }
    if env.kind == Kind::System {
        return (spec, None);
    }

    let vars = spec.env.get_or_insert_with(HashMap::new);
    if !vars.contains_key("PATH") {
        let mut dirs = vec![env.prefix.join("bin")];
        if let Some(path) = &search.path {
            dirs.extend(std::env::split_paths(path));
        }
        if let Ok(path) = std::env::join_paths(dirs) {
            vars.insert("PATH".into(), path.to_string_lossy().into_owned());
        }
    }
    let prefix = env.prefix.to_string_lossy().into_owned();
    match env.kind {
        Kind::Conda => {
            vars.entry("CONDA_PREFIX".into()).or_insert(prefix);
            vars.entry("CONDA_DEFAULT_ENV".into())
                .or_insert_with(|| env.name.clone());
        }
        Kind::Venv => {
            vars.entry("VIRTUAL_ENV".into()).or_insert(prefix);
        }
        Kind::Pyenv | Kind::System => {}
    }
    // Kernels found inside an environment all call themselves "Python 3
    // (ipykernel)"; name the environment instead. A kernel the user
    // registered keeps the name they gave it.
    if source_env.is_some() {
        spec.display_name = env_display_name(&spec, &env.name);
    }
    (spec, Some(env.name))
}

fn env_display_name(spec: &JupyterKernelspec, env: &str) -> String {
    let language = spec.language.to_lowercase();
    if language.starts_with("python") {
        format!("Python ({env})")
    } else if language == "r" {
        format!("R ({env})")
    } else {
        format!("{} ({env})", spec.display_name)
    }
}

/// The environment of a kernel registered with an absolute path to its
/// program, such as `<env>/bin/python` after `ipykernel install --user`.
fn command_env(argv: &[String], search: &Search) -> Option<Env> {
    let program = Path::new(argv.first()?);
    let bin = program.parent()?;
    if !program.is_absolute() || bin.file_name()? != "bin" {
        return None;
    }
    classify(bin.parent()?, search)
}

/// What kind of environment a prefix is, if it is one.
fn classify(prefix: &Path, search: &Search) -> Option<Env> {
    let env = |name: String, kind| {
        Some(Env {
            prefix: prefix.to_path_buf(),
            name,
            kind,
        })
    };
    if prefix.join("conda-meta").is_dir() {
        return env(conda_name(prefix), Kind::Conda);
    }
    if prefix.join("pyvenv.cfg").is_file() {
        // Project environments are usually `<project>/.venv`.
        let name = file_name(prefix);
        let name = match (name.as_str(), prefix.parent()) {
            (".venv" | "venv" | "env", Some(parent)) => file_name(parent),
            _ => name,
        };
        return env(name, Kind::Venv);
    }
    if search
        .version_dirs
        .iter()
        .any(|dir| prefix.parent() == Some(dir.as_path()))
    {
        return env(file_name(prefix), Kind::Pyenv);
    }
    None
}

/// Where the first `python3` (or `python`) on `PATH` is installed.
fn default_prefix(search: &Search) -> Option<PathBuf> {
    let path = search.path.as_ref()?;
    std::env::split_paths(path).find_map(|dir| {
        let found = ["python3", "python"]
            .iter()
            .any(|exe| dir.join(exe).is_file());
        if found && dir.file_name()? == "bin" {
            dir.parent().map(Path::to_path_buf)
        } else {
            None
        }
    })
}

/// Every conda environment, from conda's own list and the usual install
/// locations, each once.
fn conda_envs(search: &Search) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for list in &search.conda_lists {
        if let Ok(text) = std::fs::read_to_string(list) {
            candidates.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(PathBuf::from),
            );
        }
    }
    for root in &search.conda_roots {
        candidates.push(root.clone());
        candidates.extend(subfolders(&root.join("envs")));
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|prefix| prefix.join("conda-meta").is_dir() && seen.insert(canonical(prefix)))
        .collect()
}

/// `base` for a conda installation itself, else the environment's folder name.
fn conda_name(prefix: &Path) -> String {
    if conda_root(prefix) == prefix {
        "base".into()
    } else {
        file_name(prefix)
    }
}

fn conda_root(prefix: &Path) -> &Path {
    match prefix.parent() {
        Some(envs) if envs.file_name().is_some_and(|n| n == "envs") => {
            envs.parent().unwrap_or(prefix)
        }
        _ => prefix,
    }
}

/// nb_conda_kernels calls an environment's ipykernel `py` and IRkernel `r`.
fn short_name(kernel: &str) -> &str {
    match kernel {
        "python3" => "py",
        "ir" => "r",
        other => other,
    }
}

/// Kernel names are `[A-Za-z0-9._-]`.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// What a kernel runs, to spot the same kernel installed under two names.
fn command_key(argv: &[String]) -> Vec<String> {
    let mut key = argv.to_vec();
    if let Some(program) = key.first_mut()
        && program.starts_with('/')
    {
        *program = canonical(Path::new(program)).to_string_lossy().into_owned();
    }
    key
}

fn subfolders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Writes `<data_dir>/kernels/<name>/kernel.json`.
    fn kernel(data_dir: &Path, name: &str, spec: serde_json::Value) {
        let dir = data_dir.join("kernels").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kernel.json"), spec.to_string()).unwrap();
    }

    fn ipykernel(argv0: &str) -> serde_json::Value {
        json!({
            "argv": [argv0, "-m", "ipykernel_launcher", "-f", "{connection_file}"],
            "display_name": "Python 3 (ipykernel)",
            "language": "python",
        })
    }

    /// A conda environment with Python and ipykernel installed.
    fn conda_env(prefix: &Path) {
        std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
        python(prefix);
        kernel(
            &prefix.join("share/jupyter"),
            "python3",
            ipykernel("python"),
        );
    }

    fn python(prefix: &Path) {
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::write(prefix.join("bin/python"), "").unwrap();
        std::os::unix::fs::symlink("python", prefix.join("bin/python3")).unwrap();
    }

    fn names(found: &[Found]) -> Vec<(&str, bool)> {
        found
            .iter()
            .map(|k| (k.dir.kernel_name.as_str(), k.duplicate))
            .collect()
    }

    #[test]
    fn conda_environments_are_found_and_run_their_own_python() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("miniconda3");
        conda_env(&root);
        conda_env(&root.join("envs/omicverse"));
        // An R environment with IRkernel, whose kernel.json names R by path.
        let r_env = root.join("envs/r-seurat");
        std::fs::create_dir_all(r_env.join("conda-meta")).unwrap();
        kernel(
            &r_env.join("share/jupyter"),
            "ir",
            json!({
                "argv": [r_env.join("bin/R"), "--slave", "-e", "IRkernel::main()", "--args", "{connection_file}"],
                "display_name": "R",
                "language": "R",
            }),
        );
        let search = Search {
            conda_roots: vec![root.clone()],
            path: Some("/nowhere/bin:/elsewhere/bin".into()),
            ..Search::default()
        };

        let found = discover(&search);
        assert_eq!(
            names(&found),
            [
                ("conda-env-base-py", false),
                ("conda-env-omicverse-py", false),
                ("conda-env-r-seurat-r", false),
            ]
        );
        let omicverse = &found[1];
        assert_eq!(omicverse.env.as_deref(), Some("omicverse"));
        let spec = &omicverse.dir.kernelspec;
        assert_eq!(spec.display_name, "Python (omicverse)");
        let prefix = root.join("envs/omicverse");
        assert_eq!(spec.argv[0], prefix.join("bin/python").to_str().unwrap());
        let vars = spec.env.as_ref().unwrap();
        assert_eq!(
            vars["PATH"],
            format!(
                "{}:/nowhere/bin:/elsewhere/bin",
                prefix.join("bin").display()
            )
        );
        assert_eq!(vars["CONDA_PREFIX"], prefix.to_str().unwrap());
        assert_eq!(vars["CONDA_DEFAULT_ENV"], "omicverse");
        assert_eq!(found[2].dir.kernelspec.display_name, "R (r-seurat)");

        let listed = list(&search);
        assert_eq!(listed[1].env.as_deref(), Some("omicverse"));
        assert!(find(&search, "conda-env-omicverse-py").is_some());
        assert!(find(&search, "python3").is_none());
    }

    #[test]
    fn the_shell_environment_keeps_plain_names_and_duplicates_are_listed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("miniforge3");
        conda_env(&root);
        let env = root.join("envs/scanpy");
        conda_env(&env);
        // Registered by hand with `ipykernel install --user --name scanpy`.
        let user = tmp.path().join("Jupyter");
        kernel(
            &user,
            "scanpy",
            json!({
                "argv": [env.join("bin/python3"), "-m", "ipykernel_launcher", "-f", "{connection_file}"],
                "display_name": "Scanpy",
                "language": "python",
            }),
        );
        let list_file = tmp.path().join("environments.txt");
        std::fs::write(
            &list_file,
            format!("{}\n{}\n", root.display(), env.display()),
        )
        .unwrap();
        let search = Search {
            user_dir: Some(user),
            // The shell activates the scanpy environment.
            path: Some(format!("{}:/nowhere/bin", env.join("bin").display()).into()),
            conda_lists: vec![list_file],
            conda_roots: vec![root.clone()],
            ..Search::default()
        };

        let found = discover(&search);
        assert_eq!(
            names(&found),
            [
                ("scanpy", false),
                ("python3", true),
                ("conda-env-base-py", false),
                ("conda-env-scanpy-py", true),
            ]
        );
        // The hand-registered kernel keeps its name but runs activated.
        let scanpy = &found[0];
        assert_eq!(scanpy.dir.kernelspec.display_name, "Scanpy");
        assert_eq!(scanpy.env.as_deref(), Some("scanpy"));
        assert_eq!(
            scanpy.dir.kernelspec.env.as_ref().unwrap()["CONDA_PREFIX"],
            env.to_str().unwrap()
        );
        // `python3` still starts the shell's environment, though it isn't listed.
        let python3 = find(&search, "python3").unwrap();
        assert_eq!(
            python3.kernelspec.argv[0],
            env.join("bin/python").to_str().unwrap()
        );
        let listed: Vec<_> = list(&search).into_iter().map(|k| k.name).collect();
        assert_eq!(listed, ["conda-env-base-py", "scanpy"]);
    }

    #[test]
    fn the_shell_environment_is_listed_when_a_user_kernel_has_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("miniforge3");
        let env = root.join("envs/omicverse");
        conda_env(&env);
        // `python3 -m ipykernel install --user` with the system Python.
        let user = tmp.path().join("Jupyter");
        kernel(&user, "python3", ipykernel("/usr/bin/python3"));
        let search = Search {
            user_dir: Some(user),
            // The shell activates omicverse.
            path: Some(format!("{}:/usr/bin", env.join("bin").display()).into()),
            conda_roots: vec![root],
            ..Search::default()
        };

        let found = discover(&search);
        assert_eq!(
            names(&found),
            [("python3", false), ("conda-env-omicverse-py", false)]
        );
        assert_eq!(found[0].dir.kernelspec.argv[0], "/usr/bin/python3");
        let omicverse = &found[1].dir.kernelspec;
        assert_eq!(omicverse.display_name, "Python (omicverse)");
        assert_eq!(omicverse.argv[0], env.join("bin/python").to_str().unwrap());
    }

    #[test]
    fn standard_folders_come_first_and_system_kernels_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("first");
        let user = tmp.path().join("user");
        let system = tmp.path().join("usr");
        kernel(&first, "python3", ipykernel("/opt/py/bin/python3"));
        kernel(&user, "python3", ipykernel("/somewhere/else/python3"));
        kernel(
            &user,
            "ark",
            json!({"argv": ["ark", "--connection_file", "{connection_file}"], "display_name": "Ark R Kernel", "language": "R"}),
        );
        python(&system);
        kernel(
            &system.join("share/jupyter"),
            "python3",
            ipykernel("python3"),
        );
        kernel(&system.join("share/jupyter"), "other", ipykernel("python3"));
        let search = Search {
            jupyter_path: vec![first],
            user_dir: Some(user),
            path: Some(system.join("bin").into_os_string()),
            ..Search::default()
        };

        let found = discover(&search);
        assert_eq!(
            names(&found),
            [("python3", false), ("ark", false), ("other", false)]
        );
        assert_eq!(found[0].dir.kernelspec.argv[0], "/opt/py/bin/python3");
        assert!(found.iter().all(|k| k.env.is_none()));
        // A plain installation's kernel runs its own python but isn't renamed
        // or given an environment.
        let other = &found[2].dir.kernelspec;
        assert_eq!(other.argv[0], system.join("bin/python3").to_str().unwrap());
        assert_eq!(other.display_name, "Python 3 (ipykernel)");
        assert!(other.env.is_none());
    }

    #[test]
    fn a_second_conda_installation_gets_its_own_names() {
        let tmp = tempfile::tempdir().unwrap();
        let mini = tmp.path().join("miniconda3");
        let ana = tmp.path().join("anaconda3");
        for prefix in [&mini, &mini.join("envs/py"), &ana, &ana.join("envs/py")] {
            conda_env(prefix);
        }
        let search = Search {
            conda_roots: vec![mini, ana],
            ..Search::default()
        };
        let found: Vec<_> = discover(&search)
            .into_iter()
            .map(|k| (k.dir.kernel_name, k.env.unwrap()))
            .collect();
        let pairs: Vec<_> = found
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("conda-env-base-py", "base"),
                ("conda-env-py-py", "py"),
                ("conda-env-anaconda3-py", "anaconda3"),
                ("conda-env-anaconda3-py-py", "anaconda3/py"),
            ]
        );
    }

    #[test]
    fn pyenv_versions_and_project_venvs() {
        let tmp = tempfile::tempdir().unwrap();
        let versions = tmp.path().join(".pyenv/versions");
        let v = versions.join("3.12.4");
        python(&v);
        kernel(&v.join("share/jupyter"), "python3", ipykernel("python"));
        let venv = tmp.path().join("atlas/.venv");
        python(&venv);
        std::fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        kernel(&venv.join("share/jupyter"), "python3", ipykernel("python"));
        let search = Search {
            version_dirs: vec![versions],
            path: Some(venv.join("bin").into_os_string()),
            ..Search::default()
        };

        let found = discover(&search);
        assert_eq!(
            names(&found),
            [("python3", false), ("pyenv-3.12.4-py", false)]
        );
        assert_eq!(found[0].env.as_deref(), Some("atlas"));
        assert_eq!(found[0].dir.kernelspec.display_name, "Python (atlas)");
        let vars = found[0].dir.kernelspec.env.as_ref().unwrap();
        assert_eq!(vars["VIRTUAL_ENV"], venv.to_str().unwrap());
        assert_eq!(found[1].env.as_deref(), Some("3.12.4"));
        assert_eq!(
            found[1].dir.kernelspec.argv[0],
            v.join("bin/python").to_str().unwrap()
        );
    }
}
