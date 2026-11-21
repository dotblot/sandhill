//! What Sandhill installed, one tab-separated line per item:
//! kind, name, version, place, date, tool, reason. Older five-column lines
//! still count. The record is not permission to remove something changed.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sandhill_proto::{InstalledKey, QuickInstalled};
use serde_json::Value;

use super::install::{self, home, quote, run_command};
use crate::kernelspecs::Search;

pub const FILE: &str = "installed-by-sandhill.tsv";
pub const R_PACKAGE: &str = "r-package";
pub const PYTHON_PACKAGE: &str = "python-package";
pub const CONDA_PACKAGE: &str = "conda-package";
pub const KERNEL: &str = "kernel";
pub const AGENT: &str = "agent";
pub const PYTHON_ENV: &str = "python-env";
pub const CONDA_ENV: &str = "conda-env";
pub const IPYKERNEL: &str = "ipykernel";
pub const MINIFORGE: &str = "miniforge";
pub const BREW: &str = "brew";
pub const R_CRAN: &str = "r-cran";
pub const UV_PYTHON: &str = "uv-python";
pub const OWN_ENVIRONMENT: &str = "own-environment";
pub(crate) const RENV_LIBRARY: &str = "renv-library";
const KINDS: &[&str] = &[
    R_PACKAGE,
    PYTHON_PACKAGE,
    CONDA_PACKAGE,
    KERNEL,
    AGENT,
    PYTHON_ENV,
    CONDA_ENV,
    IPYKERNEL,
    MINIFORGE,
    BREW,
    R_CRAN,
    UV_PYTHON,
    OWN_ENVIRONMENT,
    RENV_LIBRARY,
];

pub(super) type Versions = BTreeMap<String, String>;

pub(super) fn lock(record: &Path, wait: bool) -> io::Result<File> {
    install::lock(&record.with_file_name(".install-record.lock"), wait)
}

pub(super) fn load(record: &Path) -> io::Result<Vec<QuickInstalled>> {
    match fs::read_to_string(record) {
        Ok(text) => Ok(parse(&text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

fn parse(text: &str) -> Vec<QuickInstalled> {
    let mut seen = HashSet::new();
    let mut items: Vec<_> = text
        .lines()
        .rev()
        .filter_map(|line| {
            let mut fields = line.splitn(7, '\t');
            let [kind, name, version, place, date] =
                std::array::from_fn(|_| fields.next().unwrap_or(""));
            if !KINDS.contains(&kind)
                || name.is_empty()
                || place.is_empty()
                || !seen.insert((kind, name, place))
            {
                return None;
            }
            Some(QuickInstalled {
                kind: kind.into(),
                name: name.into(),
                version: version.into(),
                place: place.into(),
                date: date.into(),
                tool: fields.next().unwrap_or_default().into(),
                reason: fields.next().unwrap_or_default().into(),
                ours: false,
            })
        })
        .collect();
    items.reverse();
    items
}

pub(super) fn save(record: &Path, items: &[QuickInstalled]) -> Result<()> {
    use std::fmt::Write as _;
    let mut tsv = String::new();
    for item in items {
        let [kind, name, version, place, date, tool, reason] = [
            &item.kind,
            &item.name,
            &item.version,
            &item.place,
            &item.date,
            &item.tool,
            &item.reason,
        ]
        .map(|s| s.replace(['\t', '\r', '\n'], " "));
        writeln!(
            tsv,
            "{kind}\t{name}\t{version}\t{place}\t{date}\t{tool}\t{reason}"
        )?;
    }
    crate::files::write_atomic(record, tsv.as_bytes())
}

pub(super) fn add(record: &Path, entries: impl IntoIterator<Item = QuickInstalled>) -> Result<()> {
    let mut entries = entries.into_iter().peekable();
    if entries.peek().is_none() {
        return Ok(());
    }
    let mut items = load(record)?;
    for item in entries {
        items.retain(|old| {
            !(old.kind == item.kind && old.name == item.name && old.place == item.place)
        });
        items.push(item);
    }
    save(record, &items)
}

pub(super) fn forget_own(record: &Path, folder: &Path) -> Result<()> {
    let mut items = load(record)?;
    items.retain(|item| !Path::new(&item.place).starts_with(folder));
    save(record, &items)
}

pub(super) fn entry(
    kind: &str,
    name: impl Into<String>,
    version: impl Into<String>,
    place: &str,
    tool: &str,
    reason: &str,
) -> QuickInstalled {
    QuickInstalled {
        kind: kind.into(),
        name: name.into(),
        version: version.into(),
        place: place.into(),
        date: today(),
        tool: tool.into(),
        reason: reason.into(),
        ours: true,
    }
}

/// Today's local date, `2026-11-20`.
pub(super) fn today() -> String {
    // SAFETY: `time` accepts a null pointer and `localtime_r` gets valid pointers.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday
    )
}

pub(super) fn key(item: &QuickInstalled) -> InstalledKey {
    InstalledKey {
        kind: item.kind.clone(),
        name: item.name.clone(),
        place: item.place.clone(),
    }
}

/// Read the record and prune entries that no longer exist. Don't make Getting
/// Ready wait for a long install that already holds the lock.
pub fn read(record: &Path) -> Vec<QuickInstalled> {
    let guard = lock(record, false).ok();
    let Ok(mut items) = load(record) else {
        return Vec::new();
    };
    let gone = check(&mut items);
    if !gone.is_empty() {
        items.retain(|item| !gone.contains(&key(item)));
        if guard.is_some()
            && let Err(error) = save(record, &items)
        {
            tracing::warn!(%error, "could not prune install record");
        }
    }
    items
}

pub(super) fn prune(record: &Path) -> Result<()> {
    let mut items = load(record)?;
    let gone = check(&mut items);
    if !gone.is_empty() {
        items.retain(|item| !gone.contains(&key(item)));
        save(record, &items)?;
    }
    Ok(())
}

pub(super) fn r_field<'a>(description: &'a str, field: &str) -> Option<&'a str> {
    description.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(field).then(|| value.trim())
    })
}

pub(super) fn r_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.')
}

pub(super) fn package_version(library: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(library.join(name).join("DESCRIPTION")).ok()?;
    (r_field(&text, "Package")? == name).then_some(())?;
    Some(r_field(&text, "Version")?.into())
}

pub(super) fn r_packages(library: &Path) -> Result<Versions> {
    Ok(fs::read_dir(library)?
        .filter_map(|dir| {
            let name = dir.ok()?.file_name().into_string().ok()?;
            if !r_name(&name) {
                return None;
            }
            let version = package_version(library, &name)?;
            Some((name, version))
        })
        .collect())
}

pub(super) fn conda_packages(prefix: &Path) -> Result<Versions> {
    Ok(fs::read_dir(prefix.join("conda-meta"))?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let (name_version, _) = name.strip_suffix(".json")?.rsplit_once('-')?;
            let (name, version) = name_version.rsplit_once('-')?;
            Some((name.into(), version.into()))
        })
        .collect())
}

pub(super) fn python_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;
    for c in name.chars() {
        if matches!(c, '_' | '-' | '.') {
            if !separator {
                normalized.push('-');
            }
            separator = true;
        } else {
            normalized.push(c.to_ascii_lowercase());
            separator = false;
        }
    }
    normalized
}

fn python_version(prefix: &Path, name: &str) -> Option<String> {
    let name = python_name(name);
    fs::read_dir(prefix.join("lib"))
        .ok()?
        .flatten()
        .filter(|dir| dir.file_name().to_string_lossy().starts_with("python"))
        .filter_map(|dir| fs::read_dir(dir.path().join("site-packages")).ok())
        .flatten()
        .flatten()
        .filter_map(|dist| {
            let folder = dist.file_name();
            let (stem, _) = folder
                .to_str()?
                .strip_suffix(".dist-info")?
                .rsplit_once('-')?;
            (python_name(stem) == name).then_some(())?;
            let content = fs::read_to_string(dist.path().join("METADATA")).ok()?;
            (python_name(r_field(&content, "Name")?) == name).then_some(())?;
            Some(r_field(&content, "Version")?.into())
        })
        .next()
}

pub(super) fn brew_packages(brew: &Path) -> Option<Versions> {
    let text = install::probe_brew(brew, &["list", "--formula", "--versions"])?;
    Some(
        text.lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let name = words.next()?;
                let versions = words.collect::<Vec<_>>().join(" ");
                (!versions.is_empty()).then(|| (name.into(), versions))
            })
            .collect(),
    )
}

fn agent_path(name: &str, method: &str) -> Option<PathBuf> {
    let path = match (name, method) {
        ("claude", "native") => home()?.join(".local/bin/claude"),
        ("codex", "npm") => crate::agents::find("codex")?,
        _ => return None,
    };
    install::executable(&path).then_some(path)
}

pub(super) fn agent_version(name: &str, method: &str) -> Option<String> {
    agent_path(name, method).map(|path| super::python::tool_version(&path))
}

/// The items of `kind` in `place` still as Sandhill left them, with why
/// they were installed: an update re-records these, not the user's own.
pub(super) fn owned_in(
    record: &Path,
    kind: &str,
    place: &Path,
    now: &Versions,
) -> Result<BTreeMap<String, String>> {
    let place = place.to_string_lossy();
    Ok(load(record)?
        .into_iter()
        .filter(|item| {
            item.kind == kind && item.place == place && now.get(&item.name) == Some(&item.version)
        })
        .map(|item| (item.name, item.reason))
        .collect())
}

#[derive(PartialEq, Eq)]
enum Status {
    Gone,
    Changed,
    Ours,
}

fn version_status(version: Option<&str>, recorded: &str) -> Status {
    match version {
        None => Status::Gone,
        Some(current) if current == recorded => Status::Ours,
        Some(_) => Status::Changed,
    }
}

pub(super) fn directory_identity(folder: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = folder.symlink_metadata().ok()?;
    if !meta.is_dir() || folder.canonicalize().ok()? != folder {
        return None;
    }
    let mut entries = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(dir) = folders.pop() {
        for ent in fs::read_dir(dir).ok()? {
            let ent = ent.ok()?;
            let path = ent.path();
            let stat = path.symlink_metadata().ok()?;
            entries.push((
                path.strip_prefix(folder)
                    .ok()?
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
                stat.ino(),
                stat.size(),
                stat.mtime(),
                stat.mtime_nsec(),
            ));
            if stat.is_dir() {
                folders.push(path);
            }
        }
    }
    entries.sort_unstable();
    let mut hash = sha2::Sha256::new();
    hash.update(meta.dev().to_le_bytes());
    hash.update(meta.ino().to_le_bytes());
    for (path, ino, size, time, nanos) in entries {
        hash.update(path.len().to_le_bytes());
        hash.update(path);
        hash.update(ino.to_le_bytes());
        hash.update(size.to_le_bytes());
        hash.update(time.to_le_bytes());
        hash.update(nanos.to_le_bytes());
    }
    Some(sandhill_proto::hex(&hash.finalize()))
}

fn check(items: &mut [QuickInstalled]) -> HashSet<InstalledKey> {
    let search = Search::from_env();
    let mut brews = None;
    let mut uv_pythons = None;
    let mut gone = HashSet::new();
    for item in &mut *items {
        let prefix = Path::new(&item.place);
        let version = item.version.as_str();
        let status = if item.kind != AGENT && !prefix.is_absolute() {
            Status::Changed
        } else {
            match item.kind.as_str() {
                R_PACKAGE if r_name(&item.name) => {
                    let dir = prefix.join(&item.name);
                    match package_version(prefix, &item.name).as_deref() {
                        None if dir.symlink_metadata().is_ok() => Status::Changed,
                        current => version_status(current, version),
                    }
                }
                PYTHON_PACKAGE => {
                    version_status(python_version(prefix, &item.name).as_deref(), version)
                }
                CONDA_PACKAGE | IPYKERNEL if prefix.join("conda-meta").is_dir() => {
                    match conda_packages(prefix) {
                        Ok(packages) => {
                            version_status(packages.get(&item.name).map(String::as_str), version)
                        }
                        Err(_) => Status::Changed,
                    }
                }
                KERNEL if prefix.exists() => {
                    if fs::read_to_string(prefix.join("kernel.json")).is_ok_and(|spec| {
                        if version.is_empty() {
                            spec.contains("IRkernel::main")
                        } else {
                            sandhill_proto::hex(&sha2::Sha256::digest(spec.as_bytes())) == version
                        }
                    }) {
                        Status::Ours
                    } else {
                        Status::Changed
                    }
                }
                // Agents update themselves; Sandhill installed this one wherever its version is now.
                AGENT => {
                    if agent_path(&item.name, &item.place).is_some() {
                        Status::Ours
                    } else {
                        Status::Gone
                    }
                }
                PYTHON_ENV if prefix.join("pyvenv.cfg").is_file() => {
                    if prefix.join("bin/python").is_file() {
                        Status::Ours
                    } else {
                        Status::Changed
                    }
                }
                PYTHON_ENV | CONDA_ENV if prefix.join("conda-meta").is_dir() => Status::Ours,
                PYTHON_ENV | CONDA_ENV if prefix.exists() => Status::Changed,
                OWN_ENVIRONMENT if crate::own_env::marked(prefix) => Status::Ours,
                OWN_ENVIRONMENT if prefix.exists() => Status::Changed,
                RENV_LIBRARY if prefix.symlink_metadata().is_ok() => {
                    if directory_identity(prefix).as_deref() == Some(version) {
                        Status::Ours
                    } else {
                        Status::Changed
                    }
                }
                MINIFORGE if prefix.exists() => Status::Changed, // checked after its environments
                BREW => {
                    let packages = brews.get_or_insert_with(|| {
                        install::brew(&search).and_then(|tool| brew_packages(&tool))
                    });
                    match packages {
                        Some(packages) => {
                            version_status(packages.get(&item.name).map(String::as_str), version)
                        }
                        None => Status::Changed,
                    }
                }
                R_CRAN if prefix.exists() => match super::runtime_plan::cran_installed() {
                    Some(current) if current == version => Status::Ours,
                    _ => Status::Changed,
                },
                UV_PYTHON => {
                    let pythons = uv_pythons.get_or_insert_with(|| {
                        install::uv(&search).and_then(|tool| super::runtime_plan::uv_pythons(&tool))
                    });
                    match pythons {
                        Some(pythons) => version_status(
                            pythons
                                .get(&item.place)
                                .filter(|(_, key)| key == &item.name)
                                .map(|(version, _)| version.as_str()),
                            version,
                        ),
                        None if prefix.exists() => Status::Changed,
                        None => Status::Gone,
                    }
                }
                _ => Status::Gone,
            }
        };
        item.ours = status == Status::Ours;
        if status == Status::Gone {
            gone.insert(key(item));
        }
    }
    let owned_envs: HashSet<PathBuf> = items
        .iter()
        .filter(|i| matches!(i.kind.as_str(), PYTHON_ENV | CONDA_ENV) && i.ours)
        .map(|item| PathBuf::from(&item.place))
        .collect();
    for item in items.iter_mut().filter(|i| i.kind == MINIFORGE) {
        let root = Path::new(&item.place);
        if !root.is_absolute() {
            continue;
        }
        let base_count = conda_packages(root).ok().map(|p| p.len().to_string());
        let envs = root.join("envs");
        let envs_owned = !envs.exists()
            || fs::read_dir(envs).is_ok_and(|dirs| {
                dirs.flatten()
                    .filter(|dir| dir.path().join("conda-meta").is_dir())
                    .all(|dir| owned_envs.contains(&dir.path()))
            });
        item.ours = base_count.as_deref() == Some(item.version.as_str()) && envs_owned;
    }
    gone
}

use sha2::Digest;

pub(super) fn digest(data: &[u8]) -> String {
    sandhill_proto::hex(&sha2::Sha256::digest(data))
}

/// IRkernel also writes an R package and its dependencies from R itself.
pub fn r_append(record: &Path) -> String {
    format!(
        r#"sandhill_record <- function(lib, before) {{
    now <- utils::installed.packages(lib.loc = lib)[, c("Package", "Version"), drop = FALSE]
    new <- now[!(now[, "Package"] %in% before), , drop = FALSE]
    if (nrow(new)) cat(paste0(paste("{R_PACKAGE}", new[, "Package"], new[, "Version"], lib, format(Sys.Date()), "r", ifelse(new[, "Package"] == "IRkernel", "R needs it to run in Sandhill", ""), sep = "\t"), "\n"), sep = "", file = {record}, append = TRUE)
  }}"#,
        record = super::package_plan::r_string(&record.to_string_lossy())
    )
}

pub fn r_append_kernel(record: &Path, dir: &str) -> String {
    format!(
        r#"cat(paste0(paste("{KERNEL}", basename({dir}), "", {dir}, format(Sys.Date()), "r", "R needs it to run in Sandhill", sep = "\t"), "\n"), sep = "", file = {record}, append = TRUE)"#,
        record = super::package_plan::r_string(&record.to_string_lossy())
    )
}

/// The command removing a group of items of one kind, from one place,
/// installed by one tool.
fn removal_command(group: &[&QuickInstalled], search: &Search) -> Option<String> {
    let item = *group.first()?;
    let prefix = Path::new(&item.place);
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    let with = |mut args: Vec<String>| {
        args.extend(group.iter().map(|i| i.name.clone()));
        quote(args)
    };
    let words = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    let python = || path(prefix.join("bin/python"));
    Some(match item.kind.as_str() {
        R_PACKAGE => quote(
            ["rm".to_string(), "-rf".into()]
                .into_iter()
                .chain(group.iter().map(|i| path(prefix.join(&i.name)))),
        ),
        CONDA_PACKAGE | IPYKERNEL => {
            let mut args = vec![path(install::conda(search, prefix)?)];
            args.extend(words(&["remove", "-y", "-p", &item.place]));
            with(args)
        }
        PYTHON_PACKAGE if item.tool == "uv" => {
            let mut args = vec![path(install::uv(search)?)];
            args.extend(words(&["pip", "uninstall", "--python", &python()]));
            with(args)
        }
        PYTHON_PACKAGE if item.tool == "pip" => {
            let mut args = vec![python()];
            args.extend(words(&["-m", "pip", "uninstall", "-y"]));
            with(args)
        }
        BREW => with(vec![path(install::brew(search)?), "uninstall".into()]),
        UV_PYTHON => with(vec![
            path(install::uv(search)?),
            "python".into(),
            "uninstall".into(),
        ]),
        _ if group.len() > 1 => return None,
        KERNEL | MINIFORGE => quote(["rm", "-rf", &item.place]),
        RENV_LIBRARY => quote(["/bin/rm", "-rf", "--", &item.place]),
        AGENT if item.name == "claude" && item.place == "native" => {
            "rm -f \"$HOME/.local/bin/claude\" && rm -rf \"$HOME/.local/share/claude\"".into()
        }
        AGENT if item.name == "codex" && item.place == "npm" => {
            quote(["npm", "uninstall", "-g", "@openai/codex"])
        }
        PYTHON_ENV if matches!(item.tool.as_str(), "uv" | "pip") => {
            quote(["rm", "-rf", &item.place])
        }
        PYTHON_ENV | CONDA_ENV => quote([
            &path(install::conda(search, prefix)?),
            "env",
            "remove",
            "-y",
            "-p",
            &item.place,
        ]),
        R_CRAN => super::runtime_plan::cran_remove()?,
        _ => return None,
    })
}

pub(super) fn undo_creation(
    record: &Path,
    item: &QuickInstalled,
    search: &Search,
) -> Result<String> {
    let mut items = load(record)?;
    check(&mut items);
    let id = key(item);
    let row = items
        .iter()
        .find(|row| row.ours && key(row) == id && row.version == item.version)
        .context("The folder has changed since Sandhill set it up, or it is no longer there.")?;
    removal_command(&[row], search).context("The tool that made this folder wasn't found.")
}

fn package_key(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

/// Of the conda packages being removed from `prefix`, those another
/// package staying there needs: conda would remove that package too.
fn conda_needed(prefix: &Path, removing: &HashSet<&str>) -> HashSet<String> {
    let packages: Vec<(String, Vec<String>)> = fs::read_dir(prefix.join("conda-meta"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let json: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let depends = json["depends"]
                .as_array()?
                .iter()
                .filter_map(|d| Some(d.as_str()?.split_whitespace().next()?.to_string()))
                .collect();
            Some((json["name"].as_str()?.to_string(), depends))
        })
        .collect();
    let mut keep = HashSet::new();
    loop {
        let needed: HashSet<String> = packages
            .iter()
            .filter(|(name, _)| !removing.contains(name.as_str()) || keep.contains(name))
            .flat_map(|(_, depends)| depends.iter())
            .filter(|name| removing.contains(name.as_str()) && !keep.contains(*name))
            .cloned()
            .collect();
        if needed.is_empty() {
            return keep;
        }
        keep.extend(needed);
    }
}

pub(super) fn remove(record: &Path, keys: &[InstalledKey]) -> Result<()> {
    let mut items = load(record)?;
    let gone = check(&mut items);
    let search = Search::from_env();
    let mut selected = HashSet::new();
    let mut chosen = Vec::new();
    let mut left = 0;
    for key in keys {
        if !selected.insert(key) {
            continue;
        }
        match items
            .iter()
            .find(|item| &self::key(item) == key && item.ours)
        {
            Some(item) => chosen.push(item),
            None => {
                println!(
                    "Left {} in {}: it is gone or isn't as Sandhill left it, so it is yours now.",
                    key.name, key.place
                );
                left += 1;
            }
        }
    }
    // What is inside an environment or R being removed goes with it.
    let containers: Vec<&QuickInstalled> = chosen
        .iter()
        .copied()
        .filter(|item| {
            matches!(
                item.kind.as_str(),
                PYTHON_ENV | CONDA_ENV | MINIFORGE | R_CRAN | RENV_LIBRARY
            )
        })
        .collect();
    chosen.retain(|item| {
        !containers.iter().any(|root| {
            !std::ptr::eq(*item, *root) && Path::new(&item.place).starts_with(&root.place)
        })
    });
    /// What removes together: kind, place, tool, and the name when each
    /// goes on its own.
    type GroupKey<'a> = (&'a str, &'a str, &'a str, &'a str);
    let mut groups: Vec<(GroupKey, Vec<&QuickInstalled>)> = Vec::new();
    let mut failed = 0;
    for item in &chosen {
        let kind = if item.kind == IPYKERNEL {
            CONDA_PACKAGE
        } else {
            item.kind.as_str()
        };
        let grouped = matches!(
            kind,
            R_PACKAGE | CONDA_PACKAGE | PYTHON_PACKAGE | BREW | UV_PYTHON
        );
        let valid = if kind == R_PACKAGE {
            r_name(&item.name)
        } else {
            !grouped || package_key(&item.name)
        };
        if !valid {
            eprintln!(
                "Couldn't remove {}: its name isn't one Sandhill can use.",
                item.name
            );
            failed += 1;
            continue;
        }
        let place = if kind == UV_PYTHON {
            ""
        } else {
            item.place.as_str()
        };
        let key = (
            kind,
            place,
            item.tool.as_str(),
            if grouped { "" } else { item.name.as_str() },
        );
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, group)) => group.push(item),
            None => groups.push((key, vec![item])),
        }
    }
    for ((kind, place, ..), group) in &mut groups {
        if *kind == CONDA_PACKAGE {
            let removing = group.iter().map(|i| i.name.as_str()).collect();
            let needed = conda_needed(Path::new(place), &removing);
            group.retain(|item| {
                let keep = needed.contains(&item.name);
                if keep {
                    println!(
                        "Left {} in {}: other packages there need it.",
                        item.name, item.place
                    );
                    left += 1;
                }
                !keep
            });
        }
        if group.is_empty() {
            continue;
        }
        let names: Vec<&str> = group.iter().map(|i| i.name.as_str()).collect();
        println!("Removing {} from {}.", names.join(", "), group[0].place);
        let Some(command) = removal_command(group, &search) else {
            eprintln!(
                "Couldn't remove {}: the tool that installed it wasn't found.",
                names.join(", ")
            );
            failed += group.len();
            continue;
        };
        if run_command(&command).is_ok() {
            continue;
        }
        if group.len() == 1 {
            failed += 1;
            continue;
        }
        // One that can't go (Homebrew refuses what another program needs)
        // shouldn't keep the others.
        for item in group.iter() {
            if removal_command(&[item], &search)
                .is_none_or(|command| run_command(&command).is_err())
            {
                failed += 1;
            }
        }
    }
    let after = check(&mut items);
    let removed = selected
        .iter()
        .filter(|key| after.contains(*key) && !gone.contains(*key))
        .count();
    items.retain(|item| !after.contains(&key(item)));
    save(record, &items)?;
    let not_removed = selected.len().saturating_sub(removed + left);
    println!(
        "Done: {removed} removed, {left} left as they were, {not_removed} couldn't be removed."
    );
    if failed != 0 || not_removed != 0 {
        bail!("Some of the chosen items couldn't be removed. The messages above say why.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_old_and_new_fields_and_replaces_a_previous_line() {
        let items = parse(
            "r-package\tfoo\t1.0\t/lib\t2026-11-19\nr-package\tbar\t9\t/lib\t2026-11-19\nr-package\tfoo\t1.1\t/lib\t2026-11-20\tr\tFor plots\n",
        );
        assert_eq!(items.len(), 2);
        assert_eq!(
            (&*items[0].date, &*items[0].tool, &*items[0].reason),
            ("2026-11-19", "", "")
        );
        assert_eq!(
            (&*items[1].version, &*items[1].tool, &*items[1].reason),
            ("1.1", "r", "For plots")
        );
    }

    #[test]
    fn checks_versions_from_python_and_conda_metadata_and_prunes_gone_items() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("env");
        let dist = prefix.join("lib/python3.12/site-packages/Some_Package-2.1.dist-info");
        fs::create_dir_all(&dist).unwrap();
        fs::write(dist.join("METADATA"), "Name: Some.Package\nVersion: 2.1\n").unwrap();
        let meta = prefix.join("conda-meta");
        fs::create_dir_all(&meta).unwrap();
        fs::write(meta.join("r-foo-bar-1.8-build_0.json"), "{}").unwrap();
        let at = prefix.to_str().unwrap();
        let path = tmp.path().join(FILE);
        add(
            &path,
            [
                entry(PYTHON_PACKAGE, "some--package", "2.1", at, "pip", ""),
                entry(CONDA_PACKAGE, "r-foo-bar", "1.7", at, "conda", ""),
                entry(CONDA_PACKAGE, "gone", "1", at, "conda", ""),
            ],
        )
        .unwrap();
        let items = read(&path);
        assert_eq!(items.len(), 2);
        assert!(items[0].ours && !items[1].ours);
        assert_eq!(load(&path).unwrap().len(), 2);
    }

    #[test]
    fn removal_uses_the_recorded_tool_and_quotes_the_path() {
        let search = Search::default();
        let pip = entry(
            PYTHON_PACKAGE,
            "seaborn",
            "1",
            "/some folder/env",
            "pip",
            "",
        );
        let numpy = entry(PYTHON_PACKAGE, "numpy", "1", "/some folder/env", "pip", "");
        assert_eq!(
            removal_command(&[&pip, &numpy], &search).unwrap(),
            "'/some folder/env/bin/python' '-m' 'pip' 'uninstall' '-y' 'seaborn' 'numpy'"
        );
        let r = entry(R_PACKAGE, "ggplot2", "1", "/some folder/R", "r", "");
        assert_eq!(
            removal_command(&[&r], &search).unwrap(),
            "'rm' '-rf' '/some folder/R/ggplot2'"
        );
        let env = entry(PYTHON_ENV, "env", "3.12", "/some folder/env", "uv", "");
        assert_eq!(
            removal_command(&[&env], &search).unwrap(),
            "'rm' '-rf' '/some folder/env'"
        );
        assert!(removal_command(&[&env, &env], &search).is_none());
        let tmp = tempfile::tempdir().unwrap();
        let conda = tmp.path().join("bin/conda");
        fs::create_dir_all(conda.parent().unwrap()).unwrap();
        fs::write(&conda, "").unwrap();
        fs::set_permissions(&conda, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let at = tmp.path().to_str().unwrap();
        let package = entry(CONDA_PACKAGE, "r-foo", "1", at, "conda", "");
        assert_eq!(
            removal_command(&[&package], &search).unwrap(),
            quote([conda.to_str().unwrap(), "remove", "-y", "-p", at, "r-foo"])
        );
    }

    #[test]
    fn conda_packages_another_package_needs_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = tmp.path().join("conda-meta");
        fs::create_dir(&meta).unwrap();
        for (name, depends) in [
            ("a", vec!["b >=1"]),
            ("b", vec!["c"]),
            ("c", vec![]),
            ("d", vec![]),
        ] {
            fs::write(
                meta.join(format!("{name}-1-0.json")),
                serde_json::json!({"name": name, "depends": depends}).to_string(),
            )
            .unwrap();
        }
        let needed = conda_needed(tmp.path(), &HashSet::from(["b", "c", "d"]));
        assert_eq!(needed, HashSet::from(["b".to_string(), "c".to_string()]));
    }
}
