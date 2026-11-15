//! Folder trust: a project opened for the first time starts nothing until
//! the user trusts it. Uses a real kernel, shell, Git and pi; each part
//! skips (and says so) when its program isn't installed.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, DaemonOptions, StateDir, agents};
use sandhill_proto::client::Client;
use sandhill_proto::{
    AgentOptions, Call, JobSpec, ProjectInfo, SessionInfo, TERMINAL_SHELL, TerminalInfo,
    TrustedFolder,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

async fn start_daemon(state: &Path) -> (Arc<Daemon>, PathBuf) {
    std::fs::create_dir_all(state).unwrap();
    let socket = state.join("test.sock");
    let options = DaemonOptions {
        executable: Path::new(env!("CARGO_BIN_EXE_sandhilld")).into(),
        socket: socket.clone(),
        started_by: String::new(),
    };
    let daemon = Daemon::open_with(StateDir::new(state), options)
        .await
        .unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    (daemon, socket)
}

fn refused<T: std::fmt::Debug>(result: sandhill_proto::client::Result<T>) {
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("Restricted Mode:"), "{error}");
}

fn shell(folder: &str) -> Call {
    Call::StartTerminal {
        kind: TERMINAL_SHELL.into(),
        cwd: None,
        project: Some(folder.into()),
        cols: Some(80),
        rows: Some(24),
        options: AgentOptions::default(),
        prompt: None,
    }
}

fn job(folder: &str) -> Call {
    Call::StartJob {
        job: JobSpec {
            command: Some("true".into()),
            ..JobSpec::default()
        },
        cwd: None,
        project: Some(folder.into()),
    }
}

fn session(kernel: &str, cwd: &str, project: &str) -> Call {
    Call::StartSession {
        kernel: kernel.into(),
        cwd: Some(cwd.into()),
        project: Some(project.into()),
    }
}

#[tokio::test]
async fn an_untrusted_project_starts_nothing_until_it_is_trusted() {
    let dir = common::tempdir().unwrap();
    let (_daemon, socket) = start_daemon(&dir.path().join("state")).await;
    let client = Client::connect_unix(&socket).await.unwrap();
    let project = dir.path().join("atlas");
    let inner = project.join("analysis");
    std::fs::create_dir_all(&inner).unwrap();
    let folder = project.to_string_lossy().into_owned();
    let inside = inner.to_string_lossy().into_owned();
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .is_ok_and(|s| s.success());
    let kernel = sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_ok();
    if !kernel {
        eprintln!("skipping the kernel part: Jupyter kernel \"python3\" is not installed");
    }
    if !git {
        eprintln!("skipping the Git part: git is not installed");
    }

    let opened: ProjectInfo = client.open_project(&folder).await.unwrap();
    assert!(!opened.trusted);
    refused(client.call::<TerminalInfo>(shell(&folder)).await);
    refused(client.call::<TerminalInfo>(shell(&inside)).await);
    refused(client.call::<TerminalInfo>(job(&folder)).await);
    refused(
        client
            .call::<Value>(Call::GitStatus {
                path: inside.clone(),
            })
            .await,
    );
    refused(
        client
            .call::<SessionInfo>(session("python3", &inside, &folder))
            .await,
    );
    assert!(client.list_sessions().await.unwrap().is_empty());
    assert!(
        client
            .call::<Vec<TerminalInfo>>(Call::ListTerminals)
            .await
            .unwrap()
            .is_empty()
    );

    client.trust_folder(&folder).await.unwrap();
    assert!(client.open_project(&folder).await.unwrap().trusted);
    let listed: Vec<TrustedFolder> = client.list_trusted_folders().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].exists);
    let terminal: TerminalInfo = client.call(shell(&inside)).await.unwrap();
    client
        .call::<Value>(Call::CloseTerminal {
            terminal: terminal.id,
        })
        .await
        .unwrap();
    if git {
        client
            .call::<Value>(Call::GitStatus {
                path: inside.clone(),
            })
            .await
            .unwrap();
    }
    if kernel {
        let started: SessionInfo =
            tokio::time::timeout(TIMEOUT, client.call(session("python3", &inside, &folder)))
                .await
                .unwrap()
                .unwrap();
        client
            .call::<Value>(Call::Shutdown {
                session: started.id,
            })
            .await
            .unwrap();
    }

    // No longer trusted: asked again, and nothing new starts.
    client.untrust_folder(&folder).await.unwrap();
    assert!(!client.open_project(&folder).await.unwrap().trusted);
    refused(client.call::<TerminalInfo>(shell(&folder)).await);
}

/// Sandhill's per-run extension answers pi's own question about a
/// project's `.pi` files for a folder trusted in Sandhill, and saves nothing
/// in pi's trust.
#[tokio::test]
async fn pi_loads_a_trusted_folders_own_files_without_asking() {
    let Some(pi) = agents::find("pi") else {
        eprintln!("skipping: pi is not installed");
        return;
    };
    let dir = common::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("proj");
    let other = dir.path().join("other");
    for folder in [&home, &other, &project.join(".pi/extensions")] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(
        project.join(".pi/extensions/local.js"),
        "export default function (pi) {\n  pi.registerCommand(\"sandhill-local-probe\", { description: \"probe\", handler: async () => {} });\n}\n",
    )
    .unwrap();
    let extension = sandhill_daemon::terminal::pi::write_extension(dir.path()).unwrap();

    let loads = async |trusted: &Path| -> bool {
        let mut command = tokio::process::Command::new(&pi);
        command
            .args(["--mode", "rpc", "--no-session", "-e"])
            .arg(&extension)
            .current_dir(&project)
            .env("HOME", &home)
            .env("PI_OFFLINE", "1")
            .env("SANDHILL_TRUSTED_FOLDER", trusted)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // pi runs under the `node` beside it, which may not be on PATH.
        let mut path: Vec<PathBuf> = pi.parent().map(PathBuf::from).into_iter().collect();
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command.env("PATH", std::env::join_paths(path).unwrap());
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"{\"id\":\"1\",\"type\":\"get_commands\"}\n")
            .await
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(TIMEOUT, async {
            while let Some(line) = lines.next_line().await.unwrap() {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message["id"] == "1" {
                    return line.contains("sandhill-local-probe");
                }
            }
            panic!("pi ended without answering");
        })
        .await
        .unwrap()
    };

    assert!(loads(&project).await);
    // Trust in another folder says nothing about this one.
    assert!(!loads(&other).await);
    assert!(!home.join(".pi/agent/trust.json").exists());
}

/// Codex reads Sandhill's per-run trust as its own, for paths a dotted key
/// could not name; its configuration file is not written.
#[tokio::test]
async fn codex_takes_a_trusted_folder_for_one_run() {
    let Some(codex) = agents::find("codex") else {
        eprintln!("skipping: Codex is not installed");
        return;
    };
    let dir = common::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    let project = dir.path().join("my.proj v2");
    let inner = project.join("analysis");
    for folder in [&home, &inner] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let (folder, cwd) = (project.to_string_lossy(), inner.to_string_lossy());
    let mut child = tokio::process::Command::new(&codex)
        .arg("-c")
        .arg(sandhill_daemon::terminal::codex::trust_override(
            &folder, &cwd,
        ))
        .arg("app-server")
        .current_dir(&inner)
        .env("CODEX_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "sandhill-test", "version": "0"}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "config/read", "params": {"includeLayers": false, "cwd": cwd}}),
    ] {
        stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let reply = tokio::time::timeout(TIMEOUT, async {
        while let Some(line) = lines.next_line().await.unwrap() {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == 2 {
                return message;
            }
        }
        panic!("Codex ended without answering");
    })
    .await
    .unwrap();
    if reply["result"]["config"].is_null() {
        eprintln!("skipping: this Codex does not report its configuration ({reply})");
        return;
    }
    let projects = &reply["result"]["config"]["projects"];
    for path in [folder.as_ref(), cwd.as_ref()] {
        assert_eq!(projects[path]["trust_level"], "trusted", "{projects}");
    }
    assert!(!home.join("config.toml").exists());
}
