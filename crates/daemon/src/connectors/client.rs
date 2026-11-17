use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sandhill_proto::ConnectorSpec;
use serde_json::{Value, json};

use super::programs::Child;

const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_RESULT: usize = 1 << 20;
const IDLE: Duration = Duration::from_secs(600);

static TIMEOUT: LazyLock<Duration> = LazyLock::new(|| {
    // Only isolated integration-test daemons set this; read once, not per call.
    std::env::var("SANDHILL_CONNECTOR_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(60))
});

pub(super) fn timeout() -> Duration {
    *TIMEOUT
}

#[derive(Default)]
struct HttpState {
    session: Option<String>,
    version: Option<String>,
}

pub(super) struct Client {
    spec: ConnectorSpec,
    secret: Option<String>,
    next_id: AtomicU64,
    initialized: tokio::sync::Mutex<bool>,
    http: Arc<Mutex<HttpState>>,
    child: tokio::sync::Mutex<Option<Arc<Child>>>,
    used: Mutex<Instant>,
    active: AtomicUsize,
    stopped: AtomicBool,
}

impl Client {
    pub fn new(spec: ConnectorSpec, secret: Option<String>) -> Arc<Self> {
        let client = Arc::new(Self {
            spec,
            secret,
            next_id: AtomicU64::new(1),
            initialized: tokio::sync::Mutex::new(false),
            http: Arc::default(),
            child: tokio::sync::Mutex::new(None),
            used: Mutex::new(Instant::now()),
            active: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(client) = weak.upgrade() else {
                    break;
                };
                let mut initialized = client.initialized.lock().await;
                if matches!(client.spec, ConnectorSpec::Command { .. })
                    && client.active.load(Ordering::Acquire) == 0
                    && client.used.lock().unwrap().elapsed() >= IDLE
                {
                    client.child.lock().await.take();
                    *initialized = false;
                }
            }
        });
        client
    }

    pub async fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(child) = self.child.lock().await.take() {
            child.stop();
        }
        let mut initialized = self.initialized.lock().await;
        if let Some(child) = self.child.lock().await.take() {
            child.stop();
        }
        *initialized = false;
        *self.http.lock().unwrap() = HttpState::default();
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        if self.stopped.load(Ordering::Acquire) {
            bail!("connector is stopped");
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        struct Active<'a>(&'a Client);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                *self.0.used.lock().unwrap() = Instant::now();
                self.0.active.fetch_sub(1, Ordering::AcqRel);
            }
        }
        let _active = Active(self);
        tokio::time::timeout(timeout(), async {
            for attempt in 0..2 {
                self.initialize().await?;
                match self.rpc(method, params.clone()).await {
                    Err(error) if error.downcast_ref::<Expired>().is_some() && attempt == 0 => {
                        let mut initialized = self.initialized.lock().await;
                        *initialized = false;
                        *self.http.lock().unwrap() = HttpState::default();
                    }
                    result => return result,
                }
            }
            unreachable!()
        })
        .await
        .context("connector call timed out")?
    }

    async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        if self.stopped.load(Ordering::Acquire) {
            bail!("connector is stopped");
        }
        if let ConnectorSpec::Command { command, args, env } = &self.spec {
            *self.child.lock().await = Some(Arc::new(Child::start(command, args, env).await?));
        }
        let mut last_error = None;
        for version in VERSIONS {
            let response = self
                .rpc(
                    "initialize",
                    json!({"protocolVersion": version, "capabilities": {},
                "clientInfo": {"name": "sandhill", "version": env!("CARGO_PKG_VERSION")}}),
                )
                .await;
            match response {
                Ok(response) => {
                    let negotiated = response["protocolVersion"]
                        .as_str()
                        .context("connector did not negotiate an MCP version")?;
                    if !VERSIONS.contains(&negotiated) {
                        bail!("connector uses an unsupported MCP version");
                    }
                    self.http.lock().unwrap().version = Some(negotiated.into());
                    self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                        .await?;
                    *initialized = true;
                    return Ok(());
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap())
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = self
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        if let Some(error) = message.get("error") {
            bail!(
                "connector: {}",
                error["message"].as_str().unwrap_or("MCP request failed")
            );
        }
        message
            .get("result")
            .cloned()
            .context("connector returned no MCP result")
    }

    async fn send(&self, message: Value) -> Result<Value> {
        match &self.spec {
            ConnectorSpec::Command { .. } => {
                let child = self
                    .child
                    .lock()
                    .await
                    .clone()
                    .context("connector program is stopped")?;
                child.send(&message).await
            }
            ConnectorSpec::Http { url, header } => {
                let (url, header, secret, state) = (
                    url.clone(),
                    header.clone(),
                    self.secret.clone(),
                    self.http.clone(),
                );
                tokio::task::spawn_blocking(move || {
                    http(&url, header.as_deref(), secret.as_deref(), &state, &message)
                })
                .await?
            }
        }
    }

    pub async fn tools(&self) -> Result<Vec<Value>> {
        // One deadline covers initialize and every tools/list page together.
        tokio::time::timeout(timeout(), async {
            let mut tools = Vec::new();
            let mut cursor = None;
            let mut seen = std::collections::HashSet::new();
            loop {
                let params = cursor
                    .as_ref()
                    .map(|c| json!({"cursor": c}))
                    .unwrap_or_else(|| json!({}));
                let page = self.request("tools/list", params).await?;
                tools.extend(
                    page["tools"]
                        .as_array()
                        .context("connector returned no tool list")?
                        .iter()
                        .cloned(),
                );
                cursor = page["nextCursor"].as_str().map(str::to_owned);
                let Some(next) = &cursor else {
                    break;
                };
                if !seen.insert(next.clone()) {
                    bail!("connector repeated a tool-list cursor");
                }
            }
            Ok(tools)
        })
        .await
        .context("connector call timed out")?
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> Result<Value> {
        let result = self
            .request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await?;
        struct Size(usize);
        impl std::io::Write for Size {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 += bytes.len();
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut size = Size(0);
        serde_json::to_writer(&mut size, &result)?;
        if size.0 > MAX_RESULT {
            Ok(
                json!({"content": [{"type": "text", "text": "Connector reply truncated: the result exceeded 1 MiB of JSON."}], "isError": result["isError"].as_bool().unwrap_or(false)}),
            )
        } else {
            Ok(result)
        }
    }
}

#[derive(Debug)]
struct Expired;
impl std::fmt::Display for Expired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("connector session expired")
    }
}
impl std::error::Error for Expired {}

fn http(
    url: &str,
    header: Option<&str>,
    secret: Option<&str>,
    state: &Mutex<HttpState>,
    message: &Value,
) -> Result<Value> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout()))
        .http_status_as_error(false)
        .build();
    let agent = config.new_agent();
    let mut request = agent
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json");
    {
        let state = state.lock().unwrap();
        if let Some(session) = &state.session {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = &state.version {
            request = request.header("MCP-Protocol-Version", version);
        }
    }
    if let (Some(header), Some(secret)) = (header, secret) {
        request = request.header(header, secret);
    }
    // Reasons only: ureq's own messages can carry the address, whose query
    // string may hold a key.
    let mut response = request
        .send(serde_json::to_vec(message)?)
        .map_err(|error| match error {
            ureq::Error::Timeout(_) => anyhow::anyhow!("the address did not answer in time"),
            ureq::Error::HostNotFound => anyhow::anyhow!("no server has that address"),
            ureq::Error::ConnectionFailed => anyhow::anyhow!("could not connect to the address"),
            _ => anyhow::anyhow!("the request to the address failed"),
        })?;
    let status = response.status().as_u16();
    if status == 404 && state.lock().unwrap().session.is_some() {
        return Err(Expired.into());
    }
    match status {
        200..=299 => {}
        401 | 403 => bail!("the service needs a sign-in or a key (HTTP {status})"),
        404 | 405 => bail!("no MCP server answers at this address (HTTP {status})"),
        _ => bail!("the service answered with an error (HTTP {status})"),
    }
    if let Some(session) = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
    {
        state.lock().unwrap().session = Some(session.into());
    }
    if message.get("id").is_none() {
        return Ok(Value::Null);
    }
    let sse = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let reader = response.body_mut().as_reader();
    if !sse {
        let value: Value =
            serde_json::from_reader(reader).context("connector returned invalid JSON")?;
        if value["id"] != message["id"] {
            bail!("connector returned a different request id");
        }
        return Ok(value);
    }
    let mut data = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line.context("reading connector event stream")?;
        if line.is_empty() {
            if let Ok(value) = serde_json::from_str::<Value>(&data)
                && value.get("id") == message.get("id")
            {
                return Ok(value);
            }
            data.clear();
        } else if let Some(part) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(part.strip_prefix(' ').unwrap_or(part));
        }
    }
    bail!("connector event stream ended without a reply")
}
