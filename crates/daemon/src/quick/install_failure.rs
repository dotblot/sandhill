use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use sandhill_proto::{
    FailureFix, InstallFailure, InstallFailureKind as Kind, InstallTool, PackageChange, PackagePlan,
};
use serde::{Deserialize, Serialize};

use super::{env_actions::Work, install::Job};

/// A write refused by the lock while Sandhill's own R and Python are
/// chosen. The app shows the same sentence under such errors.
pub const LOCKED_MESSAGE: &str = "Sandhill's own R and Python are chosen, so your other R and Python can't be changed from Sandhill. Switch on the R and Python page to change them.";

#[derive(Default, Serialize, Deserialize)]
struct LogContext {
    folder: Option<PathBuf>,
    download_mb: Option<u64>,
    package: Option<String>,
    alternative: Option<PackageChange>,
    /// The folders locked when the job started (see `own_lock`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    locked: Vec<PathBuf>,
}

impl LogContext {
    fn packages(plan: &PackagePlan) -> Self {
        use InstallTool::*;
        // pip and uv both fetch packages from PyPI. Switching between them
        // doesn't help when the site or the package on it is unavailable.
        let selected = plan.change.tool;
        let alternative =
            plan.tools
                .iter()
                .map(|info| info.tool)
                .find(|&tool| match (selected, tool) {
                    (Some(R | Pip | Uv), Conda) => {
                        plan.change.packages.iter().all(|p| !p.contains('/'))
                    }
                    (Some(Conda), R | Pip | Uv) => true,
                    _ => false,
                });
        Self {
            folder: Some(PathBuf::from(&plan.folder)),
            download_mb: None,
            package: match plan.packages.as_slice() {
                [only] => Some(only.name.clone()),
                _ => None,
            },
            alternative: alternative.map(|tool| {
                let mut change = plan.change.clone();
                change.tool = Some(tool);
                change
            }),
            locked: Vec::new(),
        }
    }
}

pub(super) fn remember(log: &Path, job: &Job) -> Result<()> {
    let mut context = match job {
        Job::Packages(plan) => LogContext::packages(plan),
        Job::Environment(Work::Packages(plan)) => LogContext::packages(plan),
        Job::OwnSetup(work) => LogContext {
            folder: Some(PathBuf::from(&work.plan().setup.folder)),
            download_mb: Some(work.plan().download_mb),
            ..Default::default()
        },
        Job::ProjectSetup(work) => LogContext {
            folder: Some(PathBuf::from(&work.plan().folder)),
            download_mb: work.plan().download_mb,
            ..Default::default()
        },
        Job::Runtime(plan) => LogContext {
            folder: Some(PathBuf::from(&plan.folder)),
            download_mb: plan.download_mb,
            ..Default::default()
        },
        _ => return Ok(()),
    };
    context.locked = super::own_lock::known();
    crate::files::write_atomic(
        &log.with_extension("meta.json"),
        &serde_json::to_vec(&context)?,
    )?;
    Ok(())
}

pub(super) fn copy_failure(
    log: &str,
    folder: &Path,
    package: &str,
    alternative: Option<PackageChange>,
) -> Option<InstallFailure> {
    classify(
        log,
        &LogContext {
            folder: Some(folder.into()),
            package: Some(package.into()),
            alternative,
            download_mb: None,
            locked: super::own_lock::known(),
        },
    )
}

pub(crate) fn explain(log_dir: &Path, path: &Path) -> Result<Option<InstallFailure>> {
    let folder = log_dir.canonicalize()?;
    let log = path.canonicalize()?;
    ensure!(
        log.starts_with(&folder) && log.extension().is_some_and(|ext| ext == "log"),
        "Only install logs in Sandhill's log folder can be explained."
    );
    let mut log_file = open_file(&log)?;
    // Keep large package download logs from taking over the daemon's memory.
    const LIMIT: u64 = 1024 * 1024;
    let length = log_file.metadata()?.len();
    log_file.seek(SeekFrom::Start(length.saturating_sub(LIMIT)))?;
    let mut bytes = Vec::new();
    log_file.take(LIMIT).read_to_end(&mut bytes)?;
    let mut context: LogContext = open_file(&log.with_extension("meta.json"))
        .ok()
        .and_then(|file| serde_json::from_reader(file.take(16 * 1024)).ok())
        .unwrap_or_default();
    if context.locked.is_empty()
        && let Some(state) = folder.parent()
    {
        context.locked = super::own_lock::locked_now(&state.join(super::record::FILE));
    }
    Ok(classify(&String::from_utf8_lossy(&bytes), &context))
}

fn open_file(path: &Path) -> Result<File> {
    // Do not follow symlinks or wait on pipes or devices.
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "This isn't an install log file."
    );
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "This isn't an install log file."
    );
    Ok(file)
}

fn classify(log: &str, context: &LogContext) -> Option<InstallFailure> {
    let log = if log.contains('\x1b') {
        std::borrow::Cow::Owned(crate::mcp::text::strip_ansi(log))
    } else {
        std::borrow::Cow::Borrowed(log)
    };
    let lines: Vec<_> = log
        .lines()
        .map(|line| (line.trim(), line.to_ascii_lowercase()))
        .collect();
    let find = |test: fn(&str) -> bool| lines.iter().rposition(|(_, lower)| test(lower));
    let one = |kind, message: &str, detail, fix| InstallFailure {
        kind,
        message: message.into(),
        detail,
        fix,
    };
    let alternative = || {
        context.alternative.as_ref().and_then(|change| {
            Some(FailureFix::OtherTool {
                tool: change.tool?,
                change: Some(change.clone()),
            })
        })
    };
    if find(|line| {
        any(
            line,
            &[
                "no space left on device",
                "not enough disk space",
                "errno 28",
                "disk quota exceeded",
            ],
        )
    })
    .is_some()
    {
        let mut detail = Vec::new();
        if let Some(folder) = &context.folder
            && let Ok(free) = super::own::free_mb(folder)
        {
            detail.push(format!(
                "{free} MB free where the packages go: {}",
                folder.display()
            ));
        }
        if let Some(size) = context.download_mb.filter(|&mb| mb > 0) {
            detail.push(format!("About {size} MB to download."));
        } else if let Some((line, _)) = lines
            .iter()
            .find(|(_, lower)| any(lower, &["total download:", "total download size:"]))
        {
            detail.push(format!(
                "Download size: {}",
                line.split_once(':').map_or(*line, |(_, size)| size.trim())
            ));
        }
        if let Some((line, _)) = lines.iter().find(|(_, lower)| {
            any(
                lower,
                &[
                    "additional space",
                    "space required",
                    "bytes required",
                    "disk space needed",
                ],
            )
        }) {
            detail.push(format!(
                "Space needed: {}",
                line.trim_start_matches('-').trim()
            ));
        }
        return Some(one(
            Kind::Space,
            "There isn't enough free space to install these packages.",
            detail,
            Some(FailureFix::FreeUpSpace),
        ));
    }
    if let Some(folder) = lines
        .iter()
        .rev()
        .filter(|(_, lower)| any(lower, &["operation not permitted", "read-only file system"]))
        .find_map(|(line, _)| locked_folder(line, &context.locked))
    {
        return Some(one(
            Kind::Permission,
            LOCKED_MESSAGE,
            vec![format!("Folder: {}", folder.display())],
            None,
        ));
    }
    if let Some(index) = find(|line| {
        any(
            line,
            &[
                "permission denied",
                "is not writable",
                "is not writeable",
                "errno 13",
                "read-only file system",
            ],
        )
    }) {
        let detail = context.folder.as_ref().map_or_else(
            || vec![lines[index].0.into()],
            |folder| vec![format!("Folder: {}", folder.display())],
        );
        return Some(one(
            Kind::Permission,
            "Sandhill couldn't write to the folder. Choose a folder you can write to.",
            detail,
            None,
        ));
    }
    if let Some(index) = find(network) {
        let host = host_in(lines[index].0)
            .or_else(|| {
                lines[..index]
                    .iter()
                    .rev()
                    .take(24)
                    .find_map(|(line, _)| host_in(line))
            })
            .or_else(|| {
                lines[index + 1..]
                    .iter()
                    .take(4)
                    .find_map(|(line, _)| host_in(line))
            });
        let message = host.map_or_else(|| "Sandhill couldn't reach the package site. Check your internet connection and try again.".into(),
            |host| format!("Sandhill couldn't download the packages from {host}. The site may be down or blocked."));
        return Some(one(
            Kind::Network,
            &message,
            host.map(|host| format!("Site: {host}"))
                .into_iter()
                .collect(),
            alternative(),
        ));
    }
    // A specific missing header is more useful than clang's generic final error.
    if let Some((library, true)) = lines
        .iter()
        .find_map(|(line, lower)| system_library(line, lower))
    {
        return Some(system_failure(library));
    }
    let rcpp = lines.iter().any(|(_, lower)| {
        any(
            lower,
            &["'rcpp.h'", "<rcpp.h>", "'rcpparmadillo.h'", "'rcppeigen.h'"],
        )
    });
    if find(|line| {
        any(
            line,
            &[
                "invalid active developer path",
                "xcode-select: error",
                "unable to find utility \"xcrun\"",
                "make: not found",
                "make: command not found",
                "gfortran: command not found",
                "gfortran: not found",
            ],
        )
    })
    .is_some()
        || !rcpp && find(|line| line.contains("clang: error")).is_some()
    {
        let message = context.package.as_ref().map_or_else(
            || "A package needs to be built on this computer.".into(),
            |name| format!("{name} needs to be built on this computer."),
        );
        if cfg!(target_os = "macos") {
            return Some(one(
                Kind::DeveloperTools,
                &format!("{message} It needs Apple's developer tools."),
                Vec::new(),
                Some(FailureFix::InstallDeveloperTools),
            ));
        }
        return Some(one(
            Kind::DeveloperTools,
            &format!("{message} It needs developer tools that aren't available."),
            Vec::new(),
            None,
        ));
    }
    if let Some(detail) = lines
        .iter()
        .find_map(|(line, lower)| namespace_conflict(line, lower))
    {
        return Some(conflict(vec![detail]));
    }
    if let Some(index) = find(|line| {
        any(
            line,
            &[
                "is not available for this version of r",
                "no matching distribution found for",
                "packagesnotfounderror",
                "nothing provides",
                "was not found in the package registry",
                "could not find a version that satisfies the requirement",
            ],
        )
    }) {
        let line = lines[index].0;
        let name = not_available_name(line)
            .or(context.package.as_deref())
            .or_else(|| {
                lines[index + 1..]
                    .iter()
                    .take(8)
                    .find_map(|(line, _)| line.strip_prefix("- "))
            });
        let detail = name
            .map(|name| format!("Package: {name}"))
            .into_iter()
            .collect();
        return Some(one(
            Kind::NotAvailable,
            "The source didn't have a version of the package that works here.",
            detail,
            alternative(),
        ));
    }
    if let Some(index) = find(|line| {
        any(
            line,
            &[
                "the following packages are incompatible",
                "conflicting dependencies",
                "resolutionimpossible",
                "libmambaunsatisfiableerror",
                "unsatisfiableerror",
                "no solution found when resolving dependencies",
            ],
        )
    }) {
        let details = lines
            .iter()
            .skip(index.saturating_sub(18))
            .take(40)
            .filter_map(|(line, _)| requirement(line))
            .take(5)
            .collect();
        return Some(conflict(details));
    }
    if let Some((library, _)) = lines
        .iter()
        .find_map(|(line, lower)| system_library(line, lower))
    {
        return Some(system_failure(library));
    }
    None
}

fn any(line: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| line.contains(p))
}

/// The locked folder a refused write in `line` was in: by a path as the
/// line writes it, or as the sandbox saw it, resolved.
fn locked_folder<'a>(line: &str, locked: &'a [PathBuf]) -> Option<&'a Path> {
    if locked.is_empty() {
        return None;
    }
    line.split(|c: char| {
        c.is_whitespace() || "'\"`\u{2018}\u{2019}\u{201c}\u{201d},;()[]{}<>".contains(c)
    })
    .map(|word| word.trim_end_matches([':', '.']))
    .filter(|word| word.starts_with('/'))
    .find_map(|word| {
        let path = Path::new(word);
        let real = super::own_lock::real(path);
        locked
            .iter()
            .find(|folder| path.starts_with(folder) || real.starts_with(folder))
    })
    .map(PathBuf::as_path)
}

fn network(line: &str) -> bool {
    any(
        line,
        &[
            "could not resolve host",
            "could not connect to",
            "failed to connect",
            "cannot open url",
            "connectionerror",
            "connection refused",
            "connection timed out",
            "sslerror",
            "ssl error",
            "ssl connect error",
            "ssl certificate",
            "certificate verify failed",
            "temporary failure in name resolution",
            "network is unreachable",
            "http 000 connection failed",
            "dns error",
            "network is offline",
        ],
    ) || line.split("http").skip(1).any(|tail| {
        let tail = tail.trim_start_matches([' ', 's', ':']);
        let tail = tail.strip_prefix("status").map_or(tail, |s| {
            s.trim_start().trim_start_matches(|c: char| {
                c.is_ascii_alphabetic() || matches!(c, ' ' | ':' | '(' | '\'' | '"')
            })
        });
        let code = tail
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or_default();
        code == "403" || code == "404" || code.len() == 3 && code.starts_with('5')
    })
}

fn host_in(line: &str) -> Option<&str> {
    let address = line
        .split_once("://")
        .map(|(_, address)| address)
        .or_else(|| line.split_once("host='").map(|(_, tail)| tail))
        .or_else(|| {
            line.split_once("Could not resolve host:")
                .map(|(_, tail)| tail.trim_start())
        })
        .or_else(|| {
            line.split_once("Failed to connect to ")
                .map(|(_, tail)| tail)
        })?;
    if address.starts_with('/') {
        return None;
    }
    let host = address
        .split(['/', '\'', '"', '>', ')', ' ', '\t', '?', '#'])
        .next()?;
    let host = host
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .trim_end_matches('.');
    (!host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')))
    .then_some(host)
}

fn system_library<'a>(line: &'a str, lower: &str) -> Option<(&'a str, bool)> {
    for name in ["libxml-2.0", "gdal-config", "pkg-config"] {
        if lower.contains(name)
            && any(
                lower,
                &[
                    "not found",
                    "could not",
                    "missing",
                    "no package",
                    "not in",
                    "was not found",
                    "pkg_config_path",
                ],
            )
        {
            return Some((
                match name {
                    "libxml-2.0" => "libxml2",
                    "gdal-config" => "GDAL",
                    _ => name,
                },
                true,
            ));
        }
    }
    if let Some((_, tail)) = line.split_once("fatal error: ")
        && any(lower, &["file not found", "no such file or directory"])
    {
        let name = tail
            .trim_start_matches(['\'', '"', '<'])
            .split(['\'', '"', '>', ':', ' '])
            .next()?;
        if !name.starts_with("Rcpp") && !name.is_empty() {
            return Some((name, true));
        }
    }
    if let Some((_, library)) = line.split_once("library not found for -l") {
        return Some((library.trim().trim_matches('\''), true));
    }
    if lower.contains("configuration failed for package") {
        return Some(("", false));
    }
    None
}

fn system_failure(library: &str) -> InstallFailure {
    InstallFailure {
        kind: Kind::SystemLibrary,
        message: "A package needs files that aren't on this computer. Sandhill's own folder can often get those files with the package.".into(),
        detail: (!library.is_empty()).then(|| format!("Missing: {library}")).into_iter().collect(),
        fix: None,
    }
}

fn conflict(detail: Vec<String>) -> InstallFailure {
    InstallFailure { kind: Kind::Conflict, message: "These packages can't be installed together. They need different versions of the same package.".into(), detail, fix: None }
}

fn requirement(line: &str) -> Option<String> {
    let plain = line.trim_start_matches([' ', '-', '+', '|', '│', '├', '└', '─', '╰', '×', '▶']);
    let plain = plain.strip_prefix("Because ").unwrap_or(plain);
    if let Some((package, needs)) = plain.split_once(" requires ") {
        // Leave out the repeated children of conda's tree.
        let needs = needs
            .split_once(", which")
            .map_or(needs, |(needs, _)| needs);
        let needs = needs.trim_end_matches([' ', ',', ':', ';']);
        if !package.contains('[') && !needs.starts_with("any potential options") {
            return Some(format!("{package} needs {needs}."));
        }
    }
    plain
        .split_once(" depends on ")
        .or_else(|| plain.split_once(" requested "))
        .map(|(package, needs)| format!("{package} needs {}.", needs.trim_end_matches('.')))
}

fn namespace_conflict(line: &str, lower: &str) -> Option<String> {
    if lower.contains("is being loaded, but") && lower.contains("is required") {
        let (_, rest) = line.split_once("namespace ")?;
        let (loaded, req) = rest.split_once(" is being loaded, but ")?;
        let (version, _) = req.split_once(" is required")?;
        let version = version.trim_start_matches(['>', '=', ' ']);
        return Some(format!(
            "{} is installed, but this package needs version {version} or later.",
            loaded.replace(['‘', '’'], "'")
        ));
    }
    None
}

fn not_available_name(line: &str) -> Option<&str> {
    if let Some((_, name)) = line
        .split_once("No matching distribution found for ")
        .or_else(|| line.split_once("Could not find a version that satisfies the requirement "))
        .or_else(|| line.split_once("nothing provides "))
    {
        return name.split_whitespace().next();
    }
    let (_, name) = line.split_once("package ")?;
    let (name, _) = name.split_once(" is not available for this version of R")?;
    Some(name.trim_matches(['‘', '’', '\'', '"']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conda_change() -> PackageChange {
        PackageChange {
            action: sandhill_proto::PackageAction::Install,
            target: sandhill_proto::InstallTarget::Python {
                prefix: "/env".into(),
                python: "/env/bin/python".into(),
            },
            packages: vec!["scanpy".into()],
            tool: Some(InstallTool::Conda),
            reason: "Imported from a package list".into(),
        }
    }

    #[test]
    fn explains_realistic_installer_errors() {
        let cases: Vec<(Kind, String, String)> = serde_json::from_str(include_str!(
            "../../tests/fixtures/install-failure-logs.json"
        ))
        .unwrap();
        let context = LogContext {
            alternative: Some(conda_change()),
            ..Default::default()
        };
        for (kind, log, detail) in cases {
            let shown =
                classify(&log, &context).unwrap_or_else(|| panic!("no explanation for {log}"));
            assert_eq!(shown.kind, kind, "{log}");
            assert!(
                detail.is_empty() || shown.detail.iter().any(|line| line.contains(&detail)),
                "{shown:?}"
            );
            let fix = match kind {
                Kind::DeveloperTools if cfg!(target_os = "macos") => {
                    Some(FailureFix::InstallDeveloperTools)
                }
                Kind::Network | Kind::NotAvailable => Some(FailureFix::OtherTool {
                    tool: InstallTool::Conda,
                    change: Some(conda_change()),
                }),
                Kind::Space => Some(FailureFix::FreeUpSpace),
                _ => None,
            };
            assert_eq!(shown.fix, fix);
        }
        assert!(classify("Using compiler: clang\nfoo.cpp:1:10: fatal error: 'Rcpp.h' file not found\nclang: error: compilation failed", &context).is_none());
        assert!(classify("Error: installation of package failed", &context).is_none());
    }

    #[test]
    fn environment_install_log_keeps_the_same_change_for_another_source() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("install.log");
        let mut change = conda_change();
        change.tool = Some(InstallTool::Pip);
        let plan = PackagePlan {
            change,
            tools: [InstallTool::Pip, InstallTool::Conda]
                .map(|tool| sandhill_proto::InstallToolInfo {
                    tool,
                    path: String::new(),
                    version: String::new(),
                })
                .into(),
            folder: "/env".into(),
            packages: Vec::new(),
            command: String::new(),
            notes: Vec::new(),
            refused: None,
            refused_fix: None,
        };
        remember(&log, &Job::Environment(Work::Packages(Box::new(plan)))).unwrap();
        fs::write(
            &log,
            "ConnectionError: HTTPSConnectionPool(host='pypi.org')",
        )
        .unwrap();
        assert_eq!(
            explain(tmp.path(), &log).unwrap().unwrap().fix,
            Some(FailureFix::OtherTool {
                tool: InstallTool::Conda,
                change: Some(conda_change()),
            })
        );
    }

    #[test]
    fn a_write_refused_by_the_lock_says_how_to_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        let library = real.join("Library/R/arm64/4.5/library");
        fs::create_dir_all(&library).unwrap();
        let linked = real.join("linked");
        std::os::unix::fs::symlink(real.join("Library"), &linked).unwrap();
        let context = LogContext {
            locked: vec![library.clone()],
            ..Default::default()
        };
        for log in [
            format!(
                "Warning in install.packages: 'lib = \"{0}\"' is not writable\nError: ERROR: failed to lock directory '{0}' for modifying\nTry removing '{0}/00LOCK-x': Operation not permitted",
                library.display()
            ),
            format!(
                "ERROR: Could not install packages due to an OSError: [Errno 1] Operation not permitted: '{}'",
                linked.join("R/arm64/4.5/library/numpy").display()
            ),
            format!("mkdir: {}/newpkg: Read-only file system", library.display()),
        ] {
            let shown = classify(&log, &context).unwrap();
            assert_eq!(shown.kind, Kind::Permission, "{log}");
            assert_eq!(shown.message, LOCKED_MESSAGE);
            assert_eq!(shown.detail, [format!("Folder: {}", library.display())]);
            assert_eq!(shown.fix, None);
        }
        // Outside a locked folder, or with nothing locked: the usual message.
        let elsewhere = format!(
            "cp: {}: Read-only file system",
            real.join("other/file").display()
        );
        assert_ne!(
            classify(&elsewhere, &context).unwrap().message,
            LOCKED_MESSAGE
        );
        let unlocked = format!("mkdir: {}/x: Read-only file system", library.display());
        assert_ne!(
            classify(&unlocked, &LogContext::default()).unwrap().message,
            LOCKED_MESSAGE
        );
        assert!(!LOCKED_MESSAGE.contains(['\u{2013}', '\u{2014}']));
    }

    #[test]
    fn rejects_logs_outside_the_state_log_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("log");
        fs::create_dir(&dir).unwrap();
        let outside = tmp.path().join("outside.log");
        fs::write(&outside, "Permission denied").unwrap();
        assert!(explain(&dir, &outside).is_err());
        let symlink = dir.join("escape.log");
        std::os::unix::fs::symlink(&outside, &symlink).unwrap();
        assert!(explain(&dir, &symlink).is_err());
        let inside = dir.join("install.log");
        fs::write(&inside, "ERROR: No space left on device").unwrap();
        assert_eq!(explain(&dir, &inside).unwrap().unwrap().kind, Kind::Space);
    }
}
