//! Terminals: programs in a terminal that keep running without the daemon,
//! with their output kept for clients that come later.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use sandhill_daemon::terminal::{Launch, Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    AgentOptions, Call, ServerMessage, TERMINAL_SHELL, TerminalEvent, TerminalInfo,
};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(20);

fn sandhilld() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_sandhilld"))
}

fn sh(script: &str, cwd: &Path) -> Launch {
    Launch {
        kind: TERMINAL_SHELL.into(),
        title: "Test".into(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        env: vec![],
        cwd: cwd.to_path_buf(),
        project: None,
        options: AgentOptions::default(),
        size: Some((80, 24)),
    }
}

/// Collects a terminal's output until `until` appears in it or it exits.
async fn read_until(
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    output: &mut Vec<u8>,
    until: &str,
) -> Option<sandhill_proto::TerminalExit> {
    let engine = base64::engine::general_purpose::STANDARD;
    loop {
        if String::from_utf8_lossy(output).contains(until) {
            return None;
        }
        let message = tokio::time::timeout(TIMEOUT, events.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {until:?}; got {:?}",
                    String::from_utf8_lossy(output)
                )
            })
            .expect("stream ended");
        let ServerMessage::Terminal { terminal } = message else {
            continue;
        };
        if let Some(data) = terminal.data {
            output.extend(engine.decode(data).unwrap());
        }
        if terminal.exit.is_some() {
            return terminal.exit;
        }
    }
}

#[tokio::test]
async fn a_program_gets_input_and_its_output_is_kept_after_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    let info = terminals
        .start(sh(
            "printf 'ready\\n'; read line; echo \"got $line\"; exit 3",
            dir.path(),
        ))
        .await
        .unwrap();
    assert!(info.running);

    let (out, mut events) = mpsc::unbounded_channel();
    let stream = terminals.open(&info.id, Some(0)).await.unwrap();
    assert_eq!(stream.from, 0);
    tokio::spawn(stream.forward(out));
    let mut output = Vec::new();
    read_until(&mut events, &mut output, "ready").await;
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: "hello\n".into(),
            },
        )
        .await
        .unwrap();
    let exit = loop {
        if let Some(exit) = read_until(&mut events, &mut output, "\u{0}").await {
            break exit;
        }
    };
    assert_eq!(exit.code, Some(3));
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("got hello"), "{text}");

    // Once it has ended, the output still comes back, from the files.
    let info = terminals.info(&info.id).unwrap();
    assert!(!info.running);
    assert_eq!(info.exit.as_ref().and_then(|e| e.code), Some(3));
    assert_eq!(info.end, output.len() as u64);
    let (out, mut events) = mpsc::unbounded_channel();
    terminals
        .open(&info.id, None)
        .await
        .unwrap()
        .forward(out)
        .await;
    let mut again = Vec::new();
    let exit = read_until(&mut events, &mut again, "\u{0}").await;
    assert_eq!(again, output);
    assert_eq!(exit.and_then(|e| e.code), Some(3));

    terminals.close(&info.id).await.unwrap();
    assert!(terminals.list().is_empty());
}

#[tokio::test]
async fn a_terminal_outlives_the_daemon_that_started_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("terminals");
    let id = {
        let terminals = Terminals::new(root.clone(), sandhilld().into());
        let info = terminals
            .start(sh(
                "while read line; do echo \"echo:$line\"; done",
                dir.path(),
            ))
            .await
            .unwrap();
        terminals
            .send(
                &info.id,
                ToHost::Input {
                    data: "one\n".into(),
                },
            )
            .await
            .unwrap();
        info.id
        // The daemon's side goes away here, as when it restarts.
    };

    let terminals = Terminals::new(root, sandhilld().into());
    let listed = terminals.list();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].running);
    terminals
        .send(
            &id,
            ToHost::Input {
                data: "two\n".into(),
            },
        )
        .await
        .unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(terminals.open(&id, Some(0)).await.unwrap().forward(out));
    let mut output = Vec::new();
    read_until(&mut events, &mut output, "echo:two").await;
    assert!(String::from_utf8_lossy(&output).contains("echo:one"));

    // Closing hangs up on the program, as closing a window would.
    terminals.close(&id).await.unwrap();
    assert!(terminals.list().is_empty());
}

#[tokio::test]
async fn terminals_work_in_a_deep_state_folder() {
    let dir = tempfile::tempdir().unwrap();
    // Too deep for a socket path: the socket goes under /tmp instead.
    let root = dir.path().join("a".repeat(60)).join("terminals");
    let terminals = Terminals::new(root, sandhilld().into());
    let info = terminals
        .start(sh("read line; echo \"got $line\"", dir.path()))
        .await
        .unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: "deep\n".into(),
            },
        )
        .await
        .unwrap();
    let mut output = Vec::new();
    read_until(&mut events, &mut output, "got deep").await;
    terminals.close(&info.id).await.unwrap();
}

struct TestDaemon {
    daemon: Arc<Daemon>,
    socket: std::path::PathBuf,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        let socket = state.join("test.sock");
        let options = DaemonOptions {
            executable: sandhilld().into(),
            socket: socket.clone(),
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
        TestDaemon { daemon, socket }
    }
}

#[tokio::test]
async fn clients_run_a_shell_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let daemon = TestDaemon::start(&state).await;
    let mut client = Client::connect_unix(&daemon.socket).await.unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let info: TerminalInfo = client
        .call(Call::StartTerminal {
            kind: TERMINAL_SHELL.into(),
            cwd: None,
            project: Some(project.to_string_lossy().into()),
            cols: Some(80),
            rows: Some(24),
            options: AgentOptions::default(),
        })
        .await
        .unwrap();
    assert_eq!(info.title, "Terminal");
    assert_eq!(Path::new(&info.cwd), project);
    let subscribed = client.subscribe_terminal(&info.id, Some(0)).await.unwrap();
    assert_eq!(subscribed.from, 0);

    client.resize_terminal(&info.id, 120, 40).await.unwrap();
    client
        .terminal_input(
            &info.id,
            "stty size; echo \"$((6*7)) $SANDHILL_TERMINAL\"; pwd\n",
        )
        .await
        .unwrap();
    let engine = base64::engine::general_purpose::STANDARD;
    let mut output = Vec::new();
    let expected = format!("42 {}", state.join("terminals").join(&info.id).display());
    while !String::from_utf8_lossy(&output).contains(&expected)
        || !String::from_utf8_lossy(&output).contains("40 120")
    {
        let event: TerminalEvent = tokio::time::timeout(TIMEOUT, client.next_terminal_event())
            .await
            .unwrap_or_else(|_| panic!("got {:?}", String::from_utf8_lossy(&output)))
            .unwrap();
        assert_eq!(event.terminal, info.id);
        if let Some(data) = event.data {
            output.extend(engine.decode(data).unwrap());
        }
    }
    assert!(String::from_utf8_lossy(&output).contains(&project.to_string_lossy().to_string()));

    let listed = client.list_terminals().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].running);
    assert_eq!(
        listed[0].project.as_deref(),
        Some(&*project.to_string_lossy())
    );

    client.close_terminal(&info.id).await.unwrap();
    assert!(client.list_terminals().await.unwrap().is_empty());
    drop(daemon.daemon);
}

#[tokio::test]
async fn an_agent_reports_to_its_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    // Stands in for an agent whose hooks run `sandhilld terminal-event`.
    let script = format!(
        "echo '{{\"permission_mode\":\"acceptEdits\"}}' | '{exe}' terminal-event state busy; echo '{{\"notification_type\":\"permission_prompt\",\"message\":\"May I?\"}}' | '{exe}' terminal-event notification; echo done; read wait; '{exe}' terminal-event state busy; echo again; read wait",
        exe = sandhilld().display()
    );
    let info = terminals.start(sh(&script, dir.path())).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut output = Vec::new();
    read_until(&mut events, &mut output, "done").await;
    // The report is kept with the terminal.
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "waiting");
    assert_eq!(agent["message"], "May I?");
    // The permission mode the hook was given.
    assert_eq!(agent["mode"], "acceptEdits");

    // Once it gets going again, the question is gone.
    terminals
        .send(&info.id, ToHost::Input { data: "\n".into() })
        .await
        .unwrap();
    read_until(&mut events, &mut output, "again").await;
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "busy");
    assert!(agent.get("message").is_none(), "{agent}");
    // Run from the terminal itself, with nothing on stdin: the mode stays.
    assert_eq!(agent["mode"], "acceptEdits");
    terminals.close(&info.id).await.unwrap();
}
