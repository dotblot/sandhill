//! R, Python or Miniforge set up in an official way, shown before it runs.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    MINIFORGE_COMMAND, NewRuntime, PYTHON_SETS, PYTHON_VERSIONS, RuntimePlan, python_create_args,
    python_env_name_ok,
};

use super::install::{self, home, probe, quote, run_command};
use super::{python, record};
use crate::agents::shell_quote;
use crate::kernelspecs::Search;

const CRAN_MAC: &str = "https://cran.r-project.org/bin/macosx/";
const R_FRAMEWORK: &str = "/Library/Frameworks/R.framework";
const MINIFORGE_DIR: &str = "miniforge3";

pub(crate) fn plan(search: &Search, runtime: NewRuntime) -> RuntimePlan {
    let mut plan = RuntimePlan {
        runtime,
        tool: String::new(),
        folder: String::new(),
        command: String::new(),
        download_mb: None,
        notes: Vec::new(),
        refused: None,
    };
    match build(search, &mut plan) {
        Ok(command) => plan.command = command,
        Err(error) => plan.refused = Some(error.to_string()),
    }
    plan
}

fn build(search: &Search, plan: &mut RuntimePlan) -> Result<String> {
    match plan.runtime.clone() {
        NewRuntime::RFromCran => {
            if !cfg!(target_os = "macos") {
                bail!("CRAN's R installer is for Macs. Use Homebrew or a conda environment here.");
            }
            if Path::new(R_FRAMEWORK).exists() {
                bail!("CRAN's R is installed already in {R_FRAMEWORK}.");
            }
            let (url, file) = cran_installer()?;
            plan.tool = "Apple's Installer".into();
            plan.folder = R_FRAMEWORK.into();
            plan.download_mb = download_mb(&url);
            if let Some(version) = file.strip_prefix("R-").and_then(|f| f.split('-').next()) {
                plan.notes
                    .push(format!("This installs R {version} from CRAN."));
            }
            plan.notes
                .push("Apple's Installer opens and asks for your password.".into());
            Ok(format!(
                "dir=\"$(mktemp -d)\" && curl -fL -o \"$dir\"/{file} {url} && open -W \"$dir\"/{file}; status=$?; rm -rf \"$dir\"; exit $status",
                file = shell_quote(&file),
                url = shell_quote(&url),
            ))
        }
        NewRuntime::RFromHomebrew => {
            let brew = install::brew(search).context("Homebrew isn't on this computer.")?;
            if install::probe_brew(&brew, &["list", "--formula", "--versions", "r"])
                .is_some_and(|out| !out.trim().is_empty())
            {
                bail!("Homebrew's R is installed already.");
            }
            plan.tool = brew.to_string_lossy().into_owned();
            plan.folder = install::probe_brew(&brew, &["--prefix"])
                .map(|out| out.trim().to_string())
                .filter(|folder| !folder.is_empty())
                .context("Homebrew couldn't say where it installs programs.")?;
            plan.notes
                .push("Homebrew may also install other programs that R needs.".into());
            plan.notes.push("Homebrew's R builds packages from their source code, which can take a few minutes.".into());
            Ok(quote([plan.tool.as_str(), "install", "r"]))
        }
        NewRuntime::RConda { prefix } => {
            new_folder(&prefix, false)?;
            let (tool, _) = python::find_tool(search)
                .context("mamba or conda isn't on this computer. Install Miniforge first.")?;
            plan.tool = tool.to_string_lossy().into_owned();
            plan.folder = prefix.clone();
            plan.notes
                .push("R and IRkernel come from conda-forge.".into());
            Ok(quote([
                plan.tool.as_str(),
                "create",
                "-y",
                "-p",
                &prefix,
                "-c",
                "conda-forge",
                "--override-channels",
                "r-base",
                "r-irkernel",
            ]))
        }
        NewRuntime::PythonConda {
            prefix,
            python,
            sets,
        } => {
            check_python(&python, &sets)?;
            new_folder(&prefix, false)?;
            let (tool, _) = python::find_tool(search)
                .context("mamba or conda isn't on this computer. Install Miniforge first.")?;
            plan.tool = tool.to_string_lossy().into_owned();
            plan.folder = prefix.clone();
            plan.notes
                .push("Python and its packages come from conda-forge.".into());
            Ok(quote(python_create_args(
                &plan.tool, &prefix, &python, &sets,
            )))
        }
        NewRuntime::PythonUv {
            prefix,
            python,
            sets,
        } => {
            check_python(&python, &sets)?;
            let name = new_folder(&prefix, true)?;
            let uv = install::uv(search).context("uv isn't on this computer.")?;
            let kernel = kernel_dir(search, &name)?;
            if kernel.symlink_metadata().is_ok() {
                bail!("A kernel called {name} exists already. Choose another folder name.");
            }
            plan.tool = uv.to_string_lossy().into_owned();
            plan.folder = prefix.clone();
            if let Some(dir) = probe(&uv, &["python", "dir"])
                .map(|out| out.trim().to_string())
                .filter(|dir| !dir.is_empty())
            {
                plan.notes.push(format!("If Python {python} isn't on this computer yet, uv downloads it. uv keeps the Python it downloads in {dir}."));
            }
            plan.notes.push(format!(
                "The kernel is registered as {name} in {}.",
                kernel.display()
            ));
            let program = Path::new(&prefix)
                .join("bin/python")
                .to_string_lossy()
                .into_owned();
            let mut install = vec![
                plan.tool.clone(),
                "pip".into(),
                "install".into(),
                "--python".into(),
                program.clone(),
                "ipykernel".into(),
            ];
            install.extend(set_packages(&sets).map(String::from));
            Ok(format!(
                "{} && {} && {}",
                quote([plan.tool.as_str(), "venv", "--python", &python, &prefix]),
                quote(install),
                quote([
                    program.as_str(),
                    "-m",
                    "ipykernel",
                    "install",
                    "--user",
                    "--name",
                    &name,
                    "--display-name",
                    &format!("Python {python} ({name})")
                ]),
            ))
        }
        NewRuntime::Miniforge => {
            if let Some((tool, _)) = python::find_tool(search) {
                bail!("{} is here already.", tool.display());
            }
            let root = home().context("HOME isn't set.")?.join(MINIFORGE_DIR);
            if root.symlink_metadata().is_ok() {
                bail!("{} exists already.", root.display());
            }
            plan.tool = "conda-forge's Miniforge installer".into();
            plan.folder = root.to_string_lossy().into_owned();
            plan.notes
                .push("Your shell's startup files stay as they are.".into());
            Ok(MINIFORGE_COMMAND.into())
        }
    }
}

fn check_python(python: &str, sets: &[String]) -> Result<()> {
    if !PYTHON_VERSIONS.contains(&python) {
        bail!("Python {python} isn't one of the versions offered.");
    }
    if let Some(set) = sets
        .iter()
        .find(|set| !PYTHON_SETS.iter().any(|(id, ..)| id == set))
    {
        bail!("{set} isn't a package set Getting Ready offers.");
    }
    Ok(())
}

fn set_packages(sets: &[String]) -> impl Iterator<Item = &'static str> + '_ {
    PYTHON_SETS
        .iter()
        .filter(|(id, ..)| sets.iter().any(|set| set == id))
        .flat_map(|(.., packages)| packages.iter().copied())
}

/// A new environment's folder: a full path whose parent exists and which
/// doesn't. Returns its name.
fn new_folder(prefix: &str, spaces_ok: bool) -> Result<String> {
    let path = Path::new(prefix);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !path.is_absolute() || !python_env_name_ok(name) {
        bail!("Choose a full folder path whose last part has only letters, digits, - and _.");
    }
    if !spaces_ok && prefix.chars().any(char::is_whitespace) {
        bail!("mamba and conda can't use a folder whose path has spaces. Choose another folder.");
    }
    let parent = path.parent().unwrap_or(path);
    if !parent.is_dir() {
        bail!(
            "The folder {} doesn't exist. Choose a folder that does.",
            parent.display()
        );
    }
    if path.symlink_metadata().is_ok() {
        bail!("{prefix} exists already. Choose a new folder.");
    }
    Ok(name.into())
}

fn kernel_dir(search: &Search, name: &str) -> Result<PathBuf> {
    Ok(search
        .user_dir
        .as_ref()
        .context("Jupyter's folder for kernels wasn't found.")?
        .join("kernels")
        .join(name))
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent()
}

/// CRAN's current installer for this Mac's processor, from CRAN's page.
fn cran_installer() -> Result<(String, String)> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => bail!("CRAN has no R installer for this Mac's processor ({other})."),
    };
    let page = http()
        .get(CRAN_MAC)
        .call()
        .ok()
        .and_then(|r| r.into_body().read_to_string().ok())
        .context(
            "Sandhill couldn't reach CRAN to find its R installer. Check the internet connection.",
        )?;
    cran_link(&page, arch)
        .map(|link| {
            let file = link.rsplit('/').next().unwrap_or(link).to_string();
            (format!("{CRAN_MAC}{link}"), file)
        })
        .context("Sandhill couldn't find CRAN's R installer for this Mac on CRAN's page.")
}

fn cran_link<'a>(page: &'a str, arch: &str) -> Option<&'a str> {
    let suffix = format!("-{arch}.pkg");
    page.split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .find(|link| {
            link.ends_with(&suffix)
                && link.contains("/base/R-")
                && !link.contains("..")
                && !link.contains(':')
                && link
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b'-' | b'.' | b'_'))
        })
}

fn download_mb(url: &str) -> Option<u64> {
    let response = http().head(url).call().ok()?;
    let bytes: u64 = response
        .headers()
        .get("content-length")?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    Some(bytes.div_ceil(1_000_000))
}

/// The version of CRAN's R in `/Library/Frameworks/R.framework`.
pub(super) fn cran_installed() -> Option<String> {
    let versions = Path::new(R_FRAMEWORK).join("Versions");
    let current = versions.join(fs::read_link(versions.join("Current")).ok()?);
    let description =
        fs::read_to_string(current.join("Resources/library/base/DESCRIPTION")).ok()?;
    Some(record::r_field(&description, "Version")?.into())
}

/// Removes CRAN's R with macOS's password dialog. The `/usr/local/bin`
/// links go only when they point into CRAN's R.
pub(super) fn cran_remove() -> Option<String> {
    cfg!(target_os = "macos").then(|| {
        let script = "rm -rf /Library/Frameworks/R.framework /Applications/R.app; for f in /usr/local/bin/R /usr/local/bin/Rscript; do case $(readlink $f) in /Library/Frameworks/R.framework/*) rm -f $f;; esac; done; for p in $(pkgutil --pkgs | grep -i '^org[.]r-project[.]'); do pkgutil --forget $p; done";
        quote(["osascript", "-e", &format!("do shell script \"{script}\" with administrator privileges")])
    })
}

/// The Pythons uv installed, by folder: version and uv's name for it.
pub(super) fn uv_pythons(uv: &Path) -> Option<BTreeMap<String, (String, String)>> {
    let dir = PathBuf::from(probe(uv, &["python", "dir"])?.trim());
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Some(BTreeMap::new()),
        Err(_) => return None,
    };
    Some(
        entries
            .flatten()
            .filter_map(|entry| {
                let key = entry.file_name().into_string().ok()?;
                let version = key.split('-').nth(1)?;
                // uv also keeps links named by minor version; those aren't installs.
                (entry.file_type().ok()?.is_dir()
                    && key.starts_with(|c: char| c.is_ascii_alphabetic())
                    && version.starts_with(|c: char| c.is_ascii_digit()))
                .then_some(())?;
                Some((
                    entry.path().to_string_lossy().into_owned(),
                    (version.to_string(), key),
                ))
            })
            .collect(),
    )
}

pub(super) fn run(record_path: &Path, plan: &RuntimePlan) -> Result<()> {
    let name = Path::new(&plan.folder)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match &plan.runtime {
        NewRuntime::RFromCran => {
            let result = run_command(&plan.command);
            let version = cran_installed();
            if let Some(version) = &version {
                record::add(
                    record_path,
                    [record::entry(
                        record::R_CRAN,
                        "R",
                        version.as_str(),
                        R_FRAMEWORK,
                        "installer",
                        "Getting Ready installed R",
                    )],
                )?;
            }
            result?;
            if version.is_none() {
                bail!(
                    "R wasn't installed. Apple's Installer may have been closed before it finished."
                );
            }
        }
        NewRuntime::RFromHomebrew => {
            let brew = Path::new(&plan.tool);
            let before = record::brew_packages(brew)
                .context("Homebrew couldn't list what it has installed.")?;
            let result = run_command(&plan.command);
            let after = record::brew_packages(brew).unwrap_or_default();
            record::add(
                record_path,
                after
                    .into_iter()
                    .filter(|(formula, _)| !before.contains_key(formula))
                    .map(|(formula, version)| {
                        let reason = if formula == "r" {
                            "Getting Ready installed R"
                        } else {
                            ""
                        };
                        record::entry(record::BREW, formula, version, &plan.folder, "brew", reason)
                    }),
            )?;
            result?;
        }
        NewRuntime::RConda { prefix } | NewRuntime::PythonConda { prefix, .. } => {
            let result = run_command(&plan.command);
            let packages = record::conda_packages(Path::new(prefix)).unwrap_or_default();
            if Path::new(prefix).join("conda-meta").is_dir() {
                let (kind, package, reason) = match plan.runtime {
                    NewRuntime::RConda { .. } => {
                        (record::CONDA_ENV, "r-base", "Getting Ready made it for R")
                    }
                    _ => (
                        record::PYTHON_ENV,
                        "python",
                        "Getting Ready made it for Python",
                    ),
                };
                let version = packages.get(package).cloned().unwrap_or_default();
                record::add(
                    record_path,
                    [record::entry(
                        kind,
                        name.as_str(),
                        version,
                        prefix,
                        "conda",
                        reason,
                    )],
                )?;
            }
            result?;
        }
        NewRuntime::PythonUv { prefix, python, .. } => {
            let uv = Path::new(&plan.tool);
            let before = uv_pythons(uv).unwrap_or_default();
            let result = run_command(&plan.command);
            let mut entries: Vec<_> = uv_pythons(uv)
                .unwrap_or_default()
                .into_iter()
                .filter(|(place, _)| !before.contains_key(place))
                .map(|(place, (version, key))| {
                    record::entry(
                        record::UV_PYTHON,
                        key,
                        version,
                        &place,
                        "uv",
                        &format!("uv downloaded it for {name}"),
                    )
                })
                .collect();
            if Path::new(prefix).join("pyvenv.cfg").is_file() {
                entries.push(record::entry(
                    record::PYTHON_ENV,
                    name.as_str(),
                    python.as_str(),
                    prefix,
                    "uv",
                    "Getting Ready made it for Python",
                ));
            }
            let kernel = kernel_dir(&Search::from_env(), &name)?;
            if let Ok(spec) = fs::read(kernel.join("kernel.json")) {
                entries.push(record::entry(
                    record::KERNEL,
                    name.as_str(),
                    record::digest(&spec),
                    &kernel.to_string_lossy(),
                    "uv",
                    "Python needs it to run in Sandhill",
                ));
            }
            record::add(record_path, entries)?;
            result?;
        }
        NewRuntime::Miniforge => {
            let result = run_command(&plan.command);
            if let Ok(packages) = record::conda_packages(Path::new(&plan.folder)) {
                record::add(
                    record_path,
                    [record::entry(
                        record::MINIFORGE,
                        "miniforge",
                        packages.len().to_string(),
                        &plan.folder,
                        "installer",
                        "Getting Ready installed it for Python",
                    )],
                )?;
            }
            result?;
        }
    }
    println!("Done.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_installer_for_each_processor_on_cran_s_page() {
        let page = r#"<a href="sonoma-arm64/base/R-4.6.2-arm64.pkg">R</a><a href="big-sur-x86_64/base/R-4.6.2-x86_64.pkg">R</a><a href="old/base/R-4.2.3.pkg">old</a>"#;
        assert_eq!(
            cran_link(page, "arm64"),
            Some("sonoma-arm64/base/R-4.6.2-arm64.pkg")
        );
        assert_eq!(
            cran_link(page, "x86_64"),
            Some("big-sur-x86_64/base/R-4.6.2-x86_64.pkg")
        );
        assert_eq!(
            cran_link(
                r#"<a href="https://evil.test/base/R-1-arm64.pkg">"#,
                "arm64"
            ),
            None
        );
    }

    #[test]
    fn new_environments_need_a_new_named_folder_and_their_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = |name: &str| tmp.path().join(name).to_string_lossy().into_owned();
        let none = Search::default();
        let refused = |runtime| plan(&none, runtime).refused.unwrap();
        assert!(
            refused(NewRuntime::RConda {
                prefix: "relative".into()
            })
            .contains("full folder path")
        );
        assert!(
            refused(NewRuntime::RConda {
                prefix: prefix("a b")
            })
            .contains("letters")
        );
        assert!(
            refused(NewRuntime::RConda {
                prefix: prefix("missing/env")
            })
            .contains("doesn't exist")
        );
        assert!(
            refused(NewRuntime::RConda {
                prefix: prefix("r")
            })
            .contains("mamba or conda")
        );
        fs::create_dir(tmp.path().join("taken")).unwrap();
        assert!(
            refused(NewRuntime::PythonUv {
                prefix: prefix("taken"),
                python: "3.12".into(),
                sets: vec![]
            })
            .contains("exists already")
        );
        assert!(
            refused(NewRuntime::PythonUv {
                prefix: prefix("py"),
                python: "2.7".into(),
                sets: vec![]
            })
            .contains("versions offered")
        );
        assert!(
            refused(NewRuntime::PythonConda {
                prefix: prefix("py"),
                python: "3.12".into(),
                sets: vec!["r".into()]
            })
            .contains("package set")
        );
    }

    #[test]
    fn plans_conda_and_uv_environments_with_their_exact_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("forge");
        fs::create_dir_all(root.join("conda-meta")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        let mamba = root.join("bin/mamba");
        let uv = root.join("bin/uv");
        for tool in [&mamba, &uv] {
            fs::write(tool, "#!/bin/sh\nexit 1\n").unwrap();
            fs::set_permissions(tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        let search = Search {
            conda_roots: vec![root.clone()],
            path: Some(root.join("bin").into_os_string()),
            user_dir: Some(tmp.path().join("jupyter")),
            ..Search::default()
        };
        let prefix = tmp.path().join("envs-r").to_string_lossy().into_owned();
        let r = plan(
            &search,
            NewRuntime::RConda {
                prefix: prefix.clone(),
            },
        );
        assert_eq!(
            r.command,
            quote([
                mamba.to_str().unwrap(),
                "create",
                "-y",
                "-p",
                &prefix,
                "-c",
                "conda-forge",
                "--override-channels",
                "r-base",
                "r-irkernel"
            ])
        );
        let py = tmp.path().join("my-py").to_string_lossy().into_owned();
        let plan = plan(
            &search,
            NewRuntime::PythonUv {
                prefix: py.clone(),
                python: "3.12".into(),
                sets: vec!["analysis".into()],
            },
        );
        assert!(plan.refused.is_none(), "{plan:?}");
        assert!(plan.command.starts_with(&quote([
            uv.to_str().unwrap(),
            "venv",
            "--python",
            "3.12",
            &py
        ])));
        assert!(plan.command.contains("'ipykernel' 'numpy'"));
        assert!(
            plan.command
                .ends_with("'--name' 'my-py' '--display-name' 'Python 3.12 (my-py)'")
        );
    }

    #[test]
    fn cran_removal_asks_macos_for_the_password_and_keeps_other_links() {
        if let Some(command) = cran_remove() {
            assert!(command.starts_with("'osascript' '-e' 'do shell script"));
            assert!(command.contains("with administrator privileges"));
            assert!(command.contains("/Library/Frameworks/R.framework/*) rm -f"));
        }
    }
}
