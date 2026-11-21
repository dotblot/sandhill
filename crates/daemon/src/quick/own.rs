//! Plan and run the one-folder install through the confirmed install worker.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use sandhill_proto::{
    CORE_KIT, EnvMode, InstallPrefs, KITS, Kit, OWN_FOLDER, OwnEnvironment, OwnRuntime, OwnSetup,
    OwnSetupPlan, PlannedPackage, SetupStep,
};

use super::{install, record};
use crate::kernelspecs::{self, Search};
use crate::own_env::{self, MARKER, executable, prefix};

const MB: u64 = 1_000_000;
const NAME: &str = "Sandhill's own R and Python";
const MISSING: &str =
    "Sandhill's own R and Python aren't set up. Set them up on the R and Python page first.";
const PLATFORM: Option<&str> = if_cfg_platform();

const fn if_cfg_platform() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("osx-arm64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("osx-64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else {
        None
    }
}

/// Resolve aliases of the existing parent even when the chosen folder isn't there yet.
fn folder(path: &Path) -> Result<PathBuf> {
    fn check(path: &Path) -> Result<()> {
        ensure!(path.is_absolute(), "Choose a full path for the folder.");
        ensure!(
            path.to_str()
                .is_some_and(|p| !p.contains(char::is_whitespace)),
            "Choose a folder with no spaces in its path. Micromamba can't use paths with spaces."
        );
        ensure!(
            !path.as_os_str().as_encoded_bytes().contains(&b':')
                && !path.components().any(|c| c == Component::ParentDir),
            "Choose a folder whose path has no colon or parent folder references."
        );
        Ok(())
    }
    check(path)?;
    let parent = path
        .ancestors()
        .find(|p| p.exists())
        .context("The folder can't be reached.")?;
    let tail = path.strip_prefix(parent)?;
    ensure!(
        parent.is_dir() && (parent != path || !parent.is_symlink()),
        "Choose a folder, not a file or a link to another folder."
    );
    let parent = parent.canonicalize()?;
    let path = if tail.as_os_str().is_empty() {
        parent
    } else {
        parent.join(tail)
    };
    check(&path)?;
    ensure!(
        !path.exists() || own_env::marked(&path) || fs::read_dir(&path)?.next().is_none(),
        "The folder already has files that aren't Sandhill's. Choose an empty folder."
    );
    Ok(path)
}

fn free_mb(folder: &Path) -> Result<u64> {
    let parent = folder
        .ancestors()
        .find(|path| path.is_dir())
        .context("The folder can't be reached.")?;
    let path = CString::new(parent.as_os_str().as_bytes())?;
    // SAFETY: the path is NUL terminated and statvfs initializes the output on success.
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    #[allow(clippy::unnecessary_cast)]
    Ok((info.f_bavail as u64).saturating_mul(info.f_frsize as u64) / MB)
}

pub(crate) fn developer_tools(search: &Search) -> bool {
    if cfg!(target_os = "macos") {
        Command::new("/usr/bin/xcode-select")
            .arg("-p")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    } else {
        install::on_path(search, "cc").is_some()
    }
}

pub(crate) fn install_developer_tools() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "This button opens Apple's installer and only works on a Mac. Install a C compiler for this computer first."
    );
    let status = Command::new("/usr/bin/xcode-select")
        .arg("--install")
        .status()
        .context("Couldn't open Apple's developer tools installer.")?;
    ensure!(
        status.success(),
        "Apple's developer tools installer couldn't open."
    );
    Ok(())
}

fn all_kits() -> impl Iterator<Item = &'static Kit> {
    std::iter::once(&CORE_KIT).chain(KITS)
}

fn selections(setup: &OwnSetup) -> Result<Vec<&'static Kit>> {
    for id in &setup.kits {
        ensure!(
            all_kits().any(|kit| kit.id == id),
            "Sandhill doesn't know the kit {id:?}."
        );
    }
    Ok(all_kits()
        .filter(|kit| kit.id == CORE_KIT.id || setup.kits.iter().any(|id| id == kit.id))
        .collect())
}

fn package(name: &str, spec: String, source: &str, version: Option<String>) -> PlannedPackage {
    PlannedPackage {
        name: name.into(),
        spec,
        source: source.into(),
        version,
    }
}

fn r_package(name: &str, bioc: bool) -> String {
    format!(
        "{}-{}",
        if bioc { "bioconductor" } else { "r" },
        name.to_ascii_lowercase()
    )
}

struct Inventory {
    prefix: PathBuf,
    conda: record::Versions,
    r: bool,
}

impl Inventory {
    fn new(folder: &Path, language: &str) -> Self {
        let prefix = prefix(folder, language);
        let conda = record::conda_packages(&prefix).unwrap_or_default();
        Self {
            prefix,
            conda,
            r: language == "r",
        }
    }

    fn runtime(&self) -> (&str, &str, &str) {
        if self.r {
            ("R", "r-base", "r-base")
        } else {
            ("python", "python", "python=3.12")
        }
    }

    fn present(&self) -> bool {
        self.conda.contains_key(self.runtime().1) && executable(&self.program())
    }

    fn program(&self) -> PathBuf {
        self.prefix.join("bin").join(self.runtime().0)
    }

    fn r_version(&self, name: &str, spec: &str) -> Option<String> {
        self.conda
            .get(spec)
            .cloned()
            .or_else(|| record::package_version(&self.prefix.join("lib/R/library"), name))
    }

    fn packages(&self, kit: &Kit, platform: &str) -> Vec<PlannedPackage> {
        if self.r {
            kit.r
                .iter()
                .map(|&(name, bioc)| {
                    let conda = r_package(name, bioc);
                    let version = self.r_version(name, &conda);
                    if platform == "osx-arm64" && name == "GSVA" {
                        package(
                            name,
                            name.into(),
                            "Bioconductor (built on this Mac)",
                            version,
                        )
                    } else {
                        package(
                            name,
                            conda,
                            if bioc { "bioconda" } else { "conda-forge" },
                            version,
                        )
                    }
                })
                .chain(kit.conda_only.iter().map(|&name| {
                    package(
                        name,
                        name.into(),
                        "conda-forge",
                        self.conda.get(name).cloned(),
                    )
                }))
                .collect()
        } else {
            kit.python
                .iter()
                .map(|&(name, pypi)| {
                    package(
                        pypi,
                        name.into(),
                        if name == "gseapy" {
                            "bioconda"
                        } else {
                            "conda-forge"
                        },
                        self.conda.get(name).cloned(),
                    )
                })
                .collect()
        }
    }
}

fn step(plan: &mut OwnSetupPlan, what: &str, command: String) {
    plan.steps.push(SetupStep {
        what: what.into(),
        command,
    });
}

pub(crate) fn plan(setup: OwnSetup) -> OwnSetupPlan {
    plan_for(
        setup,
        PLATFORM,
        || developer_tools(&Search::from_env()),
        free_mb,
    )
}

fn plan_for(
    setup: OwnSetup,
    platform: Option<&str>,
    tools: impl Fn() -> bool,
    free: impl FnOnce(&Path) -> Result<u64>,
) -> OwnSetupPlan {
    let mut plan = OwnSetupPlan {
        setup,
        steps: Vec::new(),
        r_packages: Vec::new(),
        python_packages: Vec::new(),
        download_mb: 0,
        free_mb: 0,
        minutes: 2,
        notes: vec!["Nothing outside this folder changes.".into()],
        needs_developer_tools: false,
        refused: None,
    };
    if let Err(error) = build(&mut plan, platform, tools, free) {
        plan.refused = Some(error.to_string());
    }
    plan
}

fn build(
    plan: &mut OwnSetupPlan,
    platform: Option<&str>,
    tools: impl Fn() -> bool,
    free: impl FnOnce(&Path) -> Result<u64>,
) -> Result<()> {
    let platform =
        platform.context("Sandhill can't set up its own R and Python on this computer.")?;
    ensure!(
        plan.setup.r || plan.setup.python,
        "Choose R, Python, or both."
    );
    let kits = selections(&plan.setup)?;
    let requested = if plan.setup.folder.is_empty() {
        install::home()
            .context("Sandhill couldn't find your home folder. Choose a folder.")?
            .join(OWN_FOLDER)
    } else {
        PathBuf::from(&plan.setup.folder)
    };
    let folder = folder(&requested)?;
    plan.setup.folder = folder.to_string_lossy().into_owned();
    plan.free_mb =
        free(&folder).context("Couldn't check how much space is free for this folder.")?;
    let shim = folder.join("bin/micromamba");
    let binary = folder.join("micromamba");
    let binary = binary.to_string_lossy();
    if !executable(Path::new(binary.as_ref())) {
        let url = format!("https://micro.mamba.pm/api/micromamba/{platform}/latest");
        let target = install::quote([binary.as_ref()]);
        step(
            plan,
            "Download micromamba, conda's one-file installer",
            format!(
                "umask 077; mkdir -p {} && curl -q -fsSL {url} | /usr/bin/env -u TAR_OPTIONS tar -xjOf - bin/micromamba > {target} && chmod 700 {target}",
                install::quote([plan.setup.folder.as_str()])
            ),
        );
        plan.download_mb += 7;
    }
    step(
        plan,
        "Keep R, Python and shell settings in this folder",
        own_env::settings(&folder),
    );
    let mut gsva = None;
    let mut conda_mb = 0;
    for (lang, chosen) in [("r", plan.setup.r), ("python", plan.setup.python)] {
        if !chosen {
            continue;
        }
        let inventory = Inventory::new(&folder, lang);
        let present = inventory.present();
        let needs_pip = !inventory.r && !inventory.conda.contains_key("pip");
        let mut wanted = Vec::new();
        let mut seen = HashSet::new();
        let mut download = 0;
        let (base_mb, cap) = if inventory.r { (274, 837) } else { (110, 251) };
        for kit in &kits {
            let mut missing = 0_u64;
            let mut total = 0_u64;
            for pkg in inventory.packages(kit, platform) {
                if !seen.insert(pkg.spec.clone()) {
                    continue;
                }
                total += 1;
                if pkg.version.is_some() {
                    continue;
                }
                if pkg.name == "GSVA" && platform == "osx-arm64" {
                    let available = tools();
                    gsva = Some((pkg, available));
                    missing += u64::from(available);
                } else {
                    missing += 1;
                    wanted.push(pkg);
                }
            }
            let kit_mb = if kit.id == CORE_KIT.id {
                base_mb
            } else if !inventory.r && !kit.r.is_empty() {
                if kit.id == "plots" { 15 } else { 40 }
            } else {
                kit.download_mb
            };
            if kit.id == CORE_KIT.id && !present {
                download += base_mb;
            } else if total != 0 {
                download += (kit_mb * missing).div_ceil(total);
            }
        }
        if present && needs_pip && download == 0 {
            download = 2;
        }
        if !present || !wanted.is_empty() || needs_pip {
            let shim = shim.to_string_lossy();
            let prefix = inventory.prefix.to_string_lossy();
            let (display, _, runtime) = inventory.runtime();
            let display = if inventory.r { display } else { "Python" };
            let mut argv = vec![
                shim.as_ref(),
                if present { "install" } else { "create" },
                "-y",
                "-p",
                prefix.as_ref(),
                "--override-channels",
                "-c",
                "conda-forge",
                "-c",
                "bioconda",
            ];
            if present {
                argv.push("--freeze-installed");
            } else {
                argv.push(runtime);
            }
            if needs_pip {
                argv.push("pip");
            }
            argv.extend(wanted.iter().map(|pkg| pkg.spec.as_str()));
            step(
                plan,
                &if present {
                    format!("Add packages to Sandhill's {display}")
                } else {
                    format!("Set up Sandhill's {display}")
                },
                install::quote(argv),
            );
        }
        let download = download.min(cap);
        conda_mb += download;
        plan.download_mb += download;
        if inventory.r {
            plan.r_packages = wanted;
            if let Some((pkg, can_build)) = gsva.take() {
                if can_build {
                    let rscript = inventory.prefix.join("bin/Rscript");
                    let code = "options(repos = c(CRAN = 'https://cloud.r-project.org')); BiocManager::install('GSVA', type = 'source', ask = FALSE, update = FALSE, Ncpus = 2L); stopifnot(requireNamespace('GSVA', quietly = TRUE))";
                    let command = own_env::command(
                        &folder,
                        &install::quote([
                            rscript.to_string_lossy().as_ref(),
                            "--no-save",
                            "--no-restore",
                            "-e",
                            code,
                        ]),
                    );
                    let isolated =
                        install::quote([format!("HOME={}", folder.join("home").display())]);
                    step(
                        plan,
                        "Build GSVA from Bioconductor on this Mac",
                        format!(
                            "cd {} && SDKROOT=$(/usr/bin/xcrun --show-sdk-path) && export SDKROOT && /usr/bin/env {isolated} {command}",
                            install::quote([plan.setup.folder.as_str()])
                        ),
                    );
                    plan.r_packages.push(pkg);
                } else {
                    plan.needs_developer_tools = true;
                    plan.notes.push("The Pathways kit will be installed without GSVA. Install Apple's developer tools, then add the kit again to get GSVA.".into());
                }
            }
        } else {
            plan.python_packages = wanted;
        }
    }
    let required = ((conda_mb * 3 + plan.download_mb) * 12).div_ceil(10);
    ensure!(
        plan.free_mb >= required,
        "This setup needs about {required} MB of free space. Only {} MB is free in this folder.",
        plan.free_mb
    );
    plan.minutes = ((plan.download_mb.div_ceil(300) * 2) as u32
        + if plan
            .r_packages
            .iter()
            .any(|p| p.name == "GSVA" && p.spec == "GSVA")
        {
            5
        } else {
            0
        })
    .max(2);
    Ok(())
}

pub(crate) fn set_mode(path: &Path, mode: Option<EnvMode>) -> Result<InstallPrefs> {
    let _lock =
        record::lock(path, false).context("Wait for the current install or removal to finish.")?;
    let prefs = install::prefs(path);
    if mode == Some(EnvMode::Own) {
        let folder = Path::new(&prefs.own_folder);
        ensure!(
            own_env::marked(folder)
                && ["r", "python"]
                    .into_iter()
                    .any(|lang| Inventory::new(folder, lang).present()),
            "{MISSING}"
        );
    }
    install::update_prefs(path, |prefs| prefs.mode = mode)
}

pub(super) fn run(record_path: &Path, plan: &OwnSetupPlan) -> Result<()> {
    ensure!(plan.refused.is_none(), "This setup wasn't ready to run.");
    let folder = folder(Path::new(&plan.setup.folder))?;
    ensure!(
        folder == Path::new(&plan.setup.folder),
        "The chosen folder changed after Sandhill checked it."
    );
    let new = !own_env::marked(&folder);
    let mut snapshots = Vec::new();
    if !new {
        use sandhill_proto::{InstallTarget, InstallTool};
        for (language, wanted) in [("r", plan.setup.r), ("python", plan.setup.python)] {
            let prefix = own_env::prefix(&folder, language);
            if !wanted || !prefix.join("conda-meta").is_dir() {
                continue;
            }
            let program = prefix.join(if language == "r" {
                "bin/R"
            } else {
                "bin/python"
            });
            let program = program.to_string_lossy().into_owned();
            let library = prefix.join("lib/R/library");
            let target = if language == "r" {
                InstallTarget::R {
                    r: program,
                    library: library.to_string_lossy().into_owned(),
                }
            } else {
                InstallTarget::Python {
                    python: program,
                    prefix: prefix.to_string_lossy().into_owned(),
                }
            };
            let mut tracks = vec![super::undo::Track {
                target: target.clone(),
                folder: prefix,
                tool: InstallTool::Conda,
                path: folder.join("bin/micromamba").to_string_lossy().into_owned(),
            }];
            if language == "r"
                && plan
                    .r_packages
                    .iter()
                    .any(|p| p.name == "GSVA" && p.spec == "GSVA")
            {
                tracks.push(super::undo::Track {
                    target,
                    folder: library,
                    tool: InstallTool::R,
                    path: String::new(),
                });
            }
            snapshots.push(super::undo::begin(
                record_path,
                "the last kit additions".into(),
                tracks,
            )?);
        }
    }
    for step in &plan.steps {
        println!("{}", step.what);
        if let Err(error) = install::run_command(&format!("set -e -o pipefail\n{}", step.command)) {
            if new {
                // Remove only the files this first setup made, not the chosen folder.
                for name in [
                    "bin", "conda", "home", "cache", "r", "python", "shell", "tmp",
                ] {
                    let _ = fs::remove_dir_all(folder.join(name));
                }
                for name in ["condarc", "micromamba"] {
                    let _ = fs::remove_file(folder.join(name));
                }
            }
            for snapshot in snapshots {
                snapshot.finish(record_path)?;
            }
            return Err(error);
        }
    }
    for snapshot in snapshots {
        snapshot.finish(record_path)?;
    }
    crate::files::write_atomic(&folder.join(MARKER), b"Sandhill's own R and Python\n")?;
    record::add(
        record_path,
        [record::entry(
            record::OWN_ENVIRONMENT,
            NAME,
            "",
            &plan.setup.folder,
            "micromamba",
            "Set up from the R and Python page",
        )],
    )?;
    install::update_prefs(record_path, |prefs| {
        prefs.mode = Some(EnvMode::Own);
        prefs.own_folder.clone_from(&plan.setup.folder);
    })?;
    println!("Sandhill's own R and Python are ready.");
    Ok(())
}

pub(crate) fn removable(folder: &Path) -> Result<()> {
    ensure!(
        own_env::marked(folder),
        "This folder doesn't have Sandhill's marker. Sandhill won't remove it."
    );
    Ok(())
}

/// Called only while the install worker holds the lock, after kernels stop.
pub(super) fn remove(record: &Path, folder: &Path) -> Result<()> {
    removable(folder)?;
    fs::remove_dir_all(folder).with_context(|| format!("Couldn't remove {}.", folder.display()))?;
    if let Some(home) = install::home().and_then(|home| home.canonicalize().ok())
        && let Some(parent) = folder
            .parent()
            .filter(|parent| *parent == home.join(".sandhill"))
    {
        let _ = fs::remove_dir(parent);
    }
    record::forget_own(record, folder)?;
    install::update_prefs(record, |prefs| {
        if Path::new(&prefs.own_folder) == folder {
            prefs.mode = None;
            prefs.own_folder.clear();
        }
        prefs.last.retain(|choice| match &choice.target {
            sandhill_proto::InstallTarget::R { r, .. } => !Path::new(r).starts_with(folder),
            sandhill_proto::InstallTarget::Python { python, .. } => {
                !Path::new(python).starts_with(folder)
            }
        });
    })?;
    println!("Removed {}.", folder.display());
    Ok(())
}

pub(super) fn readiness(search: &Search, folder: &Path) -> Option<OwnEnvironment> {
    if !own_env::marked(folder) {
        return None;
    }
    let search = search.clone().in_own(Some(folder.to_path_buf()));
    let kernels = kernelspecs::discover(&search);
    let r = Inventory::new(folder, "r");
    let python = Inventory::new(folder, "python");
    let runtime = |inventory: &Inventory, language: &str| {
        inventory.present().then(|| OwnRuntime {
            version: inventory.conda[inventory.runtime().1].clone(),
            prefix: inventory.prefix.to_string_lossy().into_owned(),
            kernel: kernels
                .iter()
                .filter(|k| {
                    !k.duplicate
                        && k.dir.kernelspec.language.eq_ignore_ascii_case(language)
                        && k.env.as_deref() == inventory.prefix.file_name().and_then(|p| p.to_str())
                })
                .map(|k| &k.dir.kernel_name)
                .min()
                .cloned()
                .unwrap_or_default(),
        })
    };
    let r_ready = runtime(&r, "r");
    let python_ready = runtime(&python, "python");
    let platform = PLATFORM.unwrap_or_default();
    let kits = all_kits()
        .filter(|kit| {
            let packages = r_ready
                .iter()
                .flat_map(|_| r.packages(kit, platform))
                .chain(
                    python_ready
                        .iter()
                        .flat_map(|_| python.packages(kit, platform)),
                );
            let mut packages = packages.peekable();
            packages.peek().is_some() && packages.all(|pkg| pkg.version.is_some())
        })
        .map(|kit| kit.id.to_string())
        .collect();
    Some(OwnEnvironment {
        folder: folder.to_string_lossy().into_owned(),
        r: r_ready,
        python: python_ready,
        kits,
        size_mb: disk_bytes(folder, &mut HashSet::new()).div_ceil(MB),
    })
}

fn disk_bytes(path: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    let bytes = if meta.nlink() > 1 && !meta.is_dir() && !seen.insert((meta.dev(), meta.ino())) {
        0
    } else {
        meta.blocks() * 512
    };
    if meta.is_dir() {
        bytes
            + fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| disk_bytes(&e.path(), seen))
                .sum::<u64>()
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(folder: &Path) -> OwnSetup {
        OwnSetup {
            folder: folder.to_string_lossy().into_owned(),
            r: true,
            python: true,
            kits: Vec::new(),
        }
    }

    #[test]
    fn own_setup_core_and_all_kits_have_platform_caps() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("environment");
        let mut setup = setup(&root);
        let core = plan_for(
            setup.clone(),
            Some("linux-64"),
            || unreachable!(),
            |_| Ok(5000),
        );
        assert_eq!(core.refused, None);
        assert_eq!(core.download_mb, 391);
        assert_eq!(core.steps.len(), 4);
        assert!(core.steps[0].command.contains("/linux-64/latest"));
        assert!(core.steps[2].command.contains("'r-base' 'r-irkernel'"));
        assert!(
            core.steps[3]
                .command
                .contains("'python=3.12' 'pip' 'ipykernel'")
        );
        setup.kits = KITS.iter().map(|k| k.id.into()).collect();
        let all = plan_for(setup, Some("linux-64"), || unreachable!(), |_| Ok(5000));
        assert_eq!(all.download_mb, 837 + 251 + 7);
        assert!(
            all.r_packages
                .iter()
                .any(|p| p.spec == "bioconductor-gsva" && p.source == "bioconda")
        );
        assert!(
            all.r_packages
                .iter()
                .any(|p| p.spec == "quarto" && p.source == "conda-forge")
        );
        assert!(
            all.python_packages
                .iter()
                .any(|p| p.name == "decoupler" && p.spec == "decoupler-py")
        );
    }

    #[test]
    fn own_add_kits_only_lists_and_estimates_new_packages() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let python = prefix(&root, "python");
        fs::create_dir_all(python.join("conda-meta")).unwrap();
        fs::create_dir_all(python.join("bin")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        for file in [root.join("micromamba"), python.join("bin/python")] {
            fs::write(&file, "").unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(root.join(MARKER), "").unwrap();
        for name in ["python", "pip"]
            .into_iter()
            .chain(CORE_KIT.python.iter().map(|&(conda, _)| conda))
        {
            fs::write(
                python
                    .join("conda-meta")
                    .join(format!("{name}-3.12.1-build.json")),
                "",
            )
            .unwrap();
        }
        let mut setup = setup(&root);
        setup.r = false;
        let core = plan_for(
            setup.clone(),
            Some("linux-64"),
            || unreachable!(),
            |_| Ok(5000),
        );
        assert!(core.python_packages.is_empty());
        assert_eq!(core.download_mb, 0);
        setup.kits = vec!["singlecell-python".into()];
        let add = plan_for(setup, Some("linux-64"), || unreachable!(), |_| Ok(5000));
        assert_eq!(add.download_mb, 101);
        assert_eq!(add.steps.len(), 2);
        assert!(add.steps[1].command.contains("'install'"));
        assert!(
            add.steps[1]
                .command
                .contains("'--freeze-installed' 'scanpy'")
        );
        assert!(!add.steps[1].command.contains("'numpy'"));
    }

    #[test]
    fn own_setup_gsva_adds_a_source_step_only_with_developer_tools() {
        let temp = tempfile::tempdir().unwrap();
        let mut setup = setup(temp.path());
        setup.kits = vec!["pathways".into()];
        for tools in [true, false] {
            let plan = plan_for(setup.clone(), Some("osx-arm64"), || tools, |_| Ok(5000));
            assert_eq!(plan.refused, None);
            assert_eq!(plan.needs_developer_tools, !tools);
            assert_eq!(
                plan.steps
                    .iter()
                    .any(|s| s.command.contains("BiocManager::install")),
                tools
            );
            assert_eq!(
                plan.r_packages
                    .iter()
                    .any(|p| p.name == "GSVA" && p.source.starts_with("Bioconductor")),
                tools
            );
        }
    }

    #[test]
    fn own_setup_refuses_bad_folders_and_low_space() {
        let temp = tempfile::tempdir().unwrap();
        for path in [
            PathBuf::from("relative"),
            temp.path().join("a space"),
            temp.path().join(".."),
            PathBuf::from("/"),
        ] {
            assert!(folder(&path).is_err(), "{}", path.display());
        }
        assert!(folder(&temp.path().join("missing/child")).is_ok());
        assert!(
            plan_for(setup(temp.path()), None, || unreachable!(), |_| Ok(5000))
                .refused
                .is_some()
        );
        assert!(
            plan_for(
                setup(temp.path()),
                Some("linux-64"),
                || unreachable!(),
                |_| Ok(100)
            )
            .refused
            .unwrap()
            .contains("free space")
        );
        fs::write(temp.path().join("personal.txt"), "keep me").unwrap();
        assert!(folder(temp.path()).is_err());
        assert!(removable(temp.path()).is_err());
        let marker = temp.path().join(MARKER);
        std::os::unix::fs::symlink(temp.path().join("personal.txt"), &marker).unwrap();
        assert!(removable(temp.path()).is_err());
        fs::remove_file(&marker).unwrap();
        fs::write(marker, "Sandhill").unwrap();
        assert!(folder(temp.path()).is_ok());
        assert!(removable(temp.path()).is_ok());
    }
}
