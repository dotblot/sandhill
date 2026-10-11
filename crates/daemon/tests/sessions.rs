//! End-to-end tests against real Jupyter kernels. Each test skips (and says
//! so) when the kernel it needs isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, CONFLICT_ERROR, CellRef, EVENT_RUN_FINISHED, EVENT_RUN_STARTED, EVENT_SESSION_ENDED,
    EntryKind, Event, RunFinished, SessionStatus,
};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_secs(90);

struct TestDaemon {
    daemon: Arc<Daemon>,
    socket: PathBuf,
    server: JoinHandle<()>,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
        let socket = state.join("test.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        TestDaemon {
            daemon,
            socket,
            server,
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }

    /// Simulates the daemon process exiting: stops serving and managing
    /// sessions, but leaves the kernels running.
    fn stop(self) {
        self.server.abort();
        self.daemon.detach_all();
    }
}

async fn have_kernel(name: &str) -> bool {
    let found = sandhill_daemon::kernel::find_kernelspec(name).await.is_ok();
    if !found {
        eprintln!("skipping: Jupyter kernel {name:?} is not installed");
    }
    found
}

/// Runs code and returns the run's events up to and including run_finished.
async fn run(client: &mut Client, session: &str, code: &str) -> (Vec<Event>, RunFinished) {
    run_cell(client, session, code, None).await
}

async fn run_cell(
    client: &mut Client,
    session: &str,
    code: &str,
    cell: Option<CellRef>,
) -> (Vec<Event>, RunFinished) {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, cell).await.unwrap().run;
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for output")
            .expect("connection closed");
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            let finished = serde_json::from_value(event.content.clone()).unwrap();
            events.push(event);
            return (events, finished);
        }
        events.push(event);
    }
}

fn stdout_of(events: &[Event]) -> String {
    events
        .iter()
        .filter(|e| e.msg_type == "stream" && e.content["name"] == "stdout")
        .map(|e| e.content["text"].as_str().unwrap())
        .collect()
}

fn result_text(events: &[Event]) -> Option<String> {
    events
        .iter()
        .find(|e| e.msg_type == "execute_result")
        .and_then(|e| e.content["data"]["text/plain"].as_str())
        .map(str::to_string)
}

#[tokio::test]
async fn python_outputs_stream_back_in_order() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;

    let (events, finished) = run(&mut client, &session, "print('hello')\n1 + 1").await;
    assert_eq!(finished.status, "ok");
    assert_eq!(finished.execution_count, Some(1));
    assert_eq!(events[0].msg_type, EVENT_RUN_STARTED);
    assert_eq!(stdout_of(&events), "hello\n");
    assert_eq!(result_text(&events).as_deref(), Some("2"));

    let (events, finished) = run(&mut client, &session, "1 / 0").await;
    assert_eq!(finished.status, "error");
    let error = events.iter().find(|e| e.msg_type == "error").unwrap();
    assert_eq!(error.content["ename"], "ZeroDivisionError");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn outputs_are_kept_while_no_client_is_connected() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let session = {
        let client = daemon.client().await;
        let session = client.start_session("python3", None).await.unwrap().id;
        client
            .execute(
                &session,
                "import time\ntime.sleep(1)\nprint('finished while you were away')",
                None,
            )
            .await
            .unwrap();
        session
        // The client disconnects here, before the code prints anything.
    };
    tokio::time::sleep(Duration::from_secs(3)).await;

    let mut client = daemon.client().await;
    client.subscribe(&session, Some(0)).await.unwrap();
    let mut replayed = Vec::new();
    while !replayed
        .iter()
        .any(|e: &Event| e.msg_type == EVENT_RUN_FINISHED)
    {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .unwrap()
            .unwrap();
        replayed.push(event);
    }
    assert_eq!(stdout_of(&replayed), "finished while you were away\n");

    // The same events are available as a plain history page.
    let page = client.history(&session, 0, None).await.unwrap();
    assert_eq!(page.events.len(), replayed.len());
    assert_eq!(page.next_seq, replayed.last().unwrap().seq);

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn kernel_and_running_cell_survive_a_daemon_restart() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let first = TestDaemon::start(dir.path()).await;
    let session = {
        let mut client = first.client().await;
        let session = client.start_session("python3", None).await.unwrap().id;
        run(&mut client, &session, "x = 41").await;
        client
            .execute(
                &session,
                "import time\nfor i in range(3):\n    time.sleep(0.7)\n    print('tick', i, flush=True)",
                None,
            )
            .await
            .unwrap();
        session
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    first.stop();

    let second = TestDaemon::start(dir.path()).await;
    let mut client = second.client().await;
    // Variables from before the restart are still there.
    let (events, _) = run(&mut client, &session, "x + 1").await;
    assert_eq!(result_text(&events).as_deref(), Some("42"));

    // Output printed after the restart was journaled, and the run that was
    // in flight during the restart was closed out. Its status is the
    // kernel's reply when that reached the new daemon, or "unknown" when the
    // reply was sent while no daemon was connected.
    let history = client.history(&session, 0, None).await.unwrap().events;
    let ticks: String = stdout_of(&history);
    assert!(ticks.contains("tick 2"), "missing late output: {ticks:?}");
    let statuses: Vec<String> = history
        .iter()
        .filter(|e| e.msg_type == EVENT_RUN_FINISHED)
        .map(|e| e.content["status"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(statuses.len(), 3, "{statuses:?}");
    assert_eq!(statuses[0], "ok");
    assert!(
        ["ok", "unknown"].contains(&statuses[1].as_str()),
        "{statuses:?}"
    );
    assert_eq!(statuses[2], "ok");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn interrupt_stops_a_long_running_cell() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;

    client.subscribe(&session, None).await.unwrap();
    let run_id = client
        .execute(&session, "import time\ntime.sleep(60)", None)
        .await
        .unwrap()
        .run;
    tokio::time::sleep(Duration::from_secs(1)).await;
    client.interrupt(&session).await.unwrap();

    let finished = loop {
        let event = tokio::time::timeout(Duration::from_secs(20), client.next_event())
            .await
            .expect("interrupt did not stop the cell")
            .unwrap();
        if event.run.as_deref() == Some(run_id.as_str()) && event.msg_type == EVENT_RUN_FINISHED {
            break event;
        }
    };
    assert_eq!(finished.content["status"], "error");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn images_are_stored_as_blobs() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;

    let code = "from IPython.display import display\n\
                display({'image/png': 'iVBORw0KGgo=', 'text/plain': '<plot>'}, raw=True)";
    let (events, _) = run(&mut client, &session, code).await;
    let display = events
        .iter()
        .find(|e| e.msg_type == "display_data")
        .unwrap();
    let png = &display.content["data"]["image/png"];
    let hash = png[BLOB_KEY]
        .as_str()
        .expect("png should be a blob reference");
    assert_eq!(display.content["data"]["text/plain"], "<plot>");
    assert_eq!(client.get_blob(hash).await.unwrap().data, "iVBORw0KGgo=");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn shutdown_ends_the_session() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let info = client.start_session("python3", None).await.unwrap();
    client.subscribe(&info.id, None).await.unwrap();
    client.shutdown(&info.id).await.unwrap();

    let event = tokio::time::timeout(TIMEOUT, client.next_event())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.msg_type, EVENT_SESSION_ENDED);
    let sessions = client.list_sessions().await.unwrap();
    assert_eq!(sessions[0].status, SessionStatus::Ended);
    assert!(client.execute(&info.id, "1", None).await.is_err());
    assert!(
        !sandhill_daemon::kernel::pid_alive(info.pid.unwrap()),
        "kernel process should be gone"
    );
}

#[tokio::test]
async fn r_kernel_runs_code() {
    let kernel = if have_kernel("ark").await {
        "ark"
    } else if have_kernel("ir").await {
        "ir"
    } else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let info = client.start_session(kernel, None).await.unwrap();
    assert_eq!(info.language, "R");

    let (events, finished) = run(&mut client, &info.id, "cat('hello from R\\n')\n1 + 1").await;
    assert_eq!(finished.status, "ok");
    assert_eq!(stdout_of(&events), "hello from R\n");
    let printed = result_text(&events)
        .or_else(|| {
            events
                .iter()
                .find(|e| e.msg_type == "display_data")
                .and_then(|e| e.content["data"]["text/plain"].as_str().map(str::to_string))
        })
        .unwrap_or_else(|| stdout_of(&events));
    assert!(
        printed.contains("[1] 2"),
        "unexpected R output: {printed:?}"
    );

    client.shutdown(&info.id).await.unwrap();
}

fn cell(file: &Path, cell_id: &str) -> Option<CellRef> {
    Some(CellRef {
        file: file.to_string_lossy().into_owned(),
        cell_id: cell_id.into(),
    })
}

/// The sidecar is written just after the run finishes; wait for it briefly.
async fn read_sidecar(doc: &Path) -> serde_json::Value {
    let index = sandhill_daemon::sidecar::dir_for(doc)
        .unwrap()
        .join("index.json");
    for _ in 0..50 {
        if let Ok(text) = std::fs::read_to_string(&index) {
            return serde_json::from_str(&text).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no sidecar at {}", index.display());
}

#[tokio::test]
async fn files_can_be_listed_read_and_saved() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("data")).unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let client = daemon.client().await;

    let path = work.join("analysis.qmd").to_string_lossy().into_owned();
    let written = client.write_file(&path, "# Title\n", None).await.unwrap();
    let read = client.read_file(&path).await.unwrap();
    assert_eq!(read.text, "# Title\n");
    assert_eq!(read.mtime_ms, written.mtime_ms);

    let entries = client.list_dir(&work.to_string_lossy()).await.unwrap();
    let listed: Vec<_> = entries.iter().map(|e| (e.name.as_str(), e.kind)).collect();
    assert_eq!(
        listed,
        [("data", EntryKind::Dir), ("analysis.qmd", EntryKind::File)]
    );

    // Someone else saves the file after we read it.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&path, "# Changed elsewhere\n").unwrap();
    let err = client
        .write_file(&path, "# Mine\n", Some(read.mtime_ms))
        .await
        .unwrap_err();
    assert!(err.to_string().contains(CONFLICT_ERROR), "{err}");

    let ping = client.ping().await.unwrap();
    assert!(!ping.home.is_empty());
}

#[tokio::test]
async fn notebook_plots_stay_in_the_daemon_between_open_and_save() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("analysis.ipynb");
    let notebook = serde_json::json!({
        "cells": [{"cell_type": "code", "execution_count": null, "id": "c1",
                   "metadata": {}, "outputs": [], "source": ["plot()"]}],
        "metadata": {}, "nbformat": 4, "nbformat_minor": 5
    });
    std::fs::write(&doc, notebook.to_string()).unwrap();
    let path = doc.to_string_lossy().into_owned();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;

    let code = "from IPython.display import display\n\
                display({'image/png': 'iVBORw0KGgo=', 'text/plain': '<plot>'}, raw=True)";
    let (_, finished) = run_cell(&mut client, &session, code, cell(&doc, "c1")).await;
    assert_eq!(finished.status, "ok");

    // The app opens the notebook and takes the run's outputs from the
    // journal, the plot still a reference, then saves.
    let read = client.read_notebook(&path).await.unwrap();
    let mut opened: serde_json::Value = serde_json::from_str(&read.text).unwrap();
    let file_outputs = client.file_outputs(&path).await.unwrap();
    let outputs = file_outputs.cells[0].outputs.clone();
    let hash = outputs[0]["data"]["image/png"][BLOB_KEY]
        .as_str()
        .expect("the plot should be a blob reference")
        .to_string();
    opened["cells"][0]["outputs"] = serde_json::Value::Array(outputs);
    let text = serde_json::to_string_pretty(&opened).unwrap();
    let written = client
        .write_notebook(&path, &text, Some(read.mtime_ms))
        .await
        .unwrap();

    // The file has the plot itself, as Jupyter expects.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&doc).unwrap()).unwrap();
    let output = &saved["cells"][0]["outputs"][0];
    assert_eq!(output["data"]["image/png"], "iVBORw0KGgo=");
    assert_eq!(output["output_type"], "display_data");

    // Opening it again gives the same reference back.
    let reread = client.read_notebook(&path).await.unwrap();
    assert_eq!(reread.mtime_ms, written.mtime_ms);
    let reopened: serde_json::Value = serde_json::from_str(&reread.text).unwrap();
    assert_eq!(
        reopened["cells"][0]["outputs"][0]["data"]["image/png"][BLOB_KEY],
        hash.as_str()
    );

    // A save over changes made elsewhere is refused, as for any file.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&doc, notebook.to_string()).unwrap();
    let err = client
        .write_notebook(&path, &text, Some(written.mtime_ms))
        .await
        .unwrap_err();
    assert!(err.to_string().contains(CONFLICT_ERROR), "{err}");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn qmd_outputs_are_saved_next_to_the_file() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("analysis.qmd");
    std::fs::write(&doc, "```{python}\nprint('hi')\n```\n").unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;

    let code = "from IPython.display import display\n\
                print('hi')\n\
                display({'image/png': 'iVBORw0KGgo=', 'text/plain': '<plot>'}, raw=True)\n\
                6 * 7";
    let (_, finished) = run_cell(&mut client, &session, code, cell(&doc, "setup")).await;
    assert_eq!(finished.status, "ok");

    let sidecar = read_sidecar(&doc).await;
    let saved = &sidecar["cells"]["setup"];
    assert_eq!(saved["code"], code);
    assert_eq!(saved["status"], "ok");
    let outputs = saved["outputs"].as_array().unwrap();
    let types: Vec<_> = outputs
        .iter()
        .map(|o| o["output_type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["stream", "display_data", "execute_result"]);
    assert_eq!(outputs[0]["text"], "hi\n");
    assert_eq!(outputs[2]["data"]["text/plain"], "42");

    // The plot was copied next to the document, so it can be served even
    // after the daemon's own store is gone.
    let hash = outputs[1]["data"]["image/png"][BLOB_KEY].as_str().unwrap();
    std::fs::remove_dir_all(daemon.daemon.state().blobs_dir()).unwrap();
    let blob = client
        .call::<sandhill_proto::Blob>(sandhill_proto::Call::GetBlob {
            hash: hash.into(),
            near: Some(doc.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
    assert_eq!(blob.data, "iVBORw0KGgo=");

    // The same outputs come back from the journal.
    let file_outputs = client.file_outputs(&doc.to_string_lossy()).await.unwrap();
    assert_eq!(file_outputs.cells.len(), 1);
    assert_eq!(file_outputs.cells[0].cell_id, "setup");
    assert_eq!(file_outputs.cells[0].outputs, *outputs);

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn reopening_mid_run_shows_output_so_far_then_the_rest() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("long.ipynb");
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let session = {
        let client = daemon.client().await;
        let session = client.start_session("python3", None).await.unwrap().id;
        let code = "import time\nfor i in range(4):\n    print('step', i, flush=True)\n    time.sleep(0.5)";
        client
            .execute(&session, code, cell(&doc, "c1"))
            .await
            .unwrap();
        session
        // The app closes here.
    };
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let mut client = daemon.client().await;
    let snapshot = client.file_outputs(&doc.to_string_lossy()).await.unwrap();
    let cell = &snapshot.cells[0];
    assert_eq!(cell.finished_ms, None, "the run should still be going");
    let mut text = cell.outputs[0]["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("step 0\n"), "{text:?}");

    client
        .subscribe(&session, Some(snapshot.seq))
        .await
        .unwrap();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .unwrap()
            .unwrap();
        if event.run.as_deref() != Some(cell.run.as_str()) {
            continue;
        }
        match event.msg_type.as_str() {
            "stream" => text.push_str(event.content["text"].as_str().unwrap()),
            EVENT_RUN_FINISHED => break,
            _ => {}
        }
    }
    assert_eq!(text, "step 0\nstep 1\nstep 2\nstep 3\n");

    client.shutdown(&session).await.unwrap();
}
