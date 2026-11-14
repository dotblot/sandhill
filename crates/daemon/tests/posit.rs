//! Posit Assistant in its own screen: a stand-in for `pa` that reads
//! Sandhill's MCP server from its administrator defaults, as the real one
//! does, and uses it with a real Python kernel. The real `pa` needs a model
//! provider's sign-in, so it isn't run here. Skips (and says so) when Python
//! or the kernel isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::agents::{self, Context};
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(90);

/// A stand-in for `pa`: it says how it was started, waits for a message
/// typed at its prompt, and answers it by adding a cell with Sandhill's
/// `add_cell`, through the MCP server its defaults name.
const STANDIN: &str = r#"#!/usr/bin/env python3
import json, os, subprocess, sys

args = sys.argv[1:]
if args[:1] == ["--version"]:
    print("Posit Assistant TUI v0.1.0\nBuild:  dev\nCommit: 8c1abe7")
    sys.exit(0)
if args[:1] in (["--help"], ["-h"]):
    print("Usage: pa [options] [prompt]")
    sys.exit(0)
print("ARGS " + json.dumps(args), flush=True)
server = json.loads(os.environ["POSIT_ASSISTANT_SETTINGS_DEFAULT"])["mcpServers"]["sandhill"]
print("Enter a prompt", flush=True)
message = sys.stdin.readline().strip()
if "6 times 7" not in message:
    print("UNEXPECTED " + message, flush=True)
    sys.exit(1)

mcp = subprocess.Popen(server["command"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def ask(id, method, params):
    mcp.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}) + "\n")
    mcp.stdin.flush()
    while True:
        reply = json.loads(mcp.stdout.readline())
        if reply.get("id") == id:
            return reply
ask(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "pa"}})
result = ask(2, "tools/call", {"name": "add_cell", "arguments": {"code": "print(6 * 7)"}})
mcp.stdin.close()
mcp.wait()
print("TOOL " + " ".join(c.get("text", "") for c in result["result"]["content"]).replace("\n", " "), flush=True)
print("DONE", flush=True)
"#;

/// Collects the terminal's output until it holds `needle`.
async fn output_until(
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    output: &mut String,
    needle: &str,
) {
    use base64::Engine;
    while !output.contains(needle) {
        let message = tokio::time::timeout(TIMEOUT, events.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; terminal:\n{output}"))
            .expect("stream ended");
        if let ServerMessage::Terminal { terminal } = message {
            if let Some(data) = &terminal.data {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap();
                output.push_str(&String::from_utf8_lossy(&bytes));
            }
            if let Some(exit) = terminal.exit
                && !output.contains(needle)
            {
                panic!("pa ended ({exit:?}) before {needle:?}; terminal:\n{output}");
            }
        }
    }
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
async fn posit_assistant_reaches_the_live_session() {
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

    // The stand-in is the `pa` on PATH.
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let pa = bin.join("pa");
    std::fs::write(&pa, STANDIN).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&pa, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // SAFETY: this test is the only one in its binary, and sets PATH
    // before starting anything that reads it.
    unsafe { std::env::set_var("PATH", &path) };

    let listed = agents::list().await;
    let info = listed.iter().find(|a| a.kind == agents::POSIT).unwrap();
    assert_eq!(info.path.as_deref(), Some(pa.to_string_lossy().as_ref()));
    assert_eq!(info.version.as_deref(), Some("0.1.0"));

    let options = AgentOptions {
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
    let launch = agents::launch(agents::POSIT, &options, &context)
        .await
        .unwrap();
    let terminals = Terminals::new(dir.path().join("state/terminals"), exe.clone());
    let started = terminals.start(launch).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&started.id, Some(0), None)
            .await
            .unwrap()
            .forward(out),
    );

    // pa's screen starts without the message, which would make it answer
    // once and quit: the app types it at the prompt.
    let mut output = String::new();
    output_until(&mut events, &mut output, "Enter a prompt").await;
    assert!(output.contains("ARGS []"), "{output}");
    terminals
        .send(
            &started.id,
            ToHost::Input {
                data: "Add a cell that prints 6 times 7\r".into(),
            },
        )
        .await
        .unwrap();

    // The MCP server finds the conversation's terminal, so the cell goes
    // into the document it was started in, and runs in the user's kernel.
    output_until(&mut events, &mut output, "DONE").await;
    let tool = output
        .lines()
        .find(|l| l.starts_with("TOOL "))
        .unwrap_or_default();
    assert!(tool.contains("42"), "{output}");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("qc.ipynb")).unwrap()).unwrap();
    assert_eq!(written["cells"][1]["source"], json!(["print(6 * 7)"]));
}
