//! Real terminal-host/bridge processes and an official-protocol agy stand-in.
//! No Google login, model request, or installed kernel is needed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use sandhill_daemon::agents::{self, Context};
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const AGY: &str = r#"#!/usr/bin/env python3
import json, os, signal, sys, time, tty
args = sys.argv[1:]
root = os.environ.get('FAKE_ROOT', '')
log = os.path.join(root, 'calls.jsonl')
history = os.path.join(root, 'history.json')
owner = os.path.join(root, 'owner')
def record(kind, **fields):
    with open(log, 'a') as out:
        out.write(json.dumps(dict(kind=kind, **fields)) + '\n')
def read_history():
    return json.load(open(history)) if os.path.exists(history) else []
def claim(kind):
    if os.path.exists(owner):
        old = int(open(owner).read())
        try:
            os.kill(old, 0)
        except ProcessLookupError:
            pass
        else:
            record('overlap', previous=old, current=os.getpid(), mode=kind)
            raise RuntimeError('conversation still owned by another agy process')
    with open(owner, 'w') as out:
        out.write(str(os.getpid()))
    record(kind, args=args, cwd=os.getcwd(), socket=os.environ.get('SANDHILL_SOCKET'),
           project=os.environ.get('SANDHILL_PROJECT'), pid=os.getpid())
def flag(name):
    return args[args.index(name)+1] if name in args else None
def emit(kind, body):
    print(json.dumps({'event':kind, kind:body}), flush=True)
def update(session_update, **fields):
    return dict(sessionUpdate=session_update, **fields)
if args == ['--help']:
    print('--input-format --output-format --print-timeout --effort --model --conversation --mode --dangerously-skip-permissions')
    sys.exit(0)
if args == ['--version']:
    print('agy version 1.5.5')
    sys.exit(0)
if args == ['models']:
    print('claude-sonnet-4-6')
    sys.exit(0)
if args[:2] == ['mcp', 'add']:
    record('mcp', args=args)
    sys.exit(0)
if args == ['acp']:
    claim('acp_start')
    for line in sys.stdin:
        request = json.loads(line)
        method = request['method']
        if method == 'initialize':
            result = {'protocolVersion':1,'agentCapabilities':{'loadSession':True}}
        elif method == 'session/load':
            assert request['params']['sessionId'] == 'conv-native'
            assert request['params']['cwd'] == os.getcwd()
            assert request['params']['mcpServers'] == []
            rows = read_history()
            for row in rows:
                print(json.dumps({'jsonrpc':'2.0','method':'session/update',
                    'params':{'sessionId':'conv-native','update':row}}), flush=True)
            result = {}
            record('loaded', count=len(rows))
        else:
            raise AssertionError('history reader must not send prompts: ' + method)
        print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
    sys.exit(0)
assert flag('--conversation') == 'conv-native'
if '--input-format' in args:
    claim('headless_start')
    assert flag('--input-format') == 'stream-json'
    assert flag('--output-format') == 'stream-json'
    wire = [json.loads(line) for line in sys.stdin if line.strip()]
    assert len(wire) == 1
    prompt = ''.join(part['text'] for part in wire[0]['message']['content'])
    record('headless_prompt', text=prompt)
    def stopped(*unused):
        # Waiting for stdout EOF alone is insufficient: release comes later.
        sys.stdout.close()
        time.sleep(0.15)
        record('headless_exit', stopped=True)
        sys.exit(130)
    signal.signal(signal.SIGINT, stopped)
    emit('init', {'model':flag('--model')})
    if prompt.startswith('slow'):
        emit('step_update', {'conversation_id':'conv-native','step_index':0,
                            'state':'ACTIVE','step_type':'agent_response','text_delta':'Thinking'})
        record('slow_ready')
        time.sleep(60)
    if 'after native' in prompt:
        assert any(row.get('content',{}).get('text') == 'native prompt' for row in read_history())
    emit('result', {'conversation_id':'conv-native','status':'SUCCESS','response':'continued from native'})
    record('headless_exit', stopped=False)
    sys.exit(0)
claim('native_start')
assert '--output-format' not in args and '--print-timeout' not in args
assert sys.stdin.isatty() and sys.stdout.isatty() and sys.stderr.isatty()
assert os.tcgetpgrp(0) == os.getpgrp(), 'native must own foreground PTY'
assert flag('--model') == 'claude-sonnet-4-6'
assert flag('--effort') == 'high'
assert '--dangerously-skip-permissions' in args
if os.path.exists(os.path.join(root,'fail-native')):
    tty.setraw(0)
    record('native_failure_ready')
    while os.read(0, 1) != b'!':
        pass
    record('native_exit', failed=True)
    sys.exit(3)
tty.setraw(0)
record('native_ready')
print('NATIVE_READY', flush=True)
buffer = b''
while True:
    key = os.read(0, 1)
    if not key:
        break
    record('key', byte=key.hex())
    if key == b'\x03':
        buffer = b''
        continue
    if key not in (b'\r', b'\n'):
        buffer += key
        continue
    text = buffer.decode()
    buffer = b''
    if text == '/exit':
        break
    if text.startswith('\x1b[A'):
        text = text[3:]
    rows = read_history()
    rows.extend([
        update('user_message_chunk',content={'type':'text','text':text}),
        update('tool_call',toolCallId='native-tool',title='native command',kind='execute',status='in_progress',rawInput={'command':'echo native'}),
        update('tool_call_update',toolCallId='native-tool',status='completed',rawOutput={'stdout':'native\n'}),
        update('agent_message_chunk',content={'type':'text','text':'native answer'}),
    ])
    with open(history,'w') as out:
        json.dump(rows,out)
    record('native_turn', text=text)
record('native_exit', failed=False)
"#;

#[derive(Default)]
struct Observed {
    chat: Vec<Value>,
    modes: Vec<String>,
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

async fn until(root: &Path, observed: &Mutex<Observed>, ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
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
            "timed out: {:?}\nchat: {:?}\nterminal: {}",
            calls(root),
            seen.chat,
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
async fn native_owns_the_same_conversation_and_imports_only_new_records() {
    if agents::find("python3").is_none() {
        eprintln!("skipping: python3 is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
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
    // Only this test runs in this integration-test binary. Child environments
    // isolate config and records from the installed CLI and the user's login.
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    unsafe {
        std::env::set_var("PATH", &path);
    }
    std::fs::write(root.join("history.json"), json!([
        {"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"existing prompt"}},
        {"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"existing answer"}}
    ]).to_string()).unwrap();
    let (_daemon, socket) = daemon(&root.join("state")).await;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_sandhilld"));
    let project_name = project.to_string_lossy().into_owned();
    let options = AgentOptions {
        model: Some("claude-sonnet-4-6".into()),
        effort: Some("high".into()),
        permission: Some("full".into()),
        ..AgentOptions::default()
    };
    let context = Context {
        executable: &exe,
        socket: &socket,
        project: Some(&project_name),
        cwd: &project,
        prompt: None,
        resume: Some("conv-native"),
    };
    let mut launch = agents::launch("antigravity", &options, &context)
        .await
        .unwrap();
    launch.env.extend([
        ("HOME".into(), home.to_string_lossy().into_owned()),
        ("FAKE_ROOT".into(), root.to_string_lossy().into_owned()),
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
                        if let Some(mode) = agent["view_mode"].as_str() {
                            seen.modes.push(mode.to_string());
                        }
                        if let Some(error) = agent["view_error"].as_str() {
                            seen.view_errors.push(error.to_string());
                        }
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
    until(root, &observed, || {
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .any(|event| event["kind"] == "session")
    })
    .await;
    send(&terminals, &info.id, json!({"kind":"say","text":"slow"})).await;
    until(root, &observed, || {
        calls(root)
            .iter()
            .any(|event| event["kind"] == "slow_ready")
    })
    .await;
    send(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"queued before native"}),
    )
    .await;
    send(
        &terminals,
        &info.id,
        json!({"kind":"view","mode":"terminal"}),
    )
    .await;
    until(root, &observed, || {
        calls(root)
            .iter()
            .any(|event| event["kind"] == "native_ready")
    })
    .await;
    let before = calls(root);
    assert_eq!(
        before
            .iter()
            .filter(|event| event["kind"] == "headless_start")
            .count(),
        1
    );
    let stopped = before
        .iter()
        .position(|event| event["kind"] == "headless_exit")
        .unwrap();
    let native = before
        .iter()
        .position(|event| event["kind"] == "native_start")
        .unwrap();
    assert!(
        stopped < native,
        "headless must be reaped before native starts"
    );
    let run = &before[native];
    assert_eq!(
        run["cwd"],
        project.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(run["socket"], socket.to_string_lossy().as_ref());
    assert_eq!(run["project"], project_name);
    send(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"queued during native"}),
    )
    .await;
    // Arrow keys and an unterminated prefix must reach the native process
    // immediately, rather than being stolen by a bridge line-reading thread.
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: "\u{1b}[Anative".into(),
            },
        )
        .await
        .unwrap();
    until(root, &observed, || {
        calls(root)
            .iter()
            .filter(|event| event["kind"] == "key")
            .count()
            >= 9
    })
    .await;
    assert_eq!(
        calls(root)
            .iter()
            .filter(|event| event["kind"] == "headless_start")
            .count(),
        1
    );
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: " prompt\r".into(),
            },
        )
        .await
        .unwrap();
    until(root, &observed, || {
        calls(root)
            .iter()
            .any(|event| event["kind"] == "native_turn")
    })
    .await;
    send(&terminals, &info.id, json!({"kind":"view","mode":"chat"})).await;
    until(root, &observed, || {
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .any(|event| event["text"] == "native answer")
    })
    .await;
    until(root, &observed, || {
        calls(root).iter().any(|event| {
            event["kind"] == "headless_prompt"
                && event["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("queued during native"))
        })
    })
    .await;
    send(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"after native"}),
    )
    .await;
    until(root, &observed, || {
        calls(root).iter().any(|event| {
            event["kind"] == "headless_prompt"
                && event["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("after native"))
        })
    })
    .await;
    until(root, &observed, || {
        terminals.info(&info.id).unwrap().agent.unwrap()["state"] == "idle"
    })
    .await;
    let rows = calls(root);
    assert!(
        !rows.iter().any(|event| event["kind"] == "overlap"),
        "{rows:?}"
    );
    let exited = rows
        .iter()
        .position(|event| event["kind"] == "native_exit")
        .unwrap();
    let imported = rows
        .iter()
        .enumerate()
        .skip(exited + 1)
        .find(|(_, event)| event["kind"] == "loaded" && event["count"] == 6)
        .unwrap()
        .0;
    assert!(
        rows.iter()
            .enumerate()
            .skip(native + 1)
            .filter(|(_, event)| event["kind"] == "headless_start")
            .all(|(at, _)| at > imported)
    );
    {
        let seen = observed.lock().unwrap();
        assert_eq!(
            seen.chat
                .iter()
                .filter(|event| event["kind"] == "prompt" && event["text"] == "native prompt")
                .count(),
            1
        );
        assert!(
            !seen
                .chat
                .iter()
                .any(|event| event["text"] == "existing prompt"
                    || event["text"] == "existing answer")
        );
        assert!(
            seen.chat
                .iter()
                .any(|event| event["kind"] == "step" && event["tool"] == "Bash")
        );
        assert!(
            seen.chat.iter().any(
                |event| event["kind"] == "step_done" && event["output"]["stdout"] == "native\n"
            )
        );
        for mode in ["switching_terminal", "terminal", "switching_chat", "chat"] {
            assert!(
                seen.modes.iter().any(|seen| seen == mode),
                "{:?}",
                seen.modes
            );
        }
    }
    // A second handoff replays the full official history but imports no old
    // turns. It also proves terminal settings and foreground ownership restore.
    send(
        &terminals,
        &info.id,
        json!({"kind":"view","mode":"terminal"}),
    )
    .await;
    until(root, &observed, || {
        calls(root)
            .iter()
            .filter(|event| event["kind"] == "native_ready")
            .count()
            == 2
    })
    .await;
    send(&terminals, &info.id, json!({"kind":"view","mode":"chat"})).await;
    until(root, &observed, || {
        calls(root)
            .iter()
            .filter(|event| event["kind"] == "loaded")
            .count()
            == 4
            && terminals.info(&info.id).unwrap().agent.unwrap()["view_mode"] == "chat"
    })
    .await;
    assert_eq!(
        observed
            .lock()
            .unwrap()
            .chat
            .iter()
            .filter(|event| event["text"] == "native answer")
            .count(),
        1
    );
    assert!(!calls(root).iter().any(|event| event["kind"] == "overlap"));
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        "conv-native"
    );
    // Unexpected native failure is visible in Chat, restores ownership, and
    // preserves commands queued while native still exclusively owned it.
    std::fs::write(root.join("fail-native"), "").unwrap();
    send(
        &terminals,
        &info.id,
        json!({"kind":"view","mode":"terminal"}),
    )
    .await;
    until(root, &observed, || {
        calls(root)
            .iter()
            .any(|event| event["kind"] == "native_failure_ready")
    })
    .await;
    send(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"after native failure"}),
    )
    .await;
    terminals
        .send(&info.id, ToHost::Input { data: "!".into() })
        .await
        .unwrap();
    until(root, &observed, || {
        observed.lock().unwrap().chat.iter().any(|event| {
            event["kind"] == "error"
                && event["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Native agent interface exited"))
        })
    })
    .await;
    until(root, &observed, || {
        terminals.info(&info.id).unwrap().agent.unwrap()["view_mode"] == "chat"
    })
    .await;
    until(root, &observed, || {
        calls(root).iter().any(|event| {
            event["kind"] == "headless_prompt"
                && event["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("after native failure"))
        })
    })
    .await;
    until(root, &observed, || {
        terminals.info(&info.id).unwrap().agent.unwrap()["state"] == "idle"
    })
    .await;
    // The handoff itself worked: nothing to say about it.
    assert!(observed.lock().unwrap().view_errors.is_empty());
    assert!(!calls(root).iter().any(|event| event["kind"] == "overlap"));
    terminals.close(&info.id).await.unwrap();
}
