//! `sandhilld attach`, the stdio bridge the app and ssh use.

use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

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
    let socket = state.path().join("sandhilld.sock");
    for _ in 0..100 {
        if tokio::net::UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

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
