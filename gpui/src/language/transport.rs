//! Shared Python hosts and session-attached R hosts, with ordered full-text sync.
use super::virtual_doc::Document;
use anyhow::{Result, anyhow};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, LspMessage, LspStarted};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

type Replies = Arc<Mutex<HashMap<(String, u64), oneshot::Sender<Result<Value>>>>>;
#[derive(Default)]
struct Host {
    id: Option<String>,
    documents: HashMap<String, (Document, i32)>,
}

/// Daemon language hosts by project, language and (for R) Ark session.
type HostKey = (String, String, Option<String>);

pub(crate) struct Transport {
    client: Arc<Client>,
    hosts: tokio::sync::Mutex<HashMap<HostKey, Host>>,
    replies: Replies,
    next: AtomicU64,
}

impl Transport {
    pub fn new(
        client: Arc<Client>,
        mut messages: mpsc::UnboundedReceiver<LspMessage>,
        rt: &tokio::runtime::Runtime,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<LspMessage>) {
        let replies: Replies = Default::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let transport = Arc::new(Self {
            client,
            hosts: Default::default(),
            replies: replies.clone(),
            next: AtomicU64::new(1),
        });
        let weak = Arc::downgrade(&transport);
        rt.spawn(async move {
            while let Some(mut event) = messages.recv().await {
                if let Some(id) = event.message.get("id").filter(|_| event.message.get("method").is_none()).and_then(Value::as_u64)
                    && let Some(reply) = replies.lock().unwrap().remove(&(event.lsp.clone(), id))
                {
                    let result = if let Some(error) = event.message.get("error") { Err(anyhow!("{error}")) }
                    else { Ok(event.message.get("result").cloned().unwrap_or(Value::Null)) };
                    let _ = reply.send(result);
                    continue;
                }
                if let Some(transport) = weak.upgrade() {
                    let hosts = transport.hosts.lock().await;
                    if let Some((key, host)) = hosts.iter().find(|(_, host)| host.id.as_deref() == Some(&event.lsp)) {
                        event.message["_sandhill"] = json!({"language":key.1,"session":key.2});
                        if event.message["method"] == "textDocument/publishDiagnostics" {
                            let uri = event.message["params"]["uri"].as_str().unwrap_or("");
                            let Some((document, version)) = host.documents.values().find(|(doc, _)| doc.uri == uri) else { continue; };
                            // The daemon translates its shared server version back
                            // to this connection's version. Never relabel an old
                            // diagnostic with the newest virtual-document hash.
                            if event.message["params"]["version"].as_i64()
                                .is_some_and(|published| published != i64::from(*version))
                            { continue; }
                            event.message["_sandhill"]["textHash"] = json!(document.text_hash());
                        }
                    } else if event.message.get("method").is_some() { continue; }
                }
                if event.message["method"] == "sandhill/lspExited" {
                    if let Some(transport) = weak.upgrade() {
                        for host in transport.hosts.lock().await.values_mut() {
                            if host.id.as_deref() == Some(&event.lsp) { host.id = None; }
                        }
                    }
                    let mut pending = replies.lock().unwrap();
                    let ids: Vec<_> = pending.keys().filter(|(lsp, _)| lsp == &event.lsp).cloned().collect();
                    for id in ids { if let Some(reply) = pending.remove(&id) { let _ = reply.send(Err(anyhow!("Language server exited"))); } }
                }
                // Server-initiated requests are not UI requests. Explicitly reject
                // unsupported ones instead of leaving the server waiting forever.
                if event.message.get("id").is_some() && event.message.get("method").is_some() {
                    if let Some(transport) = weak.upgrade() {
                        let method = event.message["method"].as_str().unwrap_or("");
                        let message = if method == "workspace/configuration" {
                            let count = event.message["params"]["items"].as_array().map_or(0, Vec::len);
                            json!({"jsonrpc":"2.0","id":event.message["id"],"result":vec![Value::Null; count]})
                        } else { json!({"jsonrpc":"2.0","id":event.message["id"],"error":{"code":-32601,"message":"Unsupported client request"}}) };
                        let _ = transport.client.call::<Value>(Call::LspSend { lsp: event.lsp.clone(), message }).await;
                    }
                    continue;
                }
                let _ = tx.send(event);
            }
            replies.lock().unwrap().clear();
        });
        (transport, rx)
    }

    async fn send(&self, id: &str, message: Value) -> Result<()> {
        self.client
            .call::<Value>(Call::LspSend {
                lsp: id.into(),
                message,
            })
            .await?;
        Ok(())
    }

    async fn synchronize(&self, project: &str, document: Document) -> Result<String> {
        let mut hosts = self.hosts.lock().await;
        if !document.alive.load(Ordering::Acquire) {
            return Err(anyhow!("Document closed"));
        }
        if hosts
            .values()
            .filter_map(|host| host.documents.get(&document.path))
            .any(|(previous, _)| {
                previous.language == document.language && previous.revision > document.revision
            })
        {
            return Err(anyhow!("Document changed"));
        }
        let session = (document.language == "r")
            .then(|| document.session.clone())
            .flatten();
        let key = (project.to_string(), document.language.clone(), session);
        // Switching a document's kernel releases its old Ark document without
        // detaching other documents that still use that session.
        let previous: Vec<_> = hosts
            .keys()
            .filter(|old| old.0 == project && old.1 == document.language && **old != key)
            .cloned()
            .collect();
        for previous in previous {
            let host = hosts.get_mut(&previous).unwrap();
            if let Some((old, _)) = host.documents.remove(&document.path)
                && let Some(id) = &host.id
            {
                self.send(id, json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":old.uri}}})).await?;
            }
            if host.documents.is_empty() {
                if let Some(id) = host.id.take() {
                    self.client.call::<Value>(Call::LspStop { lsp: id }).await?;
                }
                hosts.remove(&previous);
            }
        }
        let host = hosts.entry(key).or_default();
        let fresh = host.id.is_none();
        if fresh {
            let started: LspStarted = self
                .client
                .call(Call::LspStart {
                    project: project.into(),
                    language: document.language.clone(),
                    document_kind: Some(document.kind.into()),
                    session: document.session.clone(),
                })
                .await?;
            host.id = Some(started.lsp);
        }
        let id = host.id.as_ref().unwrap().clone();
        if fresh {
            for (previous, version) in host.documents.values_mut() {
                *version = 1;
                if previous.path == document.path {
                    *previous = document.clone();
                }
                self.send(&id, json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":previous.uri,"languageId":previous.language,"version":1,"text":previous.text}}})).await?;
            }
        }
        match host.documents.get_mut(&document.path) {
            Some((previous, version)) => {
                if previous.text != document.text || previous.uri != document.uri {
                    *version += 1;
                    self.send(&id, json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":document.uri,"version":*version},"contentChanges":[{"text":document.text}]}})).await?;
                }
                *previous = document;
            }
            None => {
                self.send(&id, json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":document.uri,"languageId":document.language,"version":1,"text":document.text}}})).await?;
                host.documents.insert(document.path.clone(), (document, 1));
            }
        }
        Ok(id)
    }

    pub async fn sync(&self, project: String, document: Document, saved: bool) -> Result<()> {
        let uri = document.uri.clone();
        let text = saved.then(|| document.text.clone());
        let id = self.synchronize(&project, document).await?;
        if let Some(text) = text {
            self.send(&id, json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":uri},"text":text}})).await?;
        }
        Ok(())
    }

    pub async fn request(
        &self,
        project: String,
        document: Document,
        method: &'static str,
        mut params: Value,
    ) -> Result<Value> {
        params["textDocument"] = json!({"uri":document.uri});
        let lsp = self.synchronize(&project, document).await?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let key = (lsp.clone(), id);
        let (tx, rx) = oneshot::channel();
        self.replies.lock().unwrap().insert(key.clone(), tx);
        let result = async {
            self.send(
                &lsp,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await?;
            tokio::time::timeout(Duration::from_secs(5), rx)
                .await
                .map_err(|_| anyhow!("Language server timed out"))?
                .map_err(|_| anyhow!("Language server disconnected"))?
        }
        .await;
        self.replies.lock().unwrap().remove(&key);
        result
    }

    pub async fn close(
        &self,
        project: String,
        path: Option<String>,
        language: Option<String>,
    ) -> Result<()> {
        let mut hosts = self.hosts.lock().await;
        let keys: Vec<_> = hosts
            .keys()
            .filter(|(p, lang, _)| {
                *p == project && language.as_ref().is_none_or(|language| lang == language)
            })
            .cloned()
            .collect();
        for key in keys {
            let host = hosts.get_mut(&key).unwrap();
            let paths: Vec<_> = host
                .documents
                .keys()
                .filter(|p| path.as_ref().is_none_or(|path| *p == path))
                .cloned()
                .collect();
            for path in paths {
                if let Some((document, _)) = host.documents.remove(&path)
                    && let Some(id) = &host.id
                {
                    self.send(id, json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":document.uri}}})).await?;
                }
            }
            if host.documents.is_empty() {
                if let Some(id) = host.id.take() {
                    self.client.call::<Value>(Call::LspStop { lsp: id }).await?;
                }
                hosts.remove(&key);
            }
        }
        Ok(())
    }
}
