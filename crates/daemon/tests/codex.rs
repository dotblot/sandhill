//! Codex in the chat: the real `codex app-server`, driven by Sandhill's
//! bridge, with a stand-in for OpenAI's API that answers from a script, and
//! a real Python kernel for Sandhill's tools. Skips (and says so) when
//! Codex or the kernel isn't installed.

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

/// The message the conversation goes on with once Codex is started again.
const RESUMED: &str = "Go on";

/// What the stand-in model does, one request after another: the output
/// items of each response.
fn script() -> Vec<Value> {
    vec![
        // The first message: Codex thinks, then adds a cell with Sandhill's tool.
        json!([
            {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "**Adding a cell**"}]},
            {"type": "function_call", "id": "fc_1", "call_id": "call_1", "namespace": "mcp__sandhill",
             "name": "add_cell", "arguments": "{\"code\": \"print(6 * 7)\"}"}
        ]),
        json!([{"type": "message", "role": "assistant", "id": "msg_1",
                "content": [{"type": "output_text", "text": "The cell printed 42."}]}]),
        // The second, in YOLO mode: a command that runs without asking.
        json!([{"type": "function_call", "id": "fc_2", "call_id": "call_2", "name": "exec_command",
                "arguments": "{\"cmd\": \"echo made > made.txt\"}"}]),
        json!([{"type": "message", "role": "assistant", "id": "msg_2",
                "content": [{"type": "output_text", "text": "Made made.txt."}]}]),
        // The third, in plan mode: a plan, then carrying it out once approved.
        json!([{"type": "message", "role": "assistant", "id": "msg_3",
                "content": [{"type": "output_text", "text": "Here is the plan.\n<proposed_plan>\n1. Load\n2. Plot\n</proposed_plan>"}]}]),
        json!([{"type": "message", "role": "assistant", "id": "msg_4",
                "content": [{"type": "output_text", "text": "Done as planned."}]}]),
        // The fourth, in Restricted mode, and the fifth, in Ask mode: each
        // tries Sandhill's tool again.
        json!([{"type": "function_call", "id": "fc_3", "call_id": "call_3", "namespace": "mcp__sandhill",
                "name": "add_cell", "arguments": "{\"code\": \"print(1)\"}"}]),
        json!([{"type": "message", "role": "assistant", "id": "msg_5",
                "content": [{"type": "output_text", "text": "I won't, then."}]}]),
        json!([{"type": "function_call", "id": "fc_4", "call_id": "call_4", "namespace": "mcp__sandhill",
                "name": "add_cell", "arguments": "{\"code\": \"print(2)\"}"}]),
        json!([{"type": "message", "role": "assistant", "id": "msg_6",
                "content": [{"type": "output_text", "text": "Here is the code to add instead."}]}]),
        // The sixth takes until it is stopped.
        json!("hang"),
    ]
}

/// A stand-in for the Responses API: answers each POST with the next
/// scripted response as server-sent events, and keeps the requests.
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
                    let body = r#"{"data": [], "models": []}"#;
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
                // The stopped turn may end before Codex asks the model, so
                // the message after it is answered by what it says.
                let resumed = String::from_utf8_lossy(&body).contains(RESUMED);
                let items = script
                    .get(n)
                    .filter(|_| !resumed)
                    .cloned()
                    .unwrap_or_else(|| {
                        json!([{"type": "message", "role": "assistant", "id": format!("m{n}"),
                                "content": [{"type": "output_text", "text": "(nothing more)"}]}])
                    });
                let created =
                    json!({"type": "response.created", "response": {"id": format!("r{n}")}});
                if items == "hang" {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\nevent: response.created\ndata: {created}\n\n"
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    return;
                }
                let mut events = vec![created];
                for item in items.as_array().unwrap() {
                    events.push(json!({"type": "response.output_item.done", "item": item}));
                }
                events.push(json!({"type": "response.completed", "response": {"id": format!("r{n}"),
                    "usage": {"input_tokens": 1200, "input_tokens_details": {"cached_tokens": 0},
                              "output_tokens": 30, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 1230}}}));
                let mut reply = String::from(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                );
                for event in events {
                    reply.push_str(&format!(
                        "event: {}\ndata: {event}\n\n",
                        event["type"].as_str().unwrap()
                    ));
                }
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
                    "timed out waiting for {kind}; got {:?}\nterminal:\n{}",
                    kinds(entries),
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
                    "Codex ended ({exit:?}) waiting for {kind}; got {entries:#?}\nterminal:\n{}",
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

#[tokio::test]
async fn codex_works_through_the_chat() {
    if agents::find("codex").is_none() {
        eprintln!("skipping: codex is not installed");
        return;
    }
    if sandhill_daemon::kernel::find_kernelspec("python3")
        .await
        .is_err()
    {
        eprintln!("skipping: Jupyter kernel \"python3\" is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_daemon, socket) = start_daemon(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let notebook = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "id": "load", "metadata": {}, "outputs": [], "source": ["n = 7"]}],
        "metadata": {"kernelspec": {"name": "python3", "display_name": "Python 3", "language": "python"}},
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    std::fs::write(project.join("qc.ipynb"), notebook.to_string()).unwrap();

    // Codex's own settings, as a user's would be: here, the stand-in model.
    let (port, requests) = model(script()).await;
    let home = dir.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        format!(
            "model = \"gpt-5.5\"\nmodel_provider = \"standin\"\n\n[model_providers.standin]\nname = \"Stand-in\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nwire_api = \"responses\"\n"
        ),
    )
    .unwrap();

    let options = AgentOptions {
        permission: Some("supervised".into()),
        documents: vec!["qc.ipynb".into()],
        ..AgentOptions::default()
    };
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    let project_name = project.to_string_lossy().into_owned();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some("Add a cell that prints 6 times 7"),
        resume: None,
    };
    let mut launch = agents::launch("codex", &options, &context).await.unwrap();
    launch
        .env
        .push(("CODEX_HOME".into(), home.to_string_lossy().into()));
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

    // Codex asks to use add_cell, on the step that shows it.
    let mut entries = Vec::new();
    let ask = chat_until(&mut events, &mut entries, 0, "ask").await;
    assert_eq!(ask["tool"], "mcp__sandhill__add_cell");
    assert_eq!(ask["input"]["code"], "print(6 * 7)");
    let step = entries
        .iter()
        .find(|e| e.event["kind"] == "step" && e.event["tool"] == ask["tool"])
        .unwrap();
    assert_eq!(ask["step"], step.event["id"]);
    assert_eq!(ask["scopes"], json!(["chat", "project"]));
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "waiting");
    assert_eq!(agent["permission"], "supervised");

    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            json!({"behavior": "allow", "scope": "once"}),
        )
        .await
        .unwrap();
    chat_until(&mut events, &mut entries, 0, "stop").await;
    assert_eq!(
        kinds(&entries),
        [
            "session",
            "prompt",
            "step",
            "step_done",
            "step",
            "ask",
            "answered",
            "step_done",
            "text",
            "stop"
        ],
        "{entries:#?}\n{}",
        requests
            .lock()
            .unwrap()
            .get(1)
            .map(|r| r["input"].to_string())
            .unwrap_or_default()
    );
    // The conversation's start, which the app waits for, names the model.
    assert_eq!(entries[0].event["source"], "startup");
    assert_eq!(entries[0].event["model"], "gpt-5.5");
    let prompt = &entries[1].event;
    assert_eq!(prompt["text"], "Add a cell that prints 6 times 7");
    // Its thinking, then the cell, which ran in the user's kernel.
    assert_eq!(entries[2].event["tool"], "reasoning");
    assert_eq!(entries[3].event["output"]["text"], "**Adding a cell**");
    let added = entries[7].event["output"].to_string();
    assert!(added.contains("42"), "{added}");
    assert_eq!(entries[8].event["text"], "The cell printed 42.");
    assert!(
        entries
            .iter()
            .skip(1)
            .filter(|e| e.event["kind"] != "answered")
            .all(|e| e.event["turn"] == prompt["turn"])
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("qc.ipynb")).unwrap()).unwrap();
    assert_eq!(written["cells"][1]["source"], json!(["print(6 * 7)"]));
    // Codex was told about Sandhill.
    let first = requests.lock().unwrap()[0].to_string();
    assert!(first.contains("You are running inside Sandhill"), "{first}");
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "idle");
    assert!(
        agent["status"]["context"].as_f64().unwrap() > 0.0,
        "{agent}"
    );

    // YOLO from now on, then another message: the command runs without asking.
    for command in [
        json!({"kind": "set", "permission": "full"}),
        json!({"kind": "say", "text": "Make a file"}),
    ] {
        terminals
            .send(&info.id, ToHost::Command { command })
            .await
            .unwrap();
    }
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["prompt", "step", "step_done", "text", "stop"]
    );
    assert_eq!(entries[after + 1].event["tool"], "Bash");
    assert_eq!(
        entries[after + 1].event["input"]["command"],
        "echo made > made.txt"
    );
    assert!(project.join("made.txt").exists());
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["permission"], "full");

    // Plan mode: Codex's plan is put to the user, as Claude Code's is.
    for command in [
        json!({"kind": "set", "permission": "plan"}),
        json!({"kind": "say", "text": "Plan a plot"}),
    ] {
        terminals
            .send(&info.id, ToHost::Command { command })
            .await
            .unwrap();
    }
    let after = entries.len();
    let ask = chat_until(&mut events, &mut entries, after, "ask").await;
    assert_eq!(ask["tool"], "ExitPlanMode");
    assert_eq!(ask["input"]["plan"], "1. Load\n2. Plot\n");
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            json!({"behavior": "allow"}),
        )
        .await
        .unwrap();
    let planned = chat_until(&mut events, &mut entries, after, "stop").await;
    // Carrying it out is a turn of its own, with no message from the user.
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(kinds(&entries[after..]), ["text", "stop"]);
    assert_ne!(entries[after].event["turn"], planned["turn"]);
    assert_eq!(entries[after].event["text"], "Done as planned.");
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["permission"], "supervised");
    let carried = requests.lock().unwrap()[5].to_string();
    assert!(carried.contains("Carry out the plan."), "{carried}");

    // Restricted: Codex asks before using Sandhill's tool, and the user
    // says no.
    for command in [
        json!({"kind": "set", "permission": "restricted"}),
        json!({"kind": "say", "text": "Add another cell"}),
    ] {
        terminals
            .send(&info.id, ToHost::Command { command })
            .await
            .unwrap();
    }
    let after = entries.len();
    let ask = chat_until(&mut events, &mut entries, after, "ask").await;
    assert_eq!(ask["tool"], "mcp__sandhill__add_cell");
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["permission"], "restricted");
    assert_eq!(agent["guard"], "restricted");
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            json!({"behavior": "deny"}),
        )
        .await
        .unwrap();
    chat_until(&mut events, &mut entries, after, "stop").await;

    // Ask: read-only. The tool call fails without asking (Codex refuses it
    // itself; Sandhill's tool would too), and Codex was told to suggest
    // instead.
    for command in [
        json!({"kind": "set", "permission": "ask"}),
        json!({"kind": "say", "text": "Add it anyway"}),
    ] {
        terminals
            .send(&info.id, ToHost::Command { command })
            .await
            .unwrap();
    }
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["prompt", "step", "step_failed", "text", "stop"],
        "{:#?}",
        &entries[after..]
    );
    assert_eq!(entries[after + 2].event["tool"], "mcp__sandhill__add_cell");
    let asked = requests.lock().unwrap()[8]["input"].to_string();
    assert!(asked.contains("Sandhill is in Ask mode"), "{asked}");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("qc.ipynb")).unwrap()).unwrap();
    assert_eq!(written["cells"].as_array().unwrap().len(), 2);
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: json!({"kind": "set", "permission": "supervised"}),
            },
        )
        .await
        .unwrap();

    // Stopping a turn.
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: json!({"kind": "say", "text": "Take your time"}),
            },
        )
        .await
        .unwrap();
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "prompt").await;
    while terminals.info(&info.id).unwrap().agent.unwrap()["state"] != "busy" {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: json!({"kind": "interrupt"}),
            },
        )
        .await
        .unwrap();
    chat_until(&mut events, &mut entries, after, "stop").await;
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "idle");

    // Started again (as when switching it, or after it ended), Codex goes
    // on with its thread, in the same chat.
    let before = terminals.info(&info.id).unwrap();
    let thread = agents::resume_id("codex", before.agent.as_ref()).unwrap();
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: Some(RESUMED),
        resume: Some(&thread),
    };
    let mut launch = agents::launch("codex", &options, &context).await.unwrap();
    launch
        .env
        .push(("CODEX_HOME".into(), home.to_string_lossy().into()));
    let restarted = terminals
        .restart(&info.id, launch, &agents::RESUME_KEYS)
        .await
        .unwrap();
    assert!(restarted.running && restarted.exit.is_none());
    assert!(restarted.started_ms > before.started_ms);
    assert_eq!(restarted.created_ms, before.created_ms);
    assert_eq!(restarted.agent.unwrap()["thread"], thread.as_str());
    let (out, mut events) = mpsc::unbounded_channel();
    let stream = terminals
        .open(&info.id, Some(before.end), Some(entries.len() as u64))
        .await
        .unwrap();
    // Its output goes on from the old program's.
    assert_eq!(stream.from, before.end);
    tokio::spawn(stream.forward(out));
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["session", "prompt", "text", "stop"]
    );
    assert_eq!(entries[after].seq, after as u64);
    assert_eq!(entries[after].event["source"], "resume");
    assert_eq!(entries[after + 1].event["text"], RESUMED);
    assert_eq!(entries[after + 2].event["text"], "(nothing more)");
    // Codex had the conversation so far.
    let resumed = requests.lock().unwrap().last().unwrap()["input"].to_string();
    assert!(
        resumed.contains("Make a file") && resumed.contains(RESUMED),
        "{resumed}"
    );
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        thread.as_str()
    );

    terminals.close(&info.id).await.unwrap();
}
