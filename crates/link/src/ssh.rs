//! Servers, reached with the system `ssh`.
//!
//! Connecting takes up to four ssh sessions. The first runs a short script
//! that reports the server's OS and CPU, the `PATH` its login shell sets up,
//! whether R is on it, and the sandhilld and Ark it has. If sandhilld is
//! missing or another build than the app's, the next copies the app's build
//! to `~/.sandhill/bin/sandhilld`; if the server has R, the same goes for Ark,
//! the R kernel, at [`ARK_PATH`]. Bundled ty and Ruff language servers are
//! installed in Sandhill's private bin directory as well. The last runs
//! `sandhilld attach --start --confirm-exit`, whose stdin and stdout carry
//! the protocol for as long as the connection lasts. The required flag rejects
//! older bridges that cannot confirm actual daemon process exit.
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

use sandhill_proto::{
    ARK_PATH, BIN_PATH, DAEMON_PATH, INSTALL_PATH, OLD_DAEMON_LAYOUT, OLD_DAEMON_PATH, RUFF_PATH,
    SANDHILL_DIR, STATE_PATH, TY_PATH,
};

use crate::{Connection, Error, Options};

/// Checking a server includes starting its login shell once.
const PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// A build is about 10 MB; this allows for a slow link.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// ssh, then serializing startup/recovery and confirming the old daemon's exit.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

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
    if server.uninstalled {
        return Err(Error::Failed(format!(
            "Sandhill was uninstalled on {destination}; an explicit reinstall is required before reconnecting"
        )));
    }
    let build = match &options.builds {
        Some(dir) => find_build(dir, &server, "sandhilld")?,
        None => None,
    };
    let program = match (&build, &server.installed) {
        (Some(build), Some(installed)) if server.current && installed.sha256 == build.sha256 => {
            DAEMON_PATH
        }
        (Some(build), _) => {
            progress(&format!("Installing sandhilld on {destination}…"));
            install(destination, options, build, DAEMON_PATH, progress).await?;
            DAEMON_PATH
        }
        (None, Some(_)) if server.current => DAEMON_PATH,
        (None, Some(installed)) => &installed.path,
        (None, None) => {
            return Err(Error::NoBuild {
                host: destination.to_string(),
                platform: format!("{} {}", server.os, server.arch),
            });
        }
    };
    let mut command = ssh(destination, options);
    command.arg(attach_command(program, server.path.as_deref())?);
    let connection = crate::attach(command, "ssh", ANSWER_TIMEOUT).await?;
    if program == DAEMON_PATH && server.legacy {
        remove_old_daemon(destination, options, progress).await;
    }
    // Attach first so startup can move any earlier helper installs before
    // we write into their new folders.
    if server.r
        && let Some(dir) = &options.builds
        && let Some(ark) = find_build(dir, &server, "ark")?
        && server.ark.as_ref() != Some(&ark.sha256)
    {
        progress(&format!("Installing the R kernel on {destination}…"));
        // A server without Ark still works, just without that R kernel.
        if let Err(err) = install(destination, options, &ark, ARK_PATH, progress).await {
            tracing::warn!("cannot install Ark on {destination}: {err}");
        }
    }
    if let Some(dir) = &options.builds {
        for (name, target, installed) in [
            ("ty", TY_PATH, &server.ty),
            ("ruff", RUFF_PATH, &server.ruff),
        ] {
            if let Some(build) = find_build(dir, &server, name)?
                && installed.as_ref() != Some(&build.sha256)
            {
                progress(&format!("Installing {name} on {destination}…"));
                if let Err(err) = install(destination, options, &build, target, progress).await {
                    tracing::warn!("cannot install {name} on {destination}: {err}");
                }
            }
        }
    }
    Ok(connection)
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
    current: bool,
    legacy: bool,
    installed: Option<Installed>,
    /// Whether R is on that `PATH`.
    r: bool,
    /// The sha256 of the Ark the app installed, if there is one.
    ark: Option<String>,
    ty: Option<String>,
    ruff: Option<String>,
    uninstalled: bool,
}

#[derive(Debug, PartialEq)]
struct Installed {
    path: String,
    sha256: String,
}

fn legacy_dir(current: &str) -> &'static str {
    OLD_DAEMON_LAYOUT
        .iter()
        .find_map(|(old, new)| (*new == current).then_some(*old))
        .expect("known Sandhill directory")
}

/// Match daemon startup, including explicitly empty state overrides.
fn state_root_script() -> String {
    let old = legacy_dir(STATE_PATH);
    format!(
        r#"
if [ "${{SANDHILL_STATE_DIR+x}}" = x ]; then
  state="${{SANDHILL_STATE_DIR:-.}}"
else
  state="$HOME/{STATE_PATH}"
  if [ ! -e "$state" ] && [ ! -L "$state" ] && [ -d "$HOME/{old}" ]; then
    state="$HOME/{old}"
  fi
fi
"#
    )
}

/// Runs on the server with `sh -s`. Prints `key=value` lines. The login
/// shell's output is cut down to its environment after a marker, because
/// shell profiles can print anything. `timeout` guards against profiles
/// that wait for input or start tmux.
const PROBE: &str = r#"
if [ -e "$state/uninstalled" ] || [ -L "$state/uninstalled" ]; then echo "uninstalled=yes"; fi
echo "os=$(uname -s)"
echo "arch=$(uname -m)"
mark=__SANDHILL_ENV__
t=
command -v timeout >/dev/null 2>&1 && t="timeout 10"
p=$($t "${SHELL:-/bin/sh}" -ilc "echo $mark; env" </dev/null 2>/dev/null | sed -n "/^$mark\$/,\$ s/^PATH=//p" | head -n 1)
[ -n "$p" ] && echo "path=$p"
[ ! -e "$legacy" ] && [ ! -L "$legacy" ] || echo "legacy=yes"
if [ -f "$d" ] && [ -x "$d" ] && [ ! -L "$d" ]; then
  echo "current=yes"
else
  d=$(PATH="${p:-$PATH}"; command -v sandhilld 2>/dev/null)
fi
if [ -n "$d" ] && [ -x "$d" ]; then
  h=$(sha256sum "$d" 2>/dev/null || shasum -a 256 "$d" 2>/dev/null)
  echo "installed=${h%% *} $d"
fi
(PATH="${p:-$PATH}"; command -v R >/dev/null 2>&1) && echo "r=yes"
a="$bin/ark"
[ -x "$a" ] || a="$old_bin/ark"
if [ -x "$a" ]; then
  h=$(sha256sum "$a" 2>/dev/null || shasum -a 256 "$a" 2>/dev/null)
  echo "ark=${h%% *}"
fi
for tool in ty ruff; do
  a="$bin/$tool"
  [ -x "$a" ] || a="$old_bin/$tool"
  if [ -x "$a" ]; then
    h=$(sha256sum "$a" 2>/dev/null || shasum -a 256 "$a" 2>/dev/null)
    echo "$tool=${h%% *}"
  fi
done
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
        let script = probe_script();
        let _ = stdin.write_all(script.as_bytes()).await;
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

fn probe_script() -> String {
    let state = state_root_script();
    let old_bin = legacy_dir(BIN_PATH);
    format!(
        "{state}\nd=\"$HOME/{DAEMON_PATH}\"\nlegacy=\"$HOME/{OLD_DAEMON_PATH}\"\nbin=\"$HOME/{BIN_PATH}\"\nold_bin=\"$HOME/{old_bin}\"\n{PROBE}"
    )
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
            "uninstalled" => server.uninstalled = value.trim() == "yes",
            "ark" => server.ark = Some(value.trim().to_string()).filter(|h| h.len() == 64),
            "ty" => server.ty = Some(value.trim().to_string()).filter(|h| h.len() == 64),
            "ruff" => server.ruff = Some(value.trim().to_string()).filter(|h| h.len() == 64),
            "current" => server.current = value == "yes",
            "legacy" => server.legacy = value == "yes",
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
        ("Darwin", "arm64" | "aarch64") => &["aarch64-apple-darwin"],
        _ => &[],
    }
}

pub(crate) struct Build {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

/// The app's build of a daemon, kernel or language server for `server`.
fn find_build(dir: &Path, server: &Server, program: &str) -> Result<Option<Build>, Error> {
    find_platform_build(dir, &server.os, &server.arch, program)
}

pub(crate) fn find_platform_build(
    dir: &Path,
    os: &str,
    arch: &str,
    program: &str,
) -> Result<Option<Build>, Error> {
    let Some(path) = triples(os, arch)
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

pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(sandhill_proto::hex(&hasher.finalize()))
}

/// Preserves pre-manifest binaries before replacing them; recorded installs
/// must still match their owned checksum. A matching probed binary is reused
/// without adopting it. Only newly installed bytes receive ownership records.
async fn install(
    destination: &Destination,
    options: &Options,
    build: &Build,
    target: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<(), Error> {
    let script = install_script(target, &build.sha256)?;
    let mut command = ssh(destination, options);
    command.arg(format!("sh -c {}", shell_quote(&script)));
    install_command(command, build, target, &destination.to_string(), progress).await
}

/// Local and SSH installs share the same ownership checks and transfer script.
pub(crate) async fn install_command(
    mut command: Command,
    build: &Build,
    target: &str,
    location: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<(), Error> {
    let name = target.rsplit('/').next().unwrap_or(target);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| Error::Failed(format!("cannot run installer on {location}: {err}")))?;
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
    .map_err(|_| Error::Failed(format!("copying {name} to {location} took too long")))?;
    let output = output.map_err(|err| Error::Failed(format!("installer failed: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    for path in stdout
        .lines()
        .filter_map(|line| line.strip_prefix("backup="))
    {
        progress(&format!(
            "Preserved previous {name} on {location} at {path}"
        ));
    }
    if !output.status.success() {
        let failed = failure(&output.stderr, "installer", output.status);
        return Err(Error::Failed(format!(
            "cannot install {name} on {location}: {failed}"
        )));
    }
    if let Err(err) = copied {
        return Err(Error::Failed(format!(
            "cannot copy {name} to {location}: {err}"
        )));
    }
    if stdout
        .lines()
        .filter(|line| !line.starts_with("backup="))
        .flat_map(str::split_whitespace)
        .next()
        != Some(build.sha256.as_str())
    {
        return Err(Error::Failed(format!(
            "the copy of {name} on {location} does not match this app's"
        )));
    }
    Ok(())
}

/// Manifest paths and fields are shared with the daemon's lifecycle module.
/// Keep paths HOME-relative: a home directory need not be valid JSON text.
fn install_manifest(component: &str, target: &str, sha256: &str) -> String {
    format!(
        "{{\"version\":1,\"component\":\"{component}\",\"path\":\"{target}\",\"sha256\":\"{sha256}\"}}"
    )
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(crate) fn install_script(target: &str, sha256: &str) -> Result<String, Error> {
    let component = match target {
        DAEMON_PATH => "sandhilld",
        ARK_PATH => "ark",
        TY_PATH => "ty",
        RUFF_PATH => "ruff",
        _ => return Err(Error::Failed("unsupported component install path".into())),
    };
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(Error::Failed("invalid component checksum".into()));
    }
    let (dir, name) = target.rsplit_once('/').expect("allowlisted path");
    let manifest = install_manifest(component, target, sha256);
    let state_root = state_root_script();
    let old_bin = legacy_dir(BIN_PATH);
    let old_install = legacy_dir(INSTALL_PATH);
    // A single-quoted outer command prevents the login shell from expanding
    // $HOME, command substitutions or filenames before sh interprets them.
    Ok(format!(
        r#"set -eu
umask 077
cd "$HOME"
{state_root}
[ ! -e "$state/uninstalled" ] && [ ! -L "$state/uninstalled" ] || {{ echo "Sandhill was uninstalled; explicit reinstall required" >&2; exit 1; }}
for p in {SANDHILL_DIR} {dir} {INSTALL_PATH}; do
  [ ! -L "$p" ] || {{ echo "refusing a linked install folder: $p" >&2; exit 1; }}
done
# While a move is postponed, leave any earlier helper with its manifest.
# Creating a replacement now would block its later move.
if [ "{component}" != sandhilld ] && [ ! -f {INSTALL_PATH}/sandhilld.json ] && \
   {{ [ -e {old_bin} ] || [ -L {old_bin} ] || [ -e {old_install} ] || [ -L {old_install} ]; }}; then
  echo "Sandhill will install {component} after its older files have moved." >&2
  exit 1
fi
if [ "{component}" != sandhilld ] && \
   {{ [ -e {old_bin}/{name} ] || [ -L {old_bin}/{name} ] || \
      [ -e {old_install}/{component}.json ] || [ -L {old_install}/{component}.json ]; }}; then
  echo "The older {component} is still in use or has not moved yet. Sandhill left it in place." >&2
  exit 1
fi
mkdir -p {dir} {INSTALL_PATH}
record={INSTALL_PATH}/{component}.json
[ ! -L "$record" ] || {{ echo "refusing symlinked ownership record" >&2; exit 1; }}
[ ! -e "$record" ] || [ -f "$record" ] || {{ echo "refusing non-regular ownership record" >&2; exit 1; }}
lock={INSTALL_PATH}/.{component}.lock
mkdir "$lock" || {{ echo "another component install is in progress" >&2; exit 1; }}
trap 'rmdir "$lock"' EXIT
stage={dir}/.{name}.install.$$
mkdir "$stage" || {{ echo "refusing component staging collision" >&2; exit 1; }}
new="$stage/{name}.new"
next="$lock/record.new"
legacy="$stage/legacy"
trap 'rm -f "$new" "$next" "$legacy"; rmdir "$stage"; rmdir "$lock"' EXIT
trap 'exit 1' HUP INT TERM
hash() {{ h=$(sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"); printf '%s' "${{h%% *}}"; }}
old=
backup=
if [ -e {target} ] || [ -L {target} ]; then
  [ -f {target} ] && [ ! -L {target} ] || {{ echo "refusing non-regular component target" >&2; exit 1; }}
  old=$(hash {target})
  if [ -e "$record" ]; then
    expected=$(printf '{{"version":1,"component":"{component}","path":"{target}","sha256":"%s"}}' "$old")
    [ "$(cat "$record")" = "$expected" ] || {{ echo "refusing to replace a modified {component}; ownership record does not match" >&2; exit 1; }}
  else
    backup={target}.legacy-sha256-"$old"
  fi
else
  [ ! -e "$record" ] || {{ echo "refusing ownership record without component target" >&2; exit 1; }}
fi
(set -C; cat > "$new")
chmod 755 "$new"
[ "$(hash "$new")" = "{sha256}" ] || {{ echo "component transfer checksum mismatch" >&2; exit 1; }}
(set -C; printf '%s\n' '{manifest}' > "$next")
if [ -n "$backup" ]; then
  if [ ! -e "$backup" ] && [ ! -L "$backup" ]; then
    (set -C; cat {target} > "$legacy")
    [ "$(hash "$legacy")" = "$old" ] || {{ echo "legacy component changed during backup" >&2; exit 1; }}
    ln "$legacy" "$backup" || {{ echo "refusing legacy backup collision: $backup" >&2; exit 1; }}
  fi
  [ -f "$backup" ] && [ ! -L "$backup" ] || {{ echo "refusing non-regular legacy backup: $backup" >&2; exit 1; }}
  [ "$(hash "$backup")" = "$old" ] || {{ echo "refusing legacy backup checksum mismatch: $backup" >&2; exit 1; }}
  printf 'backup=%s/%s\n' "$HOME" "$backup"
fi
# Recheck the target after receiving the payload and preserving any legacy bytes.
if [ -n "$old" ]; then
  [ -f {target} ] && [ ! -L {target} ] && [ "$(hash {target})" = "$old" ] || {{ echo "component changed during install" >&2; exit 1; }}
else
  [ ! -e {target} ] && [ ! -L {target} ] || {{ echo "component appeared during install" >&2; exit 1; }}
fi
[ ! -L "$record" ] && {{ [ ! -e "$record" ] || [ -f "$record" ]; }} || {{ echo "ownership record changed during install" >&2; exit 1; }}
if [ -n "$old" ] && [ -z "$backup" ]; then
  [ -f "$record" ] && [ "$(cat "$record")" = "$expected" ] || {{ echo "ownership record changed during install" >&2; exit 1; }}
else
  [ ! -e "$record" ] || {{ echo "ownership record appeared during install" >&2; exit 1; }}
fi
mv -f "$new" {target}
mv -f "$next" "$record"
printf '%s\n' '{sha256}'
"#
    ))
}

async fn remove_old_daemon(
    destination: &Destination,
    options: &Options,
    progress: &(dyn Fn(&str) + Send + Sync),
) {
    let mut command = ssh(destination, options);
    command.arg(format!(
        "sh -c {}",
        shell_quote(&remove_old_daemon_script())
    ));
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => progress(&format!(
            "The older sandhilld on {destination} was left in place: {}",
            failure(
                &output.stderr,
                "checking the older sandhilld",
                output.status
            )
        )),
        _ => progress(&format!(
            "Could not check the older sandhilld on {destination}."
        )),
    }
}

fn remove_old_daemon_script() -> String {
    let old_install = legacy_dir(INSTALL_PATH);
    let old_state = legacy_dir(STATE_PATH);
    let old_share = Path::new(old_install).parent().unwrap();
    let share = old_share.parent().unwrap();
    let bin = Path::new(OLD_DAEMON_PATH).parent().unwrap();
    let local = bin.parent().unwrap();
    let (old_share, share, bin, local) = (
        old_share.display(),
        share.display(),
        bin.display(),
        local.display(),
    );
    format!(
        r#"set -eu
cd "$HOME"
old={OLD_DAEMON_PATH}
[ -e "$old" ] || [ -L "$old" ] || exit 0
refuse() {{ echo "$1" >&2; exit 1; }}
hash() {{ h=$(sha256sum "$1" 2>/dev/null || shasum -a 256 "$1" 2>/dev/null) || return; printf '%s' "${{h%% *}}"; }}
[ ! -e {old_state} ] && [ ! -L {old_state} ] || refuse "The older state folder is still in use"
for p in {local} {bin} {share} {old_share} {old_install} {SANDHILL_DIR} {INSTALL_PATH}; do
  [ ! -L "$p" ] || refuse "An install folder is a link"
done
lock={INSTALL_PATH}/.sandhilld.lock
mkdir "$lock" || refuse "Another install is in progress"
trap 'rmdir "$lock"' EXIT
[ ! -e {old_install}/.sandhilld.lock ] && [ ! -L {old_install}/.sandhilld.lock ] || refuse "Another install is in progress"
[ -f "$old" ] && [ ! -L "$old" ] || refuse "The old path is not a regular file"
checksum=$(hash "$old")
expected=$(printf '{{"version":1,"component":"sandhilld","path":"{OLD_DAEMON_PATH}","sha256":"%s"}}' "$checksum")
record=
for p in {old_install}/sandhilld.json {INSTALL_PATH}/sandhilld.json; do
  if [ -f "$p" ] && [ ! -L "$p" ] && [ "$(cat "$p")" = "$expected" ]; then record="$p"; break; fi
done
[ -n "$record" ] || refuse "No ownership record matches the old file"
[ -f "$old" ] && [ ! -L "$old" ] && [ "$(hash "$old")" = "$checksum" ] && \
  [ -f "$record" ] && [ ! -L "$record" ] && [ "$(cat "$record")" = "$expected" ] || \
  refuse "The old file or its ownership record changed while being checked"
rm "$old"
rm "$record"
for p in {old_install} {old_share} {share} {bin} {local}; do
  rmdir "$p" 2>/dev/null || :
done
"#
    )
}

/// The command that attaches to the server's daemon, starting it with the
/// login shell's `PATH` when there is one that can be quoted safely.
fn attach_command(program: &str, path: Option<&str>) -> Result<String, Error> {
    let mut script = String::new();
    if let Some(path) = path.filter(|p| quotable(p)) {
        script.push_str(&format!("export PATH='{path}'; "));
    }
    if program == DAEMON_PATH {
        // The inner shell expands its own home, even if it has spaces or
        // characters unsuitable for embedding in the outer command.
        script.push_str(&format!(
            "exec ~/{DAEMON_PATH} attach --start --confirm-exit"
        ));
    } else if Path::new(program).is_absolute() && quotable(program) {
        script.push_str(&format!("exec '{program}' attach --start --confirm-exit"));
    } else {
        return Err(Error::Failed(
            "The server reported an unusable path to sandhilld".into(),
        ));
    }
    Ok(format!("sh -c \"{script}\""))
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
            "os=Linux\narch=x86_64\npath=/opt/conda/bin:/usr/bin\ncurrent=yes\nlegacy=yes\ninstalled={hash} /home/me/{DAEMON_PATH}\nr=yes\nark={hash}\n"
        );
        assert_eq!(
            parse_probe(&text),
            Server {
                os: "Linux".into(),
                arch: "x86_64".into(),
                path: Some("/opt/conda/bin:/usr/bin".into()),
                current: true,
                legacy: true,
                installed: Some(Installed {
                    path: format!("/home/me/{DAEMON_PATH}"),
                    sha256: hash.clone(),
                }),
                r: true,
                ark: Some(hash),
                ty: None,
                ruff: None,
                uninstalled: false,
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
        assert!(probe_script().contains(&format!("bin=\"$HOME/{BIN_PATH}\"")));
        assert!(PROBE.contains("a=\"$bin/ark\""));
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
            attach_command(DAEMON_PATH, Some("/opt/conda/bin:/usr/bin")).unwrap(),
            format!(
                "sh -c \"export PATH='/opt/conda/bin:/usr/bin'; exec ~/{DAEMON_PATH} attach --start --confirm-exit\""
            )
        );
        // A PATH that can't be quoted safely is left out; the daemon then
        // gets ssh's default PATH.
        assert_eq!(
            attach_command(DAEMON_PATH, Some("/a:$HOME/bin")).unwrap(),
            format!("sh -c \"exec ~/{DAEMON_PATH} attach --start --confirm-exit\"")
        );
    }

    fn install_in(
        home: &Path,
        target: &str,
        bytes: &[u8],
        checksum: Option<&str>,
    ) -> std::process::Output {
        use std::io::Write as _;
        let checksum = checksum
            .map(str::to_owned)
            .unwrap_or_else(|| sandhill_proto::hex(&Sha256::digest(bytes)));
        let mut child = std::process::Command::new("sh")
            .args(["-c", &install_script(target, &checksum).unwrap()])
            .env("HOME", home)
            .env_remove("SANDHILL_STATE_DIR")
            .env_remove("XDG_STATE_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A refusal may close the pipe before the payload is written.
        let _ = child.stdin.take().unwrap().write_all(bytes);
        child.wait_with_output().unwrap()
    }

    #[test]
    fn installation_records_checksums_and_protects_modified_components() {
        let home = tempfile::tempdir().unwrap();
        for (target, component) in [
            (DAEMON_PATH, "sandhilld"),
            (ARK_PATH, "ark"),
            (TY_PATH, "ty"),
            (RUFF_PATH, "ruff"),
        ] {
            let output = install_in(home.path(), target, b"first build", None);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let hash = sandhill_proto::hex(&Sha256::digest(b"first build"));
            assert_eq!(
                std::fs::read_to_string(
                    home.path()
                        .join(INSTALL_PATH)
                        .join(format!("{component}.json"))
                )
                .unwrap()
                .trim(),
                install_manifest(component, target, &hash)
            );
            assert!(
                install_in(home.path(), target, b"updated build", None)
                    .status
                    .success()
            );
            std::fs::write(home.path().join(target), b"user replacement").unwrap();
            let output = install_in(home.path(), target, b"third build", None);
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("modified"));
            assert_eq!(
                std::fs::read(home.path().join(target)).unwrap(),
                b"user replacement"
            );
            let record = home
                .path()
                .join(INSTALL_PATH)
                .join(format!("{component}.json"));
            assert_eq!(
                std::fs::read_to_string(record).unwrap().trim(),
                install_manifest(
                    component,
                    target,
                    &sandhill_proto::hex(&Sha256::digest(b"updated build"))
                )
            );
        }
    }

    #[test]
    fn installation_preserves_legacy_bytes_before_claiming_only_the_new_build() {
        for (target, component) in [
            (DAEMON_PATH, "sandhilld"),
            (ARK_PATH, "ark"),
            (TY_PATH, "ty"),
            (RUFF_PATH, "ruff"),
        ] {
            let home = tempfile::tempdir().unwrap();
            let path = home.path().join(target);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original = b"legacy installation\0with unknown bytes";
            std::fs::write(&path, original).unwrap();
            let old = sandhill_proto::hex(&Sha256::digest(original));
            let backup = home.path().join(format!("{target}.legacy-sha256-{old}"));
            let output = install_in(home.path(), target, b"app build", None);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(std::fs::read(&backup).unwrap(), original);
            assert_eq!(std::fs::read(&path).unwrap(), b"app build");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!(
                    "backup={}\n{}\n",
                    backup.display(),
                    sandhill_proto::hex(&Sha256::digest(b"app build"))
                )
            );
            let record = home
                .path()
                .join(INSTALL_PATH)
                .join(format!("{component}.json"));
            assert_eq!(
                std::fs::read_to_string(&record).unwrap().trim(),
                install_manifest(
                    component,
                    target,
                    &sandhill_proto::hex(&Sha256::digest(b"app build"))
                )
            );
            let output = install_in(home.path(), target, b"next build", None);
            assert!(output.status.success());
            assert_eq!(std::fs::read(&path).unwrap(), b"next build");
            assert_eq!(std::fs::read(&backup).unwrap(), original);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!("{}\n", sandhill_proto::hex(&Sha256::digest(b"next build")))
            );
        }
    }

    fn make_fifo(path: &Path) {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn installation_reuses_only_an_exact_regular_legacy_backup() {
        for target in [DAEMON_PATH, ARK_PATH, TY_PATH, RUFF_PATH] {
            for collision in ["matching", "different", "symlink", "directory", "fifo"] {
                let home = tempfile::tempdir().unwrap();
                let path = home.path().join(target);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"legacy build").unwrap();
                let old = sandhill_proto::hex(&Sha256::digest(b"legacy build"));
                let backup = home.path().join(format!("{target}.legacy-sha256-{old}"));
                match collision {
                    "matching" => std::fs::write(&backup, b"legacy build").unwrap(),
                    "different" => std::fs::write(&backup, b"unrelated backup").unwrap(),
                    "symlink" => std::os::unix::fs::symlink(&path, &backup).unwrap(),
                    "directory" => std::fs::create_dir(&backup).unwrap(),
                    "fifo" => make_fifo(&backup),
                    _ => unreachable!(),
                }
                let output = install_in(home.path(), target, b"app build", None);
                assert_eq!(
                    output.status.success(),
                    collision == "matching",
                    "{target}, {collision}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if collision == "matching" {
                    assert_eq!(std::fs::read(&path).unwrap(), b"app build");
                    assert_eq!(std::fs::read(&backup).unwrap(), b"legacy build");
                } else {
                    assert_eq!(std::fs::read(&path).unwrap(), b"legacy build");
                    assert!(String::from_utf8_lossy(&output.stderr).contains("legacy backup"));
                    assert!(
                        std::fs::read_dir(home.path().join(INSTALL_PATH))
                            .unwrap()
                            .next()
                            .is_none()
                    );
                    match collision {
                        "different" => {
                            assert_eq!(std::fs::read(&backup).unwrap(), b"unrelated backup");
                        }
                        "symlink" => assert_eq!(std::fs::read_link(&backup).unwrap(), path),
                        "directory" => {
                            assert!(std::fs::read_dir(&backup).unwrap().next().is_none());
                        }
                        "fifo" => {
                            use std::os::unix::fs::FileTypeExt as _;
                            assert!(backup.symlink_metadata().unwrap().file_type().is_fifo());
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
    }

    #[test]
    fn installation_rejects_bad_transfers_without_touching_legacy_bytes() {
        for target in [DAEMON_PATH, ARK_PATH, TY_PATH, RUFF_PATH] {
            let home = tempfile::tempdir().unwrap();
            let path = home.path().join(target);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"legacy build").unwrap();
            let output = install_in(
                home.path(),
                target,
                b"partial transfer",
                Some(&"0".repeat(64)),
            );
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("transfer checksum mismatch"));
            assert_eq!(std::fs::read(&path).unwrap(), b"legacy build");
            let old = sandhill_proto::hex(&Sha256::digest(b"legacy build"));
            assert!(
                !home
                    .path()
                    .join(format!("{target}.legacy-sha256-{old}"))
                    .exists()
            );
            assert!(
                std::fs::read_dir(home.path().join(INSTALL_PATH))
                    .unwrap()
                    .next()
                    .is_none()
            );
        }
    }

    #[test]
    fn installation_refuses_nonregular_targets_and_existing_locks() {
        for target in [DAEMON_PATH, ARK_PATH, TY_PATH, RUFF_PATH] {
            for kind in ["symlink", "directory", "fifo", "lock"] {
                let home = tempfile::tempdir().unwrap();
                let path = home.path().join(target);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                match kind {
                    "symlink" => std::os::unix::fs::symlink("missing", &path).unwrap(),
                    "directory" => std::fs::create_dir(&path).unwrap(),
                    "fifo" => make_fifo(&path),
                    "lock" => {
                        std::fs::write(&path, b"legacy build").unwrap();
                        let component = target.rsplit('/').next().unwrap();
                        std::fs::create_dir_all(
                            home.path()
                                .join(INSTALL_PATH)
                                .join(format!(".{component}.lock")),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                let output = install_in(home.path(), target, b"app build", None);
                assert!(!output.status.success(), "{target}, {kind}");
                assert!(path.symlink_metadata().is_ok());
                if kind == "lock" {
                    assert_eq!(std::fs::read(&path).unwrap(), b"legacy build");
                } else {
                    assert!(
                        String::from_utf8_lossy(&output.stderr).contains("non-regular component")
                    );
                }
            }
        }
    }

    #[test]
    fn installation_refuses_symlinked_directories_and_uninstalled_state() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), home.path().join(SANDHILL_DIR)).unwrap();
        assert!(
            !install_in(home.path(), ARK_PATH, b"build", None)
                .status
                .success()
        );
        assert!(
            !outside
                .path()
                .join(Path::new(BIN_PATH).file_name().unwrap())
                .exists()
        );
        let retired = tempfile::tempdir().unwrap();
        let state = retired.path().join(STATE_PATH);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("uninstalled"), b"retired").unwrap();
        assert!(
            !install_in(retired.path(), DAEMON_PATH, b"build", None)
                .status
                .success()
        );
        assert!(!retired.path().join(DAEMON_PATH).exists());
        assert!(parse_probe("uninstalled=yes\n").uninstalled);
    }

    #[test]
    fn install_script_accepts_only_fixed_paths_and_lowercase_checksums() {
        assert!(install_script("../sandhilld", &"a".repeat(64)).is_err());
        assert!(install_script(ARK_PATH, &"A".repeat(64)).is_err());
        assert!(install_script(ARK_PATH, "bad").is_err());
        assert_eq!(shell_quote("a'b $HOME"), "'a'\\''b $HOME'");
    }
}
