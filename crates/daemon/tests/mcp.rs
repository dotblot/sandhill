//! The tools Sandhill gives coding agents (`sandhilld mcp`), against real
//! kernels. Each test skips (and says so) when its kernel isn't installed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::mcp::Tools;
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{AgentOptions, Call, CellRef, EVENT_RUN_FINISHED, Focus, TerminalInfo};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        std::fs::create_dir_all(state).unwrap();
        let socket = state.join("test.sock");
        // Terminals run under sandhilld, not this test.
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
        TestDaemon {
            _daemon: daemon,
            socket,
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}

async fn have_kernel(name: &str) -> bool {
    let found = sandhill_daemon::kernel::find_kernelspec(name).await.is_ok();
    if !found {
        eprintln!("skipping: Jupyter kernel {name:?} is not installed");
    }
    found
}

/// The text of a tool's reply, or its error.
async fn call(tools: &Tools, name: &str, args: Value) -> String {
    let items = tokio::time::timeout(TIMEOUT, tools.call(name, &args))
        .await
        .expect("tool timed out")
        .unwrap_or_else(|e| panic!("{name} failed: {e:#}"));
    items
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn agents_run_python_in_the_users_session() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );

    let listed = call(&tools, "list_sessions", json!({})).await;
    assert!(listed.contains("No R or Python sessions"), "{listed}");

    // No session yet: run_code starts one in the project.
    let ran = call(
        &tools,
        "run_code",
        json!({"code": "import os\nx = 21 * 2\nprint(os.getcwd())\nx", "language": "python"}),
    )
    .await;
    assert!(ran.contains(" · ok · "), "{ran}");
    assert!(ran.contains(&*project.to_string_lossy()), "{ran}");
    assert!(ran.contains("42"), "{ran}");

    // Later calls reuse it; `x` is still there.
    let ran = call(&tools, "run_code", json!({"code": "x / 0"})).await;
    assert!(ran.contains("error"), "{ran}");
    assert!(ran.contains("ZeroDivisionError"), "{ran}");

    let listed = call(&tools, "list_sessions", json!({})).await;
    assert!(listed.contains("Python session"), "{listed}");
    assert_eq!(listed.lines().count(), 2, "{listed}");

    let variables = call(&tools, "list_variables", json!({})).await;
    assert!(variables.contains("x"), "{variables}");
    assert!(variables.contains("int"), "{variables}");
    assert!(!variables.contains("SANDHILL_VAR"), "{variables}");

    // Code that outlasts the wait keeps running in the session.
    let ran = call(
        &tools,
        "run_code",
        json!({"code": "import time\nprint('started', flush=True)\ntime.sleep(3)\nprint('done')", "timeout_seconds": 1}),
    )
    .await;
    assert!(ran.contains("still running"), "{ran}");
    assert!(ran.contains("started"), "{ran}");
    // The session is busy until it ends; listing waits for that.
    let busy = tools.call("list_variables", &json!({})).await;
    assert!(busy.is_err());

    let client = daemon.client().await;
    for session in client.list_sessions().await.unwrap() {
        client.shutdown(&session.id).await.unwrap();
    }
}

#[tokio::test]
async fn agents_see_r_data_frames() {
    let kernel = if have_kernel("ark").await {
        "ark"
    } else if have_kernel("ir").await {
        "ir"
    } else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let client = daemon.client().await;
    let session = client.start_session(kernel, None).await.unwrap().id;
    let tools = Tools::new(daemon.socket.clone(), None);

    let ran = call(
        &tools,
        "run_code",
        json!({"code": "counts <- data.frame(gene = c('CD3E', 'MS4A1'), n = c(12L, 7L))\nsum(counts$n)", "language": "r"}),
    )
    .await;
    assert!(ran.contains("19"), "{ran}");
    assert!(ran.contains(&session[..8]), "{ran}");

    let variables = call(&tools, "list_variables", json!({"language": "r"})).await;
    let line = variables
        .lines()
        .find(|l| l.contains("counts"))
        .unwrap_or_else(|| panic!("{variables}"));
    assert!(line.contains("data.frame"), "{variables}");
    assert!(
        line.contains("2 × 2") || line.contains("2 x 2"),
        "{variables}"
    );

    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn agents_read_the_outputs_of_a_document() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let doc = project.join("analysis.qmd");
    std::fs::write(&doc, "```{python}\n#| label: setup\nprint('hi')\n```\n").unwrap();
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );

    let none = call(&tools, "get_outputs", json!({"file": "analysis.qmd"})).await;
    assert!(none.contains("no cell has run"), "{none}");

    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();
    let code = "from IPython.display import display\n\
                print('hi')\n\
                display({'image/png': 'iVBORw0KGgo=', 'text/plain': '<plot>'}, raw=True)\n\
                6 * 7";
    let cell = CellRef {
        file: doc.to_string_lossy().into(),
        cell_id: "setup".into(),
    };
    let run = client
        .execute(&session, code, Some(cell))
        .await
        .unwrap()
        .run;
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .unwrap()
            .unwrap();
        if event.run.as_deref() == Some(&run) && event.msg_type == EVENT_RUN_FINISHED {
            break;
        }
    }

    let items = tools
        .call("get_outputs", &json!({"file": "analysis.qmd"}))
        .await
        .unwrap();
    let text = items[0]["text"].as_str().unwrap();
    assert!(text.contains("## setup (python) · ok"), "{text}");
    assert!(text.contains("hi"), "{text}");
    assert!(text.contains("42"), "{text}");
    assert!(text.contains("[plot 1]"), "{text}");
    assert_eq!(items[1]["type"], "image");
    assert_eq!(items[1]["data"], "iVBORw0KGgo=");

    let other = call(
        &tools,
        "get_outputs",
        json!({"file": "analysis.qmd", "cell": "plots"}),
    )
    .await;
    assert!(other.contains("cell plots has no saved outputs"), "{other}");

    client.shutdown(&session).await.unwrap();
}

/// Agents start `sandhilld mcp` themselves and talk JSON-RPC over stdio.
#[tokio::test]
async fn the_mcp_server_speaks_json_rpc_on_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("mcp")
        .arg("--project")
        .arg(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ask = async |id: u64, method: &str, params: Value| -> Value {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(TIMEOUT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id);
        reply
    };

    let init = ask(
        1,
        "initialize",
        json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    )
    .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["serverInfo"]["name"], "sandhill");

    let tools = ask(2, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_sessions",
            "list_variables",
            "run_code",
            "get_focus",
            "add_cell",
            "edit_cell",
            "get_outputs"
        ]
    );

    let listed = ask(
        3,
        "tools/call",
        json!({"name": "list_sessions", "arguments": {}}),
    )
    .await;
    assert_eq!(listed["result"]["isError"], false);
    assert!(
        listed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No R or Python sessions")
    );

    // A tool error is the tool's answer; an unknown method is a protocol error.
    let failed = ask(
        4,
        "tools/call",
        json!({"name": "run_code", "arguments": {}}),
    )
    .await;
    assert_eq!(failed["result"]["isError"], true);
    let unknown = ask(5, "resources/list", json!({})).await;
    assert!(unknown["error"].is_object());
}

/// With no window open on it, the daemon writes the cell into the file and
/// the tool runs it in the document's session.
#[tokio::test]
async fn agents_write_cells_into_documents_on_disk() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join("notes")).unwrap();
    let doc = project.join("notes").join("analysis.qmd");
    std::fs::write(&doc, "# QC\n\n```{python}\nx = 21\n```\n\nText.\n").unwrap();
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );

    let added = call(
        &tools,
        "add_cell",
        json!({"file": "notes/analysis.qmd", "code": "y = 6 * 7\nprint(y)"}),
    )
    .await;
    assert!(
        added.contains("Added Python cell chunk-2 to notes/analysis.qmd after the last cell"),
        "{added}"
    );
    assert!(added.contains(" · ok · "), "{added}");
    assert!(added.contains("42"), "{added}");
    assert_eq!(
        std::fs::read_to_string(&doc).unwrap(),
        "# QC\n\n```{python}\nx = 21\n```\n\n```{python}\ny = 6 * 7\nprint(y)\n```\n\nText.\n"
    );
    // The run belongs to the cell, in a session in the document's folder,
    // as the app would have started it.
    let outputs = call(
        &tools,
        "get_outputs",
        json!({"file": "notes/analysis.qmd", "cell": "chunk-2"}),
    )
    .await;
    assert!(outputs.contains("42"), "{outputs}");
    let client = daemon.client().await;
    let sessions = client.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(Path::new(&sessions[0].cwd), project.join("notes"));

    // A failed cell is fixed in place, in the same session.
    let fixed = call(
        &tools,
        "edit_cell",
        json!({"file": "notes/analysis.qmd", "cell": "chunk-2", "code": "print(y + 1)"}),
    )
    .await;
    assert!(fixed.contains("Changed Python cell chunk-2"), "{fixed}");
    assert!(fixed.contains("43"), "{fixed}");
    assert!(
        std::fs::read_to_string(&doc)
            .unwrap()
            .contains("```{python}\nprint(y + 1)\n```")
    );
    assert_eq!(client.list_sessions().await.unwrap().len(), 1);

    // Where the user is decides where the next cell goes, and the agent can
    // see it.
    let focus = Focus {
        file: Some(doc.to_string_lossy().into()),
        cell: Some("chunk-1".into()),
        line: Some(4),
        selection: Some("x = 21".into()),
        ..Focus::default()
    };
    client
        .set_focus(&project.to_string_lossy(), focus)
        .await
        .unwrap();
    let seen = call(&tools, "get_focus", json!({})).await;
    assert!(
        seen.contains("The user is in notes/analysis.qmd, cell chunk-1, line 4."),
        "{seen}"
    );
    assert!(seen.contains("```python\nx = 21\n```"), "{seen}");
    // Claude Code's prompt hook adds it to each prompt.
    let mut hook = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
        .arg("--socket")
        .arg(&daemon.socket)
        .args(["terminal-event", "prompt"])
        .arg(&project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = hook.stdin.take().unwrap();
    stdin
        .write_all(br#"{"hook_event_name":"UserPromptSubmit","prompt":"fix this"}"#)
        .await
        .unwrap();
    drop(stdin);
    let printed = hook.wait_with_output().await.unwrap();
    let context = String::from_utf8_lossy(&printed.stdout);
    assert!(
        context.starts_with(
            "Sandhill: the user is in notes/analysis.qmd, cell chunk-1, line 4. They have selected:\n```\nx = 21\n```"
        ),
        "{context}"
    );
    let added = call(
        &tools,
        "add_cell",
        json!({"file": "notes/analysis.qmd", "code": "print('here')", "run": false}),
    )
    .await;
    assert!(added.contains("below cell chunk-1. Not run."), "{added}");
    // The next one goes below that, in order.
    let added = call(
        &tools,
        "add_cell",
        json!({"file": "notes/analysis.qmd", "code": "print('then')", "run": false}),
    )
    .await;
    assert!(added.contains("below cell chunk-2. Not run."), "{added}");
    let text = std::fs::read_to_string(&doc).unwrap();
    assert!(
        text.starts_with("# QC\n\n```{python}\nx = 21\n```\n\n```{python}\nprint('here')\n```\n\n```{python}\nprint('then')\n```\n\n```{python}\nprint(y + 1)"),
        "{text}"
    );

    for session in client.list_sessions().await.unwrap() {
        client.shutdown(&session.id).await.unwrap();
    }
}

/// A conversation started about a notebook writes there by default.
#[tokio::test]
async fn agents_write_into_the_document_their_conversation_is_about() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let notebook = json!({
        "cells": [{"cell_type": "code", "execution_count": null, "id": "load", "metadata": {}, "outputs": [], "source": ["n = 7"]}],
        "metadata": {"kernelspec": {"name": "python3", "display_name": "Python 3", "language": "python"}},
        "nbformat": 4,
        "nbformat_minor": 5,
    });
    std::fs::write(project.join("qc.ipynb"), notebook.to_string()).unwrap();
    let client = daemon.client().await;
    let terminal: TerminalInfo = client
        .call(Call::StartTerminal {
            kind: "shell".into(),
            cwd: None,
            project: Some(project.to_string_lossy().into()),
            cols: None,
            rows: None,
            options: AgentOptions {
                documents: vec!["qc.ipynb".into()],
                ..AgentOptions::default()
            },
            prompt: None,
        })
        .await
        .unwrap();
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    )
    .with_terminal(Some(terminal.id.clone()));

    let added = call(&tools, "add_cell", json!({"code": "m = 6\nprint(m * 7)"})).await;
    assert!(added.contains("to qc.ipynb after the last cell"), "{added}");
    assert!(added.contains("42"), "{added}");
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(project.join("qc.ipynb")).unwrap()).unwrap();
    let cells = written["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 2);
    assert_eq!(cells[1]["source"], json!(["m = 6\n", "print(m * 7)"]));
    // Its output is in the journal for the app to bring in.
    let id = cells[1]["id"].as_str().unwrap();
    let outputs = client
        .file_outputs(&project.join("qc.ipynb").to_string_lossy())
        .await
        .unwrap();
    assert!(outputs.cells.iter().any(|c| c.cell_id == id));

    client.close_terminal(&terminal.id).await.unwrap();
    for session in client.list_sessions().await.unwrap() {
        client.shutdown(&session.id).await.unwrap();
    }
}

/// When the user is in no cell, the window is asked to add the cell after
/// the last one: with no `after`, rather than a null one.
#[tokio::test]
async fn a_window_adds_after_the_last_cell_when_the_user_is_in_none() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let doc = project.join("analysis.qmd");
    std::fs::write(&doc, "---\ntitle: x\n---\n\n```{r}\nx <- 1\n```\n").unwrap();
    let doc = doc.to_string_lossy().into_owned();
    let mut window = daemon.client().await;
    let focus = Focus {
        file: Some(doc.clone()),
        line: Some(1),
        open: vec![doc.clone()],
        ..Focus::default()
    };
    window
        .set_focus(&project.to_string_lossy(), focus)
        .await
        .unwrap();
    let handled = tokio::spawn(async move {
        let request = window.next_request().await.unwrap();
        window
            .answer(
                request.id,
                Ok(json!({"cell_id": "chunk-2", "language": "r"})),
            )
            .await
            .unwrap();
        request
    });
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );
    let added = call(
        &tools,
        "add_cell",
        json!({"file": "analysis.qmd", "code": "y <- 2", "run": false}),
    )
    .await;
    let request = handled.await.unwrap();
    assert!(request.params.get("after").is_none(), "{}", request.params);
    assert!(added.contains("after the last cell"), "{added}");
}

/// When a window has the document open, the window adds the cell and runs
/// it as if the user had; the tool reports that run.
#[tokio::test]
async fn an_open_window_makes_the_edit_and_runs_the_cell() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let doc = project.join("analysis.qmd");
    let original = "```{python}\n#| label: setup\nimport math\n```\n";
    std::fs::write(&doc, original).unwrap();
    let doc = doc.to_string_lossy().into_owned();

    // Stands in for the app's window.
    let mut window = daemon.client().await;
    let focus = Focus {
        file: Some(doc.clone()),
        cell: Some("setup".into()),
        open: vec![doc.clone()],
        ..Focus::default()
    };
    window
        .set_focus(&project.to_string_lossy(), focus)
        .await
        .unwrap();
    let handled = tokio::spawn({
        let doc = doc.clone();
        async move {
            let request = window.next_request().await.unwrap();
            let params = request.params.clone();
            window
                .answer(
                    request.id,
                    Ok(json!({"cell_id": "chunk-2", "language": "python"})),
                )
                .await
                .unwrap();
            let session = window.start_session("python3", None).await.unwrap().id;
            let cell = CellRef {
                file: doc,
                cell_id: "chunk-2".into(),
            };
            window
                .execute(&session, params["code"].as_str().unwrap(), Some(cell))
                .await
                .unwrap();
            (request, window, session)
        }
    });
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );
    let added = call(
        &tools,
        "add_cell",
        json!({"file": "analysis.qmd", "code": "print(6 * 7)"}),
    )
    .await;
    let (request, window, session) = handled.await.unwrap();
    assert_eq!(request.method, "add_cell");
    assert_eq!(request.params["after"], "setup");
    assert_eq!(request.params["run"], true);
    assert!(added.contains("below cell setup"), "{added}");
    assert!(added.contains("Sandhill ran it"), "{added}");
    assert!(added.contains("42"), "{added}");
    // The window made the edit; the daemon left the file alone.
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), original);
    window.shutdown(&session).await.unwrap();
}

/// In Sandhill's Ask mode, the agent's terminal is read-only: Sandhill's
/// tools that change something refuse, and those that look still work.
#[tokio::test]
async fn in_ask_mode_sandhills_tools_only_look() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&dir.path().join("state")).await;
    let terminal = dir.path().join("terminal");
    std::fs::create_dir_all(&terminal).unwrap();
    std::fs::write(terminal.join("agent.json"), r#"{"guard": "ask"}"#).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("mcp")
        .arg("--project")
        .arg(dir.path())
        .env("SANDHILL_TERMINAL", &terminal)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut call = async |id: u64, name: &str, arguments: Value| -> Value {
        let message = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                             "params": {"name": name, "arguments": arguments}});
        stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(TIMEOUT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()["result"].clone()
    };
    let refused = call(1, "run_code", json!({"code": "1 + 1"})).await;
    assert_eq!(refused["isError"], true);
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Ask mode"),
        "{refused}"
    );
    let added = call(2, "add_cell", json!({"code": "x <- 1"})).await;
    assert_eq!(added["isError"], true);
    let listed = call(3, "list_sessions", json!({})).await;
    assert_eq!(listed["isError"], false);

    // Switched out of Ask mode, the tool runs (and finds no session).
    std::fs::write(terminal.join("agent.json"), r#"{"guard": null}"#).unwrap();
    let ran = call(4, "run_code", json!({"code": "1 + 1"})).await;
    assert!(
        !ran["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Ask mode"),
        "{ran}"
    );
}
