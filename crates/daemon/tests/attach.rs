//! `sandhilld attach`, the stdio bridge the app and ssh use.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use sandhill_proto::{Call, LogTail, SessionInfo, SessionStatus};
use tokio::net::UnixStream;
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

async fn wait_for_socket(socket: &Path) {
    for _ in 0..100 {
        if UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sandhilld did not start");
}

/// The pid of the daemon listening on `socket`.
async fn daemon_pid(socket: &Path) -> i32 {
    let stream = UnixStream::connect(socket).await.unwrap();
    stream.peer_cred().unwrap().pid().unwrap()
}

#[tokio::test]
async fn attach_ends_when_the_daemon_stops() {
    let state = common::tempdir().unwrap();
    let mut serve = Command::new(DAEMON)
        .arg("--state-dir")
        .arg(state.path())
        .arg("serve")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_socket(&state.path().join("sandhilld.sock")).await;

    let mut attach = Command::new(DAEMON);
    attach.arg("--state-dir").arg(state.path()).arg("attach");
    let (mut client, mut child) = Client::connect_command(attach).await.unwrap();
    client.ping().await.unwrap();

    serve.kill().await.unwrap();
    // The client must notice, with its stdin still open, so it can reconnect.
    let closed = tokio::time::timeout(Duration::from_secs(10), client.next_event()).await;
    assert!(
        matches!(closed, Ok(None)),
        "the connection stayed open after the daemon stopped"
    );
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("attach kept running after the daemon stopped")
        .unwrap();
    assert!(status.success(), "{status}");
}

#[tokio::test]
async fn attach_start_replaces_a_daemon_of_another_build() {
    let dir = common::tempdir().unwrap();
    // Another build: the same program with a byte appended hashes differently.
    let other = dir.path().join("sandhilld-other");
    let mut program = std::fs::read(DAEMON).unwrap();
    program.push(0);
    std::fs::write(&other, program).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let state = dir.path().join("state");
    let socket = state.join("sandhilld.sock");
    let mut old = Command::new(&other)
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_socket(&socket).await;
    let old_pid = daemon_pid(&socket).await;
    let old_build = Client::connect_unix(&socket)
        .await
        .unwrap()
        .ping()
        .await
        .unwrap()
        .build;
    // Reap it as soon as it exits, as launchd would; a zombie looks alive.
    let old_exit = tokio::spawn(async move { old.wait().await });

    let attach = || {
        let mut attach = Command::new(DAEMON);
        attach
            .arg("--state-dir")
            .arg(&state)
            .args(["attach", "--start", "--confirm-exit"]);
        attach
    };
    let (client, _bridge) = Client::connect_command(attach()).await.unwrap();
    let reply = client.ping().await.unwrap();
    assert!(!reply.build.is_empty());
    assert_ne!(reply.build, old_build);
    let status = tokio::time::timeout(Duration::from_secs(15), old_exit)
        .await
        .expect("the other build kept running")
        .unwrap()
        .unwrap();
    assert!(
        status.success(),
        "the other build did not stop cleanly: {status}"
    );
    let new_pid = daemon_pid(&socket).await;
    assert_ne!(new_pid, old_pid);

    // The same build stays.
    let (again, _bridge) = Client::connect_command(attach()).await.unwrap();
    again.ping().await.unwrap();
    assert_eq!(daemon_pid(&socket).await, new_pid);

    // SAFETY: plain syscall on the daemon this test started.
    unsafe { libc::kill(new_pid, libc::SIGTERM) };
}

#[tokio::test]
async fn a_daemon_says_how_it_runs_shows_its_log_and_stops_when_asked() {
    let state = common::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    let attach = || {
        let mut attach = Command::new(DAEMON);
        attach
            .arg("--state-dir")
            .arg(state.path())
            .args(["attach", "--start", "--confirm-exit"]);
        attach
    };
    let (client, _bridge) = Client::connect_command(attach()).await.unwrap();
    let reply = client.ping().await.unwrap();
    assert_eq!(reply.started_by, "app");
    assert!(!reply.user.is_empty());
    let now = sandhill_daemon::journal::now_ms();
    assert!(
        reply.started_ms > now - 60_000 && reply.started_ms <= now,
        "{reply:?}"
    );
    let log: LogTail = client.call(Call::ReadLog { bytes: None }).await.unwrap();
    assert!(log.text.contains("sandhilld listening on"), "{log:?}");

    // A kernel keeps running while the daemon restarts.
    let kernel = sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_ok();
    let session = if kernel {
        let info: SessionInfo = client
            .call(Call::StartSession {
                kernel: "python3".into(),
                cwd: Some(state.path().display().to_string()),
                project: None,
            })
            .await
            .unwrap();
        Some(info.id)
    } else {
        eprintln!("skipping the kernel part: Jupyter kernel \"python3\" is not installed");
        None
    };
    let old_pid = daemon_pid(&socket).await;
    let _: serde_json::Value = client.call(Call::StopDaemon).await.unwrap();
    let mut stopped = false;
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_err() {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(stopped, "the daemon kept running");

    let (client, _bridge) = Client::connect_command(attach()).await.unwrap();
    client.ping().await.unwrap();
    assert_ne!(daemon_pid(&socket).await, old_pid);
    if let Some(session) = session {
        let sessions: Vec<SessionInfo> = client.call(Call::ListSessions).await.unwrap();
        let info = sessions.iter().find(|s| s.id == session).unwrap();
        assert_eq!(info.status, SessionStatus::Running, "{info:?}");
        let _: serde_json::Value = client.call(Call::Shutdown { session }).await.unwrap();
    }
    let _: serde_json::Value = client.call(Call::StopDaemon).await.unwrap();
}

/// This is the failed Quit/reopen boundary: a reachable same-build daemon has
/// stopped resources but has not exited. Reopen must replace it, not expose
/// its maintenance rejection to Welcome, even with two simultaneous attaches.
#[tokio::test]
async fn stop_resources_then_concurrent_attach_recovers_same_build() {
    let state = common::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    let mut old = Command::new(DAEMON)
        .arg("--state-dir")
        .arg(state.path())
        .args(["serve", "--started-by", "systemd"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_socket(&socket).await;
    let old_pid = daemon_pid(&socket).await;
    let client = Client::connect_unix(&socket).await.unwrap();
    let before = client.ping().await.unwrap();
    assert_eq!(before.started_by, "systemd");
    let session = if sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_ok()
    {
        Some(
            client
                .start_session("python3", Some(state.path().to_str().unwrap()))
                .await
                .unwrap(),
        )
    } else {
        None
    };
    let stopped: serde_json::Value = client
        .call(Call::StopResources { force: false })
        .await
        .unwrap();
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(client.ping().await.unwrap().lifecycle, "stopped");
    // Paired positive reproduction: this daemon really rejects Welcome work.
    assert!(
        client
            .call::<serde_json::Value>(Call::ListRecentProjects)
            .await
            .is_err()
    );
    let old_exit = tokio::spawn(async move { old.wait().await.unwrap() });

    let attach = || {
        let mut command = Command::new(DAEMON);
        command
            .arg("--state-dir")
            .arg(state.path())
            .args(["attach", "--start", "--confirm-exit"]);
        Client::connect_command(command)
    };
    let (first, second) = tokio::join!(attach(), attach());
    let (first, mut bridge) = first.unwrap();
    let (second, _second_bridge) = second.unwrap();
    let first_ping = first.ping().await.unwrap();
    let second_ping = second.ping().await.unwrap();
    assert_eq!(first_ping.lifecycle, "ready");
    assert_eq!(first_ping.build, before.build);
    assert_eq!(second_ping.started_ms, first_ping.started_ms);
    let new_pid = daemon_pid(&socket).await;
    assert_ne!(new_pid, old_pid);
    first
        .call::<serde_json::Value>(Call::ListRecentProjects)
        .await
        .unwrap();
    second
        .call::<serde_json::Value>(Call::ListRecentProjects)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(20), old_exit)
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    if let Some(session) = session {
        let sessions: Vec<SessionInfo> = first.call(Call::ListSessions).await.unwrap();
        assert_eq!(
            sessions.iter().find(|s| s.id == session.id).unwrap().status,
            SessionStatus::Ended
        );
        assert!(!sandhill_daemon::kernel::pid_alive(session.pid.unwrap()));
    }
    let identity = sandhill_daemon::lifecycle::ProcessIdentity::read(new_pid as u32).unwrap();
    first
        .call::<serde_json::Value>(Call::StopDaemon)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), first.wait_closed())
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(20), bridge.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "{status}");
    assert!(
        !identity.alive(),
        "bridge exited successfully before its daemon"
    );
}

/// A task-owned subprocess emulates only the old wire contract (no lifecycle
/// field/PrepareAttach) or a daemon whose socket closes before its process exits.
/// Nothing is installed and no real service manager or user daemon is touched.
#[test]
fn lifecycle_wire_fixture() {
    use std::io::{BufRead, Write};
    let Some(root) = std::env::var_os("SANDHILL_TEST_LIFECYCLE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mode = std::env::var("SANDHILL_TEST_LIFECYCLE_MODE").unwrap();
    // Legacy StopDaemon used a read guard; destructive maintenance used a
    // write guard. Model that actual boundary rather than just echoing replies.
    let lifecycle = std::sync::Arc::new(std::sync::RwLock::new(()));
    let listener =
        std::os::unix::net::UnixListener::bind(Path::new(&root).join("sandhilld.sock")).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mode = mode.clone();
        let root = root.clone();
        let lifecycle = lifecycle.clone();
        std::thread::spawn(move || {
            let mut lines = std::io::BufReader::new(stream.try_clone().unwrap()).lines();
            while let Some(Ok(line)) = lines.next() {
                let request: sandhill_proto::Request = serde_json::from_str(&line).unwrap();
                if matches!(request.call, Call::Ping)
                    && root.join("maintenance-active").exists()
                    && !root.join("release-maintenance").exists()
                {
                    std::fs::write(root.join("attach-blocked"), b"waiting for lifecycle gate")
                        .unwrap();
                }
                let (_read, _write) = if matches!(request.call, Call::CleanStorage { .. }) {
                    (None, Some(lifecycle.write().unwrap()))
                } else {
                    (Some(lifecycle.read().unwrap()), None)
                };
                let mut stop = false;
                let outcome = match request.call {
                    Call::Ping => {
                        let mut ping = serde_json::json!({
                            "protocol_version":1,"daemon_version":"legacy-fixture",
                            "home":"","hostname":"fixture","build":"legacy-fixture",
                            "user":"fixture","started_ms":1,"started_by":"systemd"
                        });
                        if mode == "unconfirmed" { ping["lifecycle"] = "ready".into(); }
                        if mode == "refusing" { ping["lifecycle"] = "stopped".into(); }
                        sandhill_proto::Outcome::Result(ping)
                    }
                    Call::ListRecentProjects => sandhill_proto::Outcome::Error(
                        "resources are stopped for maintenance; reconnect after an explicit daemon restart".into(),
                    ),
                    Call::ResourceInventory => {
                        let inventory = match std::fs::read_to_string(root.join("retained-pid")) {
                            Ok(pid) => {
                                let pid: u32 = pid.trim().parse().unwrap();
                                let identity =
                                    sandhill_daemon::lifecycle::ProcessIdentity::read(pid).unwrap();
                                assert!(identity.alive(), "retained work exited before inventory");
                                serde_json::json!({
                                    "resources":[{
                                        "id":format!("terminal:{pid}"),"kind":"terminal",
                                        "label":"retained independent work","state":"running","pid":pid
                                    }],
                                    "errors":["legacy terminal host cannot be stopped safely"]
                                })
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                serde_json::json!({"resources":[],"errors":[]})
                            }
                            Err(error) => panic!("cannot read retained work identity: {error}"),
                        };
                        sandhill_proto::Outcome::Result(inventory)
                    }
                    Call::CleanStorage { .. } => {
                        std::fs::write(root.join("maintenance-active"), b"write guard held").unwrap();
                        for _ in 0..3000 {
                            if root.join("release-maintenance").exists() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        assert!(root.join("release-maintenance").exists());
                        sandhill_proto::Outcome::Result(serde_json::Value::Null)
                    }
                    Call::PrepareAttach { .. } if mode == "unconfirmed" => {
                        sandhill_proto::Outcome::Result(serde_json::json!({"restart":false}))
                    }
                    // Builds before 2026-11-11 refused reopening after an
                    // incomplete Stop All.
                    Call::PrepareAttach { .. } if mode == "refusing" => {
                        sandhill_proto::Outcome::Error(
                            "resource shutdown is incomplete; inspect remaining resources before reopening".into(),
                        )
                    }
                    Call::StopDaemon => {
                        stop = true;
                        sandhill_proto::Outcome::Result(serde_json::Value::Null)
                    }
                    other => panic!("unexpected fixture call: {other:?}"),
                };
                let mut bytes = serde_json::to_vec(&sandhill_proto::ServerMessage::Response {
                    id: request.id,
                    outcome,
                })
                .unwrap();
                bytes.push(b'\n');
                stream.write_all(&bytes).unwrap();
                stream.flush().unwrap();
                if stop {
                    std::fs::write(root.join("stop-acknowledged"), b"null").unwrap();
                    if mode == "legacy" || mode == "refusing" {
                        std::process::exit(0);
                    }
                    // Drop this connection, but keep the fixture process alive.
                    return;
                }
            }
        });
    }
}

fn wire_fixture(root: &Path, mode: &str) -> tokio::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lifecycle_wire_fixture", "--nocapture"])
        .env("SANDHILL_TEST_LIFECYCLE_ROOT", root)
        .env("SANDHILL_TEST_LIFECYCLE_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn explicit_reopen_recovers_a_legacy_maintenance_daemon_cooperatively() {
    let state = common::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    // This is a real independent process, not a fixture-reported fake PID.
    // It belongs to the test, not the legacy daemon, and must survive restart.
    let mut retained = Command::new("sleep")
        .arg("120")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let retained_identity =
        sandhill_daemon::lifecycle::ProcessIdentity::read(retained.id().unwrap()).unwrap();
    std::fs::write(
        state.path().join("retained-pid"),
        retained_identity.pid.to_string(),
    )
    .unwrap();
    let mut legacy = wire_fixture(state.path(), "legacy");
    wait_for_socket(&socket).await;
    let identity = sandhill_daemon::lifecycle::ProcessIdentity::read(legacy.id().unwrap()).unwrap();
    let legacy_client = Client::connect_unix(&socket).await.unwrap();
    let inventory: serde_json::Value = legacy_client.call(Call::ResourceInventory).await.unwrap();
    assert_eq!(inventory["resources"][0]["pid"], retained_identity.pid);
    assert_eq!(inventory["errors"].as_array().unwrap().len(), 1);
    let rejection = legacy_client
        .call::<serde_json::Value>(Call::ListRecentProjects)
        .await
        .unwrap_err();
    assert!(
        rejection
            .to_string()
            .contains("resources are stopped for maintenance")
    );
    let maintenance = tokio::spawn(async move {
        legacy_client
            .call::<serde_json::Value>(Call::CleanStorage {
                categories: vec!["cache".into()],
            })
            .await
            .unwrap()
    });
    for _ in 0..100 {
        if state.path().join("maintenance-active").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(state.path().join("maintenance-active").exists());
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(state.path())
        .args(["attach", "--start", "--confirm-exit"]);
    let (client, mut bridge) = Client::connect_command(command).await.unwrap();
    let mut ready = Box::pin(client.ping());
    tokio::select! {
        result = &mut ready => panic!("reopen interrupted active legacy maintenance: {result:?}"),
        _ = async {
            for _ in 0..1000 {
                if state.path().join("attach-blocked").exists() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("attach did not reach the held legacy lifecycle gate");
        } => {}
    }
    assert!(identity.alive());
    assert!(retained_identity.alive());
    assert!(!state.path().join("stop-acknowledged").exists());
    std::fs::write(state.path().join("release-maintenance"), b"complete").unwrap();
    maintenance.await.unwrap();
    let ping = tokio::time::timeout(Duration::from_secs(30), ready)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ping.lifecycle, "ready");
    client
        .call::<serde_json::Value>(Call::ListRecentProjects)
        .await
        .unwrap();
    assert!(!identity.alive());
    assert!(legacy.wait().await.unwrap().success());
    assert!(state.path().join("stop-acknowledged").exists());
    assert!(
        retained_identity.alive() && retained.try_wait().unwrap().is_none(),
        "daemon-only restart killed retained independent work"
    );
    client
        .call::<serde_json::Value>(Call::StopDaemon)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), client.wait_closed())
        .await
        .unwrap();
    assert!(bridge.wait().await.unwrap().success());
    assert!(
        retained_identity.alive(),
        "ordinary daemon shutdown killed retained work"
    );
    // Clean up only the independent subprocess owned by this test.
    retained.kill().await.unwrap();
}

#[tokio::test]
async fn explicit_reopen_replaces_a_daemon_that_refused_after_incomplete_stop() {
    let state = common::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    // Independent work left behind by the incomplete stop; owned by the test.
    let mut retained = Command::new("sleep")
        .arg("120")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let retained_identity =
        sandhill_daemon::lifecycle::ProcessIdentity::read(retained.id().unwrap()).unwrap();
    let mut old = wire_fixture(state.path(), "refusing");
    wait_for_socket(&socket).await;
    let identity = sandhill_daemon::lifecycle::ProcessIdentity::read(old.id().unwrap()).unwrap();
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(state.path())
        .args(["attach", "--start", "--confirm-exit"]);
    let (client, mut bridge) = Client::connect_command(command).await.unwrap();
    let ping = tokio::time::timeout(Duration::from_secs(30), client.ping())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ping.lifecycle, "ready");
    assert!(state.path().join("stop-acknowledged").exists());
    assert!(!identity.alive());
    assert!(old.wait().await.unwrap().success());
    assert!(
        retained_identity.alive(),
        "replacing the daemon stopped remaining work"
    );
    client
        .call::<serde_json::Value>(Call::StopDaemon)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), client.wait_closed())
        .await
        .unwrap();
    assert!(bridge.wait().await.unwrap().success());
    retained.kill().await.unwrap();
}

#[tokio::test]
async fn stop_reply_and_socket_eof_are_not_proof_of_daemon_exit() {
    let state = common::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    let mut fake = wire_fixture(state.path(), "unconfirmed");
    wait_for_socket(&socket).await;
    let identity = sandhill_daemon::lifecycle::ProcessIdentity::read(fake.id().unwrap()).unwrap();
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(state.path())
        .args(["attach", "--start", "--confirm-exit"]);
    let (client, mut bridge) = Client::connect_command(command).await.unwrap();
    client.ping().await.unwrap();
    client
        .call::<serde_json::Value>(Call::StopDaemon)
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(20), bridge.wait())
        .await
        .unwrap()
        .unwrap();
    // Clean only this test-owned fixture before asserting failure behavior.
    let still_alive = identity.alive();
    fake.kill().await.unwrap();
    assert!(
        still_alive,
        "fixture did not exercise an uncompleted shutdown"
    );
    assert!(
        !status.success(),
        "attach mistook a socket close for daemon exit"
    );
}
