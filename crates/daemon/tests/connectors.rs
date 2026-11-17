//! Real daemon/socket and MCP integration, with local stand-in servers and no kernels.
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sandhill_daemon::mcp::Tools;
use sandhill_proto::client::Client;
use sandhill_proto::{Call, ConnectorAdd, ConnectorInfo, ConnectorList, ConnectorSpec};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const SECRET: &str = "sandhill-test-secret-should-never-leak";
const SCRIPT: &str = r#"import json, os, sys, time
with open(sys.argv[1], 'a') as f:
    f.write(str(os.getpid()) + '\n')
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if method == 'notifications/initialized':
        continue
    if len(sys.argv) > 2 and sys.argv[2] == 'hang-init':
        time.sleep(60)
    if method == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {}, 'serverInfo': {'name': 'fixture', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'description': 'First line\nFull description', 'inputSchema': {'type': 'object'}, 'annotations': {'readOnlyHint': True}}]}
    elif method == 'tools/call':
        params = request['params']
        if params['name'] == 'hang':
            time.sleep(60)
        if params['name'] == 'huge':
            result = {'content': [{'type': 'text', 'text': 'x' * (2 ** 20 + 100)}]}
        elif params['name'] == 'image':
            result = {'content': [{'type': 'image', 'data': 'aGVsbG8=', 'mimeType': 'image/png'}]}
        elif params['name'] == 'error':
            result = {'content': [{'type': 'text', 'text': 'tool failed'}, {'type': 'image', 'data': 'aGVsbG8=', 'mimeType': 'image/png'}], 'isError': True}
        else:
            result = {'content': [{'type': 'text', 'text': json.dumps(params['arguments'])}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

struct Machine {
    dir: common::TempDir,
    state: PathBuf,
    child: tokio::process::Child,
    client: Client,
}

impl Machine {
    async fn start() -> Self {
        let dir = common::tempdir().unwrap();
        let state = dir.path().join("state");
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let bad_download = dir.path().join("bad-download.tar.gz");
        fs::write(&bad_download, b"not the pinned BioMCP archive").unwrap();
        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
            .args(["--state-dir", state.to_str().unwrap(), "serve"])
            .env_clear()
            .env("HOME", home)
            .env("SHELL", "/bin/sh")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("SANDHILL_CONNECTOR_TIMEOUT_MS", "1500")
            .env(
                "SANDHILL_BIOMCP_URL",
                format!("file://{}", bad_download.display()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(client) = Client::connect_unix(state.join("sandhilld.sock")).await {
                    break client;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        Self {
            dir,
            state,
            child,
            client,
        }
    }

    fn stdio(&self, title: &str, hang: bool) -> ConnectorAdd {
        let script = self.dir.path().join("server.py");
        fs::write(&script, SCRIPT).unwrap();
        let mut args = vec![
            script.to_string_lossy().into_owned(),
            self.dir.path().join("pids").to_string_lossy().into_owned(),
        ];
        if hang {
            args.push("hang-init".into());
        }
        ConnectorAdd::Custom {
            title: title.into(),
            spec: ConnectorSpec::Command {
                command: "python3".into(),
                args,
                env: BTreeMap::new(),
            },
            secret: None,
        }
    }

    async fn add(&self, add: ConnectorAdd) -> ConnectorInfo {
        self.client.call(Call::AddConnector { add }).await.unwrap()
    }
    async fn list(&self) -> ConnectorList {
        self.client.call(Call::ListConnectors).await.unwrap()
    }
    async fn call(&self, name: &str, tool: &str) -> Value {
        self.client
            .call(Call::CallConnector {
                name: name.into(),
                tool: tool.into(),
                arguments: json!({"gene": "TP53"}),
                read_only: false,
            })
            .await
            .unwrap()
    }
    async fn tools(&self, name: Option<&str>) -> Value {
        self.client
            .call(Call::ConnectorTools {
                name: name.map(str::to_owned),
            })
            .await
            .unwrap()
    }
    async fn enabled(&self, name: &str, enabled: bool) {
        let _: Value = self
            .client
            .call(Call::SetConnectorEnabled {
                name: name.into(),
                enabled,
            })
            .await
            .unwrap();
    }
    async fn remove(&self, name: &str) {
        let _: Value = self
            .client
            .call(Call::RemoveConnector { name: name.into() })
            .await
            .unwrap();
    }
    async fn mcp(&self, method: &str, params: Value, plan: Option<&str>) -> Value {
        let terminal = self.dir.path().join("terminal");
        fs::create_dir_all(&terminal).unwrap();
        let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"));
        process.args([
            "--socket",
            self.state.join("sandhilld.sock").to_str().unwrap(),
            "mcp",
        ]);
        if let Some(key) = plan {
            let mut report = json!({});
            report[key] = json!("plan");
            fs::write(terminal.join("agent.json"), report.to_string()).unwrap();
            process.env("SANDHILL_TERMINAL", &terminal);
        } else {
            process.env_remove("SANDHILL_TERMINAL");
        }
        let mut child = process
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let line = tokio::time::timeout(Duration::from_secs(15), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(stdin);
        child.wait().await.unwrap();
        serde_json::from_str::<Value>(&line).unwrap()["result"].clone()
    }
    async fn finish(mut self) {
        let _: Value = self.client.call(Call::StopDaemon).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(30), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
async fn stopped(pid: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("connector child was not stopped");
}

#[tokio::test]
async fn stdio_lifecycle_copy_limits_and_agent_rules() {
    let machine = Machine::start().await;
    let info = machine.add(machine.stdio("My Database", false)).await;
    assert_eq!(info.name, "my-database");
    assert!(!info.read_only);
    assert_eq!(info.tools[0].description, "First line");
    assert!(info.tools[0].read_only);
    let duplicate = machine.add(machine.stdio("My Database", false)).await;
    assert_eq!(duplicate.name, "my-database-2");
    assert_eq!(machine.list().await.added.len(), 2);
    assert_eq!(
        machine.tools(None).await["connectors"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let full = machine.tools(Some(&info.name)).await;
    assert_eq!(
        full["tools"][0]["description"],
        "First line\nFull description"
    );
    assert_eq!(full["tools"][0]["inputSchema"]["type"], "object");
    assert!(
        machine.call(&info.name, "echo").await["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TP53")
    );
    let huge = machine.call(&info.name, "huge").await;
    assert!(
        huge["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("truncated")
    );
    let tools = Tools::new(machine.state.join("sandhilld.sock"), None);
    let refusal = tools
        .call(
            "query_connector",
            &json!({"connector": info.name, "tool": "echo", "arguments": {}}),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("use call_connector"), "{refusal}");
    let image = tools
        .call(
            "call_connector",
            &json!({"connector": info.name, "tool": "image", "arguments": {}}),
        )
        .await
        .unwrap();
    assert_eq!(image[0]["type"], "image");
    let error = machine.mcp("tools/call", json!({"name": "call_connector", "arguments": {"connector": info.name, "tool": "error", "arguments": {}}}), None).await;
    assert_eq!(error["isError"], true);
    assert_eq!(error["content"][1]["type"], "image");
    for key in ["mode", "permission"] {
        let refused = machine.mcp("tools/call", json!({"name": "call_connector", "arguments": {"connector": info.name, "tool": "echo", "arguments": {}}}), Some(key)).await;
        assert_eq!(refused["isError"], true);
        assert!(
            refused["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Plan mode")
        );
    }
    let listed = machine.mcp("tools/list", json!({}), None).await;
    for (name, read_only) in [
        ("list_connectors", true),
        ("query_connector", true),
        ("call_connector", false),
    ] {
        let tool = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], read_only);
    }
    let pids = fs::read_to_string(machine.dir.path().join("pids")).unwrap();
    assert_eq!(
        pids.lines().count(),
        2,
        "calls share the program started during Add"
    );
    machine.enabled(&info.name, false).await;
    stopped(pids.lines().next().unwrap().parse().unwrap()).await;
    assert_eq!(
        machine.tools(None).await["connectors"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let error = machine
        .client
        .call::<Value>(Call::CallConnector {
            name: info.name.clone(),
            tool: "echo".into(),
            arguments: json!({}),
            read_only: false,
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("that is on (list_connectors lists them)")
    );
    machine.enabled(&info.name, true).await;
    machine.call(&info.name, "echo").await;
    assert_eq!(
        fs::read_to_string(machine.dir.path().join("pids"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    let exported: ConnectorAdd = machine
        .client
        .call(Call::ExportConnector {
            name: info.name.clone(),
        })
        .await
        .unwrap();
    let second = Machine::start().await;
    let copy = second.add(exported).await;
    assert_eq!(copy.spec, info.spec);
    second.call(&copy.name, "echo").await;
    let copy_pid = fs::read_to_string(machine.dir.path().join("pids"))
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .parse()
        .unwrap();
    second.finish().await;
    stopped(copy_pid).await;
    machine.remove(&info.name).await;
    machine.remove(&duplicate.name).await;
    assert!(machine.list().await.added.is_empty());
    machine.finish().await;
}

struct Http {
    address: String,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    initialized: Arc<AtomicUsize>,
    headers: Arc<Mutex<Vec<String>>>,
}

impl Http {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let initialized = Arc::new(AtomicUsize::new(0));
        let expired = Arc::new(AtomicBool::new(false));
        let headers = Arc::new(Mutex::new(Vec::new()));
        let (stop, init, received) = (stopped.clone(), initialized.clone(), headers.clone());
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => Self::serve(socket, &init, &expired, &received),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        });
        Self {
            address,
            stopped,
            thread: Some(thread),
            initialized,
            headers,
        }
    }

    fn serve(
        mut socket: TcpStream,
        initialized: &AtomicUsize,
        expired: &AtomicBool,
        received: &Mutex<Vec<String>>,
    ) {
        // On macOS an accepted socket inherits the listener's non-blocking mode.
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(socket.try_clone().unwrap());
        let mut first = String::new();
        if reader.read_line(&mut first).unwrap() == 0 {
            return;
        }
        let sse = first.contains("/sse");
        let mut length = 0;
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
            }
            headers.push_str(&line);
        }
        received.lock().unwrap().push(headers.clone());
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let request: Value = serde_json::from_slice(&body).unwrap();
        let method = request["method"].as_str().unwrap();
        if method == "notifications/initialized" {
            write!(
                socket,
                "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            return;
        }
        if method == "tools/call" && !expired.swap(true, Ordering::AcqRel) {
            write!(
                socket,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            return;
        }
        let result = match method {
            "initialize" => {
                initialized.fetch_add(1, Ordering::AcqRel);
                json!({"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "fixture", "version": "1"}})
            }
            "tools/list" => {
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("mcp-session-id: fixture-session")
                );
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("mcp-protocol-version: 2025-03-26")
                );
                if request["params"]["cursor"] == "next" {
                    json!({"tools": [{"name": "second", "description": "another tool", "inputSchema": {"type": "object"}}]})
                } else {
                    json!({"tools": [{"name": "echo", "description": format!("First line\nFull description {SECRET}"), "inputSchema": {"type": "object"}}], "nextCursor": "next"})
                }
            }
            _ => json!({"content": [{"type": "text", "text": format!("TP53 {SECRET}")}]}),
        };
        let message = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        let body = if sse {
            format!(
                "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}}\n\ndata: {message}\n\n"
            )
        } else {
            message.to_string()
        };
        write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nMcp-Session-Id: fixture-session\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", if sse { "text/event-stream" } else { "application/json" }, body.len(), body).unwrap();
    }

    fn add(&self, title: &str, sse: bool, secret: bool) -> ConnectorAdd {
        ConnectorAdd::Custom {
            title: title.into(),
            spec: ConnectorSpec::Http {
                url: format!("{}/{}", self.address, if sse { "sse" } else { "json" }),
                header: secret.then(|| "X-Test-Key".into()),
            },
            secret: secret.then(|| SECRET.into()),
        }
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

#[tokio::test]
async fn http_json_sse_sessions_secrets_and_copy() {
    let server = Http::start();
    let machine = Machine::start().await;
    for sse in [false, true] {
        let info = machine
            .add(server.add(
                if sse { "SSE database" } else { "JSON database" },
                sse,
                true,
            ))
            .await;
        assert!(info.has_secret);
        assert_eq!(info.tools.len(), 2, "follow tools/list pagination");
        let result = machine.call(&info.name, "echo").await;
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("TP53")
        );
        assert!(!result.to_string().contains(SECRET));
        let tested: ConnectorInfo = machine
            .client
            .call(Call::TestConnector {
                name: info.name.clone(),
            })
            .await
            .unwrap();
        assert!(tested.error.is_none());
        let listed = serde_json::to_string(&machine.list().await).unwrap();
        let exported: Value = machine
            .client
            .call(Call::ExportConnector {
                name: info.name.clone(),
            })
            .await
            .unwrap();
        for output in [
            listed,
            exported.to_string(),
            machine.tools(None).await.to_string(),
            machine.tools(Some(&info.name)).await.to_string(),
        ] {
            assert!(!output.contains(SECRET), "{output}");
        }
        assert!(exported.get("secret").is_none());
        assert_eq!(
            fs::metadata(machine.state.join("connectors/secrets.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            fs::read_to_string(machine.state.join("connectors/secrets.json"))
                .unwrap()
                .contains(SECRET)
        );
        if sse {
            let second = Machine::start().await;
            let copy = second.add(serde_json::from_value(exported).unwrap()).await;
            assert!(!copy.has_secret);
            second.call(&copy.name, "echo").await;
            second.finish().await;
        }
        machine.enabled(&info.name, false).await;
        assert!(
            machine
                .client
                .call::<Value>(Call::ConnectorTools {
                    name: Some(info.name.clone())
                })
                .await
                .is_err()
        );
        machine.enabled(&info.name, true).await;
        machine.call(&info.name, "echo").await;
        machine.remove(&info.name).await;
    }
    assert!(
        server.initialized.load(Ordering::Acquire) >= 6,
        "expired sessions reinitialize once"
    );
    assert!(
        server
            .headers
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.contains(SECRET)),
        "the secret is sent only to its server"
    );
    assert_eq!(
        fs::read_to_string(machine.state.join("connectors/secrets.json"))
            .unwrap()
            .trim(),
        "{}"
    );
    let log = fs::read_to_string(machine.state.join("sandhilld.log")).unwrap_or_default();
    assert!(!log.contains(SECRET));
    assert!(machine.list().await.added.is_empty());
    machine.finish().await;
}

#[tokio::test]
async fn failed_add_timeout_error_refresh_and_bad_download() {
    let machine = Machine::start().await;
    let before = Instant::now();
    let error = machine
        .client
        .call::<Value>(Call::AddConnector {
            add: machine.stdio("Hanging", true),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(before.elapsed() < Duration::from_secs(10));
    assert!(machine.list().await.added.is_empty());
    let error = machine
        .client
        .call::<Value>(Call::AddConnector {
            add: ConnectorAdd::Suggested {
                id: "biomcp".into(),
            },
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("the download did not match its checksum"),
        "{error}"
    );
    assert!(machine.list().await.added.is_empty());
    assert!(
        fs::read_dir(machine.state.join("connectors/bin"))
            .unwrap()
            .next()
            .is_none()
    );
    let info = machine.add(machine.stdio("Calls", false)).await;
    let error = machine
        .client
        .call::<Value>(Call::CallConnector {
            name: info.name.clone(),
            tool: "hang".into(),
            arguments: json!({}),
            read_only: false,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(
        machine.list().await.added[0]
            .error
            .as_ref()
            .unwrap()
            .contains("timed out")
    );
    let tested: ConnectorInfo = machine
        .client
        .call(Call::TestConnector {
            name: info.name.clone(),
        })
        .await
        .unwrap();
    assert!(tested.error.is_none());
    machine.remove(&info.name).await;
    machine.finish().await;
}
