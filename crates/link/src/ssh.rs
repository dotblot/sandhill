//! Servers, reached with the system `ssh`.
//!
//! Connecting takes up to four ssh sessions. The first runs a short script
//! that reports the server's OS and CPU, the `PATH` its login shell sets up,
//! whether R is on it, and the sandhilld and Ark it has. If sandhilld is
//! missing or another build than the app's, the next copies the app's build
//! to `~/.local/bin/sandhilld`; if the server has R, the same goes for Ark,
//! the R kernel, at [`ARK_PATH`]. The last runs `sandhilld attach --start`,
//! whose stdin and stdout carry the protocol for as long as the connection
//! lasts.
//!
//! Commands are written for any login shell (sh, bash, zsh, fish, csh): what
//! ssh hands to it is a single `sh -c "..."` with nothing the outer shell
//! would expand.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use sandhill_proto::ARK_PATH;

use crate::{Connection, Error, Options};

/// Where sandhilld goes on a server, relative to the home directory (ssh
/// starts commands there).
const INSTALL_DIR: &str = ".local/bin";
/// Checking a server includes starting its login shell once.
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// A build is about 10 MB; this allows for a slow link.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// ssh, then starting the daemon on the server.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(45);

/// An ssh destination as you would type it after `ssh`: a `Host` from
/// `~/.ssh/config`, a host name, or `user@host`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Destination(String);

impl Destination {
    pub fn parse(host: &str) -> Result<Destination, Error> {
        // No spaces, quotes or leading dash: it must not read as an option.
        let ok = !host.is_empty()
            && host.len() <= 255
            && !host.starts_with('-')
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-@[]:%".contains(c));
        if ok {
            Ok(Destination(host.to_string()))
        } else {
            Err(Error::BadHost(host.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) async fn connect(
    destination: &Destination,
    options: &Options,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Connection, Error> {
    let server = probe(destination, options).await?;
    let build = match &options.builds {
        Some(dir) => find_build(dir, &server, "sandhilld")?,
        None => None,
    };
    let program = match (&build, &server.installed) {
        (Some(build), Some(installed)) if installed.sha256 == build.sha256 => {
            installed.path.clone()
        }
        (Some(build), _) => {
            progress(&format!("Installing sandhilld on {destination}…"));
            install(
                destination,
                options,
                build,
                &format!("{INSTALL_DIR}/sandhilld"),
            )
            .await?;
            format!("{INSTALL_DIR}/sandhilld")
        }
        (None, Some(installed)) => installed.path.clone(),
        (None, None) => {
            return Err(Error::NoBuild {
                host: destination.to_string(),
                platform: format!("{} {}", server.os, server.arch),
            });
        }
    };
    if server.r
        && let Some(dir) = &options.builds
        && let Some(ark) = find_build(dir, &server, "ark")?
        && server.ark.as_ref() != Some(&ark.sha256)
    {
        progress(&format!("Installing the R kernel on {destination}…"));
        // A server without Ark still works, just without that R kernel.
        if let Err(err) = install(destination, options, &ark, ARK_PATH).await {
            tracing::warn!("cannot install Ark on {destination}: {err}");
        }
    }
    let mut command = ssh(destination, options);
    command.arg(attach_command(&program, server.path.as_deref()));
    crate::attach(command, "ssh", ANSWER_TIMEOUT).await
}

/// `ssh` for running one command on `destination`, without prompts: the
/// app has no terminal to ask in. A server seen for the first time is
/// added to `known_hosts`; one whose key changed is refused, as ssh does.
fn ssh(destination: &Destination, options: &Options) -> Command {
    let mut command = Command::new(options.ssh.as_deref().unwrap_or(Path::new("ssh")));
    command.arg("-T");
    for option in [
        "BatchMode=yes",
        "StrictHostKeyChecking=accept-new",
        "ConnectTimeout=15",
        // Notice a dead link (a laptop that slept) within a minute.
        "ServerAliveInterval=15",
        "ServerAliveCountMax=4",
    ] {
        command.arg("-o").arg(option);
    }
    command.arg("--").arg(destination.as_str());
    command.stdin(Stdio::null()).kill_on_drop(true);
    command
}

/// What the probe learned about a server.
#[derive(Debug, Default, PartialEq)]
struct Server {
    /// `uname -s` and `uname -m`.
    os: String,
    arch: String,
    /// The `PATH` an interactive login shell sets up (where conda, R and
    /// the user's tools are), which the daemon passes on to kernels.
    path: Option<String>,
    installed: Option<Installed>,
    /// Whether R is on that `PATH`.
    r: bool,
    /// The sha256 of the Ark the app installed, if there is one.
    ark: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Installed {
    path: String,
    sha256: String,
}

/// Runs on the server with `sh -s`. Prints `key=value` lines. The login
/// shell's output is cut down to its environment after a marker, because
/// shell profiles can print anything. `timeout` guards against profiles
/// that wait for input or start tmux.
const PROBE: &str = r#"
echo "os=$(uname -s)"
echo "arch=$(uname -m)"
mark=__SANDHILL_ENV__
t=
command -v timeout >/dev/null 2>&1 && t="timeout 10"
p=$($t "${SHELL:-/bin/sh}" -ilc "echo $mark; env" </dev/null 2>/dev/null | sed -n "/^$mark\$/,\$ s/^PATH=//p" | head -n 1)
[ -n "$p" ] && echo "path=$p"
d="$HOME/.local/bin/sandhilld"
[ -x "$d" ] || d=$(PATH="${p:-$PATH}"; command -v sandhilld 2>/dev/null)
if [ -n "$d" ] && [ -x "$d" ]; then
  h=$(sha256sum "$d" 2>/dev/null || shasum -a 256 "$d" 2>/dev/null)
  echo "installed=${h%% *} $d"
fi
(PATH="${p:-$PATH}"; command -v R >/dev/null 2>&1) && echo "r=yes"
a="$HOME/.local/share/sandhill/bin/ark"
if [ -x "$a" ]; then
  h=$(sha256sum "$a" 2>/dev/null || shasum -a 256 "$a" 2>/dev/null)
  echo "ark=${h%% *}"
fi
"#;

async fn probe(destination: &Destination, options: &Options) -> Result<Server, Error> {
    let mut command = ssh(destination, options);
    command
        .args(["sh", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| Error::Failed(format!("cannot run ssh: {err}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // If ssh fails early, the write fails too; its exit says why.
        let _ = stdin.write_all(PROBE.as_bytes()).await;
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            Error::Failed(format!(
                "{destination} did not answer within {PROBE_TIMEOUT:?}"
            ))
        })?
        .map_err(|err| Error::Failed(format!("ssh failed: {err}")))?;
    if !output.status.success() {
        return Err(failure(&output.stderr, "ssh", output.status));
    }
    Ok(parse_probe(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_probe(text: &str) -> Server {
    let mut server = Server::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "os" => server.os = value.trim().to_string(),
            "arch" => server.arch = value.trim().to_string(),
            "path" => server.path = Some(value.to_string()).filter(|p| !p.is_empty()),
            "r" => server.r = value.trim() == "yes",
            "ark" => server.ark = Some(value.trim().to_string()).filter(|h| h.len() == 64),
            "installed" => {
                server.installed = value.split_once(' ').and_then(|(sha256, path)| {
                    (sha256.len() == 64).then(|| Installed {
                        path: path.to_string(),
                        sha256: sha256.to_string(),
                    })
                })
            }
            _ => {}
        }
    }
    server
}

/// Rust target triples that run on a server, best first.
fn triples(os: &str, arch: &str) -> &'static [&'static str] {
    match (os, arch) {
        ("Linux", "x86_64") => &["x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu"],
        ("Linux", "aarch64" | "arm64") => {
            &["aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"]
        }
        ("Darwin", "arm64") => &["aarch64-apple-darwin"],
        _ => &[],
    }
}

struct Build {
    path: PathBuf,
    sha256: String,
}

/// The app's build of `program` (sandhilld or ark) for `server`, if it has
/// one.
fn find_build(dir: &Path, server: &Server, program: &str) -> Result<Option<Build>, Error> {
    let Some(path) = triples(&server.os, &server.arch)
        .iter()
        .map(|triple| dir.join(format!("{program}-{triple}")))
        .find(|path| path.is_file())
    else {
        return Ok(None);
    };
    let sha256 = sha256_file(&path)
        .map_err(|err| Error::Failed(format!("cannot read {}: {err}", path.display())))?;
    Ok(Some(Build { path, sha256 }))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copies `build` to `target` (relative to the home directory) on the
/// server through ssh's stdin. The new file replaces the old one in a single
/// rename, so a daemon or kernel that is running keeps working; for
/// sandhilld, `attach --start` then replaces it with the new build.
async fn install(
    destination: &Destination,
    options: &Options,
    build: &Build,
    target: &str,
) -> Result<(), Error> {
    let (dir, name) = target.rsplit_once('/').unwrap_or((".", target));
    let new = format!("{dir}/.{name}.new");
    let script = format!(
        "mkdir -p {dir} && cat > {new} && chmod 755 {new} && mv -f {new} {target} \
         && (sha256sum {target} || shasum -a 256 {target})"
    );
    let mut command = ssh(destination, options);
    command
        .arg(format!("sh -c \"{script}\""))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| Error::Failed(format!("cannot run ssh: {err}")))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let copy = async move {
        let mut file = tokio::fs::File::open(&build.path).await?;
        tokio::io::copy(&mut file, &mut stdin).await?;
        stdin.shutdown().await
    };
    let (copied, output) = tokio::time::timeout(INSTALL_TIMEOUT, async {
        let copied = copy.await;
        (copied, child.wait_with_output().await)
    })
    .await
    .map_err(|_| Error::Failed(format!("copying {name} to {destination} took too long")))?;
    let output = output.map_err(|err| Error::Failed(format!("ssh failed: {err}")))?;
    if !output.status.success() {
        let failed = failure(&output.stderr, "ssh", output.status);
        return Err(Error::Failed(format!(
            "cannot install {name} on {destination}: {failed}"
        )));
    }
    if let Err(err) = copied {
        return Err(Error::Failed(format!(
            "cannot copy {name} to {destination}: {err}"
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.split_whitespace().next() != Some(build.sha256.as_str()) {
        return Err(Error::Failed(format!(
            "the copy of {name} on {destination} does not match this app's"
        )));
    }
    Ok(())
}

/// The command that attaches to the server's daemon, starting it with the
/// login shell's `PATH` when there is one that can be quoted safely.
fn attach_command(program: &str, path: Option<&str>) -> String {
    let mut script = String::new();
    if let Some(path) = path.filter(|p| quotable(p)) {
        script.push_str(&format!("export PATH='{path}'; "));
    }
    let program = if quotable(program) {
        program
    } else {
        "sandhilld"
    };
    script.push_str(&format!("exec '{program}' attach --start"));
    format!("sh -c \"{script}\"")
}

/// Whether `text` can sit in single quotes inside the double-quoted
/// `sh -c "..."` without any shell along the way changing it.
fn quotable(text: &str) -> bool {
    !text.is_empty()
        && !text
            .chars()
            .any(|c| matches!(c, '\'' | '"' | '$' | '`' | '\\' | '!') || c.is_control())
}

/// The reason a command failed: the last line it printed to stderr.
fn failure(stderr: &[u8], program: &str, status: std::process::ExitStatus) -> Error {
    let text = String::from_utf8_lossy(stderr);
    match text.lines().rev().find(|line| crate::worth_reporting(line)) {
        Some(line) => Error::Failed(line.trim().to_string()),
        None => Error::Failed(format!("{program} failed ({status})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_are_hosts_not_options() {
        for ok in ["lab", "huan@10.0.0.5", "gpu-01.example.org", "me@[::1]"] {
            assert!(Destination::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-oProxyCommand=touch x",
            "lab server",
            "lab;rm",
            "a'b",
            "local\n",
        ] {
            assert!(Destination::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn probe_output_is_read_line_by_line() {
        let hash = "a".repeat(64);
        let text = format!(
            "os=Linux\narch=x86_64\npath=/opt/conda/bin:/usr/bin\ninstalled={hash} /home/me/.local/bin/sandhilld\nr=yes\nark={hash}\n"
        );
        assert_eq!(
            parse_probe(&text),
            Server {
                os: "Linux".into(),
                arch: "x86_64".into(),
                path: Some("/opt/conda/bin:/usr/bin".into()),
                installed: Some(Installed {
                    path: "/home/me/.local/bin/sandhilld".into(),
                    sha256: hash.clone(),
                }),
                r: true,
                ark: Some(hash),
            }
        );
        // Nothing installed, and a shell that gave no PATH.
        assert_eq!(
            parse_probe("os=Linux\narch=aarch64\n"),
            Server {
                os: "Linux".into(),
                arch: "aarch64".into(),
                ..Server::default()
            }
        );
    }

    #[test]
    fn the_probe_looks_for_ark_where_it_is_installed() {
        assert!(PROBE.contains(&format!("$HOME/{ARK_PATH}")));
    }

    #[test]
    fn servers_get_static_linux_builds_first() {
        assert_eq!(triples("Linux", "x86_64")[0], "x86_64-unknown-linux-musl");
        assert_eq!(triples("Linux", "aarch64")[0], "aarch64-unknown-linux-musl");
        assert!(triples("FreeBSD", "amd64").is_empty());
    }

    #[test]
    fn attach_quotes_what_it_can() {
        assert_eq!(
            attach_command(".local/bin/sandhilld", Some("/opt/conda/bin:/usr/bin")),
            "sh -c \"export PATH='/opt/conda/bin:/usr/bin'; exec '.local/bin/sandhilld' attach --start\""
        );
        // A PATH that can't be quoted safely is left out; the daemon then
        // gets ssh's default PATH.
        assert_eq!(
            attach_command(".local/bin/sandhilld", Some("/a:$HOME/bin")),
            "sh -c \"exec '.local/bin/sandhilld' attach --start\""
        );
    }
}
