//! The real ACP bridge and terminal host with a stand-in Oh My Pi: config
//! option models and thinking, the approval overlay, Full access answering
//! approvals, Sandhill's instructions in the first message, and the handoff
//! to omp's own screen and back (the ACP server ends first, the session is
//! loaded again with the command line's turns).

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

const FAKE: &str = r#"
import json, os, pathlib, signal, sys, termios, tty
root = pathlib.Path(__file__).parent
session = 'fake-omp-session'
history_file = root / 'history.json'
acp_owner = root / 'acp-owner'
native_owner = root / 'native-owner'

def log(kind, **fields):
    with (root / 'calls.jsonl').open('a') as out:
        out.write(json.dumps(dict(kind=kind, **fields)) + '\n')

def load():
    return json.loads(history_file.read_text()) if history_file.exists() else []

def remember(update):
    history = load()
    history.append(update)
    history_file.write_text(json.dumps(history))

def send(message):
    print(json.dumps(dict(jsonrpc='2.0', **message)), flush=True)

def update(value, keep=True):
    if keep:
        remember(value)
    send(dict(method='session/update', params=dict(sessionId=session, update=value)))

state = dict(model='fake/a', thinking='off')

def options():
    return [
        dict(id='mode', name='Mode', category='mode', type='select', currentValue='default',
             options=[dict(value='default', name='Default'), dict(value='plan', name='Plan')]),
        dict(id='model', name='Model', category='model', type='select', currentValue=state['model'],
             options=[dict(value='fake/a', name='Fake A', description='fake/a'),
                      dict(value='fake/b', name='Fake B', description='fake/b')]),
        dict(id='thinking', name='Thinking', category='thought_level', type='select',
             currentValue=state['thinking'],
             options=[dict(value=v, name=v) for v in ('off', 'auto', 'low', 'high')]),
    ]

def session_result(**extra):
    return dict(configOptions=options(), modes=dict(currentModeId='default', availableModes=[
        dict(id='default', name='Default'), dict(id='plan', name='Plan')]), **extra)

if sys.argv[1:] == ['--help']:
    print('--approval-mode --allow-home --resume --thinking')
elif sys.argv[1:] == ['acp']:
    assert not native_owner.exists(), 'ACP server overlaps the command line'
    overlay = pathlib.Path(os.environ['PI_CONFIG_FILES']).read_text()
    assert 'approvalMode: always-ask' in overlay, overlay
    acp_owner.write_text(str(os.getpid()))
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    log('acp_start')
    active = None
    try:
        for line in sys.stdin:
            request = json.loads(line)
            method = request.get('method')
            params = request.get('params', {})
            log('rpc', method=method, params=params)
            if method == 'initialize':
                send(dict(id=request['id'], result=dict(protocolVersion=1,
                    agentCapabilities=dict(loadSession=True, mcpCapabilities=dict(http=True)))))
            elif method in ('session/new', 'session/load'):
                server = params['mcpServers'][0]
                assert server['type'] == 'http' and server['name'] == 'sandhill', server
                assert server['headers'][0]['value'].startswith('Bearer '), server
                if method == 'session/load':
                    assert params['sessionId'] == session
                    for value in load():
                        if value['sessionUpdate'] == 'model':
                            state['model'] = value['model']
                        else:
                            update(value, keep=False)
                    send(dict(id=request['id'], result=session_result()))
                else:
                    send(dict(id=request['id'], result=session_result(sessionId=session)))
            elif method == 'session/set_config_option':
                key = 'model' if params['configId'] == 'model' else 'thinking'
                state[key] = params['value']
                if key == 'model':
                    remember(dict(sessionUpdate='model', model=params['value']))
                send(dict(id=request['id'], result=dict(configOptions=options())))
            elif method == 'session/set_mode':
                send(dict(id=request['id'], result={}))
            elif method == 'session/prompt':
                active = request
                texts = [part['text'] for part in params['prompt']]
                log('prompt', texts=texts)
                update(dict(sessionUpdate='user_message_chunk', content=dict(type='text', text=texts[0])))
                tool = dict(toolCallId='edit-1', title='Edit notes', kind='edit', status='pending')
                update(dict(sessionUpdate='tool_call', **tool))
                send(dict(id='approval-1', method='session/request_permission', params=dict(
                    sessionId=session, toolCall=tool, options=[
                        dict(optionId='no', name='Reject', kind='reject_once'),
                        dict(optionId='yes', name='Allow', kind='allow_once')])))
            elif request.get('id') == 'approval-1':
                log('permission', result=request['result'])
                update(dict(sessionUpdate='tool_call_update', toolCallId='edit-1', status='completed'))
                update(dict(sessionUpdate='agent_message_chunk', content=dict(type='text', text='Chat reply.')))
                send(dict(id=active['id'], result=dict(stopReason='end_turn')))
                active = None
    finally:
        acp_owner.unlink(missing_ok=True)
        log('acp_exit')
else:
    assert not acp_owner.exists(), 'command line overlaps the ACP server'
    assert all(os.isatty(fd) for fd in (0, 1, 2)), 'command line did not get the terminal'
    native_owner.write_text(str(os.getpid()))
    log('native_start', args=sys.argv[1:], mcp=os.environ.get('SANDHILL_MCP_URL'))
    old = termios.tcgetattr(0)
    text = ''
    try:
        tty.setraw(0)
        os.write(1, b'NATIVE_READY\r\n')
        while True:
            char = os.read(0, 1)
            if char == b'\x03':
                text = ''
            elif char in (b'\r', b'\n'):
                if text == '/exit':
                    break
                if text:
                    remember(dict(sessionUpdate='user_message_chunk', content=dict(type='text', text=text)))
                    remember(dict(sessionUpdate='agent_message_chunk', content=dict(type='text', text='native reply')))
                    os.write(1, b'NATIVE_SAVED\r\n')
                text = ''
            else:
                text += char.decode()
    finally:
        termios.tcsetattr(0, termios.TCSANOW, old)
        native_owner.unlink(missing_ok=True)
        log('native_exit')
"#;

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
    .unwrap_or_else(|_| panic!("timed out: {entries:#?}\n{output}"));
}

fn records(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn omp_chat_hands_its_session_to_the_command_line_and_back() {
    let Some(python) = sandhill_daemon::agents::find("python3") else {
        eprintln!("skipping: python3 is needed for the stand-in omp");
        return;
    };
    let dir = common::tempdir().unwrap();
    let fake = dir.path().join("omp");
    std::fs::write(&fake, format!("#!{}\n{FAKE}", python.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let options = AgentOptions {
        model: Some("fake/b".into()),
        effort: Some("high".into()),
        permission: Some("full".into()),
        ..AgentOptions::default()
    };
    let terminals = Terminals::new(
        dir.path().join("terminals"),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    let info = terminals
        .start(Launch {
            kind: "omp".into(),
            title: "Stand-in Oh My Pi".into(),
            program: env!("CARGO_BIN_EXE_sandhilld").into(),
            args: vec![
                "--socket".into(),
                dir.path()
                    .join("unused.sock")
                    .to_string_lossy()
                    .into_owned(),
                "acp-chat".into(),
                "--agent".into(),
                "omp".into(),
                "--program".into(),
                fake.to_string_lossy().into_owned(),
                "--options".into(),
                serde_json::to_string(&options).unwrap(),
                "--prompt".into(),
                "In chat".into(),
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
        |entries, _, agent| {
            agent["state"] == "idle"
                && entries
                    .iter()
                    .any(|e| e["kind"] == "text" && e["text"] == "Chat reply.")
        },
    )
    .await;
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["status"]["model_id"], "fake/b");
    assert_eq!(agent["status"]["model"], "Fake B");
    assert_eq!(agent["status"]["effort"], "high");
    assert_eq!(agent["models"][0]["efforts"], json!(["low", "high"]));
    assert!(
        !entries.iter().any(|e| e["kind"] == "ask"),
        "Full access asks nothing"
    );
    let calls = records(dir.path());
    let permission = calls.iter().find(|r| r["kind"] == "permission").unwrap();
    assert_eq!(
        permission["result"],
        json!({"outcome": {"outcome": "selected", "optionId": "yes"}})
    );
    let prompt = calls.iter().find(|r| r["kind"] == "prompt").unwrap();
    assert_eq!(prompt["texts"][0], "In chat");
    assert!(
        prompt["texts"][1]
            .as_str()
            .unwrap()
            .starts_with("You are running inside Sandhill")
    );

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
    assert!(!dir.path().join("acp-owner").exists());
    command(
        &terminals,
        &info.id,
        json!({"kind": "say", "text": "queued during native"}),
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
                && agent["state"] == "idle"
                && entries
                    .iter()
                    .any(|e| e["kind"] == "prompt" && e["text"] == "queued during native")
                && entries.iter().filter(|e| e["kind"] == "stop").count() >= 3
        },
    )
    .await;
    // The command line's turn is imported once; the chat's first is not
    // repeated.
    for text in ["native input", "In chat"] {
        assert_eq!(
            entries
                .iter()
                .filter(|e| e["kind"] == "prompt" && e["text"] == text)
                .count(),
            1,
            "{text}: {entries:#?}"
        );
    }
    assert!(
        entries
            .iter()
            .any(|e| e["kind"] == "text" && e["text"] == "native reply")
    );
    let calls = records(dir.path());
    let native = calls.iter().find(|r| r["kind"] == "native_start").unwrap();
    let args: Vec<&str> = native["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    for pair in [
        ["--resume", "fake-omp-session"],
        ["--approval-mode", "yolo"],
        ["--model", "fake/b"],
        ["--thinking", "high"],
    ] {
        assert!(
            args.windows(2).any(|w| w == pair),
            "missing {pair:?}: {args:?}"
        );
    }
    assert!(args.contains(&"--allow-home"));
    let extension = args[args.iter().position(|a| *a == "--extension").unwrap() + 1];
    assert!(Path::new(extension).ends_with("omp-sandhill.js"));
    assert!(Path::new(extension).exists());
    assert!(
        native["mcp"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );
    assert_eq!(calls.iter().filter(|r| r["kind"] == "acp_start").count(), 2);
    // Only the new session's first message carries the instructions.
    let prompts: Vec<&Value> = calls.iter().filter(|r| r["kind"] == "prompt").collect();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1]["texts"][0], "queued during native");
    assert!(!prompts[1]["texts"].as_array().unwrap().iter().any(|t| {
        t.as_str()
            .unwrap()
            .starts_with("You are running inside Sandhill")
    }));
    terminals.close(&info.id).await.unwrap();
}
