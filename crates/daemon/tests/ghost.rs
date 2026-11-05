//! Ghost cells: a cell runs in a real kernel, and an agent, a stand-in
//! `claude` on PATH here, suggests the next one from its code and what it
//! showed. Skips (and says so) when the python3 kernel isn't installed.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, CellSuggestion, EVENT_RUN_FINISHED, RunStarted};
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

#[tokio::test]
async fn an_agent_suggests_the_cell_after_a_run() {
    if sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_err()
    {
        eprintln!("skipping: Jupyter kernel \"python3\" is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fake = bin.join("claude");
    // It answers with the next step, saying what it was shown.
    std::fs::write(
        &fake,
        "#!/bin/sh\n[ \"$1\" = --help ] && exit 0\nasked=$(cat)\necho 'Next:'\necho '```python'\nif echo \"$asked\" | grep -q 'n_cells = 2700'; then echo 'print(n_cells / 2)'; fi\necho '```'\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Only this test runs in this binary, so nothing else reads PATH.
    unsafe {
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    }

    let state = dir.path().join("state");
    let daemon: Arc<Daemon> = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let mut client = Client::connect_unix(&socket).await.unwrap();
    let session = client.start_session("python3", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();
    let code = "n_cells = 2700\nprint(n_cells)";
    let started: RunStarted = client
        .call(Call::Execute {
            session: session.clone(),
            code: code.into(),
            cell: None,
            quiet: false,
            by: None,
        })
        .await
        .unwrap();
    let mut shown = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() != Some(started.run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            shown.push_str(event.content["text"].as_str().unwrap_or(""));
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            break;
        }
    }
    assert_eq!(shown, "2700\n");

    let suggested: CellSuggestion = client
        .call(Call::SuggestCell {
            agent: Some("antigravity".into()),
            cwd: dir.path().to_string_lossy().into_owned(),
            language: "python".into(),
            cells: vec!["import math".into(), code.into()],
            output: shown,
        })
        .await
        .unwrap();
    assert_eq!(suggested.agent, "claude");
    assert_eq!(suggested.code.as_deref(), Some("print(n_cells / 2)"));
    client.shutdown(&session).await.unwrap();
}
