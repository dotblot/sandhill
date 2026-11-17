//! Code that asks the user something (`input()`, pdb, R's `browser()`) is
//! answered through the daemon, against real kernels. Each test skips (and
//! says so) when its kernel isn't installed.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{EVENT_INPUT_REPLY, EVENT_INPUT_REQUEST, EVENT_RUN_FINISHED, Event};
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

/// The run's events until it asks a question (returned last) or finishes.
async fn until_asked_or_done(client: &mut Client, run: &str) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() != Some(run) {
            continue;
        }
        let last = matches!(
            event.msg_type.as_str(),
            EVENT_INPUT_REQUEST | EVENT_RUN_FINISHED
        );
        events.push(event);
        if last {
            return events;
        }
    }
}

/// Waits for the run's question and returns its prompt and whether it is a
/// password, with what the run printed before it.
async fn asked(client: &mut Client, run: &str) -> (String, bool, String) {
    let events = until_asked_or_done(client, run).await;
    let question = events.last().unwrap();
    assert_eq!(
        question.msg_type, EVENT_INPUT_REQUEST,
        "the run ended without asking: {events:#?}"
    );
    (
        question.content["prompt"].as_str().unwrap().to_string(),
        question.content["password"].as_bool().unwrap(),
        stdout_of(&events),
    )
}

/// Waits for the run to finish: its status and its events.
async fn finished(client: &mut Client, run: &str) -> (String, Vec<Event>) {
    let events = until_asked_or_done(client, run).await;
    let last = events.last().unwrap();
    assert_eq!(
        last.msg_type, EVENT_RUN_FINISHED,
        "the run asked again: {events:#?}"
    );
    (last.content["status"].as_str().unwrap().to_string(), events)
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
async fn python_input_and_passwords_are_answered() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();

    let err = client.input(&session, "early").await.unwrap_err();
    assert!(
        err.to_string().contains("nothing is being asked"),
        "{err:#}"
    );

    let code = "name = input('name? ')\nprint('hi', name)";
    let run = client.execute(&session, code, None).await.unwrap().run;
    let (prompt, password, _) = asked(&mut client, &run).await;
    assert_eq!((prompt.as_str(), password), ("name? ", false));
    client.input(&session, "Huan").await.unwrap();
    let (status, events) = finished(&mut client, &run).await;
    assert_eq!(status, "ok");
    // The question and its answer read as a console shows them.
    assert_eq!(stdout_of(&events), "name? Huan\nhi Huan\n");
    assert!(events.iter().any(|e| e.msg_type == EVENT_INPUT_REPLY));

    let code = "import getpass\nsecret = getpass.getpass('pw: ')\nlen(secret)";
    let run = client.execute(&session, code, None).await.unwrap().run;
    let (prompt, password, _) = asked(&mut client, &run).await;
    assert_eq!((prompt.as_str(), password), ("pw: ", true));
    client.input(&session, "hunter2").await.unwrap();
    let (status, events) = finished(&mut client, &run).await;
    assert_eq!(status, "ok");
    assert_eq!(result_text(&events).as_deref(), Some("7"));
    // A password is not kept in the journal.
    let history = client.history(&session, 0, None).await.unwrap();
    let journaled = serde_json::to_string(&history).unwrap();
    assert!(journaled.contains("pw: "), "{journaled}");
    assert!(!journaled.contains("hunter2"), "{journaled}");

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn python_breakpoint_and_debug_stop_in_the_debugger() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();

    let code = "def f(x):\n    y = x * 2\n    breakpoint()\n    return y + 1\nf(10)";
    let run = client.execute(&session, code, None).await.unwrap().run;
    let (prompt, _, _) = asked(&mut client, &run).await;
    assert!(prompt.contains("db> "), "{prompt:?}");
    client.input(&session, "p y").await.unwrap();
    let (_, _, printed) = asked(&mut client, &run).await;
    assert!(printed.contains("20\n"), "{printed:?}");
    client.input(&session, "c").await.unwrap();
    let (status, events) = finished(&mut client, &run).await;
    assert_eq!(status, "ok");
    assert_eq!(result_text(&events).as_deref(), Some("21"));

    // %debug looks at the last error.
    let run = client
        .execute(&session, "z = 7\nz / 0", None)
        .await
        .unwrap()
        .run;
    finished(&mut client, &run).await;
    let run = client.execute(&session, "%debug", None).await.unwrap().run;
    asked(&mut client, &run).await;
    client.input(&session, "p z * 6").await.unwrap();
    let (_, _, printed) = asked(&mut client, &run).await;
    assert!(printed.contains("42\n"), "{printed:?}");
    client.input(&session, "q").await.unwrap();
    finished(&mut client, &run).await;

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn r_browser_steps_through_a_function() {
    if !have_kernel("ark").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("ark", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();

    let code = "f <- function(x) { y <- x * 2; browser(); y + 1 }\nf(10)";
    let run = client.execute(&session, code, None).await.unwrap().run;
    let (prompt, _, _) = asked(&mut client, &run).await;
    assert_eq!(prompt, "Browse[1]> ");
    client.input(&session, "y").await.unwrap();
    let (_, _, printed) = asked(&mut client, &run).await;
    assert!(printed.contains("[1] 20"), "{printed:?}");
    client.input(&session, "c").await.unwrap();
    let (status, events) = finished(&mut client, &run).await;
    assert_eq!(status, "ok");
    assert_eq!(result_text(&events).as_deref(), Some("[1] 21"));

    client.shutdown(&session).await.unwrap();
}

/// Interrupt still stops code that waits for an answer, and the session
/// goes on.
#[tokio::test]
async fn interrupt_ends_a_run_waiting_for_input() {
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for (kernel, ask, then, expected) in [
        ("python3", "input('x? ')", "1 + 1", "2"),
        ("ark", "readline('x? ')", "1 + 1", "[1] 2"),
    ] {
        if !have_kernel(kernel).await {
            continue;
        }
        let session = client.start_session(kernel, None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        let run = client.execute(&session, ask, None).await.unwrap().run;
        asked(&mut client, &run).await;
        client.interrupt(&session).await.unwrap();
        finished(&mut client, &run).await;
        let err = client.input(&session, "late").await.unwrap_err();
        assert!(
            err.to_string().contains("nothing is being asked"),
            "{err:#}"
        );

        let run = client.execute(&session, then, None).await.unwrap().run;
        let (status, events) = finished(&mut client, &run).await;
        assert_eq!(status, "ok", "{kernel}");
        assert_eq!(result_text(&events).as_deref(), Some(expected), "{kernel}");
        client.shutdown(&session).await.unwrap();
    }
}

/// A tool's quiet run is never left waiting for an answer nobody sees:
/// Python refuses to ask, and Ark (which asks anyway) is interrupted.
#[tokio::test]
async fn quiet_runs_never_wait_for_input() {
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for (kernel, ask) in [("python3", "input('x? ')"), ("ark", "readline('x? ')")] {
        if !have_kernel(kernel).await {
            continue;
        }
        let session = client.start_session(kernel, None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        let run = client.execute_quietly(&session, ask).await.unwrap().run;
        let events = until_asked_or_done(&mut client, &run).await;
        assert_eq!(
            events.last().unwrap().msg_type,
            EVENT_RUN_FINISHED,
            "{kernel}: {events:#?}"
        );
        client.shutdown(&session).await.unwrap();
    }
}

/// A question asked before the daemon restarted can still be answered by
/// the new daemon.
#[tokio::test]
async fn a_question_survives_a_daemon_restart() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();
    let code = "name = input('name? ')\nprint('hi', name)";
    let run = client.execute(&session, code, None).await.unwrap().run;
    asked(&mut client, &run).await;
    drop(client);
    daemon.stop();

    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    client.subscribe(&session, None).await.unwrap();
    client.input(&session, "again").await.unwrap();
    let (status, _) = finished(&mut client, &run).await;
    // As for any run that spans a restart, the kernel's reply may have been
    // routed to the old daemon (see sessions.rs): what counts is that the
    // code got the answer.
    assert!(["ok", "unknown"].contains(&status.as_str()), "{status}");
    let history = client.history(&session, 0, None).await.unwrap().events;
    let printed = stdout_of(&history);
    assert!(printed.contains("name? again\nhi again\n"), "{printed:?}");
    client.shutdown(&session).await.unwrap();
}
