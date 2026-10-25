//! `sandhilld attach`, the stdio bridge the app and ssh use.

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
    let state = tempfile::tempdir().unwrap();
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
    let dir = tempfile::tempdir().unwrap();
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
            .args(["attach", "--start"]);
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
    let state = tempfile::tempdir().unwrap();
    let socket = state.path().join("sandhilld.sock");
    let attach = || {
        let mut attach = Command::new(DAEMON);
        attach
            .arg("--state-dir")
            .arg(state.path())
            .args(["attach", "--start"]);
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
