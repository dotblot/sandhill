//! Finding and starting the local sandhilld.
//!
//! The app runs `sandhilld attach --start` and speaks the protocol over its
//! stdin/stdout, which starts the daemon if it isn't running. A remote server
//! will use the same command through ssh.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::client::Client;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::OnceCell;

/// Starting the daemon can take a while on first launch (it opens or
/// creates its journal and reattaches to kernels that are still running).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// The host name the UI uses for the daemon on this Mac.
pub const LOCAL: &str = "local";

/// Where a daemon runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Local,
}

impl Target {
    pub fn parse(host: &str) -> Result<Target> {
        match host {
            LOCAL => Ok(Target::Local),
            _ => bail!("unknown host {host:?}"),
        }
    }
}

pub struct Connection {
    pub client: Client,
    /// The `attach` process; dropping it ends the connection.
    pub child: Child,
}

pub async fn connect(target: &Target) -> Result<Connection> {
    match target {
        Target::Local => connect_local().await,
    }
}

async fn connect_local() -> Result<Connection> {
    let program = find()?;
    let mut command = Command::new(&program);
    if let Some(socket) = std::env::var_os("SANDHILL_SOCKET") {
        command.arg("--socket").arg(socket);
    }
    command.args(["attach", "--start"]).stderr(Stdio::piped());
    // The daemon started from here passes this on to every kernel.
    if let Some(path) = login_path().await {
        command.env("PATH", path);
    }
    if cfg!(target_os = "macos") && std::env::var_os("LANG").is_none() {
        // Apps opened from the Finder get no locale, and R warns about it.
        command.env("LANG", "en_US.UTF-8");
    }

    let (client, mut child) = Client::connect_command(command)
        .await
        .with_context(|| format!("cannot run {}", program.display()))?;
    let last_error = Arc::new(Mutex::new(String::new()));
    let stderr = child.stderr.take().map(|stderr| {
        let last_error = last_error.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!("sandhilld: {line}");
                if !line.trim().is_empty() {
                    *last_error.lock().unwrap() = line;
                }
            }
        })
    });

    let error = match tokio::time::timeout(ANSWER_TIMEOUT, client.ping()).await {
        Ok(Ok(_)) => return Ok(Connection { client, child }),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("sandhilld did not answer within {ANSWER_TIMEOUT:?}"),
    };
    // Prefer what the daemon said on its way out over "connection closed".
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    if let Some(task) = stderr {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }
    let said = last_error.lock().unwrap().clone();
    let said = said.trim_start_matches("Error: ");
    bail!("{}", if said.is_empty() { &error } else { said })
}

/// Where sandhilld is: `$SANDHILL_DAEMON`, next to the app's own binary
/// (inside the app bundle, or in `target/` during development), then the
/// usual install locations and `PATH`.
fn find() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SANDHILL_DAEMON") {
        return Ok(PathBuf::from(path));
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
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .context("sandhilld was not found next to the app, in ~/.local/bin or on PATH; build it with `cargo build -p sandhill-daemon` or set SANDHILL_DAEMON")
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
