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
async fn a_terminal_followed_while_it_starts_is_followed_live() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Arc::new(Terminals::new(
        dir.path().join("terminals"),
        sandhilld().into(),
    ));
    let starting = tokio::spawn({
        let terminals = terminals.clone();
        let cwd = dir.path().to_path_buf();
        async move {
            terminals
                .start(sh("read line; echo \"got $line\"", &cwd))
                .await
                .unwrap()
        }
    });
    // A window lists the terminals as this one starts, and follows it.
    let id = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(info) = terminals.list().pop() {
                break info.id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    let stream = terminals.open(&id, Some(0), None).await.unwrap();
    tokio::spawn(stream.forward(out));
    starting.await.unwrap();
    terminals
        .send(
            &id,
            ToHost::Input {
                data: "hello\n".into(),
            },
        )
        .await
        .unwrap();
    let mut output = Vec::new();
    let exit = read_until(&mut events, &mut output, "got hello").await;
    assert!(exit.is_none(), "followed as ended: {exit:?}");
    terminals.close(&id).await.unwrap();
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
    let stream = terminals.open(&info.id, Some(0), None).await.unwrap();
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
        .open(&info.id, None, None)
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
    tokio::spawn(
        terminals
            .open(&id, Some(0), None)
            .await
            .unwrap()
            .forward(out),
    );
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
            .open(&info.id, Some(0), None)
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
            prompt: None,
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
    // `pwd` prints last, so wait for it too: it can come in a later chunk.
    let folder = project.to_string_lossy().to_string();
    while ![expected.as_str(), "40 120", &folder]
        .iter()
        .all(|part| String::from_utf8_lossy(&output).contains(part))
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

    let listed = client.list_terminals().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].running);
    assert_eq!(
        listed[0].project.as_deref(),
        Some(&*project.to_string_lossy())
    );

    // Started again: a new shell in the same terminal and folder.
    let again: TerminalInfo = client
        .call(Call::RestartTerminal {
            terminal: info.id.clone(),
            options: None,
            cols: Some(80),
            rows: Some(24),
            prompt: None,
        })
        .await
        .unwrap();
    assert_eq!(
        (again.id.as_str(), again.cwd.as_str()),
        (info.id.as_str(), info.cwd.as_str())
    );
    assert!(again.running && again.started_ms >= info.started_ms);
    assert!(again.end >= output.len() as u64);

    // Named, as the user renames a conversation, and unnamed again.
    let named: TerminalInfo = client
        .call(Call::RenameTerminal {
            terminal: info.id.clone(),
            name: Some(" PBMC markers ".into()),
        })
        .await
        .unwrap();
    assert_eq!(named.name.as_deref(), Some("PBMC markers"));
    assert_eq!(
        client.list_terminals().await.unwrap()[0].name.as_deref(),
        Some("PBMC markers")
    );
    let unnamed: TerminalInfo = client
        .call(Call::RenameTerminal {
            terminal: info.id.clone(),
            name: Some(" ".into()),
        })
        .await
        .unwrap();
    assert_eq!(unnamed.name, None);

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
            .open(&info.id, Some(0), None)
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

#[tokio::test]
async fn a_terminal_starts_again_where_it_left_off() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    // Stands in for Claude Code: its Stop hook ends a turn of the
    // conversation and says which session it is.
    let stop = |turn: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"Stop\",\"prompt_id\":\"{turn}\",\"session_id\":\"s1\",\"permission_mode\":\"plan\"}}' | '{}' terminal-event hook",
            sandhilld().display()
        )
    };
    let first = format!("{}; echo first; read wait", stop("p1"));
    let info = terminals.start(sh(&first, dir.path())).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut output = Vec::new();
    read_until(&mut events, &mut output, "first").await;
    let before = terminals.info(&info.id).unwrap();
    let agent = before.agent.clone().unwrap();
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["session_id"], "s1");

    // Started again while it runs: the old program is ended first.
    let second = format!("echo second; read go; {}; read wait", stop("p2"));
    let mut launch = sh(&second, dir.path());
    launch.options.permission = Some("full".into());
    let again = terminals
        .restart(&info.id, launch, &["session_id"])
        .await
        .unwrap();
    assert!(
        read_until(&mut events, &mut output, "\u{0}")
            .await
            .is_some()
    );
    assert!(again.running && again.exit.is_none());
    assert_eq!(again.created_ms, before.created_ms);
    assert!(again.started_ms >= before.started_ms);
    assert_eq!(again.options.permission.as_deref(), Some("full"));
    // Of the old report, only what finds the conversation again stays.
    assert_eq!(
        again.agent.as_ref().map(|a| a["session_id"].clone()),
        Some("s1".into())
    );
    assert!(again.agent.as_ref().unwrap().get("mode").is_none());

    // Its output goes on from the old program's, and so does the conversation.
    let (out, mut events) = mpsc::unbounded_channel();
    let stream = terminals
        .open(&info.id, Some(before.end), Some(1))
        .await
        .unwrap();
    assert_eq!(stream.from, before.end);
    tokio::spawn(stream.forward(out));
    let mut more = Vec::new();
    read_until(&mut events, &mut more, "second").await;
    terminals
        .send(&info.id, ToHost::Input { data: "\n".into() })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let chat = loop {
        let chat =
            sandhill_daemon::terminal::chat::read(&dir.path().join("terminals").join(&info.id), 0);
        if chat.len() == 2 || tokio::time::Instant::now() > deadline {
            break chat;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let turns: Vec<_> = chat
        .iter()
        .map(|e| (e.seq, e.event["turn"].clone()))
        .collect();
    assert_eq!(turns, [(0, "p1".into()), (1, "p2".into())]);
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["mode"], "plan");

    // An ended program starts again the same way.
    terminals.send(&info.id, ToHost::Hangup).await.unwrap();
    assert!(read_until(&mut events, &mut more, "\u{0}").await.is_some());
    let ended = terminals.info(&info.id).unwrap();
    assert!(!ended.running && ended.exit.is_some());
    let again = terminals
        .restart(&info.id, sh("echo third; read wait", dir.path()), &[])
        .await
        .unwrap();
    assert!(again.running && again.exit.is_none() && again.agent.is_none());
    assert!(again.end >= ended.end);
    terminals.close(&info.id).await.unwrap();
}

#[tokio::test]
async fn agents_started_by_an_older_build_are_marked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("terminals");
    let terminals = Terminals::new(root.clone(), sandhilld().into());
    let agent = Launch {
        kind: "claude".into(),
        ..sh("read wait", dir.path())
    };
    let info = terminals.start(agent).await.unwrap();
    assert!(!info.outdated);
    let shell = terminals.start(sh("read wait", dir.path())).await.unwrap();

    // As if an update replaced the daemon after these started: the agent
    // keeps the old build's tools, a shell has none to miss.
    for id in [&info.id, &shell.id] {
        let path = root.join(id).join("meta.json");
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["build"] = "0123456789abcdef".into();
        std::fs::write(&path, meta.to_string()).unwrap();
    }
    let listed = terminals.list();
    let outdated = |id: &str| listed.iter().find(|t| t.id == id).unwrap().outdated;
    assert!(outdated(&info.id));
    assert!(!outdated(&shell.id));

    terminals.close(&info.id).await.unwrap();
    terminals.close(&shell.id).await.unwrap();
}

/// Reads entries of the agent's conversation until one of `kind` arrives.
/// Collects a terminal's conversation until an entry of `kind` comes, and
/// its output meanwhile: the two arrive in either order.
async fn chat_until(
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    entries: &mut Vec<sandhill_proto::ChatEntry>,
    output: &mut Vec<u8>,
    kind: &str,
) {
    let engine = base64::engine::general_purpose::STANDARD;
    while !entries.iter().any(|e| e.event["kind"] == kind) {
        let message = tokio::time::timeout(TIMEOUT, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {kind}; got {entries:?}"))
            .expect("stream ended");
        let ServerMessage::Terminal { terminal } = message else {
            continue;
        };
        if let Some(data) = terminal.data {
            output.extend(engine.decode(data).unwrap());
        }
        if let Some(entry) = terminal.chat {
            entries.push(entry);
        }
    }
}

#[tokio::test]
async fn an_agents_conversation_is_kept_and_its_questions_answered() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    // Stands in for Claude Code: runs its hooks with what Claude Code gave
    // them in a recorded turn. The permission request waits for an answer.
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/claude-hooks.jsonl");
    let script = format!(
        "n=0; while IFS= read -r line; do printf '%s' \"$line\" | '{exe}' --socket '{sock}' terminal-event hook > out.$n; n=$((n+1)); done < '{fixture}'; echo done; read wait",
        exe = sandhilld().display(),
        sock = dir.path().join("no-daemon.sock").display(),
        fixture = fixture.display(),
    );
    let info = terminals.start(sh(&script, dir.path())).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    let mut output = Vec::new();
    chat_until(&mut events, &mut entries, &mut output, "ask").await;
    let ask = entries.last().unwrap().event.clone();
    assert_eq!(ask["tool"], "Bash");
    // The agent waits for the user.
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "waiting");

    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            serde_json::json!({"behavior": "allow", "scope": "chat"}),
        )
        .await
        .unwrap();
    chat_until(&mut events, &mut entries, &mut output, "stop").await;
    read_until(&mut events, &mut output, "done").await;
    let kinds: Vec<&str> = entries
        .iter()
        .map(|e| e.event["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "session",
            "prompt",
            "step",
            "ask",
            "answered",
            "step_done",
            "text",
            "text",
            "text",
            "stop"
        ]
    );
    assert!(entries.iter().enumerate().all(|(i, e)| e.seq == i as u64));
    assert_eq!(entries[4].event["answer"]["scope"], "chat");
    // Claude Code was told to allow it for this conversation.
    let decision: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("out.3")).unwrap()).unwrap();
    let decision = &decision["hookSpecificOutput"]["decision"];
    assert_eq!(decision["behavior"], "allow");
    assert_eq!(decision["updatedPermissions"][0]["destination"], "session");
    // Hooks print nothing else.
    assert!(std::fs::read(dir.path().join("out.2")).unwrap().is_empty());

    // A client that comes later gets the rest from where it asks.
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, None, Some(6))
            .await
            .unwrap()
            .forward(out),
    );
    let mut later = Vec::new();
    chat_until(&mut events, &mut later, &mut Vec::new(), "stop").await;
    assert_eq!(later.first().unwrap().seq, 6);
    assert_eq!(later.len(), 4);

    // And after the agent has ended.
    terminals
        .send(&info.id, ToHost::Input { data: "\n".into() })
        .await
        .unwrap();
    let mut output = Vec::new();
    assert!(
        read_until(&mut events, &mut output, "never")
            .await
            .is_some()
    );
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, None, Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut ended = Vec::new();
    chat_until(&mut events, &mut ended, &mut Vec::new(), "stop").await;
    assert_eq!(ended.len(), 10);
    terminals.close(&info.id).await.unwrap();
}

/// Collects a terminal's conversation and output until `done` says so.
async fn collect(
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    entries: &mut Vec<sandhill_proto::ChatEntry>,
    output: &mut Vec<u8>,
    done: impl Fn(&[sandhill_proto::ChatEntry], &[u8]) -> bool,
) {
    let engine = base64::engine::general_purpose::STANDARD;
    while !done(entries, output) {
        let message = tokio::time::timeout(TIMEOUT, events.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out; got {entries:?}\n{}",
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
        if let Some(entry) = terminal.chat {
            entries.push(entry);
        }
    }
}

#[tokio::test]
async fn restricted_and_ask_modes_guard_claudes_steps() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    // Stands in for Claude Code: runs its hooks with what it would give
    // them, in Ask mode, then (once the user switches) in Restricted mode.
    let hook = |event: &str, id: &str, tool: &str, input: serde_json::Value| {
        let mut line = serde_json::json!({
            "hook_event_name": event,
            "prompt_id": "p1",
            "session_id": "s1",
            "permission_mode": "default",
        });
        if event == "PreToolUse" {
            line["tool_use_id"] = id.into();
            line["tool_name"] = tool.into();
            line["tool_input"] = input;
        } else {
            line["prompt"] = "What next?".into();
        }
        format!("{line}\n")
    };
    let edit = serde_json::json!({"file_path": "/data/a.R", "old_string": "a", "new_string": "b"});
    let read = serde_json::json!({"file_path": "/data/a.R"});
    let asked = [
        hook("PreToolUse", "t1", "Edit", edit),
        hook("PreToolUse", "t2", "Read", read.clone()),
        hook("UserPromptSubmit", "", "", serde_json::Value::Null),
    ];
    let restricted = [
        hook("PreToolUse", "t3", "Read", read),
        hook(
            "PreToolUse",
            "t4",
            "Bash",
            serde_json::json!({"command": "ls"}),
        ),
    ];
    std::fs::write(dir.path().join("ask.jsonl"), asked.concat()).unwrap();
    std::fs::write(dir.path().join("restricted.jsonl"), restricted.concat()).unwrap();
    let script = format!(
        "n=0; run() {{ while IFS= read -r line; do printf '%s' \"$line\" | '{exe}' --socket '{sock}' terminal-event hook > out.$n; n=$((n+1)); done < \"$1\"; }}; run ask.jsonl; echo asked; read go; run restricted.jsonl; echo done; read wait",
        exe = sandhilld().display(),
        sock = dir.path().join("no-daemon.sock").display(),
    );
    let mut launch = sh(&script, dir.path());
    launch.options.permission = Some("ask".into());
    let info = terminals.start(launch).await.unwrap();
    // It starts in the mode it was started in.
    assert_eq!(info.agent.unwrap()["guard"], "ask");
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    let mut output = Vec::new();
    // The conversation's entries and the output arrive in either order.
    collect(&mut events, &mut entries, &mut output, |e, out| {
        String::from_utf8_lossy(out).contains("asked")
            && e.iter().any(|e| e.event["kind"] == "prompt")
            && e.iter().any(|e| e.event["kind"] == "step_failed")
    })
    .await;
    let out = |n: usize| std::fs::read_to_string(dir.path().join(format!("out.{n}"))).unwrap();
    // Ask mode blocks the edit, in the chat too, and lets the read go on.
    let decision: serde_json::Value = serde_json::from_str(&out(0)).unwrap();
    let decision = &decision["hookSpecificOutput"];
    assert_eq!(decision["hookEventName"], "PreToolUse");
    assert_eq!(decision["permissionDecision"], "deny");
    assert!(
        decision["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("Ask mode")
    );
    let failed = entries
        .iter()
        .find(|e| e.event["kind"] == "step_failed")
        .unwrap();
    assert_eq!(failed.event["id"], "t1");
    assert!(failed.event["error"].as_str().unwrap().contains("Ask mode"));
    assert_eq!(out(1), "");
    // Each message tells Claude it is read-only.
    assert!(out(2).contains("Ask mode"), "{}", out(2));

    // The user switches to Restricted: every step asks first.
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: serde_json::json!({"kind": "set", "permission": "restricted"}),
            },
        )
        .await
        .unwrap();
    let guard = async || terminals.info(&info.id).unwrap().agent.unwrap()["guard"].clone();
    tokio::time::timeout(TIMEOUT, async {
        while guard().await != "restricted" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminals
        .send(&info.id, ToHost::Input { data: "\n".into() })
        .await
        .unwrap();
    let asks = |entries: &[sandhill_proto::ChatEntry]| {
        entries
            .iter()
            .filter(|e| e.event["kind"] == "ask")
            .map(|e| e.event.clone())
            .collect::<Vec<_>>()
    };
    collect(&mut events, &mut entries, &mut output, |e, _| {
        asks(e).len() == 1
    })
    .await;
    let ask = asks(&entries).remove(0);
    assert_eq!(ask["tool"], "Read");
    // On its step, and only once at a time.
    assert_eq!(ask["step"], "t3");
    assert_eq!(ask["scopes"], serde_json::json!([]));
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["state"],
        "waiting"
    );
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            serde_json::json!({"behavior": "deny"}),
        )
        .await
        .unwrap();
    collect(&mut events, &mut entries, &mut output, |e, _| {
        asks(e).len() == 2
    })
    .await;
    let ask = asks(&entries).remove(1);
    assert_eq!(ask["tool"], "Bash");
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            serde_json::json!({"behavior": "allow", "scope": "once"}),
        )
        .await
        .unwrap();
    collect(&mut events, &mut entries, &mut output, |_, out| {
        String::from_utf8_lossy(out).contains("done")
    })
    .await;
    let decision = |n: usize| -> serde_json::Value {
        let output: serde_json::Value = serde_json::from_str(&out(n)).unwrap();
        output["hookSpecificOutput"].clone()
    };
    assert_eq!(decision(3)["permissionDecision"], "deny");
    assert!(
        decision(3)["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("said no")
    );
    assert_eq!(decision(4)["permissionDecision"], "allow");

    // Back to Normal: the guard is gone.
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: serde_json::json!({"kind": "set", "permission": "supervised"}),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(TIMEOUT, async {
        while !guard().await.is_null() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminals.close(&info.id).await.unwrap();
}

#[tokio::test]
async fn the_message_boxs_choices_go_with_each_message() {
    let dir = tempfile::tempdir().unwrap();
    let terminals = Terminals::new(dir.path().join("terminals"), sandhilld().into());
    // Stands in for Claude Code: a message, then a web search and a read.
    let hook = |event: &str, id: &str, tool: &str| {
        let mut line = serde_json::json!({
            "hook_event_name": event,
            "prompt_id": "p1",
            "session_id": "s1",
            "permission_mode": "default",
        });
        if event == "PreToolUse" {
            line["tool_use_id"] = id.into();
            line["tool_name"] = tool.into();
            line["tool_input"] = serde_json::json!({"query": "PBMC markers"});
        } else {
            line["prompt"] = "What is in seu?".into();
        }
        format!("{line}\n")
    };
    let lines = [
        hook("UserPromptSubmit", "", ""),
        hook("PreToolUse", "t1", "WebSearch"),
        hook("PreToolUse", "t2", "Read"),
    ];
    std::fs::write(dir.path().join("hooks.jsonl"), lines.concat()).unwrap();
    let script = format!(
        "echo ready; read go; n=0; while IFS= read -r line; do printf '%s' \"$line\" | '{exe}' --socket '{sock}' terminal-event hook > out.$n; n=$((n+1)); done < hooks.jsonl; echo done; read wait",
        exe = sandhilld().display(),
        sock = dir.path().join("no-daemon.sock").display(),
    );
    let mut launch = sh(&script, dir.path());
    launch.options.pin = Some("fig3.qmd".into());
    launch.options.web = Some(false);
    let info = terminals.start(launch).await.unwrap();
    // It starts with the choices it was started with.
    let agent = info.agent.unwrap();
    assert_eq!(agent["pin"], "fig3.qmd");
    assert_eq!(agent["web"], false);
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    let mut output = Vec::new();
    collect(&mut events, &mut entries, &mut output, |_, out| {
        String::from_utf8_lossy(out).contains("ready")
    })
    .await;
    // The user adds a variable to the message with @, then sends it.
    let refs = serde_json::json!([{"kind": "variable", "name": "seu", "session": "r-7f3a", "language": "r"}]);
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: serde_json::json!({"kind": "set", "refs": refs}),
            },
        )
        .await
        .unwrap();
    let report = || terminals.info(&info.id).unwrap().agent.unwrap();
    tokio::time::timeout(TIMEOUT, async {
        while report()["refs"].is_null() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminals
        .send(&info.id, ToHost::Input { data: "\n".into() })
        .await
        .unwrap();
    collect(&mut events, &mut entries, &mut output, |e, out| {
        String::from_utf8_lossy(out).contains("done")
            && e.iter().any(|e| e.event["kind"] == "prompt")
            && e.iter().any(|e| e.event["kind"] == "step_failed")
    })
    .await;
    let out = |n: usize| std::fs::read_to_string(dir.path().join(format!("out.{n}"))).unwrap();
    // The message names the pinned file and what was added, and says the web is off.
    let told = out(0);
    assert!(
        told.contains("pinned this conversation to fig3.qmd"),
        "{told}"
    );
    assert!(
        told.contains("the R variable `seu` in session r-7f3a"),
        "{told}"
    );
    assert!(told.contains("web search off"), "{told}");
    let prompt = entries
        .iter()
        .find(|e| e.event["kind"] == "prompt")
        .unwrap();
    assert_eq!(prompt.event["place"]["refs"], serde_json::json!(["seu"]));
    // The message took the refs and works in the pinned file.
    tokio::time::timeout(TIMEOUT, async {
        while report()["document"] != "fig3.qmd" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(report()["refs"].is_null());
    // The web search is stopped, in the chat too; the read goes on.
    let decision: serde_json::Value = serde_json::from_str(&out(1)).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
    let failed = entries
        .iter()
        .find(|e| e.event["kind"] == "step_failed")
        .unwrap();
    assert_eq!(failed.event["id"], "t1");
    assert!(
        failed.event["error"]
            .as_str()
            .unwrap()
            .contains("web search is off")
    );
    assert_eq!(out(2), "");

    // Unpinned and with the web back on.
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: serde_json::json!({"kind": "set", "pin": null, "web": true}),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(TIMEOUT, async {
        while !report()["pin"].is_null() || report()["web"] != true {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    terminals.close(&info.id).await.unwrap();
}
