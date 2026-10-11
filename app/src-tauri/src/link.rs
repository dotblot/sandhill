//! The app's connection to sandhilld, shared by every page load.
//!
//! The UI reaches the daemon through three doors, which `app/src/backend/tauri.ts`
//! wraps: the `daemon_call` command for requests, `daemon://event` and
//! `daemon://link` events, and the `sandhill-blob:` protocol for plots and
//! other large outputs.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use sandhill_proto::client::{Client, ClientError};
use sandhill_proto::{Blob, Call};
use serde::Serialize;
use serde_json::Value;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use crate::{daemon, memory};

pub const EVENT: &str = "daemon://event";
pub const LINK: &str = "daemon://link";
pub const BLOB_SCHEME: &str = "sandhill-blob";

const RETRY_MIN: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(5);
/// Decoded blobs kept in memory, so reopening a document doesn't fetch its
/// plots again (that matters over ssh). The web view keeps its own copy of
/// every image it shows, so this only needs to cover recent ones.
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
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct Link {
    client: Mutex<Option<Arc<Client>>>,
    state: Mutex<LinkState>,
    blobs: Mutex<BlobCache>,
}

impl Default for Link {
    fn default() -> Self {
        Link {
            client: Mutex::new(None),
            state: Mutex::new(LinkState {
                status: Status::Connecting,
                error: None,
            }),
            blobs: Mutex::default(),
        }
    }
}

impl Link {
    fn client(&self) -> Option<Arc<Client>> {
        self.client.lock().unwrap().clone()
    }

    fn set_state<R: Runtime>(&self, app: &AppHandle<R>, status: Status, error: Option<String>) {
        let state = LinkState { status, error };
        *self.state.lock().unwrap() = state.clone();
        let _ = app.emit(LINK, state);
    }

    /// Serves one connection until it drops, forwarding its events to the UI.
    async fn serve<R: Runtime>(&self, app: &AppHandle<R>, connection: daemon::Connection) {
        let daemon::Connection { mut client, child } = connection;
        let Some(mut events) = client.take_events() else {
            return;
        };
        *self.client.lock().unwrap() = Some(Arc::new(client));
        self.set_state(app, Status::Connected, None);
        tracing::info!("connected to sandhilld");
        while let Some(event) = events.recv().await {
            let _ = app.emit(EVENT, event);
        }
        // Calls still waiting fail with "disconnected" once the client goes.
        self.client.lock().unwrap().take();
        drop(child);
    }
}

/// Connects in the background and reconnects whenever the connection drops.
/// The UI resubscribes from the last event it saw, so nothing is missed.
pub fn start<R: Runtime>(app: AppHandle<R>, link: Arc<Link>) {
    tauri::async_runtime::spawn(async move {
        let mut delay = RETRY_MIN;
        loop {
            match daemon::connect().await {
                Ok(connection) => {
                    delay = RETRY_MIN;
                    link.serve(&app, connection).await;
                    tracing::warn!("lost the connection to sandhilld");
                    link.set_state(
                        &app,
                        Status::Disconnected,
                        Some("lost the connection to sandhilld".into()),
                    );
                }
                Err(err) => {
                    tracing::warn!("cannot reach sandhilld: {err:#}");
                    link.set_state(&app, Status::Disconnected, Some(format!("{err:#}")));
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RETRY_MAX);
        }
    });
}

/// Sends one protocol request. `method` and `params` are exactly what the
/// daemon takes on the wire (see docs/protocol.md).
#[tauri::command]
pub async fn daemon_call(
    link: State<'_, Arc<Link>>,
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
    let client = link.client().ok_or("not connected to sandhilld")?;
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
    use std::sync::atomic::{AtomicBool, Ordering};
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

#[tauri::command]
pub fn link_state(link: State<'_, Arc<Link>>) -> LinkState {
    link.state.lock().unwrap().clone()
}

/// `sandhill-blob://localhost/<hash>?mime=<type>&near=<document>`: the
/// blob's bytes, base64-decoded for binary types, so `<img>` can load it.
pub fn blob_protocol<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let link = app.state::<Arc<Link>>();
        responder.respond(serve_blob(&link, &request.uri().to_string()).await);
    });
}

async fn serve_blob(link: &Link, uri: &str) -> Response<Vec<u8>> {
    let Some(query) = BlobQuery::parse(uri) else {
        return text_response(StatusCode::BAD_REQUEST, "not a blob URL".into());
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
        let mut near = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "mime" => mime = value.into_owned(),
                "near" => near = Some(value.into_owned()),
                _ => {}
            }
        }
        Some(BlobQuery { hash, mime, near })
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
            near: Some("/home/me/a b.qmd".into()),
        };
        let query = "mime=image%2Fpng&near=%2Fhome%2Fme%2Fa+b.qmd";
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
