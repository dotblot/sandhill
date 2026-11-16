//! The real ACP bridge and terminal host, with a local scripted OpenScience.
//! No model service, OpenScience installation, or Jupyter kernel is needed.
//! The fake also uses the supplied real MCP server to inspect and edit files.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use sandhill_daemon::terminal::{Launch, Terminals, ToHost};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::{AgentOptions, ServerMessage};
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(30);
const SESSION: &str = "fake-openscience-session";
const OPTIONS: &str = r#"[
    {"optionId":"once","name":"Allow once","kind":"allow_once"},
    {"optionId":"chat","name":"Allow for this chat","kind":"allow_always"},
    {"optionId":"project","name":"Allow for this project","kind":"allow_always"},
    {"optionId":"no","name":"Reject","kind":"reject_once"}
]"#;

const FAKE: &str = r#"
import json, os, pathlib, subprocess, sys
root = pathlib.Path(__file__).parent
session = 'fake-openscience-session'
history_file = root / 'history.json'
history = json.loads(history_file.read_text()) if history_file.exists() else []
active = None
servers = []


def record(kind, **fields):
    with (root / 'calls.jsonl').open('a') as out:
        out.write(json.dumps(dict(kind=kind, **fields)) + '\n')


def send(message):
    print(json.dumps(dict(jsonrpc='2.0', **message)), flush=True)


def reply(request, result):
    send(dict(id=request['id'], result=result))


def remember(update):
    history.append(update)
    history_file.write_text(json.dumps(history))


def update(session_update, **fields):
    value = dict(sessionUpdate=session_update, **fields)
    remember(value)
    send(dict(method='session/update', params=dict(sessionId=session, update=value)))


def text(kind, value):
    update(kind, content=dict(type='text', text=value))


def complete(value):
    global active
    reply(active, dict(stopReason=value))
    active = None


def session_info():
    return dict(
        models=dict(currentModelId='fake-model-a', availableModels=[
            dict(modelId='fake-model-a', name='Fake model A'),
            dict(modelId='fake-model-b', name='Fake model B')]),
        modes=dict(currentModeId='plan', availableModes=[
            dict(id='plan', name='Plan'), dict(id='build', name='Build')]))


def mcp_call(server, name, arguments):
    # OpenScience's remote form: {type: remote, url, headers}, reached from its
    # own (unsandboxed) process over loopback HTTP.
    import urllib.error, urllib.request
    def post(body, headers):
        request = urllib.request.Request(server['url'], data=json.dumps(body).encode(), headers=dict(
            headers, **{'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream'}))
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return response.status, response.read()
        except urllib.error.HTTPError as error:
            return error.code, b''
    def rpc(id, method, params):
        status, body = post(dict(jsonrpc='2.0', id=id, method=method, params=params), server['headers'])
        assert status == 200, status
        return json.loads(body)
    # Without the bearer token the tools are not reachable.
    status, _ = post(dict(jsonrpc='2.0', id=0, method='tools/list'), {})
    record('mcp_unauthorized', status=status)
    rpc(1, 'initialize', dict(protocolVersion='2025-06-18', capabilities={},
        clientInfo=dict(name='fake-openscience', version='1')))
    status, _ = post(dict(jsonrpc='2.0', method='notifications/initialized'), server['headers'])
    assert status == 202, status
    value = rpc(2, 'tools/call', dict(name=name, arguments=arguments))
    record('mcp', name=name, response=value)
    return value


assert sys.argv[1:] == ['acp'], sys.argv
config = json.loads(os.environ['OPENSCIENCE_CONFIG_CONTENT'])
instructions = [dict(path=path, text=pathlib.Path(path).read_text()) for path in config['instructions']]
record('boot', args=sys.argv[1:], cwd=os.getcwd(), config=config,
       instructions=instructions, terminal=os.environ['SANDHILL_TERMINAL'])
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    params = request.get('params', {})
    record('rpc', message=request)
    if method == 'initialize':
        assert params['protocolVersion'] == 1
        reply(request, dict(protocolVersion=1,
            agentCapabilities=dict(loadSession=True, promptCapabilities={},
                                   mcpCapabilities=dict(http=False, sse=False)),
            authMethods=[], agentInfo=dict(name='fake-openscience', version='1')))
    elif method in ('session/new', 'session/load'):
        assert active is None
        assert params['mcpServers'] == [], 'Sandhill passes its MCP server through the config'
        if method == 'session/new':
            reply(request, dict(sessionId=session, **session_info()))
        else:
            assert params['sessionId'] == session
            # A turn created outside this host must be imported, whereas the
            # already-journaled turns must not be duplicated on reconnect.
            remember(dict(sessionUpdate='user_message_chunk', content=dict(type='text', text='Offline question')))
            remember(dict(sessionUpdate='agent_message_chunk', content=dict(type='text', text='Offline answer')))
            for value in history:
                send(dict(method='session/update', params=dict(sessionId=session, update=value)))
            reply(request, session_info())
    elif method == 'session/set_model':
        assert params['sessionId'] == session
        assert params['modelId'] in ('fake-model-a', 'fake-model-b')
        reply(request, {})
    elif method == 'session/set_mode':
        assert params['sessionId'] == session
        assert params['modeId'] in ('plan', 'build')
        reply(request, {})
        update('current_mode_update', currentModeId=params['modeId'])
    elif method == 'session/prompt':
        assert active is None, 'queued prompt overlapped the active turn'
        assert params['sessionId'] == session
        active = request
        prompt = ''.join(part['text'] for part in params['prompt'] if part['type'] == 'text')
        record('prompt', text=prompt)
        remember(dict(sessionUpdate='user_message_chunk', content=dict(type='text', text=prompt)))
        if 'First turn' in prompt:
            text('agent_thought_chunk', 'Consider ')
            text('agent_thought_chunk', 'the evidence.')
            update('plan', entries=[dict(content='Inspect', priority='high', status='completed'),
                                    dict(content='Report', priority='medium', status='in_progress')])
            server = config['mcp']['sandhill']
            listed = mcp_call(server, 'list_sessions', {})
            added = mcp_call(server, 'add_cell', dict(file='evidence.py', code="print('added')", run=False))
            assert listed['result']['isError'] is False, listed
            assert added['result']['isError'] is False, added
            tool = dict(toolCallId='read-1', title='Inspect evidence', kind='read', status='pending',
                locations=[dict(path=str(root / 'evidence.txt'), line=2)],
                rawInput=dict(path='evidence.txt'),
                content=[dict(type='content', content=dict(type='text', text='Reading evidence'))])
            update('tool_call', **tool)
            update('tool_call_update', toolCallId='read-1', status='in_progress')
            send(dict(id='approval-1', method='session/request_permission', params=dict(
                sessionId=session, toolCall=tool, options=[
                    dict(optionId='once', name='Allow once', kind='allow_once'),
                    dict(optionId='chat', name='Allow for this chat', kind='allow_always'),
                    dict(optionId='project', name='Allow for this project', kind='allow_always'),
                    dict(optionId='no', name='Reject', kind='reject_once')])))
        elif 'Queued turn' in prompt:
            text('agent_message_chunk', 'Queued reply.')
            complete('end_turn')
        elif 'Wait for cancel' in prompt:
            text('agent_message_chunk', 'Waiting for cancellation.')
        elif 'After resume' in prompt:
            text('agent_message_chunk', 'Resumed reply.')
            complete('end_turn')
        else:
            raise AssertionError('unexpected prompt: ' + prompt)
    elif method == 'session/cancel':
        assert 'id' not in request, 'ACP cancel is a notification'
        assert params['sessionId'] == session and active is not None
        complete('cancelled')
    elif request.get('id') == 'approval-1':
        assert request['result'] == dict(outcome=dict(outcome='selected', optionId='project')), request
        record('permission', result=request['result'])
        update('tool_call_update', toolCallId='read-1', status='completed',
            rawOutput=dict(rows=2), content=[dict(type='content', content=dict(type='text', text='Two rows'))])
        update('tool_call', toolCallId='failed-1', title='Missing evidence', kind='read',
            status='in_progress', rawInput=dict(path='missing.txt'))
        update('tool_call_update', toolCallId='failed-1', status='failed',
            content=[dict(type='content', content=dict(type='text', text='File not found'))])
        text('agent_message_chunk', 'Hello ')
        text('agent_message_chunk', 'world.')
        complete('end_turn')
    else:
        raise AssertionError('unexpected ACP message: ' + json.dumps(request))
"#;

#[derive(Default)]
struct Transcript {
    entries: Vec<Value>,
    output: String,
}

impl Transcript {
    async fn until(
        &mut self,
        terminals: &Terminals,
        id: &str,
        events: &mut mpsc::UnboundedReceiver<ServerMessage>,
        ready: impl Fn(&[Value], &Value) -> bool,
    ) {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let info = terminals.info(id).unwrap();
                if ready(&self.entries, &info.agent.unwrap_or_default()) {
                    return;
                }
                assert!(
                    info.running,
                    "bridge exited: {:#?}\n{}",
                    self.entries, self.output
                );
                let message = events.recv().await.expect("terminal stream ended");
                if let ServerMessage::Terminal { terminal } = message {
                    if let Some(data) = terminal.data {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap();
                        self.output.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    if let Some(entry) = terminal.chat {
                        self.entries.push(entry.event);
                    }
                    assert!(
                        terminal.exit.is_none(),
                        "bridge exited: {:#?}\n{}",
                        self.entries,
                        self.output
                    );
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("ACP timed out: {:#?}\n{}", self.entries, self.output));
    }
}

fn records(root: &Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn reply(entries: &[Value]) -> String {
    entries
        .iter()
        .filter(|entry| entry["kind"] == "text")
        .filter_map(|entry| entry["text"].as_str())
        .collect()
}

fn launch(fake: &Path, socket: &Path, project: &Path, session: Option<&str>) -> Launch {
    let options = AgentOptions {
        permission: Some("supervised".into()),
        model: Some("fake-model-b".into()),
        refs: vec![
            json!({"kind":"file","name":"evidence.txt","path":project.join("evidence.txt")}),
        ],
        ..AgentOptions::default()
    };
    let mut args = vec![
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "acp-chat".into(),
        "--agent".into(),
        "openscience".into(),
        "--program".into(),
        fake.to_string_lossy().into_owned(),
        "--project".into(),
        project.to_string_lossy().into_owned(),
        "--options".into(),
        serde_json::to_string(&options).unwrap(),
        "--prompt".into(),
        if session.is_some() {
            "After resume"
        } else {
            "First turn"
        }
        .into(),
    ];
    if let Some(session) = session {
        args.extend(["--session".into(), session.into()]);
    }
    Launch {
        kind: "openscience".into(),
        title: "Fake OpenScience".into(),
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

async fn subscribe(
    terminals: &Terminals,
    id: &str,
    chat_from: u64,
) -> mpsc::UnboundedReceiver<ServerMessage> {
    let (out, events) = mpsc::unbounded_channel();
    tokio::spawn(
        terminals
            .open(id, Some(0), Some(chat_from))
            .await
            .unwrap()
            .forward(out),
    );
    events
}

async fn command(terminals: &Terminals, id: &str, command: Value) {
    terminals
        .send(id, ToHost::Command { command })
        .await
        .unwrap();
}

#[tokio::test]
async fn openscience_acp_chat_round_trip_and_resume() {
    let Some(python) = sandhill_daemon::agents::find("python3") else {
        eprintln!("skipping: python3 is needed for the fake ACP executable");
        return;
    };
    let dir = common::tempdir().unwrap();
    let fake = dir.path().join("openscience");
    std::fs::write(&fake, format!("#!{}\n{FAKE}", python.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("evidence.py"), "print('existing')\n").unwrap();
    let state = dir.path().join("state");
    let (_daemon, socket) = start_daemon(&state).await;
    let terminals = Terminals::new(
        state.join("terminals"),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    let info = terminals
        .start(launch(&fake, &socket, &project, None))
        .await
        .unwrap();
    let mut events = subscribe(&terminals, &info.id, 0).await;
    let mut transcript = Transcript::default();
    transcript
        .until(&terminals, &info.id, &mut events, |entries, _| {
            entries.iter().any(|entry| entry["kind"] == "ask")
        })
        .await;

    let ask = transcript
        .entries
        .iter()
        .find(|entry| entry["kind"] == "ask")
        .unwrap()
        .clone();
    assert_eq!(
        ask["options"],
        serde_json::from_str::<Value>(OPTIONS).unwrap()
    );
    assert_eq!(ask["input"]["rawInput"]["path"], "evidence.txt");
    assert_eq!(
        terminals.info(&info.id).unwrap().agent.unwrap()["permission"],
        "supervised"
    );
    let read = transcript
        .entries
        .iter()
        .find(|entry| entry["kind"] == "step" && entry["input"]["title"] == "Inspect evidence")
        .unwrap();
    let read_id = read["id"].clone();
    assert_eq!(read["input"]["title"], "Inspect evidence");
    assert_eq!(read["input"]["kind"], "read");
    assert_eq!(read["input"]["locations"][0]["line"], 2);
    assert_eq!(
        read["input"]["content"][0]["content"]["text"],
        "Reading evidence"
    );
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| entry["kind"] == "step"
                && entry["input"]["text"] == "Consider the evidence.")
    );
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| entry["kind"] == "step_done"
                && entry["output"]["text"] == "Consider the evidence.")
    );
    let plan = transcript
        .entries
        .iter()
        .find(|entry| entry["tool"] == "TodoWrite" && entry["kind"] == "step")
        .unwrap();
    assert_eq!(
        plan["input"]["todos"],
        json!([
            {"content":"Inspect","status":"completed","priority":"high"},
            {"content":"Report","status":"in_progress","priority":"medium"}
        ])
    );
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| { entry["kind"] == "step_done" && entry["id"] == plan["id"] })
    );

    // The bridge must hold this message until the permission-gated turn ends.
    command(
        &terminals,
        &info.id,
        json!({"kind":"set","permission":"plan"}),
    )
    .await;
    command(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"Queued turn"}),
    )
    .await;
    terminals
        .answer(
            &info.id,
            ask["id"].as_str().unwrap(),
            json!({"option_id":"project"}),
        )
        .await
        .unwrap();
    transcript
        .until(&terminals, &info.id, &mut events, |entries, agent| {
            entries
                .iter()
                .filter(|entry| entry["kind"] == "stop")
                .count()
                == 2
                && agent["state"] == "idle"
        })
        .await;
    assert_eq!(reply(&transcript.entries), "Hello world.Queued reply.");
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| entry["kind"] == "text" && entry["final"] == true)
    );
    let first_stop = transcript
        .entries
        .iter()
        .position(|entry| entry["kind"] == "stop")
        .unwrap();
    let queued_prompt = transcript
        .entries
        .iter()
        .position(|entry| entry["kind"] == "prompt" && entry["text"] == "Queued turn")
        .unwrap();
    assert!(queued_prompt > first_stop, "{:#?}", transcript.entries);
    let completed = transcript
        .entries
        .iter()
        .find(|entry| entry["kind"] == "step_done" && entry["id"] == read_id)
        .unwrap();
    assert_eq!(completed["output"]["rawOutput"], json!({"rows":2}));
    assert_eq!(completed["output"]["status"], "completed");
    let failed = transcript
        .entries
        .iter()
        .find(|entry| entry["kind"] == "step" && entry["input"]["title"] == "Missing evidence")
        .unwrap();
    assert!(
        transcript
            .entries
            .iter()
            .any(|entry| { entry["kind"] == "step_failed" && entry["id"] == failed["id"] })
    );
    let agent = terminals.info(&info.id).unwrap().agent.unwrap();
    assert_eq!(agent["thread"], SESSION);
    assert_eq!(agent["view_mode"], "chat");
    assert_eq!(agent["permission"], "plan");
    assert_eq!(agent["status"]["model_id"], "fake-model-b");
    assert_eq!(agent["status"]["model"], "Fake model B");
    assert_eq!(agent["models"][1]["id"], "fake-model-b");
    assert_eq!(agent["models"][1]["label"], "Fake model B");

    let calls = records(dir.path());
    let methods: Vec<_> = calls
        .iter()
        .filter_map(|call| call["message"]["method"].as_str())
        .collect();
    assert_eq!(methods.first(), Some(&"initialize"));
    assert!(
        methods
            .iter()
            .position(|method| *method == "session/new")
            .unwrap()
            < methods
                .iter()
                .position(|method| *method == "session/prompt")
                .unwrap()
    );
    let boot = calls.iter().find(|call| call["kind"] == "boot").unwrap();
    assert_eq!(boot["args"], json!(["acp"]));
    assert_eq!(
        boot["cwd"],
        project.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(boot["config"]["permission"]["python"], "deny");
    assert_eq!(boot["config"]["permission"]["r"], "deny");
    assert_eq!(boot["config"]["model"], "fake-model-b");
    assert!(
        calls
            .iter()
            .any(|call| call["message"]["method"] == "session/set_mode"
                && call["message"]["params"]["modeId"] == "build")
    );
    assert!(
        calls
            .iter()
            .any(|call| call["message"]["method"] == "session/set_mode"
                && call["message"]["params"]["modeId"] == "plan")
    );
    assert!(
        calls
            .iter()
            .any(|call| call["message"]["method"] == "session/set_model"
                && call["message"]["params"]["modelId"] == "fake-model-b")
    );
    let first_prompt = calls.iter().find(|call| call["kind"] == "prompt").unwrap();
    assert!(
        first_prompt["text"]
            .as_str()
            .unwrap()
            .contains("evidence.txt")
    );
    let terminal_dir = terminals.dir(&info.id).unwrap();
    assert_eq!(boot["terminal"], terminal_dir.to_string_lossy().as_ref());
    let instructions = boot["instructions"].as_array().unwrap();
    assert!(!instructions.is_empty());
    for instruction in instructions {
        let path = Path::new(instruction["path"].as_str().unwrap());
        assert!(
            path.is_absolute() && path.starts_with(&terminal_dir),
            "{instruction}"
        );
        let text = instruction["text"].as_str().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        assert!(text.contains("Sandhill"), "{text}");
        assert!(
            text.contains("run_code") || text.contains("add_cell"),
            "{text}"
        );
    }
    let new = calls
        .iter()
        .find(|call| call["message"]["method"] == "session/new")
        .unwrap()
        .clone();
    assert_eq!(
        new["message"]["params"]["cwd"],
        project.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(new["message"]["params"]["mcpServers"], json!([]));
    let server = &boot["config"]["mcp"]["sandhill"];
    assert_eq!(server["type"], "remote");
    assert_eq!(server["enabled"], true);
    assert_eq!(server["oauth"], false);
    assert!(
        server["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"),
        "{server}"
    );
    assert!(
        server["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .starts_with("Bearer "),
        "{server}"
    );
    // The token is required: without it the tools answer 401.
    let unauthorized: Vec<_> = calls
        .iter()
        .filter(|call| call["kind"] == "mcp_unauthorized")
        .map(|call| call["status"].clone())
        .collect();
    assert_eq!(unauthorized, vec![json!(401); 2]);
    let rules = boot["config"]["permission"]["mcp"].as_object().unwrap();
    for tool in sandhill_daemon::mcp::tool_names() {
        assert_eq!(rules[&format!("sandhill_{tool}")], "allow");
    }
    let mcp_calls: Vec<_> = calls.iter().filter(|call| call["kind"] == "mcp").collect();
    assert_eq!(mcp_calls.len(), 2);
    for call in mcp_calls {
        assert_eq!(call["response"]["result"]["isError"], false, "{call}");
    }
    let written = std::fs::read_to_string(project.join("evidence.py")).unwrap();
    assert!(written.contains("print('existing')"), "{written}");
    assert!(written.contains("print('added')"), "{written}");
    assert!(
        calls.iter().any(|call| call["kind"] == "permission"
            && call["result"]["outcome"]["optionId"] == "project")
    );

    command(
        &terminals,
        &info.id,
        json!({"kind":"set","permission":"ask"}),
    )
    .await;
    let after = transcript.entries.len();
    command(
        &terminals,
        &info.id,
        json!({"kind":"say","text":"Wait for cancel"}),
    )
    .await;
    transcript
        .until(&terminals, &info.id, &mut events, |entries, _| {
            entries[after..].iter().any(|entry| entry["kind"] == "text")
        })
        .await;
    command(&terminals, &info.id, json!({"kind":"interrupt"})).await;
    transcript
        .until(&terminals, &info.id, &mut events, |entries, agent| {
            entries[after..].iter().any(|entry| entry["kind"] == "stop") && agent["state"] == "idle"
        })
        .await;
    assert!(
        !transcript.entries[after..]
            .iter()
            .any(|entry| entry["kind"] == "error")
    );
    let calls = records(dir.path());
    assert!(
        calls
            .iter()
            .any(|call| call["message"]["method"] == "session/cancel")
    );
    let modes: Vec<_> = calls
        .iter()
        .filter(|call| call["message"]["method"] == "session/set_mode")
        .map(|call| call["message"]["params"]["modeId"].clone())
        .collect();
    assert_eq!(modes, vec![json!("build"), json!("plan"), json!("build")]);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["message"]["method"] == "initialize")
            .count(),
        1
    );

    // Restart the same journal: import an unseen turn from ACP's history,
    // without duplicating any of the three turns already recorded here.
    let before = transcript.entries.len();
    terminals
        .restart(
            &info.id,
            launch(&fake, &socket, &project, Some(SESSION)),
            &sandhill_daemon::agents::RESUME_KEYS,
        )
        .await
        .unwrap();
    let mut events = subscribe(&terminals, &info.id, before as u64).await;
    let mut resumed = Transcript::default();
    resumed
        .until(&terminals, &info.id, &mut events, |entries, agent| {
            reply(entries).contains("Resumed reply.")
                && entries.last().is_some_and(|entry| entry["kind"] == "stop")
                && agent["state"] == "idle"
        })
        .await;
    assert!(
        resumed
            .entries
            .iter()
            .any(|entry| entry["kind"] == "session" && entry["source"] == "resume")
    );
    assert_eq!(reply(&resumed.entries), "Offline answerResumed reply.");
    let prompts: Vec<_> = resumed
        .entries
        .iter()
        .filter(|entry| entry["kind"] == "prompt")
        .map(|entry| entry["text"].clone())
        .collect();
    assert_eq!(
        prompts,
        vec![json!("Offline question"), json!("After resume")]
    );
    let calls = records(dir.path());
    let load = calls
        .iter()
        .find(|call| call["message"]["method"] == "session/load")
        .unwrap();
    assert_eq!(load["message"]["params"]["sessionId"], SESSION);
    assert_eq!(
        load["message"]["params"]["mcpServers"],
        new["message"]["params"]["mcpServers"]
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["message"]["method"] == "session/new")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["message"]["method"] == "initialize")
            .count(),
        2
    );
    terminals.close(&info.id).await.unwrap();
}
