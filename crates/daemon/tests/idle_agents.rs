//! Idle retirement uses real terminal hosts and a separately running daemon.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sandhill_daemon::terminal::{Launch, Terminals, ToHost};
use sandhill_proto::client::Client;
use sandhill_proto::{AgentOptions, Call, TerminalInfo};
use serde_json::json;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");
const LIMIT: Duration = Duration::from_millis(1200);
const TIMEOUT: Duration = Duration::from_secs(20);

async fn stand_in(
    terminals: &Terminals,
    cwd: &Path,
    kind: &str,
    state: &str,
    session: bool,
    tail: &str,
) -> TerminalInfo {
    let report = if session {
        r#"{"session_id":"idle-test"}"#
    } else {
        "{}"
    };
    let script =
        format!("stty -echo; echo '{report}' | '{DAEMON}' terminal-event state {state}; {tail}");
    let info = terminals
        .start(Launch {
            kind: kind.into(),
            title: "Idle test".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
            env: vec![],
            cwd: cwd.into(),
            project: None,
            options: AgentOptions::default(),
            size: None,
            job: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if terminals
                .info(&info.id)
                .unwrap()
                .agent
                .as_ref()
                .is_some_and(|a| a["state"] == state)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    info
}

#[tokio::test]
async fn idle_resumable_agents_stop_and_the_next_message_restarts_them() {
    if !Path::new("/bin/sh").is_file()
        || std::process::Command::new("stty")
            .arg("--help")
            .output()
            .is_err()
        || std::process::Command::new("sleep")
            .arg("0")
            .status()
            .is_err()
    {
        eprintln!("skipping idle agents: /bin/sh, stty and sleep are required");
        return;
    }
    let dir = common::tempdir().unwrap();
    let state = dir.path().join("state");
    let terminals = Terminals::new(state.join("terminals"), DAEMON.into());
    let wait = "while read -r line; do :; done";
    let idle = stand_in(&terminals, dir.path(), "claude", "idle", true, wait).await;
    let busy = stand_in(&terminals, dir.path(), "claude", "busy", true, wait).await;
    let waiting = stand_in(&terminals, dir.path(), "claude", "waiting", true, wait).await;
    let posit = stand_in(&terminals, dir.path(), "posit", "idle", true, wait).await;
    // Even a Posit report with a thread must not make it resumable.
    terminals
        .send(
            &posit.id,
            ToHost::Agent {
                value: json!({"thread": "posit-thread"}),
            },
        )
        .await
        .unwrap();
    let no_session = stand_in(&terminals, dir.path(), "claude", "idle", false, wait).await;
    let shell = stand_in(&terminals, dir.path(), "shell", "idle", true, wait).await;
    let job = stand_in(&terminals, dir.path(), "job", "idle", true, wait).await;
    let input = stand_in(&terminals, dir.path(), "claude", "idle", true, wait).await;
    let command = stand_in(&terminals, dir.path(), "claude", "idle", true, wait).await;
    let answer = stand_in(&terminals, dir.path(), "claude", "idle", true, wait).await;
    let output = stand_in(
        &terminals,
        dir.path(),
        "claude",
        "idle",
        true,
        "while :; do echo output; sleep 0.1; done",
    )
    .await;
    let reports =
        format!("while :; do echo '{{}}' | '{DAEMON}' terminal-event state idle; sleep 0.1; done");
    let report = stand_in(&terminals, dir.path(), "claude", "idle", true, &reports).await;
    let thread = stand_in(&terminals, dir.path(), "codex", "idle", false, wait).await;
    terminals
        .send(
            &thread.id,
            ToHost::Agent {
                value: json!({"thread": "codex-thread"}),
            },
        )
        .await
        .unwrap();

    // RestartTerminal finds this stand-in through the daemon's private PATH.
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(&claude, format!(
        "#!/bin/sh\nif [ \"$1\" = --help ]; then echo --resume; exit 0; fi\nprintf '%s\\n' \"$@\" > '{}'; echo '{{\"session_id\":\"idle-test\"}}' | '{DAEMON}' terminal-event state busy; while read -r line; do :; done\n",
        dir.path().join("restart-args").display()
    )).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut daemon = tokio::process::Command::new(DAEMON)
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env("HOME", &home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("SANDHILL_TEST_AGENT_IDLE_MS", LIMIT.as_millis().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = state.join("sandhilld.sock");
    let mut client = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Ok(client) = Client::connect_unix(&socket).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    client.subscribe_terminal(&idle.id, Some(0)).await.unwrap();

    // No terminal echo/output is generated by these inputs. Commands have no
    // listener, and answers have no pending ask: receipt itself is activity.
    let active_until = tokio::time::Instant::now() + LIMIT * 3;
    while tokio::time::Instant::now() < active_until {
        client
            .terminal_input(&input.id, "keep alive\n")
            .await
            .unwrap();
        let _: serde_json::Value = client
            .call(Call::TellTerminal {
                terminal: command.id.clone(),
                command: json!({"kind": "stop"}),
            })
            .await
            .unwrap();
        let _: serde_json::Value = client
            .call(Call::AnswerTerminal {
                terminal: answer.id.clone(),
                id: "no-ask".into(),
                answer: json!("yes"),
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let exit = tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = client.next_terminal_event().await.unwrap();
            if event.terminal == idle.id
                && let Some(exit) = event.exit
            {
                break exit;
            }
        }
    })
    .await
    .unwrap();
    assert!(exit.idle, "live follower must receive the idle exit");
    assert!(terminals.info(&idle.id).unwrap().exit.unwrap().idle);
    assert!(
        terminals.info(&thread.id).unwrap().exit.unwrap().idle,
        "other agents resume by thread"
    );
    for info in [
        &busy,
        &waiting,
        &posit,
        &no_session,
        &shell,
        &job,
        &input,
        &command,
        &answer,
        &output,
        &report,
    ] {
        assert!(
            terminals.info(&info.id).unwrap().running,
            "{} ({}) must keep running",
            info.id,
            info.kind
        );
    }

    let _: serde_json::Value = client
        .call(Call::TrustFolder {
            path: dir.path().to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
    let restarted: TerminalInfo = client
        .call(Call::RestartTerminal {
            terminal: idle.id.clone(),
            options: None,
            cols: None,
            rows: None,
            prompt: Some("continue".into()),
        })
        .await
        .unwrap();
    assert!(restarted.running && restarted.exit.is_none());
    tokio::time::timeout(TIMEOUT, async {
        while !dir.path().join("restart-args").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let args = std::fs::read_to_string(dir.path().join("restart-args")).unwrap();
    assert!(args.contains("--resume\nidle-test\n"), "{args}");
    assert!(args.contains("continue"), "{args}");
    client.stop_terminal(&idle.id).await.unwrap();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let info = terminals.info(&idle.id).unwrap();
            if let Some(exit) = info.exit {
                assert!(!exit.idle, "a restarted run must not inherit the idle exit");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    for info in terminals.list() {
        terminals.close(&info.id).await.unwrap();
    }
    let _: serde_json::Value = client.call(Call::StopDaemon).await.unwrap();
    assert!(
        tokio::time::timeout(TIMEOUT, daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}
