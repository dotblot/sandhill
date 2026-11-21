//! Antigravity as on a Linux server reached over SSH: real terminal-host and
//! bridge processes, and an `agy` stand-in that, like Linux `agy` with
//! `SSH_CONNECTION` set, asks its controlling terminal what it is (DA2) even
//! when it runs headless. From a background process group of the
//! conversation's terminal that stops it (SIGTTOU) before its first event,
//! so a message would never end. Also: a message whose `agy` waits for a
//! sign-in fails within a bound, and the command line opens before any
//! message. No Google login, model request or kernel is needed.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use sandhill_daemon::agents::{self, Context};
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const AGY: &str = r#"#!/usr/bin/env python3
import json, os, select, sys, termios, time, tty
args = sys.argv[1:]
root = os.environ.get('FAKE_ROOT', '')
def record(kind, **fields):
    with open(os.path.join(root, 'calls.jsonl'), 'a') as out:
        out.write(json.dumps(dict(kind=kind, args=args, **fields)) + '\n')
def flag(name):
    return args[args.index(name)+1] if name in args else None
if args == ['--help']:
    print('--input-format --output-format --print-timeout --model --conversation --mode --dangerously-skip-permissions')
    sys.exit(0)
if args == ['--version']:
    print('agy version 1.5.6')
    sys.exit(0)
def probe():
    # Linux agy over SSH: terminal detection with a DA2 probe, which puts the
    # terminal in raw mode first. In a background process group that is
    # SIGTTOU, which stops the process until someone continues it.
    try:
        fd = os.open('/dev/tty', os.O_RDWR | os.O_NOCTTY)
    except OSError:
        return False
    saved = termios.tcgetattr(fd)
    tty.setraw(fd)
    os.write(fd, b'\x1b[>c')
    select.select([fd], [], [], 0.2)
    termios.tcsetattr(fd, termios.TCSANOW, saved)
    os.close(fd)
    return True
assert os.environ.get('SSH_CONNECTION'), 'the test runs agy as over SSH'
probed = probe()
if args == ['models']:
    record('models', probed=probed)
    print('gemini-3.8-flash-high     Gemini 3.8 Flash (High)')
    sys.exit(0)
if args[:2] == ['mcp', 'add']:
    record('mcp', probed=probed)
    sys.exit(0)
if args == ['acp']:
    for line in sys.stdin:
        request = json.loads(line)
        if request['method'] == 'initialize':
            result = {'protocolVersion':1,'agentCapabilities':{'loadSession':True}}
        elif request['method'] == 'session/load':
            record('loaded', probed=probed, session=request['params']['sessionId'])
            result = {}
        else:
            raise AssertionError('history reader must not send prompts: ' + request['method'])
        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    sys.exit(0)
conversation = flag('--conversation') or 'conv-remote'
def emit(kind, body, **top):
    print(json.dumps(dict(event=kind, **{kind: body}, **top)), flush=True)
if '--input-format' in args:
    record('headless', probed=probed)
    if os.path.exists(os.path.join(root, 'no-start')):
        print('Error: cannot reach the language server', file=sys.stderr, flush=True)
        sys.exit(1)
    if os.path.exists(os.path.join(root, 'signed-out')):
        # As agy does when its saved sign-in is missing: it waits for a browser.
        print('Authentication required. Please visit the URL to log in:', file=sys.stderr, flush=True)
        print('  https://accounts.example/o/oauth2/auth', file=sys.stderr, flush=True)
        time.sleep(600)
        sys.exit(1)
    emit('init', {'model': flag('--model')}, conversation_id=conversation)
    wire = [json.loads(line) for line in sys.stdin if line.strip()]
    if not wire:
        # No message: the conversation is started, without a model turn.
        record('started', conversation=conversation)
        sys.exit(0)
    prompt = ''.join(part['text'] for part in wire[0]['message']['content'])
    record('prompt', text=prompt)
    emit('step_update', {'conversation_id':conversation, 'step_index':0, 'state':'DONE',
                         'step_type':'agent_response', 'text_delta':'I am the stand-in.'})
    emit('result', {'conversation_id':conversation, 'status':'SUCCESS', 'response':'I am the stand-in.'})
    sys.exit(0)
record('native', probed=probed, conversation=flag('--conversation'))
assert sys.stdin.isatty() and os.tcgetpgrp(0) == os.getpgrp()
tty.setraw(0)
buffer = b''
while True:
    key = os.read(0, 1)
    if not key:
        break
    if key == b'\x03':
        buffer = b''
    elif key in (b'\r', b'\n'):
        if buffer == b'/exit':
            break
        buffer = b''
    else:
        buffer += key
record('native_exit')
"#;

#[derive(Default)]
struct Observed {
    chat: Vec<Value>,
    agent: Value,
    view_errors: Vec<String>,
    output: Vec<u8>,
    exit: bool,
}

fn calls(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

async fn until(root: &Path, observed: &Mutex<Observed>, wait: Duration, ready: impl Fn() -> bool) {
    tokio::time::timeout(wait, async {
        loop {
            if ready() {
                return;
            }
            {
                let seen = observed.lock().unwrap();
                assert!(
                    !seen.exit,
                    "bridge exited: {}\n{:?}",
                    String::from_utf8_lossy(&seen.output),
                    calls(root)
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let seen = observed.lock().unwrap();
        panic!(
            "timed out: {:?}\nchat: {:?}\nagent: {}\nterminal: {}",
            calls(root),
            seen.chat,
            seen.agent,
            String::from_utf8_lossy(&seen.output)
        );
    });
}

async fn send(terminals: &Terminals, id: &str, command: Value) {
    terminals
        .send(id, ToHost::Command { command })
        .await
        .unwrap();
}

async fn daemon(state: &Path) -> (Arc<Daemon>, PathBuf) {
    std::fs::create_dir_all(state).unwrap();
    let socket = state.join("daemon.sock");
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

#[tokio::test]
async fn agy_over_ssh_answers_fails_in_bounded_time_and_opens_its_command_line() {
    if agents::find("python3").is_none() {
        eprintln!("skipping: python3 is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let root = dir.path();
    let bin = root.join("bin");
    let home = root.join("home");
    let project = root.join("project");
    for path in [&bin, &home, &project] {
        std::fs::create_dir_all(path).unwrap();
    }
    let agy = bin.join("agy");
    std::fs::write(&agy, AGY).unwrap();
    std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Only this test runs in this integration-test binary.
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    unsafe {
        std::env::set_var("PATH", &path);
    }
    let (_daemon, socket) = daemon(&root.join("state")).await;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    // Sandhill's tools are already in agy's settings.
    let config = home.join(".gemini/config/mcp_config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let entry = json!({"command":exe,"args":["mcp"]});
    common::allow_antigravity_tools(&home, "sandhill", &entry);
    std::fs::write(
        &config,
        json!({"mcpServers":{"sandhill":entry}}).to_string(),
    )
    .unwrap();
    let project_name = project.to_string_lossy().into_owned();
    let options = AgentOptions {
        model: Some("gemini-3.8-flash-high".into()),
        permission: Some("supervised".into()),
        ..AgentOptions::default()
    };
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: None,
        resume: None,
    };
    let mut launch = agents::launch("antigravity", &options, &context)
        .await
        .unwrap();
    launch.env.extend([
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("FAKE_ROOT".into(), root.to_string_lossy().into_owned()),
        // What a daemon started over ssh passes on to its terminals.
        (
            "SSH_CONNECTION".into(),
            "192.0.2.1 50000 192.0.2.2 22".into(),
        ),
        ("SSH_CLIENT".into(), "192.0.2.1 50000 22".into()),
    ]);
    let terminals = Terminals::new(root.join("state/terminals"), exe);
    let info = terminals.start(launch).await.unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let observed = Arc::new(Mutex::new(Observed::default()));
    tokio::spawn({
        let observed = observed.clone();
        async move {
            while let Some(event) = events.recv().await {
                if let ServerMessage::Terminal { terminal } = event {
                    let mut seen = observed.lock().unwrap();
                    if let Some(agent) = terminal.agent {
                        if let Some(error) = agent["view_error"].as_str() {
                            seen.view_errors.push(error.to_string());
                        }
                        seen.agent = agent;
                    }
                    if let Some(entry) = terminal.chat {
                        seen.chat.push(entry.event);
                    }
                    if let Some(data) = terminal.data {
                        seen.output.extend(
                            base64::engine::general_purpose::STANDARD
                                .decode(data)
                                .unwrap(),
                        );
                    }
                    seen.exit |= terminal.exit.is_some();
                }
            }
        }
    });
    let wait = Duration::from_secs(30);
    let has = |kind: &'static str| {
        let root = root.to_path_buf();
        move || calls(&root).iter().any(|call| call["kind"] == kind)
    };
    until(root, &observed, wait, has("models")).await;
    until(root, &observed, wait, || {
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .any(|event| event["kind"] == "session")
    })
    .await;

    // A command line that can't open says why once, as `view_error` in the
    // report that ends the switch (a notice the user dismisses), not in Chat.
    std::fs::write(root.join("no-start"), "").unwrap();
    send(
        &terminals,
        &info.id,
        json!({"kind":"view","mode":"terminal"}),
    )
    .await;
    until(root, &observed, wait, || {
        let seen = observed.lock().unwrap();
        !seen.view_errors.is_empty() && seen.agent["view_mode"] == "chat"
    })
    .await;
    {
        let seen = observed.lock().unwrap();
        assert_eq!(
            seen.view_errors,
            [
                "Antigravity's command line failed: agy started no conversation (exit status: 1): Error: cannot reach the language server"
            ]
        );
        assert!(
            !seen.chat.iter().any(|event| event["kind"] == "error"),
            "{:?}",
            seen.chat
        );
    }
    assert!(!calls(root).iter().any(|call| call["kind"] == "native"));
    std::fs::remove_file(root.join("no-start")).unwrap();

    // The command line before any message: a conversation is started for it
    // (no model turn), and it opens that conversation.
    send(
        &terminals,
        &info.id,
        json!({"kind":"view","mode":"terminal"}),
    )
    .await;
    until(root, &observed, wait, has("native")).await;
    let native = calls(root)
        .into_iter()
        .find(|call| call["kind"] == "native")
        .unwrap();
    assert_eq!(native["conversation"], "conv-remote");
    // It owns the terminal, so it may ask it what it is.
    assert_eq!(native["probed"], true);
    assert!(calls(root).iter().any(|call| call["kind"] == "started"));
    assert!(!calls(root).iter().any(|call| call["kind"] == "prompt"));
    assert_eq!(observed.lock().unwrap().agent["thread"], "conv-remote");
    send(&terminals, &info.id, json!({"kind":"view","mode":"chat"})).await;
    until(root, &observed, wait, || {
        let seen = observed.lock().unwrap();
        seen.agent["view_mode"] == "chat" && seen.agent["state"] == "idle"
    })
    .await;
    until(root, &observed, wait, has("native_exit")).await;
    {
        let seen = observed.lock().unwrap();
        assert_eq!(seen.view_errors.len(), 1, "{:?}", seen.view_errors);
        assert!(seen.agent["view_error"].is_null());
        assert!(
            !seen.chat.iter().any(|event| event["kind"] == "error"),
            "{:?}",
            seen.chat
        );
    }

    // A message ends with agy's answer: headless agy has no controlling
    // terminal to probe, so nothing stops it.
    let before = observed.lock().unwrap().chat.len();
    send(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"what model are you"}),
    )
    .await;
    until(root, &observed, wait, || {
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .skip(before)
            .any(|event| event["kind"] == "stop")
    })
    .await;
    {
        let seen = observed.lock().unwrap();
        let kinds: Vec<&str> = seen.chat[before..]
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["prompt", "text", "stop"], "{:?}", seen.chat);
        assert_eq!(seen.chat[before + 1]["text"], "I am the stand-in.");
    }
    let rows = calls(root);
    let prompt = rows.iter().find(|call| call["kind"] == "prompt").unwrap();
    assert!(
        prompt["args"]
            .as_array()
            .unwrap()
            .windows(2)
            .any(|pair| pair[0] == "--conversation" && pair[1] == "conv-remote"),
        "{prompt}"
    );
    for call in &rows {
        if matches!(
            call["kind"].as_str(),
            Some("headless" | "models" | "mcp" | "loaded")
        ) {
            assert_eq!(call["probed"], false, "ran in the terminal: {call}");
        }
    }

    // agy waiting for a sign-in never says anything: the message fails,
    // within a bound, with what agy said and how to sign it in.
    std::fs::write(root.join("signed-out"), "").unwrap();
    let before = observed.lock().unwrap().chat.len();
    let asked = Instant::now();
    send(&terminals, &info.id, json!({"kind":"say","text":"hello"})).await;
    until(root, &observed, Duration::from_secs(60), || {
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .skip(before)
            .any(|event| event["kind"] == "stop")
    })
    .await;
    assert!(
        asked.elapsed() < Duration::from_secs(45),
        "{:?}",
        asked.elapsed()
    );
    {
        let seen = observed.lock().unwrap();
        let error = seen.chat[before..]
            .iter()
            .find(|event| event["kind"] == "error")
            .unwrap_or_else(|| panic!("{:?}", seen.chat));
        let text = error["text"].as_str().unwrap();
        for said in [
            "didn't start within 30 s",
            "Authentication required. Please visit the URL to log in.",
            "Sign Antigravity in",
        ] {
            assert!(text.contains(said), "{text}");
        }
        assert_eq!(seen.agent["state"], "idle");
    }
    terminals.close(&info.id).await.unwrap();
}
