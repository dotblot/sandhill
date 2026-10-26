//! Reaching sandhilld, on this machine or on a server over ssh.
//!
//! Either way the app runs `sandhilld attach --start` and speaks the wire
//! protocol over its stdin and stdout: on this machine as a child process,
//! on a server through the system `ssh`, so `~/.ssh/config`, keys, the agent
//! and jump hosts work as they do in a terminal. Before attaching to a
//! server, the link installs the app's own build of sandhilld there if the
//! server has none or another build.

pub mod forward;
mod local;
mod ssh;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sandhill_proto::client::Client;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

pub use ssh::Destination;

/// The host name for the daemon on this machine.
pub const LOCAL: &str = "local";

/// Where a daemon runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Local,
    Ssh(Destination),
}

impl Target {
    /// `local`, or an ssh destination such as `lab` (a `Host` in
    /// `~/.ssh/config`) or `huan@10.0.0.5`.
    pub fn parse(host: &str) -> Result<Target, Error> {
        if host == LOCAL {
            Ok(Target::Local)
        } else {
            Destination::parse(host).map(Target::Ssh)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Builds of sandhilld to install on servers, named
    /// `sandhilld-<target triple>`, such as `sandhilld-x86_64-unknown-linux-musl`.
    pub builds: Option<PathBuf>,
    /// The ssh program; `ssh` from `PATH` if not set.
    pub ssh: Option<PathBuf>,
}

/// A connected daemon.
pub struct Connection {
    pub client: Client,
    /// The `attach` (or `ssh`) process; dropping it ends the connection.
    pub child: Child,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0:?} is not a server name; use a Host from ~/.ssh/config or user@host")]
    BadHost(String),
    #[error(
        "sandhilld was not found next to the app, in ~/.local/bin or on PATH; build it with `cargo build -p sandhill-daemon` or set SANDHILL_DAEMON"
    )]
    NotFound,
    #[error("sandhilld is not installed on {host}, and this app has no build of it for {platform}")]
    NoBuild { host: String, platform: String },
    /// What ssh or sandhilld said when connecting failed.
    #[error("{0}")]
    Failed(String),
}

/// Connects to the daemon on `target`, starting it if it isn't running.
/// `progress` hears about slow steps, such as installing sandhilld.
pub async fn connect(
    target: &Target,
    options: &Options,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Connection, Error> {
    match target {
        Target::Local => local::connect().await,
        Target::Ssh(destination) => ssh::connect(destination, options, progress).await,
    }
}

/// Runs `command`, which bridges its stdin and stdout to a daemon, and waits
/// for the daemon to answer. If it doesn't, the error is the last thing the
/// command printed (ssh's or sandhilld's own explanation) when there is one.
async fn attach(
    mut command: Command,
    program: &str,
    timeout: Duration,
) -> Result<Connection, Error> {
    command.stderr(Stdio::piped());
    let (client, mut child) = Client::connect_command(command)
        .await
        .map_err(|err| Error::Failed(format!("cannot run {program}: {err}")))?;
    let last_line = Arc::new(Mutex::new(String::new()));
    let stderr = child.stderr.take().map(|stderr| {
        let last_line = last_line.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!("{line}");
                if worth_reporting(&line) {
                    *last_line.lock().unwrap() = line;
                }
            }
        })
    });

    let error = match tokio::time::timeout(timeout, client.ping()).await {
        Ok(Ok(_)) => return Ok(Connection { client, child }),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!("sandhilld did not answer within {timeout:?}"),
    };
    // Prefer what the command said on its way out over "connection closed".
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    if let Some(task) = stderr {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }
    let said = last_line.lock().unwrap().clone();
    Err(Error::Failed(if said.is_empty() {
        error
    } else {
        said.trim_start_matches("Error: ").to_string()
    }))
}

/// Skips blank lines and ssh's notes that aren't the reason for a failure.
fn worth_reporting(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && !line.starts_with("Warning: Permanently added")
}
