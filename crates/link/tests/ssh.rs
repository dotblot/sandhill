//! Connecting to a "server" through a stand-in for ssh that runs commands
//! here, in a fake home directory, as sshd would run them in the user's
//! home. Everything else is real: the probe script, copying the build over
//! ssh's stdin, and `sandhilld attach --start` starting a daemon there.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sandhill_link::{Error, Options, Target, connect};
use sandhill_proto::ARK_PATH;

/// The sandhilld that `cargo test --workspace` builds next to this test.
fn daemon_binary() -> Option<PathBuf> {
    let deps = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let path = deps.parent()?.join("sandhilld");
    path.is_file().then_some(path)
}

fn this_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        other => panic!("no triple for {other:?}"),
    }
}

/// Ark's Linux builds link to glibc.
fn ark_triple() -> String {
    this_triple().replace("-musl", "-gnu")
}

/// What the probe, install and attach scripts run on the server.
const TOOLS: &[&str] = &[
    "sh",
    "env",
    "uname",
    "timeout",
    "sed",
    "head",
    "sha256sum",
    "shasum",
    "mkdir",
    "cat",
    "chmod",
    "mv",
    "rm",
];

struct Server {
    /// Short, so the daemon's socket path stays under the platform limit.
    home: tempfile::TempDir,
    ssh: PathBuf,
}

impl Server {
    fn new() -> Server {
        let home = tempfile::Builder::new()
            .prefix("shs")
            .tempdir_in("/tmp")
            .unwrap();
        let ssh = home.path().join("fake-ssh");
        let script = format!(
            "#!/bin/sh\n\
             # Skips ssh's options and the destination, then runs the command\n\
             # the way sshd does: with the user's shell, in their home.\n\
             while [ \"$1\" != \"--\" ]; do shift; done\n\
             shift 2\n\
             cd '{home}' || exit 1\n\
             PATH='{home}/rbin:{home}/tools'\n\
             exec env -u XDG_STATE_HOME -u SANDHILL_STATE_DIR -u SANDHILL_SOCKET HOME='{home}' SHELL=/bin/sh sh -c \"$*\"\n",
            home = home.path().display()
        );
        std::fs::write(&ssh, script).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        // The server's PATH has only the tools the scripts use, so whether it
        // has R is up to the test, not this machine.
        let tools = home.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        for tool in TOOLS {
            if let Some(found) = std::env::split_paths(&path)
                .map(|dir| dir.join(tool))
                .find(|p| {
                    p.metadata()
                        .is_ok_and(|m| m.is_file() && m.mode() & 0o111 != 0)
                })
            {
                std::os::unix::fs::symlink(found, tools.join(tool)).unwrap();
            }
        }
        Server { home, ssh }
    }

    fn installed(&self) -> PathBuf {
        self.home.path().join(".local/bin/sandhilld")
    }

    fn ark(&self) -> PathBuf {
        self.home.path().join(ARK_PATH)
    }

    /// Puts an `R` on the server's PATH.
    fn add_r(&self) {
        let r = self.home.path().join("rbin/R");
        std::fs::create_dir_all(r.parent().unwrap()).unwrap();
        std::fs::write(&r, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&r, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn options(&self, builds: Option<&Path>) -> Options {
        Options {
            builds: builds.map(Path::to_path_buf),
            ssh: Some(self.ssh.clone()),
        }
    }
}

impl Drop for Server {
    /// Stops the daemon the test started; it would outlive the test.
    fn drop(&mut self) {
        let socket = self
            .home
            .path()
            .join(".local/state/sandhill/sandhilld.sock");
        // Dropped inside the test's runtime, which tokio's stream needs.
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(socket)
            && stream.set_nonblocking(true).is_ok()
            && let Ok(stream) = tokio::net::UnixStream::from_std(stream)
            && let Ok(Some(pid)) = stream.peer_cred().map(|c| c.pid())
        {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }
}

/// A directory of builds holding this machine's sandhilld, named for the
/// "server", which is this machine.
fn builds(daemon: &Path) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        daemon,
        dir.path().join(format!("sandhilld-{}", this_triple())),
    )
    .unwrap();
    dir
}

fn lab() -> Target {
    Target::parse("me@lab").unwrap()
}

#[tokio::test]
async fn installs_the_daemon_on_a_new_server_then_reuses_it() {
    let Some(daemon) = daemon_binary() else {
        eprintln!("skipping: sandhilld is not built (run `cargo test --workspace`)");
        return;
    };
    let server = Server::new();
    let builds = builds(&daemon);
    let options = server.options(Some(builds.path()));
    let said = Mutex::new(Vec::<String>::new());
    let progress = |note: &str| said.lock().unwrap().push(note.to_string());

    let connection = connect(&lab(), &options, &progress)
        .await
        .expect("connects");
    let ping = connection.client.ping().await.unwrap();
    assert_eq!(ping.home, server.home.path().display().to_string());
    assert_eq!(
        std::fs::read(server.installed()).unwrap(),
        std::fs::read(&daemon).unwrap()
    );
    assert_eq!(*said.lock().unwrap(), ["Installing sandhilld on me@lab…"]);
    drop(connection);

    // The second time, the server has this build already.
    said.lock().unwrap().clear();
    let connection = connect(&lab(), &options, &progress)
        .await
        .expect("connects again");
    connection.client.ping().await.unwrap();
    assert!(said.lock().unwrap().is_empty());
}

#[tokio::test]
async fn installs_ark_on_a_server_with_r() {
    let Some(daemon) = daemon_binary() else {
        eprintln!("skipping: sandhilld is not built (run `cargo test --workspace`)");
        return;
    };
    let server = Server::new();
    let builds = builds(&daemon);
    let ark = "#!/bin/sh\n# a stand-in for Ark\n";
    std::fs::write(builds.path().join(format!("ark-{}", ark_triple())), ark).unwrap();
    let options = server.options(Some(builds.path()));
    let said = Mutex::new(Vec::<String>::new());
    let progress = |note: &str| said.lock().unwrap().push(note.to_string());

    // Without R there is no use for Ark.
    let connection = connect(&lab(), &options, &progress)
        .await
        .expect("connects");
    drop(connection);
    assert!(!server.ark().exists());
    assert_eq!(*said.lock().unwrap(), ["Installing sandhilld on me@lab…"]);

    server.add_r();
    said.lock().unwrap().clear();
    let connection = connect(&lab(), &options, &progress)
        .await
        .expect("connects");
    drop(connection);
    assert_eq!(std::fs::read_to_string(server.ark()).unwrap(), ark);
    assert_eq!(
        *said.lock().unwrap(),
        ["Installing the R kernel on me@lab…"]
    );

    // The second time, the server has this Ark already.
    said.lock().unwrap().clear();
    let connection = connect(&lab(), &options, &progress)
        .await
        .expect("connects");
    connection.client.ping().await.unwrap();
    assert!(said.lock().unwrap().is_empty());
}

#[tokio::test]
async fn replaces_another_build() {
    let Some(daemon) = daemon_binary() else {
        eprintln!("skipping: sandhilld is not built (run `cargo test --workspace`)");
        return;
    };
    let server = Server::new();
    std::fs::create_dir_all(server.installed().parent().unwrap()).unwrap();
    std::fs::write(
        server.installed(),
        "#!/bin/sh\necho old build >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(server.installed(), std::fs::Permissions::from_mode(0o755)).unwrap();

    let builds = builds(&daemon);
    let connection = connect(&lab(), &server.options(Some(builds.path())), &|_| {})
        .await
        .expect("connects");
    connection.client.ping().await.unwrap();
    assert_eq!(
        std::fs::read(server.installed()).unwrap(),
        std::fs::read(&daemon).unwrap()
    );
}

#[tokio::test]
async fn uses_what_the_server_has_when_the_app_has_no_build() {
    let Some(daemon) = daemon_binary() else {
        eprintln!("skipping: sandhilld is not built (run `cargo test --workspace`)");
        return;
    };
    let server = Server::new();
    let options = server.options(None);
    let Err(Error::NoBuild { host, .. }) = connect(&lab(), &options, &|_| {}).await else {
        panic!("connected to a server without sandhilld");
    };
    assert_eq!(host, "me@lab");

    std::fs::create_dir_all(server.installed().parent().unwrap()).unwrap();
    std::fs::copy(&daemon, server.installed()).unwrap();
    let connection = connect(&lab(), &options, &|_| {}).await.expect("connects");
    connection.client.ping().await.unwrap();
}

#[tokio::test]
async fn reports_what_ssh_said() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path().join("ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\necho 'Warning: Permanently added lab' >&2\necho 'me@lab: Permission denied (publickey).' >&2\nexit 255\n",
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let options = Options {
        builds: None,
        ssh: Some(ssh),
    };
    let Err(err) = connect(&lab(), &options, &|_| {}).await else {
        panic!("connected through a failing ssh");
    };
    assert_eq!(err.to_string(), "me@lab: Permission denied (publickey).");
}

/// Against a real server, when one is named: `SANDHILL_TEST_SSH=lab`. The
/// server must be able to run this machine's build. Its daemon keeps running.
#[tokio::test]
async fn real_server() {
    let Some(host) = std::env::var("SANDHILL_TEST_SSH").ok() else {
        eprintln!("skipping: set SANDHILL_TEST_SSH to an ssh host to test against");
        return;
    };
    let Some(daemon) = daemon_binary() else {
        eprintln!("skipping: sandhilld is not built (run `cargo test --workspace`)");
        return;
    };
    let builds = builds(&daemon);
    let options = Options {
        builds: Some(builds.path().to_path_buf()),
        ssh: None,
    };
    let said = Mutex::new(Vec::<String>::new());
    let progress = |note: &str| said.lock().unwrap().push(note.to_string());
    let target = Target::parse(&host).unwrap();
    let connection = connect(&target, &options, &progress)
        .await
        .expect("connects");
    let ping = connection.client.ping().await.unwrap();
    eprintln!("{host}: {ping:?}; said {:?}", said.lock().unwrap());
}
