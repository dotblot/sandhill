//! Project-scoped language servers, shared across local and SSH connections.
mod discovery;
mod transport;
pub use discovery::find_binary;

use crate::session::SessionHandle;
use anyhow::{Context, Result, bail};
use sandhill_proto::{LspStarted, ServerMessage};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use transport::{Transport, Writer};

type Out = mpsc::UnboundedSender<ServerMessage>;
const IDLE: Duration = Duration::from_secs(60);
const START_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct Start {
    pub project: String,
    pub language: String,
    pub interpreter: Option<PathBuf>,
    pub ark: Option<Arc<SessionHandle>>,
}

#[derive(Default)]
pub struct LanguageServers {
    // Serialize only startup and attachment; all traffic runs in per-server actors.
    // Keyed by project, language and, for R, the Ark session whose live
    // globals the server reads: two R documents on different kernels must not
    // share one session's completions.
    servers: Mutex<HashMap<(String, String, String), Host>>,
}
struct Host {
    id: String,
    capabilities: Value,
    commands: mpsc::UnboundedSender<Command>,
    task: tokio::task::JoinHandle<()>,
}
enum Command {
    Attach {
        connection: u64,
        out: Out,
    },
    Send {
        connection: u64,
        message: Value,
        reply: oneshot::Sender<Result<()>>,
    },
    Detach {
        connection: u64,
    },
}

impl LanguageServers {
    pub(crate) async fn start(
        &self,
        start: Start,
        connection: u64,
        out: Out,
    ) -> Result<LspStarted> {
        let Start {
            project,
            language,
            interpreter,
            ark,
        } = start;
        let mut servers = self.servers.lock().await;
        let session = ark.as_ref().map(|ark| ark.id.clone()).unwrap_or_default();
        let key = (project.clone(), language.clone(), session);
        if servers
            .get(&key)
            .is_some_and(|host| host.commands.is_closed())
        {
            servers.remove(&key);
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = servers.entry(key.clone()) {
            let (transports, capabilities) =
                tokio::time::timeout(START_TIMEOUT, launch(&project, &language, interpreter, ark))
                    .await
                    .context("lsp_start_timeout: language server did not initialize")??;
            let id = uuid::Uuid::new_v4().to_string();
            let (commands, receiver) = mpsc::unbounded_channel();
            let task = tokio::spawn(
                Actor::new(
                    id.clone(),
                    language,
                    capabilities.clone(),
                    transports,
                    receiver,
                )
                .run(),
            );
            entry.insert(Host {
                id,
                capabilities,
                commands,
                task,
            });
        }
        let host = &servers[&key];
        host.commands
            .send(Command::Attach { connection, out })
            .context("lsp_exited: start the language server again")?;
        Ok(LspStarted {
            lsp: host.id.clone(),
            capabilities: host.capabilities.clone(),
        })
    }

    pub async fn send(&self, lsp: &str, connection: u64, message: Value) -> Result<()> {
        if !message.is_object() || message["jsonrpc"] != "2.0" {
            bail!("LSP message must be a JSON-RPC 2.0 object");
        }
        let commands = self
            .servers
            .lock()
            .await
            .values()
            .find(|host| host.id == lsp)
            .context("lsp_not_found: start the language server again")?
            .commands
            .clone();
        let (reply, receiver) = oneshot::channel();
        commands
            .send(Command::Send {
                connection,
                message,
                reply,
            })
            .context("lsp_exited: start the language server again")?;
        receiver
            .await
            .context("lsp_exited: language server stopped")?
    }

    pub async fn stop(&self, lsp: &str, connection: u64) -> Result<()> {
        let servers = self.servers.lock().await;
        let host = servers
            .values()
            .find(|host| host.id == lsp)
            .context("lsp_not_found: language server already stopped")?;
        let _ = host.commands.send(Command::Detach { connection });
        Ok(())
    }

    pub async fn disconnected(&self, connection: u64) {
        for host in self.servers.lock().await.values() {
            let _ = host.commands.send(Command::Detach { connection });
        }
    }

    pub async fn stop_all(&self) {
        let hosts = std::mem::take(&mut *self.servers.lock().await);
        for host in hosts.values() {
            host.task.abort();
        }
        for host in hosts.into_values() {
            let _ = host.task.await;
        }
    }
}

async fn launch(
    project: &str,
    language: &str,
    interpreter: Option<PathBuf>,
    ark: Option<Arc<SessionHandle>>,
) -> Result<(Vec<Transport>, Value)> {
    let root = PathBuf::from(project);
    let uri = discovery::file_uri(&root);
    let base = json!({"processId":std::process::id(),"clientInfo":{"name":"Sandhill"},"rootUri":uri,"workspaceFolders":[{"uri":uri,"name":root.file_name().unwrap_or_default().to_string_lossy()}],"capabilities":{"general":{"positionEncodings":["utf-16"]},"textDocument":{"synchronization":{"didSave":true},"completion":{"completionItem":{"snippetSupport":false}},"publishDiagnostics":{"relatedInformation":true}},"workspace":{"configuration":false,"workspaceFolders":true}},"trace":"off"});
    match language {
        "python" => {
            let mut ty =
                Transport::stdio(discovery::command(&root, interpreter.as_deref(), "ty")?)?;
            let mut ruff =
                Transport::stdio(discovery::command(&root, interpreter.as_deref(), "ruff")?)?;
            let mut params = base.clone();
            let mut settings = json!({"diagnosticMode":"off","showSyntaxErrors":false});
            if let Some(python) = interpreter {
                settings["configuration"] = json!({"environment":{"python":python}});
            }
            params["initializationOptions"] = settings;
            let mut result = ty.initialize(params).await?;
            let mut params = base;
            // No unused-import/style churn. Catch malformed code, invalid control
            // flow/comparisons, undefined names, and locals used before assignment.
            params["initializationOptions"] = json!({"settings":{"configurationPreference":"editorOnly","lint":{"enable":true,"select":["E9","F63","F7","F82"],"ignore":[]},"fixAll":false,"organizeImports":false,"showSyntaxErrors":true}});
            let ruff_result = ruff.initialize(params).await?;
            result["capabilities"]["positionEncoding"] = json!("utf-16");
            result["capabilities"]["textDocumentSync"] =
                json!({"openClose":true,"change":1,"save":{"includeText":true}});
            if let Some(capabilities) = result["capabilities"].as_object_mut() {
                capabilities.remove("diagnosticProvider");
            }
            if let Some(diagnostics) = ruff_result["capabilities"].get("diagnosticProvider") {
                result["capabilities"]["diagnosticProvider"] = diagnostics.clone();
            }
            // Formatting is explicitly out of scope.
            if let Some(capabilities) = result["capabilities"].as_object_mut() {
                capabilities.remove("documentFormattingProvider");
                capabilities.remove("documentRangeFormattingProvider");
            }
            Ok((vec![ty, ruff], result))
        }
        "r" => {
            let ark =
                ark.context("ark_session_required: start the R session for language features")?;
            let port = ark.start_ark_lsp().await?;
            let mut transport = Transport::tcp(port).await?;
            let mut result = transport.initialize(base).await?;
            result["capabilities"]["positionEncoding"] = json!("utf-16");
            Ok((vec![transport], result))
        }
        _ => bail!("lsp_language_unsupported: supported languages are r and python"),
    }
}

struct Document {
    // Versions belong to each connection; the shared server uses its own
    // monotonic counter, which may advance when another client attaches.
    clients: HashMap<u64, i64>,
    version: i64,
}
impl Document {
    fn diagnostic_for(&self, connection: u64, message: &Value) -> Option<Value> {
        let version = *self.clients.get(&connection)?;
        let published = message["params"]["version"].as_i64();
        if published.is_some_and(|published| published != self.version) {
            return None;
        }
        let mut message = message.clone();
        if published.is_some() {
            message["params"]["version"] = json!(version);
        }
        Some(message)
    }
}
struct Actor {
    id: String,
    language: String,
    capabilities: Value,
    writers: Vec<Writer>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    incoming: mpsc::UnboundedReceiver<(usize, Result<Value>)>,
    commands: mpsc::UnboundedReceiver<Command>,
    clients: HashMap<u64, Out>,
    pending: HashMap<String, (u64, Value)>,
    next_request: u64,
    documents: HashMap<String, Document>,
    idle_since: Option<tokio::time::Instant>,
}
impl Drop for Actor {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
    }
}

impl Actor {
    fn new(
        id: String,
        language: String,
        capabilities: Value,
        transports: Vec<Transport>,
        commands: mpsc::UnboundedReceiver<Command>,
    ) -> Self {
        let (to, incoming) = mpsc::unbounded_channel();
        let (writers, readers) = transports
            .into_iter()
            .enumerate()
            .map(|(index, transport)| transport.receive(index, to.clone()))
            .unzip();
        Self {
            id,
            language,
            capabilities,
            writers,
            readers,
            incoming,
            commands,
            clients: HashMap::new(),
            pending: HashMap::new(),
            next_request: 0,
            documents: HashMap::new(),
            idle_since: Some(tokio::time::Instant::now()),
        }
    }

    async fn run(mut self) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(Command::Attach { connection, out }) => { self.clients.insert(connection, out); self.idle_since = None; },
                    Some(Command::Detach { connection }) => { if let Err(error) = self.detach(connection).await { self.crashed(error); break; } },
                    Some(Command::Send { connection, message, reply }) => {
                        let result = self.send(connection, message).await;
                        let _ = reply.send(result);
                    },
                    None => break,
                },
                message = self.incoming.recv() => match message {
                    Some((endpoint, Ok(message))) => if let Err(error) = self.received(endpoint, message).await { self.crashed(error); break; },
                    Some((_, Err(error))) => { self.crashed(error); break; },
                    None => break,
                },
                _ = tick.tick() => if self.idle_since.is_some_and(|since| since.elapsed() >= IDLE) { break; },
            }
        }
    }

    fn push(&self, connection: u64, message: Value) {
        if let Some(out) = self.clients.get(&connection) {
            let _ = out.send(ServerMessage::LspMessage {
                lsp: self.id.clone(),
                message,
            });
        }
    }
    fn broadcast(&self, message: Value) {
        for connection in self.clients.keys() {
            self.push(*connection, message.clone());
        }
    }
    fn crashed(&self, error: anyhow::Error) {
        for (connection, original) in self.pending.values() {
            self.push(*connection, json!({"jsonrpc":"2.0","id":original,"error":{"code":-32097,"message":format!("language server exited: {error}")}}));
        }
        self.broadcast(json!({"jsonrpc":"2.0","method":"sandhill/lspExited","params":{"code":"lsp_exited","message":error.to_string()}}));
    }

    async fn detach(&mut self, connection: u64) -> Result<()> {
        self.clients.remove(&connection);
        self.pending.retain(|_, (client, _)| *client != connection);
        let mut closed = Vec::new();
        for (uri, document) in &mut self.documents {
            document.clients.remove(&connection);
            if document.clients.is_empty() {
                closed.push(uri.clone());
            }
        }
        for uri in closed {
            self.documents.remove(&uri);
            for writer in &mut self.writers {
                transport::write(writer, &json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}})).await?;
            }
        }
        if self.clients.is_empty() {
            self.idle_since = Some(tokio::time::Instant::now());
        }
        Ok(())
    }

    async fn send(&mut self, connection: u64, mut message: Value) -> Result<()> {
        if !self.clients.contains_key(&connection) {
            bail!("lsp_not_subscribed: call lsp_start on this connection first");
        }
        let method = message["method"].as_str().unwrap_or_default().to_string();
        if method == "initialize" {
            let id = message.get("id").context("initialize needs an id")?;
            self.push(
                connection,
                json!({"jsonrpc":"2.0","id":id,"result":self.capabilities}),
            );
            return Ok(());
        }
        if method == "initialized" || method == "exit" {
            return Ok(());
        }
        if method == "shutdown" {
            self.push(
                connection,
                json!({"jsonrpc":"2.0","id":message["id"],"result":null}),
            );
            return Ok(());
        }
        if method == "workspace/didChangeConfiguration" {
            bail!("LSP configuration is managed by Sandhill");
        }
        if method == "textDocument/formatting" || method == "textDocument/rangeFormatting" {
            bail!("LSP formatting is not enabled");
        }
        if method == "$/cancelRequest" {
            let original = &message["params"]["id"];
            if let Some((id, _)) = self
                .pending
                .iter()
                .find(|(_, (client, request))| *client == connection && request == original)
            {
                message["params"]["id"] = json!(id);
            } else {
                return Ok(());
            }
        }
        if method == "textDocument/didOpen" {
            let uri = message["params"]["textDocument"]["uri"]
                .as_str()
                .context("didOpen requires a URI")?
                .to_string();
            let document = self.documents.entry(uri.clone()).or_insert(Document {
                clients: HashMap::new(),
                version: 0,
            });
            let existing = !document.clients.is_empty();
            document.clients.insert(
                connection,
                message["params"]["textDocument"]["version"]
                    .as_i64()
                    .unwrap_or(0),
            );
            document.version += 1;
            let version = document.version;
            if existing {
                message = json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":message["params"]["textDocument"]["text"]}]}});
            } else {
                message["params"]["textDocument"]["version"] = json!(version);
            }
        } else if method == "textDocument/didChange" {
            let uri = message["params"]["textDocument"]["uri"]
                .as_str()
                .context("didChange requires a URI")?;
            let document = self
                .documents
                .get_mut(uri)
                .context("didOpen must precede didChange")?;
            if !document.clients.contains_key(&connection) {
                bail!("didOpen must precede didChange on this connection");
            }
            if message["params"]["contentChanges"]
                .as_array()
                .is_none_or(|changes| changes.iter().any(|change| change.get("range").is_some()))
            {
                bail!("Sandhill requires full-text contentChanges");
            }
            document.clients.insert(
                connection,
                message["params"]["textDocument"]["version"]
                    .as_i64()
                    .unwrap_or(0),
            );
            document.version += 1;
            message["params"]["textDocument"]["version"] = json!(document.version);
        } else if method == "textDocument/didClose" {
            let uri = message["params"]["textDocument"]["uri"]
                .as_str()
                .context("didClose requires a URI")?;
            if let Some(document) = self.documents.get_mut(uri) {
                document.clients.remove(&connection);
                if !document.clients.is_empty() {
                    return Ok(());
                }
            }
            self.documents.remove(uri);
        }
        if let Some(original) = message.get("id").cloned() {
            if method.is_empty() {
                bail!("server requests are handled by the daemon");
            }
            self.next_request += 1;
            let id = format!("sandhill-{}", self.next_request);
            self.pending.insert(id.clone(), (connection, original));
            message["id"] = json!(id);
        }
        let diagnostics = method == "textDocument/diagnostic" || method == "workspace/diagnostic";
        if self.language == "python" && message.get("id").is_none() {
            for writer in &mut self.writers {
                transport::write(writer, &message).await?;
            }
        } else {
            let endpoint = usize::from(self.language == "python" && diagnostics);
            transport::write(&mut self.writers[endpoint], &message).await?;
        }
        Ok(())
    }

    async fn received(&mut self, endpoint: usize, mut message: Value) -> Result<()> {
        if let Some(reply) = transport::server_reply(&message) {
            transport::write(&mut self.writers[endpoint], &reply).await?;
            return Ok(());
        }
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            if let Some((connection, original)) = self.pending.remove(id) {
                message["id"] = original;
                self.push(connection, message);
            }
        } else if !(self.language == "python"
            && endpoint == 0
            && message["method"] == "textDocument/publishDiagnostics")
        {
            if message["method"] == "textDocument/publishDiagnostics" {
                if let Some(document) = message["params"]["uri"]
                    .as_str()
                    .and_then(|uri| self.documents.get(uri))
                {
                    for &connection in document.clients.keys() {
                        if let Some(message) = document.diagnostic_for(connection, &message) {
                            self.push(connection, message);
                        }
                    }
                }
            } else {
                self.broadcast(message);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Document;
    use serde_json::json;

    #[test]
    fn diagnostics_use_recipient_versions_and_drop_obsolete_ranges() {
        let document = Document {
            clients: [(10, 2), (20, 1)].into_iter().collect(),
            version: 3,
        };
        let stale = json!({"method":"textDocument/publishDiagnostics",
            "params":{"uri":"file:///report.py","version":2,"diagnostics":[]}});
        assert!(document.diagnostic_for(10, &stale).is_none());
        let mut current = stale;
        current["params"]["version"] = json!(3);
        assert_eq!(
            document.diagnostic_for(10, &current).unwrap()["params"]["version"],
            2
        );
        assert_eq!(
            document.diagnostic_for(20, &current).unwrap()["params"]["version"],
            1
        );
        assert!(document.diagnostic_for(30, &current).is_none());
        current["params"].as_object_mut().unwrap().remove("version");
        assert!(document.diagnostic_for(10, &current).unwrap()["params"]["version"].is_null());
    }
}
