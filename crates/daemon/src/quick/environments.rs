//! Environment discovery and the cards on the R and Python page.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sandhill_proto::{EnvCard, EnvMode, InstallTarget, InstallTool, RSource};

use super::{install, package_plan, record, setup, undo, upgrades};
use crate::{irkernel, kernelspecs, own_env};
use kernelspecs::Search;

#[derive(Clone)]
pub(super) struct Found {
    pub program: PathBuf,
    pub prefix: Option<PathBuf>,
    kernel: Option<String>,
    name: String,
    pub version: String,
    own: Option<PathBuf>,
    pub r: bool,
    explore: bool,
}

pub(super) fn id(target: &InstallTarget) -> String {
    match target {
        InstallTarget::R { r, .. } => format!("r:{}", r_identity(Path::new(r)).display()),
        InstallTarget::Python { prefix, .. } => format!(
            "python:{}",
            kernelspecs::canonical(Path::new(prefix)).display()
        ),
    }
}

fn discover(search: &Search, record: &Path, include_existing: bool) -> Vec<Found> {
    let prefs = install::prefs(record);
    let own = PathBuf::from(&prefs.own_folder);
    let only_own = !include_existing && (prefs.mode == Some(EnvMode::Own) || search.own.is_some());
    let own = search.own.clone().unwrap_or(own);
    let own = own_env::marked(&own).then_some(own);
    let mut rs = Vec::new();
    let mut pythons = Vec::new();
    let mut seen_r = BTreeSet::new();
    let mut seen_py = BTreeSet::new();

    for scope in own.iter().map(Some).chain((!only_own).then_some(None)) {
        let scoped = search.clone().in_own(scope.cloned());
        let specs: HashMap<_, _> = kernelspecs::discover(&scoped)
            .into_iter()
            .map(|k| (k.dir.kernel_name.clone(), k.dir.kernelspec))
            .collect();
        let mut list = kernelspecs::list(&scoped);
        // Match Explore a Table's preference: IRkernel, then the newest R.
        list.sort_by(|a, b| {
            let ir = |k: &sandhill_proto::KernelspecInfo| k.engine.as_deref() == Some("IRkernel");
            ir(b)
                .cmp(&ir(a))
                .then_with(|| setup::version_key(&b.version).cmp(&setup::version_key(&a.version)))
        });
        let own_r = scope.map(|root| own_env::prefix(root, "r").join("bin/R"));
        let own_py = scope.map(|root| own_env::prefix(root, "python"));
        for k in list {
            let Some(spec) = specs.get(&k.name) else {
                continue;
            };
            if k.language.eq_ignore_ascii_case("r") {
                let Some(program) = irkernel::spec_r(spec, &scoped) else {
                    continue;
                };
                if own_r.as_ref().is_some_and(|r| !same_r(r, &program)) {
                    continue;
                }
                let program = own_r.clone().unwrap_or(program);
                if !seen_r.insert(r_identity(&program)) {
                    continue;
                }
                rs.push(Found {
                    program,
                    prefix: None,
                    kernel: Some(k.name),
                    name: k.display_name,
                    version: k.version.unwrap_or_default(),
                    own: scope.cloned(),
                    r: true,
                    explore: false,
                });
            } else if k.language.to_ascii_lowercase().starts_with("python") {
                let Some(program) = kernelspecs::program(spec, scoped.path.as_deref()) else {
                    continue;
                };
                let Some(python) =
                    super::python::found(&program, k.version, scope.map(PathBuf::as_path))
                else {
                    continue;
                };
                let prefix = kernelspecs::canonical(Path::new(&python.prefix));
                if own_py
                    .as_ref()
                    .is_some_and(|py| !package_plan::same_folder(py, &prefix))
                    || !seen_py.insert(prefix.clone())
                {
                    continue;
                }
                pythons.push(Found {
                    program,
                    prefix: Some(prefix),
                    kernel: Some(k.name),
                    name: k.display_name,
                    version: python.version,
                    own: scope.cloned(),
                    r: false,
                    explore: false,
                });
            }
        }
        if let Some(root) = scope {
            let program = own_r.unwrap();
            if install::executable(&program) && seen_r.insert(r_identity(&program)) {
                rs.push(Found {
                    program,
                    prefix: None,
                    kernel: None,
                    name: String::new(),
                    version: String::new(),
                    own: Some(root.clone()),
                    r: true,
                    explore: false,
                });
            }
            let prefix = own_py.unwrap();
            let program = prefix.join("bin/python");
            if install::executable(&program) && seen_py.insert(kernelspecs::canonical(&prefix)) {
                pythons.push(Found {
                    program,
                    prefix: Some(prefix),
                    kernel: None,
                    name: String::new(),
                    version: String::new(),
                    own: Some(root.clone()),
                    r: false,
                    explore: false,
                });
            }
        } else {
            let mut candidates = irkernel::find(&scoped);
            candidates
                .sort_by_key(|c| std::cmp::Reverse(setup::version_key(&Some(c.version.clone()))));
            for c in candidates {
                if !seen_r.insert(r_identity(&c.r)) {
                    continue;
                }
                rs.push(Found {
                    program: c.r,
                    prefix: None,
                    kernel: None,
                    name: String::new(),
                    version: c.version,
                    own: None,
                    r: true,
                    explore: false,
                });
            }
        }
    }
    if let Some(explore) = rs
        .iter_mut()
        .find(|r| only_own == r.own.is_some() && r.kernel.is_some())
    {
        explore.explore = true;
    } else if let Some(explore) = rs.iter_mut().find(|r| {
        only_own == r.own.is_some() && (only_own || setup::source(&r.program) != RSource::Conda)
    }) {
        explore.explore = true;
    }
    let mut found = rs;
    found.extend(pythons);
    found.sort_by_key(|f| (f.own.is_none(), !f.explore));
    found
}

fn same_r(a: &Path, b: &Path) -> bool {
    r_identity(a) == r_identity(b)
}

fn r_identity(program: &Path) -> PathBuf {
    match conda_prefix(program) {
        Some(prefix) if prefix.join("bin/R").is_file() => {
            kernelspecs::canonical(&prefix.join("bin/R"))
        }
        _ => kernelspecs::canonical(program),
    }
}

pub(super) fn conda_prefix(program: &Path) -> Option<PathBuf> {
    let real = kernelspecs::canonical(program);
    program
        .ancestors()
        .chain(real.ancestors())
        .find(|prefix| prefix.join("conda-meta").is_dir())
        .map(kernelspecs::canonical)
}

pub(super) fn r_probe(program: &Path, code: &str) -> Option<String> {
    install::probe_in(
        program,
        &["--no-save", "--no-restore", "-s", "-e", code],
        own_env::containing(program).as_deref(),
    )
}

#[derive(Debug)]
pub(super) struct RPackage {
    pub name: String,
    pub version: String,
    pub library: String,
    pub description: String,
}

pub(super) fn r_packages(libraries: &[sandhill_proto::RLibrary]) -> Vec<RPackage> {
    libraries
        .iter()
        .flat_map(|library| {
            fs::read_dir(&library.path)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(move |entry| {
                    let name = entry.file_name().into_string().ok()?;
                    if !record::r_name(&name) {
                        return None;
                    }
                    let description = fs::read_to_string(entry.path().join("DESCRIPTION")).ok()?;
                    if record::r_field(&description, "Package")? != name {
                        return None;
                    }
                    Some(RPackage {
                        name,
                        version: record::r_field(&description, "Version")?.into(),
                        library: library.path.clone(),
                        description,
                    })
                })
        })
        .collect()
}

fn r_minor(version: &str) -> Option<(&str, &str)> {
    let mut parts = version.split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    (major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok()).then_some((major, minor))
}

pub(super) fn wrong_r_build<'a>(description: &'a str, version: &str) -> Option<&'a str> {
    let value = record::r_field(description, "Built")?
        .strip_prefix("R ")?
        .split(';')
        .next()?
        .trim();
    (r_minor(value)? != r_minor(version)?).then_some(value)
}
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Environment {
    pub card: EnvCard,
    pub version: String,
    pub own: Option<PathBuf>,
    pub conda: Option<PathBuf>,
    #[serde(skip)]
    pub packages: record::Versions,
}

impl Environment {
    pub fn is_r(&self) -> bool {
        self.card.language == "r"
    }
    pub fn tool(&self, tool: InstallTool) -> Option<&sandhill_proto::InstallToolInfo> {
        self.card.tools.iter().find(|info| info.tool == tool)
    }
    pub fn target(&self, library: Option<&str>) -> Result<InstallTarget> {
        let card = &self.card;
        Ok(if self.is_r() {
            let library = library
                .or_else(|| {
                    card.libraries
                        .iter()
                        .find(|lib| lib.writable)
                        .map(|l| l.path.as_str())
                })
                .context("R has no library you can write to.")?;
            InstallTarget::R {
                r: card.program.clone(),
                library: library.into(),
            }
        } else {
            InstallTarget::Python {
                prefix: card.folder.clone(),
                python: card.program.clone(),
            }
        })
    }
}

fn probe(
    found: Found,
    search: &Search,
    path: &Path,
    installed: &[sandhill_proto::QuickInstalled],
    card_details: bool,
) -> Option<Environment> {
    use InstallTool::{Conda, Pip, R, Uv};
    let Found {
        program,
        prefix,
        kernel,
        name,
        version,
        own,
        r,
        explore,
    } = found;
    let conda = if r {
        conda_prefix(&program)
    } else {
        prefix
            .as_ref()
            .filter(|p| p.join("conda-meta").is_dir())
            .cloned()
    };
    let version_info = if r {
        let code = format!(
            "cat('SANDHILL_HOME\\t', R.home(), '\\n', sep=''); cat('SANDHILL_VERSION\\t', as.character(getRversion()), '\\n', sep=''); {}",
            irkernel::LIBRARIES_CODE
        );
        r_probe(&program, &code)
    } else {
        None
    };
    let libraries = version_info
        .as_deref()
        .map(irkernel::parse_libraries)
        .unwrap_or_default();
    let folder = prefix.unwrap_or_else(|| {
        version_info
            .as_deref()
            .and_then(|text| text.lines().find_map(|l| l.strip_prefix("SANDHILL_HOME\t")))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                program
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or(&program)
                    .to_path_buf()
            })
    });
    let version = version_info
        .as_deref()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("SANDHILL_VERSION\t"))
        })
        .map(str::to_owned)
        .or_else(|| (!version.is_empty()).then_some(version))
        .or_else(|| {
            (!r).then(|| {
                install::probe(
                    &program,
                    &[
                        "-I",
                        "-c",
                        "import platform; print(platform.python_version())",
                    ],
                )
            })
            .flatten()
            .map(|v| v.trim().into())
        })
        .unwrap_or_default();
    let library_packages = if r {
        r_packages(&libraries)
    } else {
        Vec::new()
    };
    let mut packages = record::Versions::new();
    if r {
        for pkg in &library_packages {
            packages
                .entry(pkg.name.clone())
                .or_insert_with(|| pkg.version.clone());
        }
    } else {
        packages = package_plan::python_inventory(&program, &folder).unwrap_or_default();
    }
    let mut repairs: Vec<_> = library_packages
        .iter()
        .filter_map(|p| Some((p.name.clone(), wrong_r_build(&p.description, &version)?)))
        .collect();
    repairs.sort_unstable();
    let foreign_versions: BTreeSet<_> = repairs
        .iter()
        .map(|(_, v)| r_minor(v).map_or_else(|| v.to_string(), |(x, y)| format!("{x}.{y}")))
        .collect();
    let mut needs_repair: Vec<_> = repairs.into_iter().map(|(name, _)| name).collect();
    needs_repair.dedup();
    let mut kit_ids = Vec::new();
    let mut kit_labels = Vec::new();
    for kit in package_plan::kits() {
        let complete = if r {
            !kit.r.is_empty() && kit.r.iter().all(|(p, _)| packages.contains_key(*p))
        } else {
            !kit.python.is_empty()
                && kit
                    .python
                    .iter()
                    .all(|(_, p)| packages.contains_key(&record::python_name(p)))
        };
        let extra = kit.conda_only.is_empty()
            || (r
                && kit.conda_only.iter().all(|name| {
                    conda
                        .as_ref()
                        .is_some_and(|prefix| install::executable(&prefix.join("bin").join(name)))
                        || install::on_path(search, name).is_some()
                }))
            || !r;
        if complete && extra {
            kit_ids.push(kit.id.into());
            if kit.id != sandhill_proto::CORE_KIT.id {
                kit_labels.push(kit.label);
            }
        }
    }
    let missing = setup::PACKAGES
        .iter()
        .filter(|&&(name, _, tool)| {
            tool == sandhill_proto::QuickTool::Plot && !packages.contains_key(name)
        })
        .count();
    let status = status(
        r,
        explore,
        kernel.is_some(),
        &version,
        packages.len(),
        missing,
        &kit_labels,
        &needs_repair,
        foreign_versions,
    );
    let kind = if own.is_some() {
        "own"
    } else if conda.is_some() {
        "conda"
    } else if r {
        match setup::source(&program) {
            RSource::Homebrew => "homebrew",
            RSource::Cran => "cran",
            _ if program.starts_with("/usr/bin") => "system",
            _ => "other",
        }
    } else {
        match super::python::describe_python(&program, &folder).0.as_str() {
            "Homebrew" => "homebrew",
            "other" if program.starts_with("/usr/bin") => "system",
            "other" => "other",
            "uv" => "uv",
            "venv" => "venv",
            "python.org" => "python.org",
            _ => "other",
        }
    };
    let by_sandhill = kind == "own"
        || installed.iter().any(|item| {
            item.ours
                && matches!(
                    item.kind.as_str(),
                    record::CONDA_ENV
                        | record::PYTHON_ENV
                        | record::R_CRAN
                        | record::UV_PYTHON
                        | record::BREW
                )
                && (Path::new(&item.place) == folder
                    || conda.as_ref().is_some_and(|p| Path::new(&item.place) == p)
                    || program.starts_with(&item.place))
        });
    let mut tools = Vec::new();
    if let Some(conda) = conda
        .as_deref()
        .and_then(|prefix| install::conda(search, prefix))
    {
        tools.push(install::info(Conda, &conda));
    }
    if r && program.with_file_name("Rscript").is_file() {
        tools.push(sandhill_proto::InstallToolInfo {
            tool: R,
            path: program.to_string_lossy().into_owned(),
            version: version.clone(),
        });
    } else if !r {
        let venv = folder.join("pyvenv.cfg").is_file();
        if venv && let Some(uv) = install::uv(search) {
            tools.push(install::info(Uv, &uv));
        }
        if venv
            || conda.is_some()
            || package_plan::inspect_python(&program).is_some_and(|p| !p.managed)
        {
            let pip = install::info(Pip, &program);
            if !pip.version.is_empty() {
                tools.push(pip);
            }
        }
    }
    let language = if r { "r" } else { "python" };
    install::prefer(&mut tools, &install::prefs(path), language);
    let bioconductor = bioconductor(&library_packages);
    let program = program.to_string_lossy().into_owned();
    let folder = folder.to_string_lossy().into_owned();
    let id = id(&if r {
        InstallTarget::R {
            r: program.clone(),
            library: String::new(),
        }
    } else {
        InstallTarget::Python {
            python: program.clone(),
            prefix: folder.clone(),
        }
    });
    let name = if name.is_empty() {
        let env = conda.as_deref().map(kernelspecs::conda_name);
        sandhill_proto::kernel_label(
            language,
            (!version.is_empty()).then_some(version.as_str()),
            env.as_deref(),
            None,
        )
    } else {
        name
    };
    let (size_mb, undo, check) = if card_details {
        let paths = if r {
            libraries
                .iter()
                .map(|lib| PathBuf::from(&lib.path))
                .collect()
        } else {
            vec![PathBuf::from(&folder)]
        };
        (size_mb(paths), undo::info(path, &id), read_check(path, &id))
    } else {
        (0, None, None)
    };
    Some(Environment {
        version,
        own,
        conda,
        card: EnvCard {
            id,
            language: language.into(),
            name,
            kind: kind.into(),
            program,
            folder,
            libraries,
            kernel,
            status,
            kits: kit_ids,
            needs_repair,
            size_mb,
            packages: packages.len().try_into().unwrap_or(u32::MAX),
            by_sandhill,
            tools,
            undo,
            check,
            upgraded_from: None,
            bioconductor,
        },
        packages,
    })
}

#[allow(clippy::too_many_arguments)]
fn status(
    r: bool,
    explore: bool,
    kernel: bool,
    version: &str,
    count: usize,
    missing: usize,
    kits: &[&str],
    repairs: &[String],
    built: BTreeSet<String>,
) -> String {
    if !repairs.is_empty() {
        let versions = built.into_iter().collect::<Vec<_>>().join(", ");
        return if repairs.len() == 1 {
            format!("Needs repair: 1 package was built for R {versions}")
        } else {
            format!(
                "Needs repair: {} packages were built for R {versions}",
                repairs.len()
            )
        };
    }
    if explore && missing > 0 {
        let plural = if missing == 1 { "package" } else { "packages" };
        return format!("{missing} {plural} missing for Explore a Table");
    }
    if explore && !kernel {
        return "Needs an R kernel for Explore a Table".into();
    }
    let mut ready = Vec::new();
    if explore {
        ready.push("Explore a Table");
    }
    ready.extend(kits.iter().copied());
    if let Some((last, rest)) = ready.split_last() {
        if rest.is_empty() {
            return format!("Ready for {last}");
        }
        return format!("Ready for {} and {last}", rest.join(", "));
    }
    let lang = if r { "R" } else { "Python" };
    let plural = if count == 1 { "package" } else { "packages" };
    if version.is_empty() {
        format!("{lang} with {count} {plural}")
    } else {
        format!("{lang} {version} with {count} {plural}")
    }
}

type SizeCache = HashMap<Vec<PathBuf>, (Instant, u64)>;
fn size_mb(paths: Vec<PathBuf>) -> u64 {
    static CACHE: LazyLock<Mutex<SizeCache>> = LazyLock::new(Mutex::default);
    let age = Duration::from_secs(60);
    if let Some((_, size)) = CACHE
        .lock()
        .unwrap()
        .get(&paths)
        .filter(|(when, _)| when.elapsed() < age)
    {
        return *size;
    }
    fn size(path: &Path) -> u64 {
        let Ok(meta) = path.symlink_metadata() else {
            return 0;
        };
        if meta.is_dir() {
            fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| size(&entry.path()))
                .sum()
        } else if meta.is_file() {
            meta.len()
        } else {
            0
        }
    }
    let megabytes = paths
        .iter()
        .map(|path| size(&kernelspecs::canonical(path)))
        .sum::<u64>()
        .div_ceil(1_000_000);
    let mut cache = CACHE.lock().unwrap();
    cache.retain(|_, (when, _)| when.elapsed() < age);
    cache.insert(paths, (Instant::now(), megabytes));
    megabytes
}

pub(crate) fn list(search: &Search, path: &Path) -> Vec<EnvCard> {
    undo::prune(path);
    let installed = record::read(path);
    let found = discover(search, path, false);
    let envs: Vec<Environment> = std::thread::scope(|scope| {
        let installed = &installed;
        found
            .into_iter()
            .map(|f| scope.spawn(move || probe(f, search, path, installed, true)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|job| job.join().ok().flatten())
            .collect()
    });
    let present: BTreeSet<_> = envs
        .iter()
        .filter(|env| env.is_r())
        .filter_map(|env| upgrades::minor(&env.version))
        .collect();
    let roots = upgrades::Roots::here();
    envs.into_iter()
        .map(|mut env| {
            if env.is_r() && !matches!(env.card.kind.as_str(), "own" | "conda") {
                let have = env.packages.keys().map(String::as_str).collect();
                env.card.upgraded_from = upgrades::detect(&roots, &env.version, &have, &present);
            }
            env.card
        })
        .collect()
}

/// The Bioconductor release an R uses, from the installed `BiocVersion`'s
/// version (`3.22.0` → `3.22`); its DESCRIPTION is only read.
fn bioconductor(packages: &[RPackage]) -> Option<String> {
    let version = &packages.iter().find(|p| p.name == "BiocVersion")?.version;
    let mut parts = version.split(['.', '-']);
    let (major, minor) = (parts.next()?, parts.next()?);
    (major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok())
        .then(|| format!("{major}.{minor}"))
}

pub(super) fn get(search: &Search, path: &Path, wanted: &str) -> Result<Environment> {
    let search = search.clone().in_own(own_env::chosen(path));
    let found = discover(&search, path, false)
        .into_iter()
        .find(|f| {
            let (language, path) = if f.r {
                ("r", &f.program)
            } else {
                ("python", f.prefix.as_ref().unwrap())
            };
            wanted.split_once(':').is_some_and(|(lang, requested)| {
                lang == language
                    && if f.r {
                        same_r(Path::new(requested), path)
                    } else {
                        package_plan::same_folder(Path::new(requested), path)
                    }
            })
        })
        .context("This environment couldn't be found. Refresh the R and Python page.")?;
    probe(found, &search, path, &[], false).context("This environment couldn't be read.")
}

/// The variables that put a Terminal in the environment `wanted` (Open
/// Terminal Here): `(name, value)`, with `None` for one to unset. Paths are
/// this computer's, so a server's environments get the server's.
pub(crate) fn activation(
    search: &Search,
    path: &Path,
    wanted: &str,
) -> Result<Vec<(String, Option<String>)>> {
    let env = get(search, path, wanted)?;
    Ok(activation_for(
        &env,
        crate::paths::program_path().as_deref(),
    ))
}

fn activation_for(
    env: &Environment,
    rest: Option<&std::ffi::OsStr>,
) -> Vec<(String, Option<String>)> {
    let text = |value: &std::ffi::OsStr| value.to_string_lossy().into_owned();
    if let Some(folder) = &env.own {
        return own_env::UNSET
            .iter()
            .map(|key| (key.to_string(), None))
            .chain(
                own_env::vars(folder, rest)
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), Some(text(&value)))),
            )
            .collect();
    }
    let path_with = |bin: &Path| {
        let mut path = bin.as_os_str().to_os_string();
        if let Some(rest) = rest.filter(|r| !r.is_empty()) {
            path.push(":");
            path.push(rest);
        }
        ("PATH".to_string(), Some(text(&path)))
    };
    let unset = |key: &str| (key.to_string(), None);
    let folder = Path::new(&env.card.folder);
    if let Some(prefix) = &env.conda {
        vec![
            ("CONDA_PREFIX".into(), Some(text(prefix.as_os_str()))),
            (
                "CONDA_DEFAULT_ENV".into(),
                Some(kernelspecs::conda_name(prefix)),
            ),
            unset("VIRTUAL_ENV"),
            unset("PYTHONHOME"),
            path_with(&prefix.join("bin")),
        ]
    } else if !env.is_r() && folder.join("pyvenv.cfg").is_file() {
        vec![
            ("VIRTUAL_ENV".into(), Some(env.card.folder.clone())),
            unset("CONDA_PREFIX"),
            unset("PYTHONHOME"),
            path_with(&folder.join("bin")),
        ]
    } else {
        let program = Path::new(&env.card.program);
        vec![path_with(program.parent().unwrap_or(program))]
    }
}

pub(super) fn copy_sources(search: &Search, record: &Path, ids: &[String]) -> Result<Vec<Found>> {
    let found = discover(search, record, true);
    ids.iter()
        .map(|id| {
            found
                .iter()
                .find(|f| {
                    f.own.is_none()
                        && id.split_once(':').is_some_and(|(lang, path)| {
                            if f.r {
                                lang == "r" && same_r(Path::new(path), &f.program)
                            } else {
                                lang == "python"
                                    && f.prefix.as_ref().is_some_and(|prefix| {
                                        package_plan::same_folder(Path::new(path), prefix)
                                    })
                            }
                        })
                })
                .cloned()
                .context("A copy source couldn't be found. Refresh the R and Python page.")
        })
        .collect()
}

pub(super) fn state_path(record: &Path, area: &str, id: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    record
        .with_file_name(area)
        .join(sandhill_proto::hex(&Sha256::digest(id.as_bytes())))
}

fn read_check(path: &Path, id: &str) -> Option<sandhill_proto::EnvCheck> {
    serde_json::from_slice(&fs::read(state_path(path, "checks", id)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_prioritizes_repairs_and_explores_needs() {
        assert_eq!(wrong_r_build("Built: R 3.6.0; ;", "4.5.2"), Some("3.6.0"));
        assert_eq!(wrong_r_build("Built: R 4.5.0; ;", "4.5.2"), None);
        assert_eq!(
            status(true, true, false, "4.5.2", 33, 2, &[], &[], BTreeSet::new()),
            "2 packages missing for Explore a Table"
        );
        assert_eq!(
            status(
                true,
                true,
                true,
                "4.5.2",
                33,
                0,
                &["bulk RNA-seq"],
                &[],
                BTreeSet::new()
            ),
            "Ready for Explore a Table and bulk RNA-seq"
        );
        assert_eq!(
            status(
                false,
                false,
                true,
                "3.12",
                214,
                0,
                &[],
                &[],
                BTreeSet::new()
            ),
            "Python 3.12 with 214 packages"
        );
    }

    #[test]
    fn the_bioconductor_release_comes_from_biocversion_in_the_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let library = |name: &str, packages: &[(&str, &str)]| {
            let path = dir.path().join(name);
            for (package, version) in packages {
                fs::create_dir_all(path.join(package)).unwrap();
                fs::write(
                    path.join(package).join("DESCRIPTION"),
                    format!("Package: {package}\nVersion: {version}\n"),
                )
                .unwrap();
            }
            sandhill_proto::RLibrary {
                path: path.to_string_lossy().into_owned(),
                writable: true,
                packages: packages.len() as u32,
            }
        };
        let user = library("user", &[("DESeq2", "1.50.2")]);
        let site = library(
            "site",
            &[("BiocVersion", "3.22.0"), ("BiocManager", "1.30.27")],
        );
        let packages = r_packages(&[user.clone(), site]);
        assert_eq!(bioconductor(&packages).as_deref(), Some("3.22"));
        assert_eq!(bioconductor(&r_packages(&[user])), None);
    }

    #[test]
    fn activation_sets_the_variables_of_each_kind_of_environment() {
        let dir = tempfile::tempdir().unwrap();
        let env = |language: &str, program: &Path, folder: &Path| Environment {
            card: serde_json::from_value(serde_json::json!({
                "id": "x", "language": language, "name": "x", "kind": "other",
                "program": program, "folder": folder, "status": "",
            }))
            .unwrap(),
            version: String::new(),
            own: None,
            conda: None,
            packages: Default::default(),
        };
        let rest = std::ffi::OsStr::new("/usr/bin:/bin");
        let value = |vars: &[(String, Option<String>)], key: &str| {
            vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        let conda = dir.path().join("envs/scanpy");
        let mut python = env("python", &conda.join("bin/python"), &conda);
        python.conda = Some(conda.clone());
        let vars = activation_for(&python, Some(rest));
        assert_eq!(
            value(&vars, "CONDA_PREFIX"),
            Some(Some(conda.display().to_string()))
        );
        assert_eq!(
            value(&vars, "PATH"),
            Some(Some(format!(
                "{}:/usr/bin:/bin",
                conda.join("bin").display()
            )))
        );
        assert_eq!(value(&vars, "VIRTUAL_ENV"), Some(None));

        let venv = dir.path().join("project/.venv");
        fs::create_dir_all(&venv).unwrap();
        fs::write(venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        let vars = activation_for(&env("python", &venv.join("bin/python"), &venv), Some(rest));
        assert_eq!(
            value(&vars, "VIRTUAL_ENV"),
            Some(Some(venv.display().to_string()))
        );
        assert!(
            value(&vars, "PATH")
                .unwrap()
                .unwrap()
                .starts_with(&venv.join("bin").display().to_string())
        );

        let r = env(
            "r",
            Path::new("/opt/homebrew/bin/R"),
            Path::new("/opt/homebrew/lib/R"),
        );
        assert_eq!(
            activation_for(&r, Some(rest)),
            [(
                "PATH".to_string(),
                Some("/opt/homebrew/bin:/usr/bin:/bin".to_string())
            )]
        );

        let own = dir.path().join("own");
        let mut mine = env("r", &own.join("conda/envs/sandhill-r/bin/R"), &own);
        mine.own = Some(own.clone());
        let vars = activation_for(&mine, Some(rest));
        assert_eq!(value(&vars, "R_LIBS"), Some(None));
        assert_eq!(
            value(&vars, "CONDARC"),
            Some(Some(own.join("condarc").display().to_string()))
        );
        assert!(
            value(&vars, "PATH")
                .unwrap()
                .unwrap()
                .ends_with(":/usr/bin:/bin")
        );
    }
}
