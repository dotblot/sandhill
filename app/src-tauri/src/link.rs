//! The app's connections to sandhilld, one per host, shared by every window
//! that works on that host.
//!
//! The UI reaches a daemon through three doors, which `app/src/backend/tauri.ts`
//! wraps: the `daemon_call` command for requests, `daemon://event`,
//! `daemon://terminal`, `daemon://request` and `daemon://link` events, and the `sandhill-blob:`
//! protocol for plots and other large outputs. A window first calls `use_host`; events for a host
//! go only to the windows using it, as `{ host, body }` since a window can use several.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use sandhill_proto::client::{Client, ClientError};
use sandhill_proto::{Blob, Call};
use serde::Serialize;
use serde_json::Value;
use tauri::async_runtime::JoinHandle;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{
    AppHandle, Emitter, EventTarget, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder,
    WebviewWindow,
};

use crate::memory;
use sandhill_link::{Connection, Options, Target};

pub const EVENT: &str = "daemon://event";
pub const TERMINAL: &str = "daemon://terminal";
/// Requests for windows (an agent's edit to an open document). Each window
/// takes the ones for its project and answers with `daemon_call`.
pub const REQUEST: &str = "daemon://request";
pub const LINK: &str = "daemon://link";
pub const BLOB_SCHEME: &str = "sandhill-blob";

const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(5);
/// Tries that may fail while a link still shows as reconnecting rather
/// than disconnected.
const RECONNECTING_TRIES: u32 = 3;
/// Decoded blobs kept in memory per host, so reopening a document doesn't
/// fetch its plots again (that matters over ssh). The web view keeps its own
/// copy of every image it shows, so this only needs to cover recent ones.
const BLOB_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkState {
    pub host: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What a slow connect is doing, such as installing sandhilld on a server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Disconnected on request: it waits to be connected again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
}

/// Every host's connection, and which windows use which host.
#[derive(Default)]
pub struct Links {
    /// How to reach servers: the builds of sandhilld to install on them.
    options: Options,
    hosts: Mutex<HashMap<String, Arc<Link>>>,
    /// Window label -> hosts it uses.
    windows: Mutex<HashMap<String, HashSet<String>>>,
}

impl Links {
    pub fn new(options: Options) -> Self {
        Links {
            options,
            ..Links::default()
        }
    }

    /// Connects to `host` for `window`, if nothing is connected yet.
    fn use_host<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        window: &str,
        host: &str,
    ) -> Result<LinkState, String> {
        let target = Target::parse(host).map_err(|err| err.to_string())?;
        self.windows
            .lock()
            .unwrap()
            .entry(window.to_string())
            .or_default()
            .insert(host.to_string());
        let link = self
            .hosts
            .lock()
            .unwrap()
            .entry(host.to_string())
            .or_insert_with(|| {
                Link::start(app.clone(), host.to_string(), target, self.options.clone())
            })
            .clone();
        Ok(link.state())
    }

    /// Drops `host`'s connection, if any, and connects again at once.
    fn reconnect<R: Runtime>(&self, app: &AppHandle<R>, host: &str) -> Result<(), String> {
        let target = Target::parse(host).map_err(|err| err.to_string())?;
        let mut hosts = self.hosts.lock().unwrap();
        if let Some(old) = hosts.remove(host) {
            old.stop();
        }
        let link = Link::start(app.clone(), host.to_string(), target, self.options.clone());
        hosts.insert(host.to_string(), link);
        Ok(())
    }

    /// Drops `host`'s connection until `reconnect`.
    fn disconnect<R: Runtime>(&self, app: &AppHandle<R>, host: &str) {
        if let Some(link) = self.link(host) {
            link.stop();
            link.set_state(app, Status::Disconnected, None, None);
        }
    }

    fn link(&self, host: &str) -> Option<Arc<Link>> {
        self.hosts.lock().unwrap().get(host).cloned()
    }

    fn windows_using(&self, host: &str) -> Vec<String> {
        self.windows
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, hosts)| hosts.contains(host))
            .map(|(label, _)| label.clone())
            .collect()
    }

    /// Forgets a closed window, and disconnects from hosts no window uses.
    pub fn release_window(&self, window: &str) {
        let Some(hosts) = self.windows.lock().unwrap().remove(window) else {
            return;
        };
        for host in hosts {
            if self.windows_using(&host).is_empty()
                && let Some(link) = self.hosts.lock().unwrap().remove(&host)
            {
                link.stop();
            }
        }
    }

    fn emit<R: Runtime, S: Serialize + Clone>(
        &self,
        app: &AppHandle<R>,
        host: &str,
        event: &str,
        payload: S,
    ) {
        for label in self.windows_using(host) {
            let _ = app.emit_to(EventTarget::webview_window(label), event, payload.clone());
        }
    }

    /// Sends a host's message to its windows, with the host's name: a window
    /// with a tab per machine uses several.
    fn emit_hosted<R: Runtime, S: Serialize + Clone>(
        &self,
        app: &AppHandle<R>,
        host: &str,
        event: &str,
        body: S,
    ) {
        self.emit(app, host, event, Hosted { host, body });
    }
}

/// A message from a host, as its windows get it: `{ "host", "body" }`.
#[derive(Clone, Serialize)]
struct Hosted<'a, S> {
    host: &'a str,
    body: S,
}

pub struct Link {
    host: String,
    client: Mutex<Option<Arc<Client>>>,
    state: Mutex<LinkState>,
    blobs: Mutex<BlobCache>,
    task: Mutex<Option<JoinHandle<()>>>,
    stopped: AtomicBool,
}

impl Link {
    /// Connects in the background and reconnects whenever the connection
    /// drops. The UI resubscribes from the last event it saw, so nothing is
    /// missed.
    fn start<R: Runtime>(
        app: AppHandle<R>,
        host: String,
        target: Target,
        options: Options,
    ) -> Arc<Link> {
        let link = Arc::new(Link {
            host: host.clone(),
            client: Mutex::new(None),
            state: Mutex::new(LinkState {
                host,
                status: Status::Connecting,
                error: None,
                detail: None,
                stopped: false,
            }),
            blobs: Mutex::default(),
            task: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        let task = tauri::async_runtime::spawn({
            let link = link.clone();
            async move {
                let mut delay = RETRY_MIN;
                let mut failures = 0;
                let progress = |note: &str| {
                    link.set_state(&app, Status::Connecting, None, Some(note.to_string()));
                };
                loop {
                    match sandhill_link::connect(&target, &options, &progress).await {
                        Ok(connection) => {
                            delay = RETRY_MIN;
                            failures = 0;
                            link.serve(&app, connection).await;
                            tracing::warn!("lost the connection to sandhilld on {}", link.host);
                            link.set_state(
                                &app,
                                Status::Connecting,
                                Some("lost the connection to sandhilld".into()),
                                None,
                            );
                        }
                        Err(err) => {
                            tracing::warn!("cannot reach sandhilld on {}: {err:#}", link.host);
                            // Reconnecting for the first few tries, then disconnected.
                            failures += 1;
                            let status = if failures < RECONNECTING_TRIES {
                                Status::Connecting
                            } else {
                                Status::Disconnected
                            };
                            link.set_state(&app, status, Some(err.to_string()), None);
                        }
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(RETRY_MAX);
                }
            }
        });
        *link.task.lock().unwrap() = Some(task);
        link
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.lock().unwrap().take() {
            // Dropping the connection kills its `attach` process.
            task.abort();
        }
        self.client.lock().unwrap().take();
    }

    fn client(&self) -> Option<Arc<Client>> {
        self.client.lock().unwrap().clone()
    }

    fn state(&self) -> LinkState {
        self.state.lock().unwrap().clone()
    }

    fn set_state<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        status: Status,
        error: Option<String>,
        detail: Option<String>,
    ) {
        let state = LinkState {
            host: self.host.clone(),
            status,
            error,
            detail,
            stopped: self.stopped.load(Ordering::Relaxed),
        };
        *self.state.lock().unwrap() = state.clone();
        app.state::<Arc<Links>>().emit(app, &self.host, LINK, state);
    }

    /// Serves one connection until it drops, forwarding its events to the
    /// windows that use this host.
    async fn serve<R: Runtime>(&self, app: &AppHandle<R>, connection: Connection) {
        let Connection { mut client, child } = connection;
        let (Some(mut events), Some(mut terminals), Some(mut requests)) = (
            client.take_events(),
            client.take_terminal_events(),
            client.take_requests(),
        ) else {
            return;
        };
        *self.client.lock().unwrap() = Some(Arc::new(client));
        self.set_state(app, Status::Connected, None, None);
        tracing::info!("connected to sandhilld on {}", self.host);
        let links = app.state::<Arc<Links>>();
        // Both end when the connection drops.
        let terminals = async {
            while let Some(event) = terminals.recv().await {
                links.emit_hosted(app, &self.host, TERMINAL, event);
            }
        };
        let sessions = async {
            while let Some(event) = events.recv().await {
                links.emit_hosted(app, &self.host, EVENT, event);
            }
        };
        let requests = async {
            while let Some(request) = requests.recv().await {
                links.emit_hosted(app, &self.host, REQUEST, request);
            }
        };
        tokio::join!(sessions, terminals, requests);
        // Calls still waiting fail with "disconnected" once the client goes.
        self.client.lock().unwrap().take();
        drop(child);
    }
}

/// Starts (or joins) the connection to `host` for the calling window and
/// returns its current state; changes arrive as `daemon://link` events.
#[tauri::command]
pub fn use_host<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    links: State<'_, Arc<Links>>,
    host: String,
) -> Result<LinkState, String> {
    links.use_host(&app, window.label(), &host)
}

/// Drops the connection to `host` and connects again, as the status bar's
/// Reconnect and Connect do.
#[tauri::command]
pub fn reconnect_host<R: Runtime>(
    app: AppHandle<R>,
    links: State<'_, Arc<Links>>,
    host: String,
) -> Result<(), String> {
    links.reconnect(&app, &host)
}

/// Drops the connection to `host` until `reconnect_host`.
#[tauri::command]
pub fn disconnect_host<R: Runtime>(app: AppHandle<R>, links: State<'_, Arc<Links>>, host: String) {
    links.disconnect(&app, &host);
}

/// Sends one protocol request to `host`. `method` and `params` are exactly
/// what the daemon takes on the wire (see docs/protocol.md).
#[tauri::command]
pub async fn daemon_call(
    links: State<'_, Arc<Links>>,
    host: String,
    method: String,
    params: Option<Value>,
) -> Result<Value, String> {
    let mut request = serde_json::Map::new();
    request.insert("method".into(), Value::String(method));
    if let Some(params) = params.filter(|p| !p.is_null()) {
        request.insert("params".into(), params);
    }
    let call: Call = serde_json::from_value(Value::Object(request))
        .map_err(|err| format!("bad request: {err}"))?;
    let large = matches!(&call, Call::WriteFile { text, .. } | Call::WriteNotebook { text, .. }
        if text.len() > memory::LARGE);
    let client = links
        .link(&host)
        .and_then(|link| link.client())
        .ok_or("not connected to sandhilld")?;
    let result = client.call(call).await.map_err(|err| match err {
        // The UI matches some daemon errors by prefix (`conflict:`).
        ClientError::Remote(message) => message,
        other => other.to_string(),
    });
    if large || result.as_ref().is_ok_and(|v| text_size(v) > memory::LARGE) {
        release_soon();
    }
    result
}

/// Bytes of text in a value, which is nearly all of a large reply.
fn text_size(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(text_size).sum(),
        Value::Object(map) => map.iter().map(|(k, v)| k.len() + text_size(v)).sum(),
        _ => 0,
    }
}

/// Releases freed memory once the reply has been handed to the web view
/// and its copies dropped. Calls close together share one release.
fn release_soon() {
    static PENDING: AtomicBool = AtomicBool::new(false);
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        PENDING.store(false, Ordering::Release);
        memory::release_free_memory();
    });
}

/// `sandhill-blob://localhost/<hash>?mime=<type>&host=<host>&near=<document>`:
/// the blob's bytes, base64-decoded for binary types, so `<img>` can load it.
pub fn blob_protocol<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let links = app.state::<Arc<Links>>();
        responder.respond(serve_blob(&links, &request.uri().to_string()).await);
    });
}

async fn serve_blob(links: &Links, uri: &str) -> Response<Vec<u8>> {
    let Some(query) = BlobQuery::parse(uri) else {
        return text_response(StatusCode::BAD_REQUEST, "not a blob URL".into());
    };
    let Some(link) = links.link(&query.host) else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("not connected to {}", query.host),
        );
    };
    let binary = is_binary(&query.mime);
    let key = (query.hash.clone(), binary);
    let cached = link.blobs.lock().unwrap().get(&key);
    let bytes = match cached {
        Some(bytes) => bytes,
        None => {
            let Some(client) = link.client() else {
                return text_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "not connected to sandhilld".into(),
                );
            };
            let call = Call::GetBlob {
                hash: query.hash.clone(),
                near: query.near.clone(),
            };
            let blob: Blob = match client.call(call).await {
                Ok(blob) => blob,
                Err(err) => return text_response(StatusCode::NOT_FOUND, err.to_string()),
            };
            let bytes = if binary {
                match base64::engine::general_purpose::STANDARD.decode(blob.data.trim()) {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        return text_response(StatusCode::UNPROCESSABLE_ENTITY, err.to_string());
                    }
                }
            } else {
                blob.data.into_bytes()
            };
            let bytes = Arc::new(bytes);
            link.blobs.lock().unwrap().insert(key, bytes.clone());
            bytes
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, query.mime)
        .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes.as_ref().clone())
        .unwrap_or_default()
}

fn text_response(status: StatusCode, text: String) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(text.into_bytes())
        .unwrap_or_default()
}

/// Binary values travel as base64 in notebook outputs.
fn is_binary(mime: &str) -> bool {
    (mime.starts_with("image/") && mime != "image/svg+xml") || mime == "application/pdf"
}

#[derive(Debug, PartialEq)]
struct BlobQuery {
    hash: String,
    mime: String,
    host: String,
    near: Option<String>,
}

impl BlobQuery {
    /// Accepts both forms webviews use for custom schemes:
    /// `sandhill-blob://localhost/<hash>` and `http://sandhill-blob.localhost/<hash>`.
    fn parse(uri: &str) -> Option<Self> {
        let url = tauri::Url::parse(uri).ok()?;
        let hash = url.path().trim_start_matches('/').to_string();
        let valid = hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !valid {
            return None;
        }
        let mut mime = "application/octet-stream".to_string();
        let mut host = sandhill_link::LOCAL.to_string();
        let mut near = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "mime" => mime = value.into_owned(),
                "host" => host = value.into_owned(),
                "near" => near = Some(value.into_owned()),
                _ => {}
            }
        }
        Some(BlobQuery {
            hash,
            mime,
            host,
            near,
        })
    }
}

/// Least recently added blobs go first once the cache is full.
#[derive(Default)]
struct BlobCache {
    items: HashMap<(String, bool), Arc<Vec<u8>>>,
    order: VecDeque<(String, bool)>,
    bytes: usize,
}

impl BlobCache {
    fn get(&self, key: &(String, bool)) -> Option<Arc<Vec<u8>>> {
        self.items.get(key).cloned()
    }

    fn insert(&mut self, key: (String, bool), value: Arc<Vec<u8>>) {
        if value.len() > BLOB_CACHE_BYTES / 4 || self.items.contains_key(&key) {
            return;
        }
        self.bytes += value.len();
        self.items.insert(key.clone(), value);
        self.order.push_back(key);
        while self.bytes > BLOB_CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.items.remove(&oldest) {
                self.bytes -= removed.len();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "8c7dd922ad47494fc02c388e12c00eac278d0d5c1b55ae4a0e1d0e0ad6b2f5a1";

    #[test]
    fn blob_urls_parse_on_every_platform() {
        let expected = BlobQuery {
            hash: HASH.into(),
            mime: "image/png".into(),
            host: "local".into(),
            near: Some("/home/me/a b.qmd".into()),
        };
        let query = "mime=image%2Fpng&host=local&near=%2Fhome%2Fme%2Fa+b.qmd";
        for base in [
            "sandhill-blob://localhost",
            "http://sandhill-blob.localhost",
        ] {
            let parsed = BlobQuery::parse(&format!("{base}/{HASH}?{query}"));
            assert_eq!(parsed.as_ref(), Some(&expected), "{base}");
        }
        assert!(BlobQuery::parse("sandhill-blob://localhost/../etc/passwd").is_none());
    }

    #[test]
    fn requests_take_the_wire_format() {
        let call: Call = serde_json::from_value(serde_json::json!({"method": "ping"})).unwrap();
        assert_eq!(call, Call::Ping);
        let call: Call = serde_json::from_value(serde_json::json!({
            "method": "read_file",
            "params": {"path": "/a.qmd"}
        }))
        .unwrap();
        assert_eq!(
            call,
            Call::ReadFile {
                path: "/a.qmd".into()
            }
        );
    }

    #[test]
    fn the_blob_cache_stays_bounded() {
        let mut cache = BlobCache::default();
        let big = Arc::new(vec![0u8; BLOB_CACHE_BYTES / 4]);
        for i in 0..6 {
            cache.insert((format!("{i}"), true), big.clone());
        }
        assert!(cache.bytes <= BLOB_CACHE_BYTES);
        assert!(cache.get(&("0".into(), true)).is_none());
        assert!(cache.get(&("5".into(), true)).is_some());
    }
}
