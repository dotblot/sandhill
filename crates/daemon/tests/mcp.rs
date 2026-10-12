//! The tools Sandhill gives coding agents (`sandhilld mcp`), against real
//! kernels. Each test skips (and says so) when its kernel isn't installed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::mcp::Tools;
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{CellRef, EVENT_RUN_FINISHED};
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
        let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
        let socket = state.join("test.sock");
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
        ["list_sessions", "list_variables", "run_code", "get_outputs"]
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
