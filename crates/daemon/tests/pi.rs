//! pi in the chat: the real `pi --mode rpc`, driven by Sandhill's bridge,
//! with its home in a temporary folder and a stand-in for an
//! OpenAI-compatible endpoint (configured as pi's user would, in its
//! `models.json`) that answers from a script. No model service is called and
//! the user's own pi settings are never read or written. Skips (and says so)
//! when pi isn't installed.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sandhill_daemon::agents::{self, Context};
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ChatEntry, ServerMessage};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(90);

/// What the bridge printed in its terminal, for when the test fails.
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The message the conversation goes on with once pi is started again.
const RESUMED: &str = "Go on";

/// pi has no permission modes, including old and unrecognized values.
const PERMISSIONS: &[Option<&str>] = &[
    None,
    Some("supervised"),
    Some("readonly"),
    Some("edits"),
    Some("auto"),
    Some("full"),
    Some("plan"),
    Some("restricted"),
    Some("ask"),
    Some("unknown"),
];

/// What the stand-in model does, one request after another.
fn script() -> Vec<Value> {
    let mut steps = vec![
        // Commands and Sandhill's MCP tools run without Sandhill approval.
        json!({"tool": "bash", "arguments": {"command": "echo made > made.txt"}}),
        json!({"tool": "mcp__sandhill__list_sessions", "arguments": {}}),
        json!({"text": "Done."}),
    ];
    for permission in PERMISSIONS {
        let name = permission.unwrap_or("default");
        steps.push(json!({"tool": "bash", "arguments": {
            "command": format!("echo {name} > {name}.txt")
        }}));
        steps.push(json!({"text": "Written."}));
    }
    // The last message takes until it is stopped.
    steps.push(json!("hang"));
    steps
}

/// Server-sent events of an OpenAI chat completion stream.
fn completion(n: usize, step: &Value) -> String {
    let chunk = |delta: Value, finish: Value| {
        json!({"id": format!("c{n}"), "object": "chat.completion.chunk", "created": 0, "model": "thinker",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    };
    let mut events = vec![chunk(
        json!({"role": "assistant", "content": ""}),
        Value::Null,
    )];
    if let Some(tool) = step["tool"].as_str() {
        events.push(chunk(
            json!({"tool_calls": [{"index": 0, "id": format!("call_{n}"), "type": "function",
                "function": {"name": tool, "arguments": step["arguments"].to_string()}}]}),
            Value::Null,
        ));
        events.push(chunk(json!({}), json!("tool_calls")));
    } else {
        events.push(chunk(json!({"content": step["text"]}), Value::Null));
        events.push(chunk(json!({}), json!("stop")));
    }
    let mut last = chunk(json!({}), Value::Null);
    last["choices"] = json!([]);
    last["usage"] = json!({"prompt_tokens": 1200, "completion_tokens": 30, "total_tokens": 1230});
    events.push(last);
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A stand-in for an OpenAI-compatible endpoint: answers each POST with
/// the next scripted step, and keeps the requests.
async fn model(script: Vec<Value>) -> (u16, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    tokio::spawn(async move {
        let script = Arc::new(script);
        while let Ok((mut stream, _)) = listener.accept().await {
            let script = script.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some((head, body)) = read_request(&mut stream).await else {
                    return;
                };
                if !head.starts_with("POST") {
                    let body = r#"{"object": "list", "data": []}"#;
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                    return;
                }
                let n = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(serde_json::from_slice(&body).unwrap_or(Value::Null));
                    seen.len() - 1
                };
                // The stopped turn may end before pi asks the model, so the
                // message after it is answered by what it says.
                let resumed = String::from_utf8_lossy(&body).contains(RESUMED);
                let step = script
                    .get(n)
                    .filter(|_| !resumed)
                    .cloned()
                    .unwrap_or_else(|| json!({"text": "(nothing more)"}));
                if step == "hang" {
                    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n";
                    let _ = stream.write_all(head.as_bytes()).await;
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    return;
                }
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{}",
                    completion(n, &step)
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, requests)
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&data[..end]).to_string();
        let length = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);
        while data.len() < end + 4 + length {
            let n = stream.read(&mut buf).await.ok()?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }
        return Some((head, data[end + 4..].to_vec()));
    }
}

/// pi's home for the test: its agent folder holds only the stand-in
/// endpoint, as a user's `models.json` would.
fn pi_home(root: &Path, port: u16) -> PathBuf {
    let home = root.join("home");
    let agent = home.join(".pi/agent");
    std::fs::create_dir_all(&agent).unwrap();
    let models = json!({"providers": {"standin": {
        "baseUrl": format!("http://127.0.0.1:{port}/v1"),
        "api": "openai-completions",
        "apiKey": "standin",
        "models": [
            {"id": "plain", "name": "Plain stand-in"},
            {"id": "thinker", "name": "Thinking stand-in", "reasoning": true},
        ],
    }}});
    std::fs::write(agent.join("models.json"), models.to_string()).unwrap();
    home
}

fn pi_env(home: &Path) -> Vec<(String, String)> {
    vec![
        ("HOME".into(), home.to_string_lossy().into()),
        (
            "PI_CODING_AGENT_DIR".into(),
            home.join(".pi/agent").to_string_lossy().into(),
        ),
        ("PI_OFFLINE".into(), "1".into()),
    ]
}

/// Collects the conversation until an entry of `kind` (after `after`
/// entries) comes.
async fn chat_until(
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    entries: &mut Vec<ChatEntry>,
    after: usize,
    kind: &str,
) -> Value {
    loop {
        if let Some(entry) = entries.iter().skip(after).find(|e| e.event["kind"] == kind) {
            return entry.event.clone();
        }
        let message = tokio::time::timeout(TIMEOUT, events.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for {kind}; got {:#?}\nterminal:\n{}",
                    entries.iter().map(|e| &e.event).collect::<Vec<_>>(),
                    String::from_utf8_lossy(&OUTPUT.lock().unwrap())
                )
            })
            .expect("stream ended");
        if let ServerMessage::Terminal { terminal } = message {
            if let Some(data) = &terminal.data {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                OUTPUT.lock().unwrap().extend(bytes);
            }
            if let Some(entry) = terminal.chat {
                entries.push(entry);
            }
            if let Some(exit) = terminal.exit {
                panic!(
                    "pi ended ({exit:?}) waiting for {kind}; got {entries:#?}\nterminal:\n{}",
                    String::from_utf8_lossy(&OUTPUT.lock().unwrap())
                );
            }
        }
    }
}

fn kinds(entries: &[ChatEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| e.event["kind"].as_str().unwrap().to_string())
        .collect()
}

async fn agent_until(terminals: &Terminals, id: &str, ready: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let agent = terminals.info(id).unwrap().agent.unwrap_or_default();
            if ready(&agent) {
                return agent;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "pi's report never got there: {:#?}\nterminal:\n{}",
            terminals.info(id).unwrap().agent,
            String::from_utf8_lossy(&OUTPUT.lock().unwrap())
        )
    })
}

async fn start_daemon(state: &Path) -> (Arc<Daemon>, PathBuf) {
    std::fs::create_dir_all(state).unwrap();
    let socket = state.join("test.sock");
    let options = DaemonOptions {
        executable: env!("CARGO_BIN_EXE_sandhilld").into(),
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
    (daemon, socket)
}

async fn say(terminals: &Terminals, id: &str, command: Value) {
    terminals
        .send(id, ToHost::Command { command })
        .await
        .unwrap();
}

#[tokio::test]
async fn pi_works_through_the_chat() {
    if agents::find("pi").is_none() {
        eprintln!("skipping: pi is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let (_daemon, socket) = start_daemon(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, requests) = model(script()).await;
    let home = pi_home(dir.path(), port);

    let options = AgentOptions {
        model: Some("standin/thinker".into()),
        effort: Some("high".into()),
        permission: Some("supervised".into()),
        ..AgentOptions::default()
    };
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    let project_name = project.to_string_lossy().into_owned();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some("Make a file"),
        resume: None,
    };
    let mut launch = agents::launch("pi", &options, &context).await.unwrap();
    launch.env.extend(pi_env(&home));
    let terminals = Terminals::new(dir.path().join("state/terminals"), exe.clone());
    let info = terminals.start(launch).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );

    // A command runs immediately, with no Sandhill approval card.
    let mut entries = Vec::new();
    chat_until(&mut events, &mut entries, 0, "stop").await;
    let step = entries
        .iter()
        .find(|e| e.event["kind"] == "step" && e.event["tool"] == "Bash")
        .unwrap()
        .event
        .clone();
    assert_eq!(step["input"]["command"], "echo made > made.txt");
    let ran = entries
        .iter()
        .find(|e| e.event["kind"] == "step_done" && e.event["id"] == step["id"])
        .unwrap();
    assert!(ran.event["output"]["stdout"].is_string(), "{:?}", ran.event);
    assert_eq!(
        std::fs::read_to_string(project.join("made.txt")).unwrap(),
        "made\n"
    );
    // Sandhill's MCP server is still available through pi's extension.
    let listed = entries
        .iter()
        .find(|e| e.event["kind"] == "step" && e.event["tool"] == "mcp__sandhill__list_sessions")
        .expect("Sandhill's tool step");
    assert!(
        entries
            .iter()
            .any(|e| e.event["kind"] == "step_done" && e.event["id"] == listed.event["id"]),
        "{:#?}",
        entries.iter().map(|e| &e.event).collect::<Vec<_>>()
    );
    assert!(!entries.iter().any(|e| e.event["kind"] == "ask"));
    let reply: String = entries
        .iter()
        .filter(|e| e.event["kind"] == "text")
        .filter_map(|e| e.event["text"].as_str())
        .collect();
    assert_eq!(reply, "Done.");
    assert!(!entries.iter().any(|e| e.event["kind"] == "error"));
    assert_eq!(entries[0].event["kind"], "session");
    assert_eq!(entries[0].event["source"], "startup");
    let prompt = entries
        .iter()
        .find(|e| e.event["kind"] == "prompt")
        .unwrap();
    assert_eq!(prompt.event["text"], "Make a file");

    // The model and effort the user picked, and pi's session as the key.
    let agent = agent_until(&terminals, &info.id, |a| a["state"] == "idle").await;
    assert_eq!(agent["status"]["model_id"], "standin/thinker");
    assert_eq!(agent["status"]["model"], "Thinking stand-in");
    assert_eq!(agent["status"]["effort"], "high");
    assert_eq!(agent["view_mode"], "chat");
    let thread = agents::resume_id("pi", Some(&agent)).expect("pi's session");
    let models: Vec<&str> = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(models, ["standin/plain", "standin/thinker"]);
    {
        let requests = requests.lock().unwrap();
        let first = &requests[0];
        assert_eq!(first["model"], "thinker");
        // Sandhill's instructions and tools went with it.
        assert!(
            first
                .to_string()
                .contains("You are running inside Sandhill")
        );
        let tools: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(tools.contains(&"bash"), "{tools:?}");
        assert!(tools.contains(&"mcp__sandhill__list_sessions"), "{tools:?}");
    }
    let terminal_dir = terminals.dir(&info.id).unwrap();
    assert!(terminal_dir.join("pi-sandhill.js").exists());

    // Every permission value is ignored: commands still execute, no Ask
    // instruction is appended, and no active plan permission is reported
    // to Sandhill's MCP tools.
    for permission in PERMISSIONS {
        say(
            &terminals,
            &info.id,
            json!({"kind": "set", "permission": permission}),
        )
        .await;
        let after = entries.len();
        say(
            &terminals,
            &info.id,
            json!({"kind": "say", "text": "Write another"}),
        )
        .await;
        chat_until(&mut events, &mut entries, after, "stop").await;
        let turn = &entries[after..];
        assert!(
            turn.iter()
                .any(|e| e.event["kind"] == "step_done" && e.event["tool"] == "Bash"),
            "permission {permission:?}: {turn:?}"
        );
        assert!(!turn.iter().any(|e| matches!(
            e.event["kind"].as_str(),
            Some("ask" | "step_failed" | "error")
        )));
        let name = permission.unwrap_or("default");
        assert_eq!(
            std::fs::read_to_string(project.join(format!("{name}.txt"))).unwrap(),
            format!("{name}\n")
        );
        let agent = agent_until(&terminals, &info.id, |a| a["state"] == "idle").await;
        assert!(agent["permission"].is_null());
    }
    assert!(
        !requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.to_string().contains("Sandhill is in Ask mode"))
    );

    // Stopped while it waits on the model.
    let after = entries.len();
    say(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "Take your time"}),
    )
    .await;
    chat_until(&mut events, &mut entries, after, "prompt").await;
    agent_until(&terminals, &info.id, |a| a["state"] == "busy").await;
    while requests.lock().unwrap().len() < 4 + PERMISSIONS.len() * 2 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    say(&terminals, &info.id, json!({"kind": "interrupt"})).await;
    chat_until(&mut events, &mut entries, after, "stop").await;
    agent_until(&terminals, &info.id, |a| a["state"] == "idle").await;

    // Started again, pi goes on with its session in the same chat, without
    // showing its turns twice.
    let before = terminals.info(&info.id).unwrap();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some(RESUMED),
        resume: Some(&thread),
    };
    let mut launch = agents::launch("pi", &options, &context).await.unwrap();
    launch.env.extend(pi_env(&home));
    let restarted = terminals
        .restart(&info.id, launch, &agents::RESUME_KEYS)
        .await
        .unwrap();
    assert!(restarted.running && restarted.exit.is_none());
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(before.end), Some(entries.len() as u64))
            .await
            .unwrap()
            .forward(out),
    );
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["session", "prompt", "text", "stop"]
    );
    assert_eq!(entries[after].event["source"], "resume");
    assert_eq!(entries[after + 1].event["text"], RESUMED);
    assert_eq!(entries[after + 2].event["text"], "(nothing more)");
    let resumed = requests.lock().unwrap().last().unwrap().to_string();
    assert!(
        resumed.contains("Make a file") && resumed.contains(RESUMED),
        "{resumed}"
    );
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        thread.as_str()
    );
    // Each of pi's replies is in Usage, with the tokens its model reported.
    let client = sandhill_proto::client::Client::connect_unix(&socket)
        .await
        .unwrap();
    let usage = client.usage_stats(2, 0).await.unwrap();
    // The message that was stopped never finished, so it has none.
    let finished = requests.lock().unwrap().len() as u64 - 1;
    assert_eq!(usage.totals.requests, finished, "{:?}", usage.totals);
    assert_eq!(
        usage.totals.input + usage.totals.cache_read,
        1200 * finished
    );
    assert_eq!(usage.totals.output, 30 * finished);
    assert_eq!(usage.models[0].agent, "pi", "{usage:?}");
    terminals.close(&info.id).await.unwrap();
}

/// Collects the chat and the terminal's output until `ready`.
async fn until(
    terminals: &Terminals,
    id: &str,
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    seen: &mut (Vec<Value>, String),
    ready: impl Fn(&[Value], &str, &Value) -> bool,
) {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let agent = terminals.info(id).unwrap().agent.unwrap_or_default();
            if ready(&seen.0, &seen.1, &agent) {
                return;
            }
            let Ok(message) = tokio::time::timeout(Duration::from_millis(200), events.recv()).await
            else {
                continue;
            };
            if let Some(ServerMessage::Terminal { terminal }) = message {
                if let Some(data) = &terminal.data {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap();
                    seen.1.push_str(&String::from_utf8_lossy(&bytes));
                }
                if let Some(entry) = terminal.chat {
                    seen.0.push(entry.event);
                }
                assert!(
                    terminal.exit.is_none(),
                    "pi ended: {:#?}\n{}",
                    seen.0,
                    seen.1
                );
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out: {:#?}\nreport: {:#?}\nterminal:\n{}",
            seen.0,
            terminals.info(id).unwrap().agent,
            seen.1
        )
    });
}

/// Command-line mode: pi's own interface takes the session in the same
/// terminal, a message typed there shows in the chat afterwards, and the
/// chat goes on with the session.
#[tokio::test]
async fn pi_hands_its_session_to_its_command_line_and_back() {
    if agents::find("pi").is_none() {
        eprintln!("skipping: pi is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let (_daemon, socket) = start_daemon(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, requests) = model(vec![
        json!({"text": "Hi from the chat."}),
        json!({"text": "Typed reply."}),
        json!({"text": "Back again."}),
    ])
    .await;
    let home = pi_home(dir.path(), port);
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    let project_name = project.to_string_lossy().into_owned();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some("Hello"),
        resume: None,
    };
    let options = AgentOptions::default();
    let mut launch = agents::launch("pi", &options, &context).await.unwrap();
    launch.env.extend(pi_env(&home));
    let terminals = Terminals::new(dir.path().join("state/terminals"), exe.clone());
    let info = terminals.start(launch).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut seen = (Vec::new(), String::new());
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut seen,
        |entries, _, agent| entries.iter().any(|e| e["kind"] == "stop") && agent["state"] == "idle",
    )
    .await;
    let thread = terminals.info(&info.id).unwrap().agent.unwrap()["thread"]
        .as_str()
        .unwrap()
        .to_string();

    say(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "terminal"}),
    )
    .await;
    let shown = seen.1.len();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut seen,
        |_, output, agent| agent["view_mode"] == "terminal" && output.len() > shown + 200,
    )
    .await;
    // pi's interface draws itself; then a message typed there.
    tokio::time::sleep(Duration::from_secs(2)).await;
    for data in ["From the command line", "\r"] {
        terminals
            .send(&info.id, ToHost::Input { data: data.into() })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut seen,
        |_, output, _| output.contains("Typed reply."),
    )
    .await;
    assert_eq!(requests.lock().unwrap().len(), 2);

    let after = seen.0.len();
    say(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "chat"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut seen,
        |entries, _, agent| {
            agent["view_mode"] == "chat"
                && agent["state"] == "idle"
                && entries[after..].iter().any(|e| e["kind"] == "stop")
        },
    )
    .await;
    // The turn made in pi's interface, once; the chat's own not again.
    let imported: Vec<&Value> = seen.0[after..].iter().collect();
    let prompts: Vec<&Value> = imported
        .iter()
        .filter(|e| e["kind"] == "prompt")
        .map(|e| &e["text"])
        .collect();
    assert_eq!(prompts, [&json!("From the command line")], "{imported:#?}");
    assert!(
        imported
            .iter()
            .any(|e| e["kind"] == "text" && e["text"] == "Typed reply.")
    );

    let after = seen.0.len();
    say(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "And now?"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut seen,
        |entries, _, agent| {
            entries[after..].iter().any(|e| e["kind"] == "stop") && agent["state"] == "idle"
        },
    )
    .await;
    let reply: String = seen.0[after..]
        .iter()
        .filter(|e| e["kind"] == "text")
        .filter_map(|e| e["text"].as_str())
        .collect();
    assert_eq!(reply, "Back again.");
    let last = requests.lock().unwrap().last().unwrap().to_string();
    assert!(
        last.contains("Hello") && last.contains("From the command line"),
        "{last}"
    );
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        thread.as_str()
    );
    terminals.close(&info.id).await.unwrap();
}

/// A new conversation's model menu lists pi's models before anything is
/// sent: the daemon asks a pi started only for that, which neither saves a
/// session nor calls a model.
#[tokio::test]
async fn pi_lists_its_models_before_a_conversation() {
    if agents::find("pi").is_none() {
        eprintln!("skipping: pi is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let (port, requests) = model(Vec::new()).await;
    let home = pi_home(dir.path(), port);
    // Its own daemon, so pi finds this home.
    let state = dir.path().join("state");
    let mut serve = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"));
    serve
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .envs(pi_env(&home))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let _serve = serve.spawn().unwrap();
    let mut client = None;
    for _ in 0..100 {
        if let Ok(c) =
            sandhill_proto::client::Client::connect_unix(&state.join("sandhilld.sock")).await
        {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let client = client.expect("sandhilld did not start");
    let models = client.agent_models("pi").await.unwrap();
    assert_eq!(
        models,
        json!([
            {"id": "standin/plain", "label": "Plain stand-in", "description": "standin", "default": true},
            {"id": "standin/thinker", "label": "Thinking stand-in", "description": "standin",
             "efforts": ["off", "minimal", "low", "medium", "high"]},
        ])
    );
    assert!(client.list_terminals().await.unwrap().is_empty());
    assert!(
        requests.lock().unwrap().is_empty(),
        "listing models sent a model request"
    );
    // No session was saved.
    let sessions = home.join(".pi/agent/sessions");
    let saved = walk(&sessions);
    assert!(saved.is_empty(), "{saved:?}");
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

/// User extensions still ask their own questions; only Sandhill's tool
/// approval hook is gone.
#[tokio::test]
async fn pi_keeps_native_extension_questions() {
    if agents::find("pi").is_none() {
        eprintln!("skipping: pi is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let (_daemon, socket) = start_daemon(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, _) = model(vec![
        json!({"tool": "bash", "arguments": {"command": "echo native > native.txt"}}),
        json!({"text": "Done."}),
    ])
    .await;
    let home = pi_home(dir.path(), port);
    let extensions = home.join(".pi/agent/extensions");
    std::fs::create_dir_all(&extensions).unwrap();
    std::fs::write(
        extensions.join("questions.js"),
        r#"
export default function (pi) {
  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName !== "bash") return;
    const selected = await ctx.ui.select("Native selection", ["First", "Second"]);
    const confirmed = await ctx.ui.confirm("Native confirmation", selected ?? "No selection");
    if (selected !== "Second" || !confirmed) {
      return { block: true, reason: "The native extension did not get its answers." };
    }
  });
}
"#,
    )
    .unwrap();
    let options = AgentOptions {
        model: Some("standin/plain".into()),
        permission: Some("plan".into()),
        ..AgentOptions::default()
    };
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    let project_name = project.to_string_lossy().into_owned();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some("Run the command"),
        resume: None,
    };
    let mut launch = agents::launch("pi", &options, &context).await.unwrap();
    launch.env.extend(pi_env(&home));
    let terminals = Terminals::new(dir.path().join("state/terminals"), exe);
    let info = terminals.start(launch).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    for (title, option_id) in [("Native selection", "1"), ("Native confirmation", "yes")] {
        let after = entries.len();
        let question = chat_until(&mut events, &mut entries, after, "ask").await;
        assert_eq!(question["tool"], title);
        assert!(!project.join("native.txt").exists());
        let agent = terminals.info(&info.id).unwrap().agent.unwrap();
        assert_eq!(agent["state"], "waiting");
        assert!(agent["permission"].is_null());
        terminals
            .answer(
                &info.id,
                question["id"].as_str().unwrap(),
                json!({"option_id": option_id}),
            )
            .await
            .unwrap();
    }
    chat_until(&mut events, &mut entries, 0, "stop").await;
    assert_eq!(
        std::fs::read_to_string(project.join("native.txt")).unwrap(),
        "native\n"
    );
    assert_eq!(
        entries.iter().filter(|e| e.event["kind"] == "ask").count(),
        2
    );
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e.event["kind"].as_str(), Some("error" | "step_failed")))
    );
    terminals.close(&info.id).await.unwrap();
}
