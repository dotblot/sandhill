//! Device-local MCP connections, shared by every conversation (Huan, 2026-11-16).
mod catalog;
mod client;
mod oauth;
mod programs;
mod store;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    Call, ConnectorAdd, ConnectorInfo, ConnectorList, ConnectorSpec, SignInTarget,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use store::{Index, Record, Secret, Secrets, load, redact, save, save_both, summaries};

#[derive(Default)]
pub struct Connectors {
    gate: Mutex<()>,
    clients: Mutex<HashMap<String, Arc<client::Client>>>,
    listed: Mutex<HashSet<String>>,
    pending: oauth::PendingSignIns,
}

struct Prepared {
    record: Record,
    secret: Option<Secret>,
    downloaded: bool,
}

fn unknown(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no connector named {name} that is on (list_connectors lists them)")
}

fn binary_folder(root: &Path, info: &ConnectorInfo) -> Option<PathBuf> {
    let ConnectorSpec::Command { command, .. } = &info.spec else {
        return None;
    };
    let parent = Path::new(command).parent()?;
    let bin = std::fs::canonicalize(root.join("connectors/bin")).ok()?;
    (info.suggested.as_deref() == Some("biomcp") && parent.parent() == Some(bin.as_path()))
        .then(|| parent.to_path_buf())
}

fn can_query(info: &ConnectorInfo, tool: &str) -> bool {
    info.read_only
        || (info.suggested.is_some() && info.tools.iter().any(|t| t.name == tool && t.read_only))
}

impl Connectors {
    pub(crate) async fn call(&self, root: &Path, call: Call) -> Result<Value> {
        let mut sensitive = load::<Secrets>(root, "secrets.json")?
            .values()
            .flat_map(|s| s.sensitive().into_iter().map(str::to_owned))
            .collect::<Vec<_>>();
        match &call {
            Call::AddConnector {
                add:
                    ConnectorAdd::Custom {
                        secret: Some(secret),
                        ..
                    },
            }
            | Call::BeginConnectorSignIn {
                target:
                    SignInTarget::Add {
                        add:
                            ConnectorAdd::Custom {
                                secret: Some(secret),
                                ..
                            },
                    },
                ..
            } => sensitive.push(secret.clone()),
            Call::FinishConnectorSignIn {
                code: Some(code), ..
            } => sensitive.push(code.clone()),
            _ => {}
        }
        let result = self.dispatch(root, call).await;
        sensitive.extend(
            load::<Secrets>(root, "secrets.json")?
                .values()
                .flat_map(|s| s.sensitive().into_iter().map(str::to_owned)),
        );
        let private = sensitive.iter().map(String::as_str).collect::<Vec<_>>();
        match result {
            Ok(value) => Ok(store::sanitize_values(value, &private)),
            Err(error) => Err(anyhow::anyhow!(store::redact_values(
                &format!("{error:#}"),
                &private
            ))),
        }
    }

    async fn connection(&self, info: &ConnectorInfo, secrets: &Secrets) -> Arc<client::Client> {
        self.clients
            .lock()
            .await
            .entry(info.name.clone())
            .or_insert_with(|| {
                client::Client::new(
                    info.title.clone(),
                    info.spec.clone(),
                    secrets.get(&info.name).cloned(),
                )
            })
            .clone()
    }

    async fn stop(&self, name: &str) {
        self.listed.lock().await.remove(name);
        if let Some(client) = self.clients.lock().await.remove(name) {
            client.stop().await;
        }
    }

    async fn remember(
        &self,
        root: &Path,
        name: &str,
        client: &Arc<client::Client>,
        result: &Result<Vec<Value>>,
    ) -> Result<()> {
        let credential = client.secret().await;
        let _gate = self.gate.lock().await;
        // A late reply from a stopped/replaced client must not replace new tokens.
        if !self
            .clients
            .lock()
            .await
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, client))
        {
            return Ok(());
        }
        let mut index: Index = load(root, "index.json")?;
        let mut secrets: Secrets = load(root, "secrets.json")?;
        if let Some(record) = index.get_mut(name) {
            if let Some(secret) = credential {
                secrets.insert(name.into(), secret);
            }
            match result {
                Ok(tools) => {
                    record.info.tools = summaries(tools, &secrets);
                    record.info.error = None;
                    self.listed.lock().await.insert(name.into());
                }
                Err(error) => record.info.error = Some(redact(&format!("{error:#}"), &secrets)),
            }
            save_both(root, &index, &secrets)?;
        }
        Ok(())
    }

    async fn prepare(
        &self,
        root: &Path,
        add: ConnectorAdd,
        index: &Index,
        signing_in: bool,
    ) -> Result<Prepared> {
        let (
            title,
            description,
            spec,
            suggested,
            read_only,
            sign_in,
            secret,
            client_id,
            downloaded,
        ) = match add {
            ConnectorAdd::Suggested { id } => {
                if index
                    .values()
                    .any(|r| r.info.suggested.as_deref() == Some(&id))
                {
                    bail!("{id} is already added");
                }
                let entry = catalog::entries()?
                    .into_iter()
                    .find(|e| e.id == id)
                    .context("no suggested connector with that id")?;
                if entry.sign_in && !signing_in {
                    return Err(oauth::needed(&entry.title, false));
                }
                let root = root.to_path_buf();
                tokio::task::spawn_blocking(move || -> Result<_> {
                    let (spec, downloaded) = entry.spec(&root)?;
                    Ok((
                        entry.title,
                        entry.description,
                        spec,
                        Some(entry.id),
                        entry.read_only,
                        entry.sign_in,
                        None,
                        None,
                        downloaded,
                    ))
                })
                .await??
            }
            ConnectorAdd::Custom {
                title,
                spec,
                secret,
                client_id,
            } => {
                if title.trim().is_empty() {
                    bail!("a connector needs a title");
                }
                match &spec {
                    ConnectorSpec::Http { url, header } => {
                        oauth::host(url)
                            .context("a connector needs an http or https web address")?;
                        if secret.is_some() && header.as_ref().is_none_or(|h| h.trim().is_empty()) {
                            bail!("a secret needs a header name");
                        }
                    }
                    ConnectorSpec::Command { command, .. } if command.trim().is_empty() => {
                        bail!("a connector needs a command")
                    }
                    _ => {}
                }
                (
                    title.trim().into(),
                    String::new(),
                    spec,
                    None,
                    false,
                    signing_in,
                    secret.map(Secret::Header),
                    client_id,
                    false,
                )
            }
        };
        let base = suggested.clone().unwrap_or_else(|| {
            let name: String = title
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            name.split('-')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        });
        let base = if base.is_empty() {
            "connector".into()
        } else {
            base
        };
        let reserved = if suggested.is_none() {
            catalog::entries()?
                .into_iter()
                .map(|e| e.id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut name = base.clone();
        let mut n = 2;
        while index.contains_key(&name) || reserved.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        Ok(Prepared {
            record: Record {
                info: ConnectorInfo {
                    name,
                    title,
                    description,
                    spec,
                    suggested,
                    read_only,
                    sign_in,
                    enabled: true,
                    added_ms: crate::journal::now_ms(),
                    has_secret: false,
                    tools: Vec::new(),
                    error: None,
                },
                client_id,
            },
            secret,
            downloaded,
        })
    }

    // The caller holds the mutation gate. Failed adds leave no index or tokens.
    async fn install(
        &self,
        root: &Path,
        mut prepared: Prepared,
        index: &mut Index,
        mut secrets: Secrets,
        protected: &[String],
    ) -> Result<Value> {
        let name = prepared.record.info.name.clone();
        let client = client::Client::new(
            prepared.record.info.title.clone(),
            prepared.record.info.spec.clone(),
            prepared.secret,
        );
        client.protect(protected);
        let result = async {
            let tools = client.tools().await?;
            if let Some(secret) = client.secret().await {
                secrets.insert(name.clone(), secret);
            }
            prepared.record.info.tools = summaries(&tools, &secrets);
            index.insert(name.clone(), prepared.record.clone());
            save_both(root, index, &secrets)
        }
        .await;
        if let Err(error) = result {
            client.stop().await;
            if prepared.downloaded
                && let Some(folder) = binary_folder(root, &prepared.record.info)
            {
                let _ = std::fs::remove_dir_all(folder);
            }
            return Err(error);
        }
        self.clients.lock().await.insert(name.clone(), client);
        prepared.record.info.has_secret = matches!(secrets.get(&name), Some(Secret::Header(_)));
        Ok(serde_json::to_value(prepared.record.info)?)
    }

    async fn begin_sign_in(
        &self,
        root: &Path,
        target: SignInTarget,
        redirect_uri: String,
    ) -> Result<Value> {
        oauth::expire(&self.pending).await;
        oauth::validate_redirect(&redirect_uri)?;
        let (url, service, client_id) = {
            let _gate = self.gate.lock().await;
            match &target {
                SignInTarget::Existing { name } => {
                    let index: Index = load(root, "index.json")?;
                    let record = index
                        .get(name)
                        .with_context(|| format!("no connector named {name}"))?;
                    let ConnectorSpec::Http { url, .. } = &record.info.spec else {
                        bail!("only web connectors support browser sign-in");
                    };
                    (
                        url.clone(),
                        record.info.title.clone(),
                        record.client_id.clone(),
                    )
                }
                SignInTarget::Add {
                    add: ConnectorAdd::Suggested { id },
                } => {
                    let entry = catalog::entries()?
                        .into_iter()
                        .find(|e| e.id == *id)
                        .context("no suggested connector with that id")?;
                    (entry.web_address()?.to_owned(), entry.title, None)
                }
                SignInTarget::Add {
                    add:
                        ConnectorAdd::Custom {
                            spec, client_id, ..
                        },
                } => {
                    let ConnectorSpec::Http { url, .. } = spec else {
                        bail!("only web connectors support browser sign-in");
                    };
                    (url.clone(), oauth::host(url)?, client_id.clone())
                }
            }
        };
        let (pending, result) = oauth::begin(target, service, url, redirect_uri, client_id).await?;
        oauth::insert(&self.pending, result.state.clone(), pending).await;
        Ok(serde_json::to_value(result)?)
    }

    async fn finish_sign_in(
        &self,
        root: &Path,
        state: String,
        code: Option<String>,
        error: Option<String>,
    ) -> Result<Value> {
        let pending = oauth::take(&self.pending, &state).await?;
        let mut protected = pending.sensitive();
        if let Some(code) = &code {
            protected.push(code.clone());
        }
        let result = async {
            if let Some(error) = error {
                bail!("the service refused this sign-in: {error}");
            }
            let code = code
                .filter(|s| !s.is_empty())
                .with_context(|| format!("the sign-in to {} returned no code", pending.service))?;
            let tokens = pending.exchange(code).await?;
            protected.extend(
                Secret::OAuth(tokens.clone())
                    .sensitive()
                    .into_iter()
                    .map(str::to_owned),
            );
            let _gate = self.gate.lock().await;
            let mut index: Index = load(root, "index.json")?;
            let mut secrets: Secrets = load(root, "secrets.json")?;
            match pending.target.clone() {
                SignInTarget::Add { add } => {
                    let mut prepared = self.prepare(root, add, &index, true).await?;
                    prepared.secret = Some(Secret::OAuth(tokens));
                    self.install(root, prepared, &mut index, secrets, &protected)
                        .await
                }
                SignInTarget::Existing { name } => {
                    let record = index
                        .get_mut(&name)
                        .with_context(|| format!("no connector named {name}"))?;
                    self.stop(&name).await;
                    let client = client::Client::new(
                        record.info.title.clone(),
                        record.info.spec.clone(),
                        Some(Secret::OAuth(tokens)),
                    );
                    client.protect(&protected);
                    let result = client.tools().await;
                    let tools = match result {
                        Ok(tools) => tools,
                        Err(error) => {
                            client.stop().await;
                            return Err(error);
                        }
                    };
                    if let Some(secret) = client.secret().await {
                        secrets.insert(name.clone(), secret);
                    }
                    record.info.tools = summaries(&tools, &secrets);
                    record.info.sign_in = true;
                    record.info.error = None;
                    record.info.has_secret = false;
                    let info = record.info.clone();
                    if let Err(error) = save_both(root, &index, &secrets) {
                        client.stop().await;
                        return Err(error);
                    }
                    if info.enabled {
                        self.clients.lock().await.insert(name, client);
                    } else {
                        client.stop().await;
                    }
                    Ok(serde_json::to_value(info)?)
                }
            }
        }
        .await;
        let sensitive = protected.iter().map(String::as_str).collect::<Vec<_>>();
        match result {
            Ok(value) => Ok(store::sanitize_values(value, &sensitive)),
            Err(error) => Err(anyhow::anyhow!(store::redact_values(
                &format!("{error:#}"),
                &sensitive
            ))),
        }
    }

    async fn dispatch(&self, root: &Path, call: Call) -> Result<Value> {
        match call {
            Call::BeginConnectorSignIn {
                target,
                redirect_uri,
            } => self.begin_sign_in(root, target, redirect_uri).await,
            Call::FinishConnectorSignIn { state, code, error } => {
                self.finish_sign_in(root, state, code, error).await
            }
            Call::ListConnectors => {
                let _gate = self.gate.lock().await;
                let index: Index = load(root, "index.json")?;
                let secrets: Secrets = load(root, "secrets.json")?;
                let mut added: Vec<_> = index.values().map(|r| r.info.clone()).collect();
                for info in &mut added {
                    info.has_secret = matches!(secrets.get(&info.name), Some(Secret::Header(_)));
                }
                added.sort_by(|a, b| a.title.cmp(&b.title));
                let ids: Vec<_> = index
                    .values()
                    .filter_map(|r| r.info.suggested.clone())
                    .collect();
                let suggested = tokio::task::spawn_blocking(move || -> Result<_> {
                    Ok(catalog::entries()?
                        .iter()
                        .map(|entry| entry.suggested(ids.contains(&entry.id)))
                        .collect())
                })
                .await??;
                Ok(serde_json::to_value(ConnectorList { added, suggested })?)
            }
            Call::AddConnector { add } => {
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                let secrets: Secrets = load(root, "secrets.json")?;
                let prepared = self.prepare(root, add, &index, false).await?;
                self.install(root, prepared, &mut index, secrets, &[]).await
            }
            Call::SetConnectorEnabled { name, enabled } => {
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                index
                    .get_mut(&name)
                    .with_context(|| format!("no connector named {name}"))?
                    .info
                    .enabled = enabled;
                save(root, "index.json", &index)?;
                if !enabled {
                    self.stop(&name).await;
                }
                Ok(Value::Null)
            }
            Call::RemoveConnector { name } => {
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                let mut secrets: Secrets = load(root, "secrets.json")?;
                let record = index
                    .remove(&name)
                    .with_context(|| format!("no connector named {name}"))?;
                self.stop(&name).await;
                secrets.remove(&name);
                save_both(root, &index, &secrets)?;
                if let Some(folder) = binary_folder(root, &record.info)
                    && !index.values().any(|r| match &r.info.spec {
                        ConnectorSpec::Command { command, .. } => {
                            Path::new(command).parent() == Some(folder.as_path())
                        }
                        _ => false,
                    })
                    && folder.exists()
                {
                    std::fs::remove_dir_all(folder)?;
                }
                Ok(Value::Null)
            }
            Call::ExportConnector { name } => {
                let _gate = self.gate.lock().await;
                let index: Index = load(root, "index.json")?;
                let record = index
                    .get(&name)
                    .with_context(|| format!("no connector named {name}"))?;
                Ok(serde_json::to_value(match &record.info.suggested {
                    Some(id) => ConnectorAdd::Suggested { id: id.clone() },
                    None => ConnectorAdd::Custom {
                        title: record.info.title.clone(),
                        spec: record.info.spec.clone(),
                        secret: None,
                        client_id: record.client_id.clone(),
                    },
                })?)
            }
            Call::TestConnector { name } => {
                let (info, client) = {
                    let _gate = self.gate.lock().await;
                    let index: Index = load(root, "index.json")?;
                    let record = index
                        .get(&name)
                        .with_context(|| format!("no connector named {name}"))?;
                    self.stop(&name).await;
                    let secrets = load::<Secrets>(root, "secrets.json")?;
                    (
                        record.info.clone(),
                        self.connection(&record.info, &secrets).await,
                    )
                };
                let result =
                    if info.sign_in && !matches!(client.secret().await, Some(Secret::OAuth(_))) {
                        Err(oauth::needed(&info.title, true))
                    } else {
                        client.tools().await
                    };
                self.remember(root, &name, &client, &result).await?;
                let _gate = self.gate.lock().await;
                let index: Index = load(root, "index.json")?;
                let secrets: Secrets = load(root, "secrets.json")?;
                let mut info = index
                    .get(&name)
                    .context("connector was removed")?
                    .info
                    .clone();
                info.has_secret = matches!(secrets.get(&name), Some(Secret::Header(_)));
                if !info.enabled {
                    self.stop(&name).await;
                }
                // Ordinary test failures are rows; sign-in failures ask the app to sign in.
                if let Some(error) = &info.error
                    && error.starts_with(sandhill_proto::CONNECTOR_SIGN_IN)
                {
                    bail!("{error}");
                }
                Ok(serde_json::to_value(info)?)
            }
            Call::ConnectorTools { name: None } => {
                let names = {
                    let _gate = self.gate.lock().await;
                    load::<Index>(root, "index.json")?
                        .into_values()
                        .filter(|r| r.info.enabled)
                        .map(|r| r.info.name)
                        .collect::<Vec<_>>()
                };
                let mut connectors = Vec::new();
                for name in names {
                    let (mut info, client) = self.enabled(root, &name).await?;
                    if info.tools.is_empty() && !self.listed.lock().await.contains(&name) {
                        let result = client.tools().await;
                        self.remember(root, &name, &client, &result).await?;
                        info.tools = summaries(&result?, &load(root, "secrets.json")?);
                    }
                    connectors.push(json!({"name": info.name, "title": info.title, "description": info.description, "read_only": info.read_only,
                        "tools": info.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "read_only": t.read_only})).collect::<Vec<_>>() }));
                }
                Ok(json!({"connectors": connectors}))
            }
            Call::ConnectorTools { name: Some(name) } => {
                let (info, client) = self.enabled(root, &name).await?;
                let result = client.tools().await;
                self.remember(root, &name, &client, &result).await?;
                let tools = result?.into_iter().map(|tool| json!({"name": tool["name"], "description": tool["description"], "inputSchema": tool["inputSchema"], "read_only": tool["annotations"]["readOnlyHint"].as_bool().unwrap_or(false)})).collect::<Vec<_>>();
                Ok(
                    json!({"name": info.name, "title": info.title, "read_only": info.read_only, "tools": tools}),
                )
            }
            Call::CallConnector {
                name,
                tool,
                arguments,
                read_only,
            } => {
                let (mut info, client) = self.enabled(root, &name).await?;
                if read_only
                    && info.suggested.is_some()
                    && !info.read_only
                    && !info.tools.iter().any(|t| t.name == tool)
                {
                    let result = client.tools().await;
                    self.remember(root, &name, &client, &result).await?;
                    info.tools = summaries(&result?, &load(root, "secrets.json")?);
                }
                if read_only && !can_query(&info, &tool) {
                    bail!(
                        "this tool is not marked read_only; use call_connector instead of query_connector"
                    );
                }
                let result = client.call(&tool, arguments).await;
                let credential = client.secret().await;
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                let mut secrets: Secrets = load(root, "secrets.json")?;
                if self
                    .clients
                    .lock()
                    .await
                    .get(&name)
                    .is_some_and(|current| Arc::ptr_eq(current, &client))
                    && let Some(record) = index.get_mut(&name)
                {
                    if let Some(secret) = credential {
                        secrets.insert(name.clone(), secret);
                    }
                    record.info.error = match &result {
                        Err(error) => Some(redact(&format!("{error:#}"), &secrets)),
                        Ok(value) if value["isError"] == true => {
                            Some("The connector tool returned an error".into())
                        }
                        _ => None,
                    };
                    save_both(root, &index, &secrets)?;
                }
                result
            }
            _ => bail!("not a connector call"),
        }
    }

    async fn enabled(
        &self,
        root: &Path,
        name: &str,
    ) -> Result<(ConnectorInfo, Arc<client::Client>)> {
        let _gate = self.gate.lock().await;
        let mut index: Index = load(root, "index.json")?;
        let record = index
            .get_mut(name)
            .filter(|r| r.info.enabled)
            .ok_or_else(|| unknown(name))?;
        let secrets: Secrets = load(root, "secrets.json")?;
        if record.info.sign_in && !matches!(secrets.get(name), Some(Secret::OAuth(_))) {
            let error = oauth::needed(&record.info.title, true);
            record.info.error = Some(error.to_string());
            save(root, "index.json", &index)?;
            return Err(error);
        }
        let client = self.connection(&record.info, &secrets).await;
        Ok((record.info.clone(), client))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sandhill_proto::ConnectorTool;

    #[test]
    fn queries_require_a_read_only_connector_or_a_suggested_read_only_tool() {
        let mut info: ConnectorInfo = serde_json::from_value(json!({"name":"fixture", "title":"Fixture", "description":"", "spec":{"kind":"http", "url":"https://example.org/mcp"}, "enabled":true, "added_ms":0})).unwrap();
        info.tools.push(ConnectorTool {
            name: "read".into(),
            description: String::new(),
            read_only: true,
        });
        assert!(!can_query(&info, "read"));
        info.suggested = Some("fixture".into());
        info.sign_in = true;
        assert!(can_query(&info, "read"));
        assert!(!can_query(&info, "write"));
        info.tools.push(ConnectorTool {
            name: "write".into(),
            description: String::new(),
            read_only: false,
        });
        assert!(!can_query(&info, "write"));
        info.read_only = true;
        assert!(can_query(&info, "write"));
    }
}
