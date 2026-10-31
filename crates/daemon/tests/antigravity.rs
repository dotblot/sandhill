//! Antigravity in the chat: Sandhill's bridge driving a stand-in for
//! Google's `agy` CLI that speaks its documented headless mode (NDJSON on
//! stdout, the message on stdin, `agy mcp add` for MCP servers), and uses
//! Sandhill's real MCP server with a real Python kernel. The real `agy`
//! needs a Google sign-in, so it isn't run here. Skips (and says so) when
//! Python or the kernel isn't installed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sandhill_daemon::agents::{self, Context};
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ChatEntry, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(90);

/// What the bridge printed in its terminal, for when the test fails.
static OUTPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// A stand-in for `agy`, as its headless mode is documented: each message
/// is a run, with `init`, `step_update` and `result` events. It keeps what
/// it was run with in `$STANDIN_LOG`, and its MCP servers in
/// `$STANDIN_CONFIG`.
const STANDIN: &str = r#"#!/usr/bin/env python3
import json, os, signal, subprocess, sys, time

args = sys.argv[1:]
config = os.environ.get("STANDIN_CONFIG", "")

def record(entry):
    with open(os.environ["STANDIN_LOG"], "a") as f:
        f.write(json.dumps(entry) + "\n")

def servers():
    return json.load(open(config)) if config and os.path.exists(config) else {}

if args[:1] == ["--help"]:
    print("Usage: agy [flags]\n  --output-format\n  --print-timeout\n  --model\n  --effort\n  --conversation\n  --mode\n  --dangerously-skip-permissions")
    sys.exit(0)
if args[:1] == ["--version"]:
    print("agy version 2.0.0")
    sys.exit(0)
if args[:1] == ["models"]:
    print("Available models:\n  gemini-3.6-flash-high (default)\n  gemini-3.1-pro-low")
    sys.exit(0)
if args[:2] == ["mcp", "list"]:
    for name, server in servers().items():
        print(name, server["command"], " ".join(server["args"]))
    sys.exit(0)
if args[:2] == ["mcp", "add"]:
    rest = args[2:]
    name, rest = rest[0], rest[1:]
    if rest[:1] == ["--"]:
        rest = rest[1:]
    all = servers()
    all[name] = {"command": rest[0], "args": rest[1:]}
    json.dump(all, open(config, "w"))
    record({"added": name, "command": rest})
    sys.exit(0)

prompt = sys.stdin.read()
record({"args": args, "prompt": prompt})
flag = lambda name: args[args.index(name) + 1] if name in args else None
conversation = flag("--conversation") or "conv-1"

def emit(kind, body):
    print(json.dumps({"event": kind, kind: body}), flush=True)

def step(body):
    body["conversation_id"] = conversation
    emit("step_update", body)

def reply(index, *pieces):
    for piece in pieces:
        step({"step_index": index, "state": "ACTIVE", "step_type": "agent_response", "text_delta": piece})
    step({"step_index": index, "state": "DONE", "step_type": "agent_response"})

def done(response):
    emit("result", {"conversation_id": conversation, "status": "SUCCESS", "response": response,
                    "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}})

def call_tool(name, arguments):
    server = servers()["sandhill"]
    mcp = subprocess.Popen([server["command"]] + server["args"], stdin=subprocess.PIPE,
                           stdout=subprocess.PIPE, text=True)
    def ask(id, method, params):
        mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}) + "\n")
        mcp.stdin.flush()
        while True:
            message = json.loads(mcp.stdout.readline())
            if message.get("id") == id:
                return message
    ask(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "agy"}})
    result = ask(2, "tools/call", {"name": name, "arguments": arguments})
    mcp.stdin.close()
    mcp.wait()
    return "\n".join(c.get("text", "") for c in result["result"]["content"])

emit("init", {"cwd": os.getcwd(), "tools": ["run_command", "mcp_sandhill_add_cell"],
              "permission_mode": "always-proceed" if "--dangerously-skip-permissions" in args else "request-review",
              "model": flag("--model") or "gemini-3.6-flash-high"})
step({"step_index": 0, "state": "DONE", "step_type": "user_input"})

if "6 times 7" in prompt:
    parameters = {"code": "print(6 * 7)"}
    step({"step_index": 1, "state": "ACTIVE", "step_type": "tool", "tool_name": "mcp_sandhill_add_cell",
          "tool_info": {"name": "mcp_sandhill_add_cell", "parameters": parameters}})
    output = call_tool("add_cell", parameters)
    step({"step_index": 1, "state": "DONE", "step_type": "tool", "duration_seconds": 1.5,
          "tool_info": {"name": "mcp_sandhill_add_cell", "parameters": parameters, "output": output}})
    reply(2, "The cell ", "printed 42.")
    done("The cell printed 42.")
elif "Make a file" in prompt:
    parameters = {"CommandLine": "echo made > made.txt", "Cwd": os.getcwd()}
    step({"step_index": 1, "state": "ACTIVE", "step_type": "tool", "tool_name": "run_command",
          "tool_info": {"name": "run_command", "parameters": parameters}})
    if "--dangerously-skip-permissions" in args:
        subprocess.run(parameters["CommandLine"], shell=True)
        info = {"name": "run_command", "parameters": parameters, "output": ""}
        response = "Made made.txt."
    else:
        print("Tool run_command was denied: it needs approval. Allow it with command(echo) in permissions.allow", file=sys.stderr)
        info = {"name": "run_command", "parameters": parameters,
                "error": {"type": "PERMISSION_DENIED", "message": "run_command needs approval"}}
        response = "I wasn't allowed to run it."
    step({"step_index": 1, "state": "DONE", "step_type": "tool", "tool_info": info})
    reply(2, response)
    done(response)
elif "Carry out the plan." in prompt:
    reply(1, "Done as planned.")
    done("Done as planned.")
elif "Plan" in prompt:
    reply(1, "1. Load\n", "2. Plot")
    done("1. Load\n2. Plot")
elif "Take your time" in prompt:
    def stop(*_):
        emit("result", {"conversation_id": conversation, "status": "CANCELED", "response": ""})
        sys.exit(130)
    signal.signal(signal.SIGINT, stop)
    step({"step_index": 1, "state": "ACTIVE", "step_type": "agent_response", "text_delta": "Thinking"})
    time.sleep(60)
else:
    print('AGY_ERROR: {"message": "model gemini-9 not found"}', file=sys.stderr)
    sys.exit(3)
"#;

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
                    "agy-chat ended ({exit:?}) waiting for {kind}; got {entries:#?}\nterminal:\n{}",
                    String::from_utf8_lossy(&OUTPUT.lock().unwrap())
                );
            }
        }
    }
}

/// The entries' kinds, with a reply's pieces as one `text`.
fn kinds(entries: &[ChatEntry]) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for entry in entries {
        let kind = entry.event["kind"].as_str().unwrap().to_string();
        if kind == "text" && kinds.last().is_some_and(|k| k == "text") {
            continue;
        }
        kinds.push(kind);
    }
    kinds
}

/// A reply's text, from its pieces.
fn reply(entries: &[ChatEntry]) -> String {
    entries
        .iter()
        .filter(|e| e.event["kind"] == "text")
        .filter_map(|e| e.event["text"].as_str())
        .collect()
}

/// What the stand-in was run with for each message.
fn runs(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|l| l.get("args").is_some())
        .collect()
}

fn args(run: &Value) -> Vec<String> {
    run["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
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

async fn say(terminals: &Terminals, id: &str, commands: &[Value]) {
    for command in commands {
        terminals
            .send(
                id,
                ToHost::Command {
                    command: command.clone(),
                },
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn antigravity_works_through_the_chat() {
    if agents::find("python3").is_none() {
        eprintln!("skipping: python3 is not installed");
        return;
    }
    if sandhill_daemon::kernel::find_kernelspec("python3", None)
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

    // The stand-in is the `agy` on PATH.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let agy = bin.join("agy");
    std::fs::write(&agy, STANDIN).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // SAFETY: this test is the only one in its binary, and sets PATH
    // before starting anything that reads it.
    unsafe { std::env::set_var("PATH", &path) };
    let log = dir.path().join("agy.log");
    let config = dir.path().join("mcp_config.json");

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
    let mut launch = agents::launch("antigravity", &options, &context)
        .await
        .unwrap();
    launch.env.extend([
        (
            "STANDIN_LOG".to_string(),
            log.to_string_lossy().into_owned(),
        ),
        (
            "STANDIN_CONFIG".to_string(),
            config.to_string_lossy().into_owned(),
        ),
    ]);
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

    // The first message: agy adds a cell with Sandhill's tool, which runs
    // in the user's kernel, and says so.
    let mut entries = Vec::new();
    chat_until(&mut events, &mut entries, 0, "stop").await;
    assert_eq!(
        kinds(&entries),
        ["session", "prompt", "step", "step_done", "text", "stop"],
        "{entries:#?}"
    );
    assert_eq!(entries[0].event["source"], "startup");
    let prompt = &entries[1].event;
    assert_eq!(prompt["text"], "Add a cell that prints 6 times 7");
    assert_eq!(entries[2].event["tool"], "mcp__sandhill__add_cell");
    assert_eq!(entries[2].event["input"]["code"], "print(6 * 7)");
    let added = entries[3].event["output"].to_string();
    assert!(added.contains("42"), "{added}");
    assert_eq!(entries[3].event["ms"], 1500);
    assert_eq!(reply(&entries), "The cell printed 42.");
    assert!(
        entries
            .iter()
            .skip(1)
            .all(|e| e.event["turn"] == prompt["turn"])
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("qc.ipynb")).unwrap()).unwrap();
    assert_eq!(written["cells"][1]["source"], json!(["print(6 * 7)"]));
    // Sandhill's tools were added to agy's settings, as `agy mcp add` does.
    let added: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(added["sandhill"]["command"], exe.to_string_lossy().as_ref());
    assert_eq!(added["sandhill"]["args"], json!(["mcp"]));
    // agy ran headless, was told about Sandhill, and named its conversation.
    let first = &runs(&log)[0];
    assert_eq!(
        args(first),
        ["--output-format", "stream-json", "--print-timeout", "24h"]
    );
    let told = first["prompt"].as_str().unwrap();
    assert!(
        told.starts_with("You are running inside Sandhill"),
        "{told}"
    );
    assert!(told.contains("Add a cell that prints 6 times 7"), "{told}");
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "idle");
    assert_eq!(agent["thread"], "conv-1");
    assert_eq!(agent["status"]["model"], "Gemini 3.6 Flash High");
    assert_eq!(agent["models"][1]["id"], "gemini-3.1-pro-low");

    // Normal: headless agy can't ask, so the command is turned down, and
    // the chat says how to let it.
    say(
        &terminals,
        &info.id,
        &[json!({"kind": "say", "text": "Make a file"})],
    )
    .await;
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["prompt", "step", "step_failed", "text", "error", "stop"],
        "{:#?}",
        &entries[after..]
    );
    assert_eq!(entries[after + 1].event["tool"], "Bash");
    assert_eq!(
        entries[after + 1].event["input"]["command"],
        "echo made > made.txt"
    );
    assert_eq!(
        entries[after + 2].event["error"],
        "run_command needs approval"
    );
    let error = entries
        .iter()
        .skip(after)
        .find(|e| e.event["kind"] == "error")
        .unwrap();
    assert!(
        error.event["text"]
            .as_str()
            .unwrap()
            .contains("Switch to YOLO"),
        "{error:?}"
    );
    assert!(!project.join("made.txt").exists());
    let second = &runs(&log)[1];
    assert!(args(second).ends_with(&["--conversation".into(), "conv-1".into()]));
    // Sandhill's instructions go with the first message only.
    assert_eq!(
        second["prompt"].as_str().unwrap().lines().next(),
        Some("Make a file")
    );

    // YOLO, with another model and effort: the command runs.
    say(
        &terminals,
        &info.id,
        &[
            json!({"kind": "set", "permission": "full", "model": "gemini-3.1-pro-low", "effort": "high"}),
            json!({"kind": "say", "text": "Make a file"}),
        ],
    )
    .await;
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(
        kinds(&entries[after..]),
        ["prompt", "step", "step_done", "text", "stop"]
    );
    assert!(project.join("made.txt").exists());
    let third = args(&runs(&log)[2]);
    for flag in [
        "--dangerously-skip-permissions",
        "gemini-3.1-pro-low",
        "high",
    ] {
        assert!(third.contains(&flag.to_string()), "{third:?}");
    }
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["permission"], "full");
    assert_eq!(agent["status"]["model"], "Gemini 3.1 Pro Low");
    assert_eq!(agent["status"]["effort"], "high");

    // Plan mode: agy's plan is put to the user, as Claude Code's is, and
    // carried out once approved.
    say(
        &terminals,
        &info.id,
        &[
            json!({"kind": "set", "permission": "plan"}),
            json!({"kind": "say", "text": "Plan a plot"}),
        ],
    )
    .await;
    let after = entries.len();
    let ask = chat_until(&mut events, &mut entries, after, "ask").await;
    assert_eq!(ask["tool"], "ExitPlanMode");
    assert_eq!(ask["input"]["plan"], "1. Load\n2. Plot");
    assert!(args(&runs(&log)[3]).ends_with(&["--mode".into(), "plan".into()]));
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            json!({"behavior": "allow"}),
        )
        .await
        .unwrap();
    let planned = chat_until(&mut events, &mut entries, after, "stop").await;
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(kinds(&entries[after..]), ["text", "stop"]);
    assert_ne!(entries[after].event["turn"], planned["turn"]);
    assert_eq!(reply(&entries[after..]), "Done as planned.");
    let carried = &runs(&log)[4];
    assert!(!args(carried).contains(&"plan".to_string()));
    assert!(
        carried["prompt"]
            .as_str()
            .unwrap()
            .starts_with("Carry out the plan.")
    );
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["permission"], "supervised");

    // Stopping a turn: agy is asked to stop, and the turn ends quietly.
    say(
        &terminals,
        &info.id,
        &[json!({"kind": "say", "text": "Take your time"})],
    )
    .await;
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "text").await;
    say(&terminals, &info.id, &[json!({"kind": "interrupt"})]).await;
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(kinds(&entries[after..]), ["prompt", "text", "stop"]);
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["state"], "idle");

    // agy failing says why.
    say(
        &terminals,
        &info.id,
        &[json!({"kind": "say", "text": "Use gemini-9"})],
    )
    .await;
    let after = entries.len();
    chat_until(&mut events, &mut entries, after, "stop").await;
    assert_eq!(kinds(&entries[after..]), ["prompt", "error", "stop"]);
    assert_eq!(
        entries[after + 1].event["text"],
        "model gemini-9 not found."
    );

    // Started again, it goes on with agy's conversation.
    let before = terminals.info(&info.id).unwrap();
    assert_eq!(
        agents::resume_id("antigravity", before.agent.as_ref()).as_deref(),
        Some("conv-1")
    );
    let context = Context {
        resume: Some("conv-1"),
        prompt: Some("Make a file"),
        ..context
    };
    let mut launch = agents::launch("antigravity", &options, &context)
        .await
        .unwrap();
    launch.env.extend([
        (
            "STANDIN_LOG".to_string(),
            log.to_string_lossy().into_owned(),
        ),
        (
            "STANDIN_CONFIG".to_string(),
            config.to_string_lossy().into_owned(),
        ),
    ]);
    let restarted = terminals
        .restart(&info.id, launch, &agents::RESUME_KEYS)
        .await
        .unwrap();
    assert_eq!(restarted.agent.unwrap()["thread"], "conv-1");
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(before.end), Some(entries.len() as u64))
            .await
            .unwrap()
            .forward(out),
    );
    let mut resumed = Vec::new();
    chat_until(&mut events, &mut resumed, 0, "stop").await;
    assert_eq!(resumed[0].event["source"], "resume");
    let last = runs(&log).pop().unwrap();
    assert!(args(&last).ends_with(&["--conversation".into(), "conv-1".into()]));
    // Resumed, it was told about Sandhill before, and is added once.
    assert_eq!(
        last["prompt"].as_str().unwrap().lines().next(),
        Some("Make a file")
    );
    let added = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| l.contains("\"added\""))
        .count();
    assert_eq!(added, 1);
    terminals.close(&info.id).await.unwrap();
}

/// A new conversation's model menu lists agy's models before anything is
/// sent.
#[tokio::test]
async fn agy_lists_its_models_before_a_conversation() {
    use std::os::unix::fs::PermissionsExt;
    if sandhill_daemon::agents::find("python3").is_none() {
        eprintln!("skipping: python3 is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let agy = dir.path().join("agy");
    std::fs::write(&agy, STANDIN).unwrap();
    std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
    let models = sandhill_daemon::terminal::antigravity::list_models(&agy)
        .await
        .unwrap();
    assert_eq!(models[0]["id"], "gemini-3.6-flash-high");
    assert_eq!(models[0]["default"], true);
    assert_eq!(models[1]["id"], "gemini-3.1-pro-low");
}
