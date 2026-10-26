//! End-to-end tests against real Jupyter kernels. Each test skips (and says
//! so) when the kernel it needs isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, CONFLICT_ERROR, Call, CellRef, Deleted, DirEntry, EVENT_RUN_FINISHED,
    EVENT_RUN_STARTED, EVENT_SESSION_ENDED, EXISTS_ERROR, EntryKind, Event, FileBytes, FileWritten,
    RunFinished, SessionStatus,
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
    let found = sandhill_daemon::kernel::find_kernelspec(name, None)
        .await
        .is_ok();
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
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for (kernel, code) in [
        ("python3", "import time\ntime.sleep(60)"),
        // IRkernel answers with the status `abort`, which the daemon mends to `error`.
        ("ir", "Sys.sleep(60)"),
        ("ark", "Sys.sleep(60)"),
    ] {
        if !have_kernel(kernel).await {
            eprintln!("skipping {kernel}: not installed");
            continue;
        }
        let session = client.start_session(kernel, None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        let run_id = client.execute(&session, code, None).await.unwrap().run;
        tokio::time::sleep(Duration::from_secs(1)).await;
        client.interrupt(&session).await.unwrap();

        let finished = loop {
            let event = tokio::time::timeout(Duration::from_secs(20), client.next_event())
                .await
                .unwrap_or_else(|_| panic!("interrupt did not stop {kernel}'s cell"))
                .unwrap();
            if event.run.as_deref() == Some(run_id.as_str()) && event.msg_type == EVENT_RUN_FINISHED
            {
                break event;
            }
        };
        // Ark says an interrupted run went well; the others that it failed.
        if kernel != "ark" {
            assert_eq!(
                finished.content["status"], "error",
                "{kernel}: {finished:?}"
            );
        }
        client.shutdown(&session).await.unwrap();
    }
}

/// The app tells busy sessions from idle ones by this count, even for
/// sessions it never subscribed to.
/// Ark says "R version 4.6.1 (2026-06-24)"; a session gives only the number,
/// as IRkernel and ipykernel do.
#[tokio::test]
async fn r_sessions_give_their_version_as_a_number() {
    for kernel in ["ark", "ir"] {
        if !have_kernel(kernel).await {
            eprintln!("skipping {kernel}: not installed");
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let daemon = TestDaemon::start(dir.path()).await;
        let client = daemon.client().await;
        let info = client.start_session(kernel, None).await.unwrap();
        let version = info.language_version.clone().expect("no language version");
        assert!(
            version.split('.').count() == 3
                && version.split('.').all(|part| part.parse::<u32>().is_ok()),
            "{kernel}: {version}"
        );
    }
}

#[tokio::test]
async fn sessions_report_their_language_version_and_memory() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let info = client.start_session("python3", None).await.unwrap();
    let version = info.language_version.clone().expect("no language version");
    assert!(version.starts_with('3'), "{version}");
    let listed = client.list_sessions().await.unwrap();
    assert_eq!(listed[0].language_version, Some(version));

    // A kernel holding 200 MB shows it; the disk is the one the folder is on.
    let (_, finished) = run(&mut client, &info.id, "x = b\"x\" * (200 * 2**20)").await;
    assert_eq!(finished.status, "ok");
    let stats = client
        .system_stats(Some(dir.path().to_str().unwrap()))
        .await
        .unwrap();
    let held = stats
        .sessions
        .iter()
        .find(|s| s.session == info.id)
        .unwrap();
    assert!(held.rss > 200 << 20, "kernel holds {} bytes", held.rss);
    assert!(stats.cpu.cores >= 1 && stats.memory.total > held.rss);
    assert_eq!(stats.disk.unwrap().path, dir.path().to_str().unwrap());

    client.shutdown(&info.id).await.unwrap();
    let stats = client.system_stats(None).await.unwrap();
    assert!(stats.sessions.iter().all(|s| s.session != info.id));
}

#[tokio::test]
async fn sessions_count_their_unfinished_runs() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    let pending = |sessions: Vec<sandhill_proto::SessionInfo>| {
        sessions
            .into_iter()
            .find(|s| s.id == session)
            .unwrap()
            .pending_runs
    };
    assert_eq!(pending(client.list_sessions().await.unwrap()), 0);

    client.subscribe(&session, None).await.unwrap();
    let slow = client
        .execute(&session, "import time\ntime.sleep(3)", None)
        .await
        .unwrap()
        .run;
    let queued = client.execute(&session, "1 + 1", None).await.unwrap().run;
    assert_eq!(pending(client.list_sessions().await.unwrap()), 2);

    let mut left = vec![slow, queued];
    while !left.is_empty() {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the runs to finish")
            .unwrap();
        if event.msg_type == EVENT_RUN_FINISHED {
            left.retain(|run| Some(run.as_str()) != event.run.as_deref());
        }
    }
    assert_eq!(pending(client.list_sessions().await.unwrap()), 0);

    // A run cut off by the shutdown doesn't count as pending afterwards.
    client
        .execute(&session, "import time\ntime.sleep(60)", None)
        .await
        .unwrap();
    client.shutdown(&session).await.unwrap();
    assert_eq!(pending(client.list_sessions().await.unwrap()), 0);
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
async fn the_file_tree_makes_renames_copies_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let client = daemon.client().await;
    let at = |rel: &str| work.join(rel).to_string_lossy().into_owned();

    // A new file with a path makes its folders; making it again fails.
    let new_file = |path: String| Call::WriteFile {
        path,
        text: "# raw\n".into(),
        expected_mtime_ms: None,
        base64: false,
        create: true,
    };
    let _: FileWritten = client.call(new_file(at("raw/samples.csv"))).await.unwrap();
    let err = client
        .call::<FileWritten>(new_file(at("raw/samples.csv")))
        .await
        .unwrap_err();
    assert!(err.to_string().contains(EXISTS_ERROR), "{err}");
    let _: FileWritten = client
        .call(Call::MakeDir {
            path: at("results/figures"),
        })
        .await
        .unwrap();

    let _: FileWritten = client
        .call(Call::MovePath {
            from: at("raw"),
            to: at("data"),
        })
        .await
        .unwrap();
    let _: FileWritten = client
        .call(Call::CopyPath {
            from: at("data"),
            to: at("data copy"),
        })
        .await
        .unwrap();
    // The @ menu lists the files under the folder, nearest first.
    let files: Vec<String> = client
        .call(Call::ListFiles {
            path: at(""),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(files, ["data/samples.csv", "data copy/samples.csv"]);
    assert_eq!(
        std::fs::read_to_string(work.join("data copy/samples.csv")).unwrap(),
        "# raw\n"
    );
    let deleted: Deleted = client
        .call(Call::DeletePath {
            path: at("data copy"),
            trash: false,
        })
        .await
        .unwrap();
    assert_eq!(deleted.trashed_to, None);

    // Folder sizes, for sorting by size.
    let sized: Vec<DirEntry> = client
        .call(Call::ListDir {
            path: at(""),
            sizes: true,
        })
        .await
        .unwrap();
    let listed: Vec<_> = sized.iter().map(|e| (e.name.as_str(), e.size)).collect();
    assert_eq!(listed, [("data", 6), ("results", 0)]);

    // Downloading reads a file in parts.
    let part: FileBytes = client
        .call(Call::ReadBytes {
            path: at("data/samples.csv"),
            offset: 2,
            limit: Some(3),
        })
        .await
        .unwrap();
    assert_eq!((part.data.as_str(), part.size), ("cmF3", 6));
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

#[tokio::test]
async fn projects_keep_their_state_and_their_sessions() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("my-analysis");
    std::fs::create_dir_all(&project).unwrap();
    let project_path = project.to_string_lossy().into_owned();
    let state_dir = dir.path().join("state");

    let first = TestDaemon::start(&state_dir).await;
    let session = {
        let client = first.client().await;
        // A trailing slash names the same project.
        let opened = client
            .open_project(&format!("{project_path}/"))
            .await
            .unwrap();
        assert_eq!(opened.path, project_path);
        assert_eq!(opened.name, "my-analysis");
        assert!(opened.state.is_null());
        client
            .save_project_state(&project_path, serde_json::json!({"tabs": ["a.qmd"]}))
            .await
            .unwrap();
        let info: sandhill_proto::SessionInfo = client
            .call(sandhill_proto::Call::StartSession {
                kernel: "python3".into(),
                cwd: Some(project_path.clone()),
                project: Some(project_path.clone()),
            })
            .await
            .unwrap();
        assert_eq!(info.project.as_deref(), Some(project_path.as_str()));
        info.id
    };
    first.stop();

    // After a daemon restart the project, its state and its session's tag
    // are all still there.
    let second = TestDaemon::start(&state_dir).await;
    let mut client = second.client().await;
    let recent = client.list_recent_projects().await.unwrap();
    assert_eq!(recent.len(), 1);
    assert!(recent[0].exists);
    let reopened = client.open_project(&project_path).await.unwrap();
    assert_eq!(reopened.state, serde_json::json!({"tabs": ["a.qmd"]}));
    let sessions = client.list_sessions().await.unwrap();
    let tagged = sessions.iter().find(|s| s.id == session).unwrap();
    assert_eq!(tagged.project.as_deref(), Some(project_path.as_str()));
    let (events, _) = run(&mut client, &session, "import os; print(os.getcwd())").await;
    assert!(stdout_of(&events).contains("my-analysis"));

    client.forget_project(&project_path).await.unwrap();
    assert!(client.list_recent_projects().await.unwrap().is_empty());
    assert!(
        project.is_dir(),
        "forgetting a project must not touch its files"
    );
    client.shutdown(&session).await.unwrap();
}

/// Runs queued behind one that fails still finish, so the session doesn't
/// look busy forever (kernels abort them, or run them).
#[tokio::test]
async fn runs_queued_behind_an_error_finish() {
    for (kernel, fails) in [("python3", "raise ValueError('x')"), ("ir", "stop('x')")] {
        if !have_kernel(kernel).await {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let daemon = TestDaemon::start(dir.path()).await;
        let mut client = daemon.client().await;
        let session = client.start_session(kernel, None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        let mut left = Vec::new();
        for code in ["1", fails, "2", "3"] {
            left.push(client.execute(&session, code, None).await.unwrap().run);
        }
        while !left.is_empty() {
            let event = tokio::time::timeout(Duration::from_secs(20), client.next_event())
                .await
                .unwrap_or_else(|_| panic!("{kernel}: {} runs never finished", left.len()))
                .unwrap();
            if event.msg_type == EVENT_RUN_FINISHED {
                left.retain(|run| Some(run.as_str()) != event.run.as_deref());
            }
        }
        let sessions = client.list_sessions().await.unwrap();
        assert_eq!(
            sessions
                .iter()
                .find(|s| s.id == session)
                .unwrap()
                .pending_runs,
            0
        );
        client.shutdown(&session).await.unwrap();
    }
}
