//! Environment shared by kernels, shells and the programs they start.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sandhill_proto::EnvMode;

use crate::agents::shell_quote;
use crate::quick::install;

pub(crate) const MARKER: &str = ".sandhill-environment";
pub(crate) const SWITCH_HINT: &str =
    "Sandhill is using its own R and Python. Switch on the R and Python page to use others.";

pub(crate) fn marked(folder: &Path) -> bool {
    folder.is_absolute()
        && std::fs::symlink_metadata(folder).is_ok_and(|m| m.is_dir())
        && std::fs::symlink_metadata(folder.join(MARKER)).is_ok_and(|m| m.is_file())
}

pub(crate) fn chosen(record: &Path) -> Option<PathBuf> {
    let prefs = install::prefs(record);
    (prefs.mode == Some(EnvMode::Own)).then(|| PathBuf::from(prefs.own_folder))
}

pub(crate) fn prefix(folder: &Path, language: &str) -> PathBuf {
    folder.join("conda/envs").join(if language == "r" {
        "sandhill-r"
    } else {
        "sandhill-python"
    })
}

pub(crate) fn inside(folder: &Path, program: &Path) -> bool {
    program
        .canonicalize()
        .ok()
        .zip(folder.canonicalize().ok())
        .is_some_and(|(program, folder)| program.starts_with(folder))
}

/// The selected folder also owns package changes when temporarily using
/// an existing environment elsewhere. Find it by the marked ancestor.
pub(crate) fn containing(program: &Path) -> Option<PathBuf> {
    let real = program.canonicalize().ok()?;
    real.ancestors().find(|p| marked(p)).map(Path::to_path_buf)
}

/// The shared variables take precedence over inherited and project settings.
/// Clear variables that could otherwise redirect writes even with these set.
pub(crate) const UNSET: &[&str] = &[
    "R_LIBS",
    "R_HOME",
    "R_USER",
    "R_ENVIRON",
    "R_PROFILE",
    "PYTHONPATH",
    "PYTHONHOME",
    "PIP_TARGET",
    "PIP_PREFIX",
    "PIP_ROOT",
    "JUPYTER_PATH",
    "JUPYTER_CONFIG_PATH",
    "VIRTUAL_ENV",
    "QUARTO_PYTHON",
    "CONDA_ENVS_PATH",
    "CONDA_PKGS_DIRS",
    "MAMBARC",
    "RENV_PATHS_LIBRARY",
    "RENV_PATHS_LIBRARY_STAGING",
    "RENV_PATHS_SANDBOX",
    "RENV_PATHS_LOCAL",
    "RENV_PATHS_CELLAR",
    "RENV_PATHS_SOURCE",
    "RENV_PATHS_BINARY",
    "RENV_PATHS_RENV",
    "RENV_PATHS_LOCKFILE",
    "RENV_PATHS_PREFIX",
    "RENV_PROFILE",
];

const PATHS: &[(&str, &str)] = &[
    ("R_ENVIRON_USER", "r/Renviron"),
    ("R_PROFILE_USER", "r/Rprofile"),
    ("R_LIBS_USER", "conda/envs/sandhill-r/lib/R/library"),
    ("R_LIBS_SITE", "conda/envs/sandhill-r/lib/R/library"),
    ("R_USER_CACHE_DIR", "r/cache"),
    ("R_USER_DATA_DIR", "r/data"),
    ("R_USER_CONFIG_DIR", "r/config"),
    ("RENV_PATHS_ROOT", "renv"),
    ("RENV_PATHS_LIBRARY_ROOT", "renv/library"),
    ("RENV_PATHS_CACHE", "renv/cache"),
    ("RENV_PATHS_SANDBOX", "renv/sandbox"),
    ("RENV_PATHS_LIBRARY_STAGING", "renv/staging"),
    ("PYTHONUSERBASE", "python/user"),
    ("PIP_CONFIG_FILE", "python/pip.conf"),
    ("PIP_CACHE_DIR", "python/cache"),
    ("IPYTHONDIR", "python/ipython"),
    ("JUPYTER_CONFIG_DIR", "python/jupyter/config"),
    ("JUPYTER_DATA_DIR", "python/jupyter/data"),
    ("JUPYTER_RUNTIME_DIR", "python/jupyter/runtime"),
    ("MPLCONFIGDIR", "python/matplotlib"),
    ("XDG_CACHE_HOME", "cache"),
    ("CONDARC", "condarc"),
    ("MAMBA_ROOT_PREFIX", "conda"),
    ("TMPDIR", "tmp"),
];

const BINS: &[&str] = &[
    "conda/envs/sandhill-python/bin",
    "conda/envs/sandhill-r/bin",
    "bin",
];

fn bin_path(folder: &Path) -> OsString {
    std::env::join_paths(BINS.iter().map(|p| folder.join(p))).expect("validated environment path")
}

pub(crate) fn path(folder: &Path, rest: Option<&OsStr>) -> OsString {
    let mut path = bin_path(folder);
    if let Some(rest) = rest.filter(|p| !p.is_empty() && **p != path) {
        if rest.as_encoded_bytes().starts_with(path.as_encoded_bytes())
            && rest.as_encoded_bytes().get(path.len()) == Some(&b':')
        {
            return rest.to_os_string();
        }
        path.push(":");
        path.push(rest);
    }
    path
}

pub(crate) fn vars(folder: &Path, rest: Option<&OsStr>) -> Vec<(&'static str, OsString)> {
    PATHS
        .iter()
        .map(|&(k, p)| (k, folder.join(p).into_os_string()))
        .chain([("PATH", path(folder, rest))])
        .collect()
}

pub(crate) fn apply(command: &mut std::process::Command, folder: &Path) {
    apply_with_path(command, folder, crate::paths::program_path().as_deref());
}

fn apply_with_path(command: &mut std::process::Command, folder: &Path, rest: Option<&OsStr>) {
    command.envs(vars(folder, rest));
    for key in UNSET {
        command.env_remove(key);
    }
}

/// The terminal host and the live agents it starts use the same overrides.
/// A child may have an explicit PATH, for example with the CLI's Node first.
pub(crate) fn apply_to_shell(command: &mut std::process::Command, folder: &Path) {
    let rest = command_value(command, "PATH").unwrap_or_else(crate::paths::program_path);
    apply_with_path(command, folder, rest.as_deref());
    configure_shell(command, folder);
}

/// An explicit removal must not fall back to the parent's value.
fn command_value(command: &std::process::Command, key: &str) -> Option<Option<OsString>> {
    command
        .get_envs()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.map(OsStr::to_os_string))
}

fn inherited_value(command: &std::process::Command, key: &str) -> Option<OsString> {
    command_value(command, key).unwrap_or_else(|| std::env::var_os(key))
}

/// A shell-free prefix for the commands shown by package planning.
pub(crate) fn command(folder: &Path, line: &str) -> String {
    use std::fmt::Write as _;
    let mut command = String::from("/usr/bin/env");
    for key in UNSET {
        let _ = write!(command, " -u {key}");
    }
    for (key, value) in vars(folder, crate::paths::program_path().as_deref()) {
        let _ = write!(
            command,
            " {}",
            shell_quote(&format!("{key}={}", value.to_string_lossy()))
        );
    }
    format!("{command} {line}")
}

/// Restore the folder's values after a user's shell files have run.
fn restore(folder: &Path) -> String {
    use std::fmt::Write as _;
    let bins = shell_quote(&bin_path(folder).to_string_lossy());
    let mut script = format!(
        "case $PATH in\n  {bins}|{bins}:*) ;;\n  *) export PATH={bins}${{PATH:+:$PATH}} ;;\nesac\nunset {}\n",
        UNSET.join(" ")
    );
    // A mamba shell hook can define a function of the same name as our
    // shim. Functions and aliases beat PATH even after it is restored.
    script.push_str("unalias R Rscript python python3 pip pip3 micromamba 2>/dev/null || :\nunset -f R Rscript python python3 pip pip3 micromamba 2>/dev/null || :\n");
    for (key, value) in PATHS {
        let _ = writeln!(
            script,
            "export {key}={}",
            shell_quote(&folder.join(value).to_string_lossy())
        );
    }
    script
}

pub(crate) fn apply_renv(command: &mut std::process::Command, project: &Path, own: Option<&Path>) {
    let library = crate::quick::project_packages::renv_root(project, own);
    if (own.is_some() || library.is_dir())
        && project.join("renv.lock").is_file()
        && project.join("renv/activate.R").is_file()
    {
        command.env("RENV_PATHS_LIBRARY", library);
        command.env("RENV_PATHS_RENV", project.join("renv"));
        command.env("RENV_PATHS_LOCKFILE", project.join("renv.lock"));
        command.env_remove("RENV_PROFILE");
    }
}

const R_PROFILE: &str = r#"local({
    project <- normalizePath(getwd(), mustWork = FALSE)
    user_home <- normalizePath(path.expand("~"), mustWork = FALSE)
    profile <- file.path(project, ".Rprofile")
    home_profile <- normalizePath(file.path(user_home, ".Rprofile"), mustWork = FALSE)
    if (project != user_home && file.exists(profile) &&
        normalizePath(profile, mustWork = FALSE) != home_profile)
        sys.source(profile, envir = .GlobalEnv)
})
"#;

fn zsh_wrapper(folder: &Path, file: &str) -> String {
    let own_zdotdir = shell_quote(&folder.join("shell/zsh").to_string_lossy());
    // Give the user's files their own ZDOTDIR while sourcing, remembering
    // any change they make for the next user file, without losing ours.
    format!(
        r#"ZDOTDIR=${{SANDHILL_USER_ZDOTDIR:-$HOME}}
if [ "$ZDOTDIR" != {own_zdotdir} ] && [ -r "$ZDOTDIR/{file}" ]; then
    . "$ZDOTDIR/{file}"
fi
export SANDHILL_USER_ZDOTDIR=${{ZDOTDIR:-$HOME}}
export ZDOTDIR={own_zdotdir}
{}"#,
        restore(folder)
    )
}

fn bash_wrapper(folder: &Path) -> String {
    let rc = shell_quote(&folder.join("shell/bashrc").to_string_lossy());
    format!(
        r#"if [ "${{SANDHILL_BASH_LOGIN:-}}" = 1 ]; then
    unset SANDHILL_BASH_LOGIN
    [ ! -r /etc/profile ] || . /etc/profile
    for sandhill_rc in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [ -r "$sandhill_rc" ]; then . "$sandhill_rc"; break; fi
    done
    unset sandhill_rc
elif [[ $- == *i* ]]; then
    [ ! -r "$HOME/.bashrc" ] || . "$HOME/.bashrc"
elif [ -n "${{SANDHILL_USER_BASH_ENV:-}}" ] && [ "$SANDHILL_USER_BASH_ENV" != {rc} ] && [ -r "$SANDHILL_USER_BASH_ENV" ]; then
    . "$SANDHILL_USER_BASH_ENV"
fi
export BASH_ENV={rc}
{}"#,
        restore(folder)
    )
}

fn configure_shell(command: &mut std::process::Command, folder: &Path) {
    for (user, active, relative) in [
        ("SANDHILL_USER_ZDOTDIR", "ZDOTDIR", "shell/zsh"),
        ("SANDHILL_USER_BASH_ENV", "BASH_ENV", "shell/bashrc"),
    ] {
        let wrapper = folder.join(relative);
        let original = match inherited_value(command, active) {
            Some(current) if current == wrapper => inherited_value(command, user),
            other => other,
        };
        if let Some(original) = original {
            command.env(user, original);
        } else {
            command.env_remove(user);
        }
        command.env(active, wrapper);
    }
}

/// This command is both shown in Review and run by the install worker.
/// `channels` are the conda channels at the chosen download source.
pub(crate) fn settings(folder: &Path, channels: &[&str]) -> String {
    use std::fmt::Write as _;
    let qpath = |relative: &str| shell_quote(&folder.join(relative).to_string_lossy());
    let mut script = "umask 077\nmkdir -p".to_string();
    for dir in [
        "bin",
        "conda/pkgs",
        "home",
        "cache",
        "r/cache",
        "r/data",
        "r/config",
        "python/cache",
        "python/user",
        "python/ipython",
        "python/jupyter/config",
        "python/jupyter/data",
        "python/jupyter/runtime",
        "python/matplotlib",
        "shell/zsh",
        "tmp",
    ] {
        let _ = write!(script, " {}", qpath(dir));
    }
    script.push('\n');
    let mut file = |relative: &str, content: &str| {
        let _ = writeln!(
            script,
            "printf '%s' {} > {}",
            shell_quote(content),
            qpath(relative)
        );
    };
    let pkgs =
        serde_json::to_string(&folder.join("conda/pkgs")).expect("validated environment path");
    let channels = channels
        .iter()
        .map(|c| serde_json::to_string(c).expect("channel names"))
        .collect::<Vec<_>>()
        .join(", ");
    file(
        "condarc",
        &format!(
            "channels: [{channels}]\nchannel_priority: strict\npkgs_dirs: [{pkgs}]\nshow_banner: false\n"
        ),
    );
    file("r/Renviron", "");
    file("r/Rprofile", R_PROFILE);
    file(
        "python/pip.conf",
        "[global]\ndisable-pip-version-check = true\n",
    );
    for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        file(&format!("shell/zsh/{name}"), &zsh_wrapper(folder, name));
    }
    file("shell/bashrc", &bash_wrapper(folder));
    let shim = format!(
        "#!/bin/sh\nunset CONDA_PKGS_DIRS CONDA_ENVS_PATH CONDA_PREFIX CONDA_DEFAULT_ENV CONDA_SHLVL MAMBARC\nexport HOME={} XDG_CACHE_HOME={} CONDARC={} MAMBA_ROOT_PREFIX={} TMPDIR={}\nexec {} --rc-file \"$CONDARC\" \"$@\"\n",
        qpath("home"),
        qpath("cache"),
        qpath("condarc"),
        qpath("conda"),
        qpath("tmp"),
        qpath("micromamba")
    );
    file("bin/micromamba", &shim);
    let _ = writeln!(script, "chmod 700 {}", qpath("bin/micromamba"));
    script
}

pub(crate) fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_inside_settings_override_or_clear_inherited_settings() {
        let folder = Path::new("/home/test/environment");
        let vars: std::collections::HashMap<_, _> = vars(folder, Some(OsStr::new("/usr/bin")))
            .into_iter()
            .collect();
        for (key, relative) in PATHS {
            assert_eq!(vars[key], folder.join(relative).as_os_str());
        }
        assert_eq!(
            vars["PATH"],
            "/home/test/environment/conda/envs/sandhill-python/bin:/home/test/environment/conda/envs/sandhill-r/bin:/home/test/environment/bin:/usr/bin"
        );
        for key in ["PYTHONPATH", "R_LIBS", "PIP_TARGET"] {
            assert!(UNSET.contains(&key));
            assert!(!vars.contains_key(key));
        }
    }

    #[test]
    fn shell_wrappers_source_users_then_restore_the_folder() {
        let folder = Path::new("/sandhill");
        for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            let text = zsh_wrapper(folder, name);
            assert!(text.contains("[ \"$ZDOTDIR\" != '/sandhill/shell/zsh' ]"));
            assert!(text.contains("export SANDHILL_USER_ZDOTDIR=${ZDOTDIR:-$HOME}"));
            assert!(
                text.find(&format!(". \"$ZDOTDIR/{name}\"")).unwrap()
                    < text.find("export ZDOTDIR='/sandhill/shell/zsh'").unwrap()
            );
            assert!(text.find("export ZDOTDIR=").unwrap() < text.find("export PATH=").unwrap());
        }
        let text = bash_wrapper(folder);
        assert!(text.find(". \"$HOME/.bashrc\"").unwrap() < text.find("export PATH=").unwrap());
    }

    #[test]
    fn own_shell_reasserts_the_folder_after_user_startup() {
        let temp = tempfile::tempdir().unwrap();
        let temp = temp.path().canonicalize().unwrap();
        let folder = temp.join("own");
        let home = temp.join("user");
        let moved = temp.join("custom-zsh");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&moved).unwrap();
        let user_startup = "export PATH=/elsewhere:/usr/bin:/bin\nexport PYTHONPATH=outside\nexport TEST_RC=loaded\nmicromamba() { echo outside; }\n";
        std::fs::write(home.join(".bash_profile"), user_startup).unwrap();
        std::fs::write(
            home.join(".zshenv"),
            format!("ZDOTDIR={}\n", shell_quote(moved.to_str().unwrap())),
        )
        .unwrap();
        std::fs::write(moved.join(".zlogin"), user_startup).unwrap();
        let status = std::process::Command::new("/bin/bash")
            .args(["-ec", &settings(&folder, &["conda-forge", "bioconda"])])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(status.success());

        for shell in ["/bin/bash", "/bin/zsh"] {
            if !Path::new(shell).is_file() {
                continue;
            }
            let mut command = std::process::Command::new(shell);
            command
                .env_clear()
                .env("HOME", &home)
                .env("PATH", "/usr/bin:/bin");
            if shell.ends_with("bash") {
                command
                    .arg("--rcfile")
                    .arg(folder.join("shell/bashrc"))
                    .env("SANDHILL_BASH_LOGIN", "1");
            } else {
                command.arg("-l").env("ZDOTDIR", folder.join("shell/zsh"));
            }
            let output = command.args(["-ic", "printf '%s\\n' \"$TEST_RC\" \"$PATH\" \"${PYTHONPATH-not-set}\"; command -v micromamba"]).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let mut lines = text.lines();
            assert_eq!(lines.next(), Some("loaded"));
            assert!(
                lines
                    .next()
                    .unwrap()
                    .starts_with(bin_path(&folder).to_str().unwrap())
            );
            assert_eq!(lines.next(), Some("not-set"));
            assert_eq!(lines.next(), folder.join("bin/micromamba").to_str());
        }
    }
}
