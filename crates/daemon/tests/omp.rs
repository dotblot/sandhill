//! The ACP bridge with the real Oh My Pi (`omp acp`), protocol only: model
//! listing, initialize, `session/new` with Sandhill's MCP server, model and
//! thinking changes through config options, and `session/load`. No message is
//! sent, so no model turn is spent. omp runs in a temporary HOME with a dummy
//! Anthropic key (for a model list), never the user's own `~/.omp`. Skips
//! when `omp` is not installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::terminal::{Launch, Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(90);

fn launch(omp: &Path, socket: &Path, project: &Path, session: Option<&str>) -> Launch {
    let options = AgentOptions {
        permission: Some("supervised".into()),
        ..AgentOptions::default()
    };
    let mut args = vec![
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "acp-chat".into(),
        "--agent".into(),
        "omp".into(),
        "--program".into(),
        omp.to_string_lossy().into_owned(),
        "--project".into(),
        project.to_string_lossy().into_owned(),
        "--options".into(),
        serde_json::to_string(&options).unwrap(),
    ];
    if let Some(session) = session {
        args.extend(["--session".into(), session.into()]);
    }
    Launch {
        kind: "omp".into(),
        title: "Oh My Pi".into(),
        program: env!("CARGO_BIN_EXE_sandhilld").into(),
        args,
        env: vec![],
        cwd: project.to_path_buf(),
        project: Some(project.to_string_lossy().into_owned()),
        options,
        size: Some((100, 30)),
        job: None,
    }
}

async fn start_daemon(state: &Path) -> (Arc<Daemon>, PathBuf) {
    std::fs::create_dir_all(state).unwrap();
    let socket = state.join("test.sock");
    let daemon = Daemon::open_with(
        StateDir::new(state),
        DaemonOptions {
            executable: env!("CARGO_BIN_EXE_sandhilld").into(),
            socket: socket.clone(),
            started_by: String::new(),
        },
    )
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

/// Waits until the agent's report and chat satisfy `ready`.
async fn until(
    terminals: &Terminals,
    id: &str,
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    entries: &mut Vec<Value>,
    ready: impl Fn(&[Value], &Value) -> bool,
) -> Value {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let info = terminals.info(id).unwrap();
            let agent = info.agent.unwrap_or_default();
            if ready(entries, &agent) {
                return agent;
            }
            assert!(info.running, "bridge exited: {entries:#?}");
            if let Ok(Some(ServerMessage::Terminal { terminal })) =
                tokio::time::timeout(Duration::from_millis(500), events.recv()).await
            {
                if let Some(entry) = terminal.chat {
                    entries.push(entry.event);
                }
                assert!(terminal.exit.is_none(), "bridge exited: {entries:#?}");
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "omp timed out: {entries:#?}\n{:#?}",
            terminals.info(id).unwrap().agent
        )
    })
}

async fn subscribe(terminals: &Terminals, id: &str) -> mpsc::UnboundedReceiver<ServerMessage> {
    let (out, events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    events
}

#[tokio::test]
async fn omp_lists_models_and_starts_switches_and_loads_a_session_without_a_turn() {
    let Some(omp) = sandhill_daemon::agents::find("omp") else {
        eprintln!("skipping: Oh My Pi (`omp`) is not installed");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this file's only test sets these before starting any process
    // or thread that reads the environment.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-sandhill-test");
        for key in [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
        ] {
            std::env::remove_var(key);
        }
    }

    let listed = sandhill_daemon::terminal::acp::list_models("omp", &omp)
        .await
        .unwrap();
    let listed = listed.as_array().unwrap();
    assert!(!listed.is_empty(), "omp listed no models");
    for model in listed {
        assert!(model["id"].as_str().unwrap().contains('/'), "{model}");
        assert!(model["label"].is_string(), "{model}");
    }

    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state = dir.path().join("state");
    let (_daemon, socket) = start_daemon(&state).await;
    let terminals = Terminals::new(
        state.join("terminals"),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    let info = terminals
        .start(launch(&omp, &socket, &project, None))
        .await
        .unwrap();
    let mut events = subscribe(&terminals, &info.id).await;
    let mut entries = Vec::new();
    let agent = until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        |_, agent| {
            agent["state"] == "idle" && agent["models"].as_array().is_some_and(|m| !m.is_empty())
        },
    )
    .await;
    let session = agent["thread"].as_str().unwrap().to_string();
    assert!(!session.is_empty());
    assert_eq!(agent["view_mode"], "chat");
    assert!(agent["status"]["model_id"].is_string(), "{agent:#}");
    let dir_of = state.join("terminals").join(&info.id);
    assert!(dir_of.join("omp-config.yml").exists());
    assert!(dir_of.join("omp-sandhill.js").exists());
    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "session" && e["source"] == "startup")
    );

    // A thinking level and another model, through session/set_config_option.
    // The dummy Anthropic key gives omp's Anthropic models, which think.
    let efforts: Vec<String> = agent["models"][0]["efforts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str().map(str::to_string))
        .collect();
    let effort = efforts
        .iter()
        .find(|e| Some(e.as_str()) != agent["status"]["effort"].as_str())
        .unwrap_or_else(|| panic!("no other thinking level: {agent:#}"))
        .clone();
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: json!({"kind": "set", "effort": effort}),
            },
        )
        .await
        .unwrap();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        |_, agent| agent["status"]["effort"] == json!(effort),
    )
    .await;
    let other = agent["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .find(|id| Some(*id) != agent["status"]["model_id"].as_str())
        .unwrap_or_else(|| panic!("no other model: {agent:#}"))
        .to_string();
    terminals
        .send(
            &info.id,
            ToHost::Command {
                command: json!({"kind": "set", "model": other}),
            },
        )
        .await
        .unwrap();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        |_, agent| agent["status"]["model_id"] == json!(other),
    )
    .await;
    terminals.close(&info.id).await.unwrap();

    // The same session again, through session/load.
    let info = terminals
        .start(launch(&omp, &socket, &project, Some(&session)))
        .await
        .unwrap();
    let mut events = subscribe(&terminals, &info.id).await;
    let mut entries = Vec::new();
    let agent = until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        |entries, agent| {
            agent["state"] == "idle"
                && entries
                    .iter()
                    .any(|e| e["kind"] == "session" && e["source"] == "resume")
        },
    )
    .await;
    assert_eq!(agent["thread"], json!(session));
    assert_eq!(agent["status"]["model_id"], json!(other));
    terminals.close(&info.id).await.unwrap();
}
