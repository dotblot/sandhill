//! The real bridge and terminal host, with a fake Codex speaking the documented
//! app-server protocol and a raw interactive CLI on the same controlling PTY.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use base64::Engine;
use sandhill_daemon::terminal::{Launch, Terminals, ToHost};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(30);
const THREAD: &str = "019546a0-0000-7000-8000-000000000001";

async fn command(terminals: &Terminals, id: &str, command: Value) {
    terminals
        .send(id, ToHost::Command { command })
        .await
        .unwrap();
}

async fn until(
    terminals: &Terminals,
    id: &str,
    events: &mut mpsc::UnboundedReceiver<ServerMessage>,
    entries: &mut Vec<Value>,
    output: &mut String,
    ready: impl Fn(&[Value], &str, &Value) -> bool,
) {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let info = terminals.info(id).unwrap();
            let agent = info.agent.unwrap_or_default();
            if ready(entries, output, &agent) {
                break;
            }
            assert!(info.running, "bridge exited: {entries:#?}\n{output}");
            if let Some(ServerMessage::Terminal { terminal }) = events.recv().await {
                if let Some(data) = terminal.data {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap();
                    output.push_str(&String::from_utf8_lossy(&bytes));
                }
                if let Some(entry) = terminal.chat {
                    entries.push(entry.event);
                }
                assert!(
                    terminal.exit.is_none(),
                    "bridge exited: {entries:#?}\n{output}"
                );
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("native transition timed out: {entries:#?}\n{output}"));
}

fn records(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn native_codex_exclusively_resumes_the_chat_thread_and_imports_new_turns() {
    let Some(python) = sandhill_daemon::agents::find("python3") else {
        eprintln!("skipping: python3 is needed for the fake Codex process");
        return;
    };
    let dir = common::tempdir().unwrap();
    let fake = dir.path().join("codex");
    let script = r#"
import json, os, pathlib, sys, termios, tty
root = pathlib.Path(__file__).parent
thread = '019546a0-0000-7000-8000-000000000001'
history = root / 'history.json'
server_owner = root / 'server-owner'
native_owner = root / 'native-owner'

def log(kind, **fields):
    with (root / 'calls.jsonl').open('a') as out:
        out.write(json.dumps(dict(kind=kind, **fields)) + '\n')

def load():
    return json.loads(history.read_text())

def save(turns):
    history.write_text(json.dumps(turns))

def send(message):
    print(json.dumps(message), flush=True)

def event(method, params):
    send(dict(method=method, params=params))

def reply(request, result):
    send(dict(id=request['id'], result=result))

if 'app-server' in sys.argv:
    assert not native_owner.exists(), 'app-server overlaps native CLI'
    server_owner.write_text(str(os.getpid()))
    log('server_start', args=sys.argv[1:])
    active = None
    try:
        for line in sys.stdin:
            request = json.loads(line)
            method = request.get('method')
            params = request.get('params', {})
            assert not native_owner.exists(), 'bridge sent RPC while native CLI owned thread'
            log('rpc', method=method, params=params)
            if method == 'initialize':
                reply(request, {})
            elif method == 'initialized':
                pass
            elif method in ('thread/start', 'thread/resume'):
                assert params['threadId'] == thread
                turns = load()
                paginated = any(turn['id'].startswith('native-') for turn in turns)
                reply(request, dict(thread=dict(id=thread, turns=[] if paginated else turns), model='native-model', reasoningEffort='high', turnsBackwardsCursor='all-turns' if paginated else None))
            elif method == 'thread/turns/list':
                assert params['itemsView'] == 'full'
                reply(request, dict(data=list(reversed(load())), nextCursor=None))
            elif method == 'model/list':
                reply(request, dict(data=[]))
            elif method == 'account/read':
                reply(request, dict(account=None, requiresOpenaiAuth=False))
            elif method == 'turn/start':
                text = params['input'][0]['text']
                turns = load()
                active = dict(id='chat-' + str(len(turns)), status='inProgress', items=[dict(type='userMessage', id='user-' + str(len(turns)), content=[dict(type='text', text=text)])])
                turns.append(active)
                save(turns)
                event('turn/started', dict(turn=dict(id=active['id'])))
                reply(request, dict(turn=dict(id=active['id'])))
                if text != 'active':
                    active['status'] = 'completed'
                    item = dict(type='agentMessage', id='reply-' + active['id'], text='chat reply: ' + text)
                    active['items'].append(item)
                    save(turns)
                    event('item/completed', dict(turnId=active['id'], item=item))
                    event('turn/completed', dict(turn=active))
                    active = None
            elif method == 'turn/interrupt':
                reply(request, {})
                assert active is not None
                turns = load()
                turns[-1]['status'] = 'interrupted'
                save(turns)
                event('turn/completed', dict(turn=turns[-1]))
                active = None
            elif method == 'thread/unsubscribe':
                assert active is None, 'unsubscribed before active turn stopped'
                reply(request, dict(status='unsubscribed'))
            else:
                reply(request, {})
    finally:
        server_owner.unlink(missing_ok=True)
        log('server_exit')
else:
    assert not server_owner.exists(), 'native CLI overlaps app-server'
    assert all(os.isatty(fd) for fd in (0, 1, 2)), 'native CLI did not inherit the terminal'
    assert os.tcgetpgrp(0) == os.getpgrp(), 'native CLI is not foreground'
    if (root / 'fail-native').exists():
        log('native_failure')
        sys.exit(9)
    native_owner.write_text(str(os.getpid()))
    log('native_start', args=sys.argv[1:], cwd=os.getcwd(), tty=True)
    old = termios.tcgetattr(0)
    text = ''
    try:
        tty.setraw(0)
        os.write(1, b'NATIVE_READY\r\n')
        while True:
            char = os.read(0, 1)
            if char == b'\x03':
                log('native_interrupt')
                text = ''
            elif char in (b'\r', b'\n'):
                if text == '/exit':
                    break
                if text:
                    log('native_input', text=text)
                    turns = load()
                    turns.append(dict(id='native-' + str(len(turns)), status='completed', items=[
                        dict(type='userMessage', id='native-user', content=[dict(type='text', text=text)]),
                        dict(type='commandExecution', id='native-command', command='echo native', status='completed', aggregatedOutput='native output', exitCode=0),
                        dict(type='agentMessage', id='native-reply', text='native reply'),
                    ]))
                    save(turns)
                    os.write(1, b'NATIVE_SAVED\r\n')
                text = ''
            else:
                text += char.decode()
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)
        native_owner.unlink(missing_ok=True)
        log('native_exit')
"#;
    std::fs::write(&fake, format!("#!{}\n{script}", python.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.path().join("history.json"), json!([{
        "id": "old-turn", "status": "completed", "items": [
            {"type": "userMessage", "id": "old-user", "content": [{"type": "text", "text": "old prompt"}]},
            {"type": "agentMessage", "id": "old-reply", "text": "old reply"}
        ]
    }]).to_string()).unwrap();
    let options = AgentOptions {
        model: Some("native-model".into()),
        effort: Some("high".into()),
        permission: Some("auto".into()),
        ..AgentOptions::default()
    };
    let terminals = Terminals::new(
        dir.path().join("terminals"),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    let info = terminals
        .start(Launch {
            kind: "codex".into(),
            title: "Native Codex".into(),
            program: env!("CARGO_BIN_EXE_sandhilld").into(),
            args: vec![
                "--socket".into(),
                dir.path()
                    .join("unused.sock")
                    .to_string_lossy()
                    .into_owned(),
                "codex-chat".into(),
                "--codex".into(),
                fake.to_string_lossy().into_owned(),
                "--thread".into(),
                THREAD.into(),
                "--options".into(),
                serde_json::to_string(&options).unwrap(),
                "--".into(),
                "-c".into(),
                "mcp_servers.sandhill.command=\"sandhill-mcp\"".into(),
            ],
            env: vec![],
            cwd: dir.path().to_path_buf(),
            project: None,
            options,
            size: Some((80, 24)),
            job: None,
        })
        .await
        .unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    let mut output = String::new();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |entries, _, _| entries.iter().any(|event| event["kind"] == "session"),
    )
    .await;

    command(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "active"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |entries, _, _| {
            entries
                .iter()
                .any(|event| event["kind"] == "prompt" && event["text"] == "active")
        },
    )
    .await;
    command(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "queued before native"}),
    )
    .await;
    command(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "terminal"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |_, output, agent| output.contains("NATIVE_READY") && agent["view_mode"] == "terminal",
    )
    .await;
    assert!(!dir.path().join("server-owner").exists());
    command(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "queued during native"}),
    )
    .await;
    command(
        &terminals,
        &info.id,
        json!({"kind": "set", "effort": "low"}),
    )
    .await;
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: "native input\r".into(),
            },
        )
        .await
        .unwrap();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |_, output, _| output.contains("NATIVE_SAVED"),
    )
    .await;
    assert_eq!(
        records(dir.path())
            .iter()
            .filter(|record| record["kind"] == "rpc" && record["method"] == "turn/start")
            .count(),
        1
    );
    assert!(!entries.iter().any(|event| event["text"] == "native reply"));
    command(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "chat"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |entries, _, agent| {
            agent["view_mode"] == "chat"
                && entries.iter().any(|event| {
                    event["kind"] == "prompt" && event["text"] == "queued during native"
                })
                && entries
                    .iter()
                    .filter(|event| event["kind"] == "stop")
                    .count()
                    >= 4
        },
    )
    .await;
    assert_eq!(
        entries
            .iter()
            .filter(|event| event["kind"] == "prompt" && event["text"] == "native input")
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|event| event["kind"] == "text" && event["text"] == "native reply")
            .count(),
        1
    );
    assert!(entries.iter().any(|event| event["kind"] == "step_done"
        && event["tool"] == "Bash"
        && event["output"]["stdout"] == "native output"));
    assert!(!entries.iter().any(|event| event["turn"] == "old-turn"));
    let calls = records(dir.path());
    let native = calls
        .iter()
        .find(|record| record["kind"] == "native_start")
        .unwrap();
    let args = native["args"].as_array().unwrap();
    let resume = args.iter().position(|arg| arg == "resume").unwrap();
    assert_eq!(args[resume + 1], THREAD);
    for expected in [
        "mcp_servers.sandhill.command=\"sandhill-mcp\"",
        "model_reasoning_effort=\"high\"",
        "sandbox_mode=\"workspace-write\"",
        "approval_policy=\"on-request\"",
        "approvals_reviewer=\"auto_review\"",
    ] {
        assert!(
            args.iter().any(|arg| arg == expected),
            "missing {expected}: {args:?}"
        );
    }
    assert!(args.iter().any(|arg| arg.as_str().is_some_and(|arg| {
        arg.starts_with("developer_instructions=\"You are running inside Sandhill")
    })));
    assert!(args.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|arg| arg.starts_with("mcp_servers.sandhill.env.SANDHILL_TERMINAL="))
    }));
    assert!(
        args.windows(2)
            .any(|args| args[0] == "--model" && args[1] == "native-model")
    );
    assert!(args.iter().any(|arg| arg == "--no-daemon"));
    assert_eq!(
        native["cwd"],
        dir.path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    let native_start = calls
        .iter()
        .position(|record| record["kind"] == "native_start")
        .unwrap();
    let native_exit = calls
        .iter()
        .position(|record| record["kind"] == "native_exit")
        .unwrap();
    assert!(
        calls[native_start..native_exit]
            .iter()
            .all(|record| record["kind"] != "rpc" && record["kind"] != "server_start")
    );
    assert!(
        calls
            .iter()
            .any(|record| record["kind"] == "native_interrupt")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|record| record["kind"] == "rpc" && record["method"] == "thread/resume")
            .count(),
        2
    );
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        THREAD
    );

    // Returning without a new native turn must not replay the previous import.
    let after = entries.len();
    let output_after = output.len();
    command(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "terminal"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |_, output, agent| {
            output[output_after..].contains("NATIVE_READY") && agent["view_mode"] == "terminal"
        },
    )
    .await;
    terminals
        .send(
            &info.id,
            ToHost::Input {
                data: "/exit\r".into(),
            },
        )
        .await
        .unwrap();
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |entries, _, agent| {
            agent["view_mode"] == "chat"
                && entries[after..]
                    .iter()
                    .any(|event| event["source"] == "resume")
        },
    )
    .await;
    assert!(
        !entries[after..]
            .iter()
            .any(|event| event["kind"] == "prompt" || event["kind"] == "text")
    );

    // A failed native invocation still restores Chat and its original thread.
    std::fs::write(dir.path().join("fail-native"), "").unwrap();
    let after = entries.len();
    command(
        &terminals,
        &info.id,
        json!({"kind": "view", "mode": "terminal"}),
    )
    .await;
    until(
        &terminals,
        &info.id,
        &mut events,
        &mut entries,
        &mut output,
        |entries, _, agent| {
            agent["view_mode"] == "chat"
                && entries[after..].iter().any(|event| {
                    event["kind"] == "error"
                        && event["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("Native agent interface exited"))
                })
                && entries[after..]
                    .iter()
                    .any(|event| event["source"] == "resume")
        },
    )
    .await;
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["thread"],
        THREAD
    );
    terminals.close(&info.id).await.unwrap();
}

#[tokio::test]
async fn codex_sends_initial_say_and_set_skills_as_native_turn_input() {
    let Some(python) = sandhill_daemon::agents::find("python3") else {
        eprintln!("skipping: python3 is needed for the fake Codex process");
        return;
    };
    let dir = common::tempdir().unwrap();
    let fake = dir.path().join("codex-skills");
    let script = r#"
import json, pathlib, sys, time
root = pathlib.Path(__file__).parent
number = 0
def send(value):
    print(json.dumps(value), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    params = request.get('params', {})
    result = {}
    if method == 'initialized':
        continue
    if method == 'thread/start':
        result = dict(thread=dict(id='skill-thread', turns=[]), model='test-model')
    elif method == 'model/list':
        result = dict(data=[])
    elif method == 'account/read':
        result = dict(account=None)
    elif method == 'turn/start':
        with (root / 'calls.jsonl').open('a') as out:
            out.write(json.dumps(dict(kind='turn', input=params['input'])) + '\n')
        number += 1
        turn = dict(id='turn-' + str(number), status='completed', items=[])
        result = dict(turn=turn)
    send(dict(id=request['id'], result=result))
    if method == 'turn/start':
        time.sleep(0.05)
        send(dict(method='turn/completed', params=dict(turn=turn)))
"#;
    std::fs::write(&fake, format!("#!{}\n{script}", python.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
    let skill = |name: &str| json!({"kind": "skill", "name": name, "path": format!("/skills/{name}/SKILL.md")});
    let options = AgentOptions {
        refs: vec![skill("initial")],
        ..AgentOptions::default()
    };
    let terminals = Terminals::new(
        dir.path().join("terminals"),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    let info = terminals
        .start(Launch {
            kind: "codex".into(),
            title: "Codex skills".into(),
            program: env!("CARGO_BIN_EXE_sandhilld").into(),
            args: vec![
                "--socket".into(),
                dir.path()
                    .join("unused.sock")
                    .to_string_lossy()
                    .into_owned(),
                "codex-chat".into(),
                "--codex".into(),
                fake.to_string_lossy().into_owned(),
                "--prompt".into(),
                "First message".into(),
                "--options".into(),
                serde_json::to_string(&options).unwrap(),
            ],
            env: vec![],
            cwd: dir.path().to_path_buf(),
            project: None,
            options,
            size: None,
            job: None,
        })
        .await
        .unwrap();
    let (out, mut events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(&info.id, Some(0), Some(0))
            .await
            .unwrap()
            .forward(out),
    );
    let mut entries = Vec::new();
    let mut output = String::new();
    for (index, text) in [
        "First message",
        "Second message",
        "Third message",
        "No skill",
    ]
    .iter()
    .enumerate()
    {
        if index == 1 {
            command(
                &terminals,
                &info.id,
                json!({"kind": "say", "text": text, "refs": [skill("say")]}),
            )
            .await;
        } else if index == 2 {
            command(
                &terminals,
                &info.id,
                json!({"kind": "set", "refs": [skill("set")]}),
            )
            .await;
            command(&terminals, &info.id, json!({"kind": "say", "text": text})).await;
        } else if index == 3 {
            command(&terminals, &info.id, json!({"kind": "say", "text": text})).await;
        }
        until(
            &terminals,
            &info.id,
            &mut events,
            &mut entries,
            &mut output,
            |entries, _, agent| {
                entries.iter().filter(|e| e["kind"] == "stop").count() > index
                    && agent["state"] == "idle"
            },
        )
        .await;
    }
    let turns = records(dir.path());
    assert_eq!(turns.len(), 4);
    for (turn, name) in turns[..3].iter().zip(["initial", "say", "set"]) {
        let input = turn["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "text");
        assert_eq!(
            input[1],
            json!({"type": "skill", "name": name, "path": format!("/skills/{name}/SKILL.md")})
        );
        assert!(!input[0]["text"].as_str().unwrap().contains("saved as"));
    }
    assert_eq!(turns[3]["input"].as_array().unwrap().len(), 1);
    for name in ["initial", "say", "set"] {
        assert!(
            entries
                .iter()
                .any(|e| e["kind"] == "prompt" && e["place"]["refs"] == json!([name]))
        );
    }
    terminals.close(&info.id).await.unwrap();
}
