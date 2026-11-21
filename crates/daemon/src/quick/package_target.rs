use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use jupyter_protocol::JupyterKernelspec;
use sandhill_proto::{InstallPrefs, InstallTarget, RLibrary};
use serde::Deserialize;

use super::package_plan::{same_folder, writable};
use crate::irkernel::{LIBRARIES_CODE, parse_libraries};

pub(super) const R_BASE: &[&str] = &[
    "base",
    "compiler",
    "datasets",
    "graphics",
    "grDevices",
    "grid",
    "methods",
    "parallel",
    "splines",
    "stats",
    "stats4",
    "tcltk",
    "tools",
    "utils",
];

pub(crate) struct Checked {
    pub(crate) target: InstallTarget,
    pub(crate) absent: Vec<String>,
}

/// Names have already been checked to contain only letters, numbers, dots or underscores.
pub(crate) fn session_code(language: &str, names: &[String]) -> String {
    if language == "r" {
        let names = names
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"base::local({{
  cat("SANDHILL_R\t", file.path(R.home("bin"), "R"), "\n", sep = "")
  {LIBRARIES_CODE}
  x <- c({names})
  present <- vapply(x, function(p) nzchar(system.file(package = p)), TRUE)
  for (p in x[!present]) cat("SANDHILL_MISSING\t", p, "\n", sep = "")
}}, envir = base::new.env(parent = base::baseenv()))"#
        )
    } else {
        let names = serde_json::to_string(names).expect("strings serialize as JSON");
        let code = format!(
            r#"import importlib.util, json, sys
print('SANDHILL_PY\t' + json.dumps([sys.executable, sys.prefix]))
stdlib = getattr(sys, 'stdlib_module_names', sys.builtin_module_names)
for name in {names}:
    if name in stdlib:
        continue
    try:
        present = importlib.util.find_spec(name) is not None
    except ValueError:
        present = name in sys.modules and sys.modules[name] is not None
    if not present:
        print('SANDHILL_MISSING\t' + name)"#
        );
        // Keep the check out of the user's Python namespace.
        format!(
            "exec({}, {{}})",
            serde_json::to_string(&code).expect("code serializes as JSON")
        )
    }
}

pub(crate) fn session_result(
    language: &str,
    output: &str,
    prefs: &InstallPrefs,
    own: Option<&Path>,
) -> Result<Checked> {
    let field = |marker| output.lines().find_map(|line| line.strip_prefix(marker));
    let absent = output
        .lines()
        .filter_map(|line| line.strip_prefix("SANDHILL_MISSING\t"))
        .filter(|name| language != "r" || !R_BASE.contains(name))
        .map(str::to_owned)
        .collect();
    let target = if language == "r" {
        let r = field("SANDHILL_R\t").context("R couldn't tell Sandhill where its packages go.")?;
        r_target(r, &parse_libraries(output), prefs, own)
    } else {
        let line = field("SANDHILL_PY\t")
            .context("Python couldn't tell Sandhill where its packages go.")?;
        let [python, prefix] = serde_json::from_str(line)?;
        InstallTarget::Python { prefix, python }
    };
    Ok(Checked { target, absent })
}

fn r_target(
    r: &str,
    libraries: &[RLibrary],
    prefs: &InstallPrefs,
    own: Option<&Path>,
) -> InstallTarget {
    let writable = |lib: &&RLibrary| {
        lib.writable
            && own.is_none_or(|folder| crate::own_env::inside(folder, Path::new(&lib.path)))
    };
    let last = prefs.last_for("r").and_then(|choice| match &choice.target {
        InstallTarget::R { r: old, library } if same_folder(Path::new(old), Path::new(r)) => {
            libraries
                .iter()
                .filter(writable)
                .find(|lib| lib.path == *library)
        }
        _ => None,
    });
    let library = last
        .or_else(|| libraries.iter().find(writable))
        .map(|lib| lib.path.clone())
        .unwrap_or_default();
    InstallTarget::R {
        r: r.into(),
        library,
    }
}

pub(crate) fn r_files(
    program: &Path,
    spec: &JupyterKernelspec,
    names: Vec<String>,
    project: Option<&Path>,
    prefs: &InstallPrefs,
    own: Option<&Path>,
    restricted: bool,
) -> Result<Checked> {
    let mut libraries;
    let version;
    if restricted {
        libraries = r_file_libraries(program, spec, own)?;
        version = None;
    } else {
        let vars = project.map(crate::projects::kernel_env).unwrap_or_default();
        let mut command = Command::new(program);
        command.args(
            spec.argv
                .iter()
                .take_while(|arg| *arg != "-e")
                .filter(|arg| {
                    matches!(
                        arg.as_str(),
                        "--vanilla" | "--no-environ" | "--no-init-file" | "--no-site-file"
                    )
                }),
        );
        command.current_dir(
            std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from),
        );
        for (key, value) in vars
            .iter()
            .map(|(k, v)| (k, v))
            .chain(spec.env.iter().flatten())
        {
            if matches!(key.as_str(), "R_LIBS" | "R_LIBS_USER" | "R_LIBS_SITE") {
                command.env(key, value);
            }
        }
        if let Some(folder) = own {
            crate::own_env::apply(&mut command, folder);
            command.current_dir(folder);
        }
        let code = format!(
            "{LIBRARIES_CODE}; cat('SANDHILL_R_VERSION\\t', as.character(getRversion()), '\\t', R.version$platform, '\\n', sep='')"
        );
        let output = super::install::probe_command_with(
            command,
            &["--no-save", "--no-restore", "-s", "-e", &code],
            Duration::from_secs(8),
            false,
        )
        .context("R couldn't tell Sandhill where its packages go.")?;
        libraries = parse_libraries(&output);
        version = output
            .lines()
            .find_map(|l| l.strip_prefix("SANDHILL_R_VERSION\t"))
            .and_then(|s| {
                let (version, platform) = s.split_once('\t')?;
                Some((r_minor(version)?.to_string(), platform.to_string()))
            });
    }
    if !restricted
        && let Some(project) = project.filter(|p| p.join("renv.lock").is_file())
        && let Some((version, platform)) = version
    {
        let root = super::project_packages::renv_root(project, own);
        let version = format!("R-{version}");
        // renv may add a platform folder above R-<major>.<minor> as well as below it.
        let candidates =
            std::iter::once(root.join(&version)).chain(subdirs(&root).map(|p| p.join(&version)));
        let renv = candidates
            .map(|p| p.join(&platform))
            .filter(|p| p.is_dir() && own.is_none_or(|folder| crate::own_env::inside(folder, p)));
        // The project library is where renv would install first.
        libraries.splice(0..0, renv.map(file_library));
    }
    let target = r_target(&program.to_string_lossy(), &libraries, prefs, own);
    let absent = names
        .into_iter()
        .filter(|name| {
            !R_BASE.contains(&name.as_str())
                && !libraries.iter().any(|lib| {
                    Path::new(&lib.path)
                        .join(name)
                        .join("DESCRIPTION")
                        .is_file()
                })
        })
        .collect();
    Ok(Checked { target, absent })
}

fn r_minor(version: &str) -> Option<&str> {
    let minor_start = version.find('.')? + 1;
    Some(
        version[minor_start..]
            .find('.')
            .map_or(version, |dot| &version[..minor_start + dot]),
    )
}

/// The paths R typically uses, without running an R program or a profile.
fn r_file_libraries(
    program: &Path,
    spec: &JupyterKernelspec,
    own: Option<&Path>,
) -> Result<Vec<RLibrary>> {
    let actual = program
        .canonicalize()
        .unwrap_or_else(|_| program.to_path_buf());
    let standard = [program, &actual]
        .into_iter()
        .flat_map(|p| p.ancestors().take(8))
        .flat_map(|p| [p.join("library"), p.join("lib/R/library")])
        .find(|p| p.join("base/DESCRIPTION").is_file())
        .context("R's packages couldn't be checked in Restricted Mode.")?;
    let mut libraries = Vec::new();
    if own.is_none() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        for key in ["R_LIBS", "R_LIBS_USER", "R_LIBS_SITE"] {
            let configured = spec
                .env
                .as_ref()
                .and_then(|vars| vars.get(key))
                .map(std::ffi::OsString::from)
                .or_else(|| std::env::var_os(key));
            if let Some(configured) = configured {
                libraries
                    .extend(std::env::split_paths(&configured).filter(|path| path.is_absolute()));
            } else if key == "R_LIBS_USER"
                && let Some(home) = &home
                && let Ok(base) = std::fs::read_to_string(standard.join("base/DESCRIPTION"))
                && let Some(version) = super::record::r_field(&base, "Version").and_then(r_minor)
            {
                let arch = std::env::consts::ARCH;
                libraries.extend(
                    subdirs(&home.join("R"))
                        .filter(|path| {
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| {
                                    name.starts_with(arch) && name.ends_with("-library")
                                })
                        })
                        .map(|path| path.join(version)),
                );
                if cfg!(target_os = "macos") {
                    let arch = if arch == "aarch64" { "arm64" } else { arch };
                    libraries.push(
                        home.join("Library/R")
                            .join(arch)
                            .join(version)
                            .join("library"),
                    );
                }
            }
        }
    }
    libraries.push(standard);
    Ok(libraries
        .into_iter()
        .filter(|p| p.is_dir() && own.is_none_or(|folder| crate::own_env::inside(folder, p)))
        .map(file_library)
        .collect())
}

fn file_library(path: PathBuf) -> RLibrary {
    RLibrary {
        path: path.to_string_lossy().into_owned(),
        writable: writable(&path),
        packages: 0,
    }
}

const PYTHON_INFO: &str = r#"import json, site, sys, sysconfig
print(json.dumps(dict(prefix=sys.prefix, version='python%d.%d' % sys.version_info[:2],
    stdlib_path=sysconfig.get_path('stdlib'),
    stdlib=list(getattr(sys, 'stdlib_module_names', sys.builtin_module_names)),
    sites=site.getsitepackages(), user_site=site.getusersitepackages())))"#;

#[derive(Deserialize)]
pub(super) struct PythonInfo {
    pub prefix: PathBuf,
    pub version: String,
    pub stdlib_path: PathBuf,
    pub stdlib: HashSet<String>,
    pub sites: Vec<PathBuf>,
    pub user_site: PathBuf,
}

#[derive(Hash, PartialEq, Eq)]
struct PythonKey {
    program: PathBuf,
    modified: Option<std::time::SystemTime>,
}

pub(super) fn python_info(program: &Path, restricted: bool) -> Option<Arc<PythonInfo>> {
    type Probe = Arc<OnceLock<Option<Arc<PythonInfo>>>>;
    static CACHE: LazyLock<Mutex<HashMap<PythonKey, Probe>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = PythonKey {
        program: program.to_path_buf(),
        modified: program.metadata().and_then(|m| m.modified()).ok(),
    };
    let probe = {
        let mut cache = CACHE.lock().unwrap();
        if restricted {
            // Reading an already-known answer is safe in Restricted Mode.
            return cache.get(&key)?.get()?.clone();
        }
        cache.entry(key).or_default().clone()
    };
    probe
        .get_or_init(|| {
            // -I removes cwd and environment from imports; -S prevents .pth files
            // and sitecustomize from running.
            let mut command = Command::new(program);
            command.current_dir("/").args(["-B", "-I", "-S"]);
            let output = super::install::probe_command_with(
                command,
                &["-c", PYTHON_INFO],
                Duration::from_secs(8),
                false,
            )?;
            Some(Arc::new(serde_json::from_str(&output).ok()?))
        })
        .clone()
}

pub(crate) fn python_files(
    program: &Path,
    spec: &JupyterKernelspec,
    names: Vec<String>,
    document: Option<&Path>,
    project: Option<&Path>,
    own: Option<&Path>,
    restricted: bool,
) -> Result<Checked> {
    let program_prefix = program
        .parent()
        .and_then(Path::parent)
        .context("Python's folder wasn't found.")?;
    let venv = std::fs::read_to_string(program_prefix.join("pyvenv.cfg")).ok();
    let inherit_base = venv.as_deref().is_some_and(|cfg| {
        config(cfg, "include-system-site-packages")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    });
    let info = python_info(program, restricted);
    let prefix = if venv.is_some() || program_prefix.join("conda-meta").is_dir() {
        program_prefix
    } else {
        info.as_ref()
            .map_or(program_prefix, |info| info.prefix.as_path())
    };
    let disk_python = || {
        let actual = program.canonicalize().ok();
        let base = venv
            .as_deref()
            .and_then(|cfg| config(cfg, "home"))
            .and_then(|p| Path::new(p).parent());
        [
            base,
            Some(prefix),
            actual
                .as_deref()
                .and_then(Path::parent)
                .and_then(Path::parent),
        ]
        .into_iter()
        .flatten()
        .find_map(|p| subdirs(&p.join("lib")).find(|p| p.join("os.py").is_file()))
    };
    let stdlib = info
        .as_ref()
        .map(|info| info.stdlib_path.clone())
        .or_else(disk_python)
        .context("Python's packages couldn't be checked in this folder.")?;
    let version = info
        .as_ref()
        .map(|info| info.version.as_str())
        .or_else(|| stdlib.file_name()?.to_str())
        .context("Python's version couldn't be found.")?;
    let mut sites = Vec::new();
    if venv.is_some() {
        sites.extend(crate::kernelspecs::base_site_dirs(prefix, version));
    }
    if venv.is_none() || inherit_base {
        if let Some(info) = &info {
            sites.extend(info.sites.iter().cloned());
            let no_user_site = spec
                .env
                .as_ref()
                .is_some_and(|vars| vars.contains_key("PYTHONNOUSERSITE"))
                || spec.argv.iter().any(|arg| arg == "-s" || arg == "-I");
            if !no_user_site && own.is_none() {
                sites.push(info.user_site.clone());
            }
        } else {
            let base = stdlib.parent().and_then(Path::parent).unwrap_or(prefix);
            sites.extend(crate::kernelspecs::base_site_dirs(base, version));
        }
    }
    if let Some(own) = own {
        sites.retain(|site| crate::own_env::inside(own, site));
    }
    let extensions = stdlib.join("lib-dynload");
    let bare_paths = [document, project]
        .into_iter()
        .flatten()
        .chain(sites.iter().map(PathBuf::as_path));
    let mut absent: Vec<_> = names
        .into_iter()
        .filter(|name| {
            !info.as_ref().is_some_and(|info| info.stdlib.contains(name))
                && !PYTHON_BUILTINS.contains(&name.as_str())
                && !python_module_at(&stdlib, name)
                && !python_module_at(&extensions, name)
                && !bare_paths
                    .clone()
                    .any(|folder| python_module_at(folder, name))
        })
        .collect();
    if !absent.is_empty() {
        let installed = distribution_imports(&sites, &absent);
        let mut index = 0;
        absent.retain(|_| {
            let keep = !installed.contains(&index);
            index += 1;
            keep
        });
    }
    Ok(Checked {
        target: InstallTarget::Python {
            prefix: prefix.to_string_lossy().into_owned(),
            python: program.to_string_lossy().into_owned(),
        },
        absent,
    })
}

const PYTHON_BUILTINS: &[&str] = &[
    "_ast",
    "_io",
    "_thread",
    "atexit",
    "builtins",
    "errno",
    "faulthandler",
    "gc",
    "itertools",
    "marshal",
    "posix",
    "pwd",
    "sys",
    "time",
    "xxsubtype",
];

fn config<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(name, value)| (name.trim() == key).then_some(value.trim()))
}

pub(super) fn subdirs(path: &Path) -> impl Iterator<Item = PathBuf> + use<> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.is_dir())
}

fn python_module_at(folder: &Path, name: &str) -> bool {
    // A namespace package need not have __init__.py.
    folder.join(name).is_dir()
        || folder.join(format!("{name}.py")).is_file()
        || folder.join(format!("{name}.so")).is_file()
}

fn distribution_imports(sites: &[PathBuf], names: &[String]) -> HashSet<usize> {
    let mut wanted: HashMap<_, _> = names.iter().map(String::as_str).zip(0..).collect();
    let mut found = HashSet::new();
    for dist in sites.iter().flat_map(|site| subdirs(site)).filter(|p| {
        p.extension()
            .is_some_and(|ext| ext == "dist-info" || ext == "egg-info")
    }) {
        let mut see = |name: &str| {
            if let Some(name) = wanted.remove(name) {
                found.insert(name);
            }
            wanted.is_empty()
        };
        if let Ok(top) = std::fs::read_to_string(dist.join("top_level.txt"))
            && top.lines().any(|line| see(line.trim()))
        {
            break;
        }
        if let Ok(record) = std::fs::read_to_string(dist.join("RECORD"))
            && record
                .lines()
                .any(|line| record_module(line).is_some_and(&mut see))
        {
            break;
        }
    }
    found
}

pub(super) fn record_module(line: &str) -> Option<&str> {
    let path = if let Some(quoted) = line.strip_prefix('"') {
        quoted.split_once("\",")?.0
    } else {
        line.split_once(',')?.0
    };
    if let Some((top, _)) = path.split_once('/') {
        Some(top)
    } else if path.ends_with(".py") || path.ends_with(".so") {
        Some(path.split_once('.')?.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_metadata_modules_without_importing_them() {
        let tmp = tempfile::tempdir().unwrap();
        let dist = tmp.path().join("example-1.dist-info");
        std::fs::create_dir(&dist).unwrap();
        std::fs::write(dist.join("top_level.txt"), "renamed\n").unwrap();
        std::fs::write(
            dist.join("RECORD"),
            "other/sub.py,,\next.cpython-312-darwin.so,,\n../../../bin/script,,\n",
        )
        .unwrap();
        let names = ["renamed", "other", "ext", "script", "absent"].map(str::to_owned);
        let found = distribution_imports(&[tmp.path().to_path_buf()], &names);
        assert_eq!(found, HashSet::from([0, 1, 2]));
    }

    #[test]
    fn quiet_r_probe_prefers_a_writable_remembered_library() {
        let r = "/R/bin/R";
        let library = "/second";
        let prefs = InstallPrefs {
            last: vec![sandhill_proto::InstallChoice {
                language: "r".into(),
                target: InstallTarget::R {
                    r: r.into(),
                    library: library.into(),
                },
                tool: sandhill_proto::InstallTool::R,
            }],
            ..Default::default()
        };
        let output = "SANDHILL_R\t/R/bin/R\nSANDHILL_LIBPATH\t/first\tyes\t1\nSANDHILL_LIBPATH\t/second\tyes\t2\nSANDHILL_MISSING\tbase\nSANDHILL_MISSING\tmissing\n";
        let checked = session_result("r", output, &prefs, None).unwrap();
        assert_eq!(checked.target, prefs.last[0].target);
        assert_eq!(checked.absent, ["missing"]);
    }
}
