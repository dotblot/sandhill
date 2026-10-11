//! End-to-end tests against real Jupyter kernels. Each test skips (and says
//! so) when the kernel it needs isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, EVENT_RUN_FINISHED, EVENT_RUN_STARTED, EVENT_SESSION_ENDED, Event, RunFinished,
    SessionStatus,
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
    let found = jupyter_zmq_client::find_kernelspec(name).await.is_ok();
    if !found {
        eprintln!("skipping: Jupyter kernel {name:?} is not installed");
    }
    found
}

/// Runs code and returns the run's events up to and including run_finished.
async fn run(client: &mut Client, session: &str, code: &str) -> (Vec<Event>, RunFinished) {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, None).await.unwrap().run;
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
