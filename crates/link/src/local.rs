//! The daemon on this machine.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::{Connection, Error};

/// Starting the daemon can take a while on first launch (it opens or
/// creates its journal and reattaches to kernels that are still running).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn connect() -> Result<Connection, Error> {
    let program = find().ok_or(Error::NotFound)?;
    let mut command = Command::new(&program);
    if let Some(socket) = std::env::var_os("SANDHILL_SOCKET") {
        command.arg("--socket").arg(socket);
    }
    command.args(["attach", "--start"]);
    // The daemon started from here passes this on to every kernel.
    if let Some(path) = login_path().await {
        command.env("PATH", path);
    }
    if cfg!(target_os = "macos") && std::env::var_os("LANG").is_none() {
        // Apps opened from the Finder get no locale, and R warns about it.
        command.env("LANG", "en_US.UTF-8");
    }

    crate::attach(command, &program.display().to_string(), ANSWER_TIMEOUT).await
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

/// The `PATH` a login shell sets up. Apps opened from the Finder or the Dock
/// get only `/usr/bin:/bin:/usr/sbin:/sbin`, which misses Homebrew, conda and
/// the R and Python the user set up in their shell profile. Resolved once.
async fn login_path() -> Option<String> {
    static PATH: OnceCell<Option<String>> = OnceCell::const_new();
    if !cfg!(target_os = "macos") {
        return None;
    }
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
