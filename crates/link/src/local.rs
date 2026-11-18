//! The daemon on this machine.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::{ARK_PATH, RUFF_PATH, TY_PATH};
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::{Connection, Error, Options, ssh};

/// Startup may wait for a stopped/older daemon's verified exit before opening
/// the journal and reattaching live kernels.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) async fn connect(
    options: &Options,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Connection, Error> {
    let program = find().ok_or(Error::NotFound)?;
    let path = login_path().await;
    if let Some(dir) = &options.builds
        && let Some(home) = std::env::var_os("HOME")
    {
        let os = if cfg!(target_os = "macos") {
            "Darwin"
        } else {
            "Linux"
        };
        // On Linux the app also carries the Ark, Posit's R kernel, it
        // installs on servers; it goes here too when R can run it.
        let ark = os == "Linux" && r_on(path.as_deref());
        install_bundled(
            dir,
            Path::new(&home),
            os,
            std::env::consts::ARCH,
            ark,
            progress,
        )
        .await;
    }
    let mut command = Command::new(&program);
    if let Some(socket) = std::env::var_os("SANDHILL_SOCKET") {
        command.arg("--socket").arg(socket);
    }
    command.args(["attach", "--start", "--confirm-exit"]);
    // The daemon started from here passes this on to every kernel.
    if let Some(path) = path {
        command.env("PATH", path);
    }
    if cfg!(target_os = "macos") && std::env::var_os("LANG").is_none() {
        // Apps opened from the Finder get no locale, and R warns about it.
        command.env("LANG", "en_US.UTF-8");
    }

    crate::attach(command, &program.display().to_string(), ANSWER_TIMEOUT).await
}

/// Whether `R` runs from `path` (else this process's `PATH`).
fn r_on(path: Option<&str>) -> bool {
    let path = path.map(Into::into).or_else(|| std::env::var_os("PATH"));
    path.is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            dir.join("R")
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// No package manager, shell profile or project environment is changed. Use
/// exactly the same transfer/ownership script as remote installation.
async fn install_bundled(
    dir: &Path,
    home: &Path,
    os: &str,
    arch: &str,
    ark: bool,
    progress: &(dyn Fn(&str) + Send + Sync),
) {
    let here = if cfg!(target_os = "macos") {
        "this Mac"
    } else {
        "this computer"
    };
    let ark = ark.then_some(("ark", ARK_PATH));
    for (name, target) in [("ty", TY_PATH), ("ruff", RUFF_PATH)]
        .into_iter()
        .chain(ark)
    {
        let result = async {
            let Some(build) = ssh::find_platform_build(dir, os, arch, name)? else {
                return Ok::<(), Error>(());
            };
            let installed = home.join(target);
            if installed
                .metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
                && ssh::sha256_file(&installed).ok().as_ref() == Some(&build.sha256)
            {
                // Matching unowned binaries are reused, never adopted.
                return Ok(());
            }
            progress(&format!("Installing {name} on {here}…"));
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", &ssh::install_script(target, &build.sha256)?])
                .env("HOME", home)
                .kill_on_drop(true);
            ssh::install_command(command, &build, target, here, progress).await
        }
        .await;
        if let Err(err) = result {
            // Missing language services must not prevent daemon attachment.
            tracing::warn!("cannot install {name} on {here}: {err}");
            progress(&format!("Cannot install {name}: {err}"));
        }
    }
}

/// Where sandhilld is: `$SANDHILL_DAEMON`, next to the app's own binary
/// (inside the app bundle, or in `target/` during development), then the
/// usual install locations and `PATH`.
pub(crate) fn find() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SANDHILL_DAEMON") {
        return Some(PathBuf::from(path));
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("sandhilld"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".local/bin/sandhilld"));
        candidates.push(home.join(".cargo/bin/sandhilld"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("sandhilld")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// The `PATH` a login shell sets up. Apps opened from the Finder, the Dock
/// or a desktop launcher get the session's `PATH`, which misses Homebrew,
/// conda and the R and Python the user set up in their shell profile.
/// Resolved once.
async fn login_path() -> Option<String> {
    static PATH: OnceCell<Option<String>> = OnceCell::const_new();
    PATH.get_or_init(read_login_path).await.clone()
}

async fn read_login_path() -> Option<String> {
    const MARK: &str = "__SANDHILL_PATH__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let output = Command::new(&shell)
        .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(Duration::from_secs(5), output).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            tracing::warn!("cannot run {shell} to read PATH: {err}");
            return None;
        }
        Err(_) => {
            tracing::warn!("{shell} took too long to start; using the default PATH");
            return None;
        }
    };
    // Profiles can print anything; take what's between the markers.
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find(MARK)? + MARK.len();
    let end = start + text[start..].find(MARK)?;
    let path = text[start..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payloads(dir: &Path, bytes: &[u8]) {
        for name in ["ty", "ruff"] {
            std::fs::write(dir.join(format!("{name}-aarch64-apple-darwin")), bytes).unwrap();
        }
    }

    #[tokio::test]
    async fn bundled_language_servers_install_reuse_upgrade_and_preserve_modified_files() {
        let home = tempfile::tempdir().unwrap();
        let builds = tempfile::tempdir().unwrap();
        payloads(builds.path(), b"first build");
        install_bundled(
            builds.path(),
            home.path(),
            "Darwin",
            "aarch64",
            false,
            &|_| {},
        )
        .await;
        for (name, target) in [("ty", TY_PATH), ("ruff", RUFF_PATH)] {
            assert_eq!(
                std::fs::read(home.path().join(target)).unwrap(),
                b"first build"
            );
            let record = home
                .path()
                .join(format!(".local/share/sandhill/install/{name}.json"));
            let text = std::fs::read_to_string(&record).unwrap();
            assert!(text.contains(&ssh::sha256_file(&home.path().join(target)).unwrap()));
            std::fs::remove_file(record).unwrap();
        }
        // Matching unowned installs remain unowned.
        install_bundled(
            builds.path(),
            home.path(),
            "Darwin",
            "aarch64",
            false,
            &|_| {
                panic!("matching builds must be reused");
            },
        )
        .await;
        for name in ["ty", "ruff"] {
            assert!(
                !home
                    .path()
                    .join(format!(".local/share/sandhill/install/{name}.json"))
                    .exists()
            );
        }
        payloads(builds.path(), b"second build");
        install_bundled(
            builds.path(),
            home.path(),
            "Darwin",
            "aarch64",
            false,
            &|_| {},
        )
        .await;
        for target in [TY_PATH, RUFF_PATH] {
            let hash = sandhill_proto::hex(&sha2::Sha256::digest(b"first build"));
            assert_eq!(
                std::fs::read(home.path().join(format!("{target}.legacy-sha256-{hash}"))).unwrap(),
                b"first build"
            );
            std::fs::write(home.path().join(target), b"user replacement").unwrap();
        }
        payloads(builds.path(), b"third build");
        install_bundled(
            builds.path(),
            home.path(),
            "Darwin",
            "aarch64",
            false,
            &|_| {},
        )
        .await;
        for target in [TY_PATH, RUFF_PATH] {
            assert_eq!(
                std::fs::read(home.path().join(target)).unwrap(),
                b"user replacement"
            );
        }
    }

    /// On Linux the bundled Ark goes where the daemon looks for it, only
    /// when R is there to run it.
    #[tokio::test]
    async fn bundled_ark_installs_on_linux_only_with_r() {
        let home = tempfile::tempdir().unwrap();
        let builds = tempfile::tempdir().unwrap();
        std::fs::write(
            builds.path().join("ark-x86_64-unknown-linux-gnu"),
            b"ark build",
        )
        .unwrap();
        install_bundled(
            builds.path(),
            home.path(),
            "Linux",
            "x86_64",
            false,
            &|_| {},
        )
        .await;
        assert!(!home.path().join(ARK_PATH).exists());
        install_bundled(builds.path(), home.path(), "Linux", "x86_64", true, &|_| {}).await;
        assert_eq!(
            std::fs::read(home.path().join(ARK_PATH)).unwrap(),
            b"ark build"
        );
    }

    #[test]
    fn r_is_found_only_as_an_executable_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        assert!(!r_on(Some(path)));
        std::fs::write(dir.path().join("R"), b"#!/bin/sh\n").unwrap();
        assert!(!r_on(Some(path)));
        std::fs::set_permissions(dir.path().join("R"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert!(r_on(Some(path)));
    }

    use sha2::Digest as _;
}
