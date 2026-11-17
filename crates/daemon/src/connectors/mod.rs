//! Device-local MCP connections, shared by every conversation (Huan, 2026-11-16).
mod catalog;
mod client;
mod programs;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sandhill_proto::{
    Call, ConnectorAdd, ConnectorInfo, ConnectorList, ConnectorSpec, ConnectorTool,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::sync::Mutex;

type Index = BTreeMap<String, ConnectorInfo>;
type Secrets = BTreeMap<String, String>;

#[derive(Default)]
pub struct Connectors {
    gate: Mutex<()>,
    clients: Mutex<HashMap<String, Arc<client::Client>>>,
    listed: Mutex<HashSet<String>>,
}

fn load<T: DeserializeOwned + Default>(root: &Path, file: &str) -> Result<T> {
    match std::fs::read(root.join("connectors").join(file)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

fn save<T: Serialize>(root: &Path, file: &str, value: &T) -> Result<()> {
    let folder = root.join("connectors");
    std::fs::create_dir_all(&folder)?;
    let temp = folder.join(format!(".{file}-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        // Secrets are private from the first open, including temporary files.
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        serde_json::to_writer_pretty(&mut output, value)?;
        output.write_all(b"\n")?;
        output.sync_all()?;
        std::fs::rename(&temp, folder.join(file))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

fn save_both(root: &Path, index: &Index, secrets: &Secrets) -> Result<()> {
    let previous: Secrets = load(root, "secrets.json")?;
    save(root, "secrets.json", secrets)?;
    if let Err(error) = save(root, "index.json", index) {
        save(root, "secrets.json", &previous)?;
        return Err(error);
    }
    Ok(())
}

fn redact(text: &str, secrets: &Secrets) -> String {
    let mut text = text.to_owned();
    for secret in secrets.values().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    text
}

fn sanitized(value: Value, secrets: &Secrets) -> Value {
    match value {
        Value::String(text) => Value::String(redact(&text, secrets)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| sanitized(v, secrets)).collect())
        }
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(k, v)| (redact(&k, secrets), sanitized(v, secrets)))
                .collect(),
        ),
        other => other,
    }
}

fn summaries(tools: &[Value], secrets: &Secrets) -> Vec<ConnectorTool> {
    tools
        .iter()
        .filter_map(|tool| {
            Some(ConnectorTool {
                name: redact(tool["name"].as_str()?, secrets),
                description: redact(
                    tool["description"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or(""),
                    secrets,
                ),
                read_only: tool["annotations"]["readOnlyHint"]
                    .as_bool()
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn unknown(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no connector named {name} that is on (list_connectors lists them)")
}

fn binary_folder(root: &Path, info: &ConnectorInfo) -> Option<PathBuf> {
    let ConnectorSpec::Command { command, .. } = &info.spec else {
        return None;
    };
    let path = Path::new(command);
    let parent = path.parent()?;
    let bin = std::fs::canonicalize(root.join("connectors/bin")).ok()?;
    (info.suggested.as_deref() == Some("biomcp") && parent.parent() == Some(bin.as_path()))
        .then(|| parent.to_path_buf())
}

impl Connectors {
    pub(crate) async fn call(&self, root: &Path, call: Call) -> Result<Value> {
        let mut secrets: Secrets = load(root, "secrets.json")?;
        if let Call::AddConnector {
            add:
                ConnectorAdd::Custom {
                    secret: Some(secret),
                    ..
                },
        } = &call
        {
            secrets.insert(String::new(), secret.clone());
        }
        match self.dispatch(root, call).await {
            Ok(value) => Ok(sanitized(value, &secrets)),
            Err(error) => Err(anyhow::anyhow!(redact(&format!("{error:#}"), &secrets))),
        }
    }

    async fn connection(&self, info: &ConnectorInfo, secrets: &Secrets) -> Arc<client::Client> {
        self.clients
            .lock()
            .await
            .entry(info.name.clone())
            .or_insert_with(|| {
                client::Client::new(info.spec.clone(), secrets.get(&info.name).cloned())
            })
            .clone()
    }

    async fn stop(&self, name: &str) {
        self.listed.lock().await.remove(name);
        if let Some(client) = self.clients.lock().await.remove(name) {
            client.stop().await;
        }
    }

    async fn refresh(&self, root: &Path, name: &str, result: &Result<Vec<Value>>) -> Result<()> {
        let _gate = self.gate.lock().await;
        let mut index: Index = load(root, "index.json")?;
        let secrets: Secrets = load(root, "secrets.json")?;
        if let Some(info) = index.get_mut(name) {
            match result {
                Ok(tools) => {
                    info.tools = summaries(tools, &secrets);
                    info.error = None;
                    self.listed.lock().await.insert(name.to_owned());
                }
                Err(error) => info.error = Some(redact(&format!("{error:#}"), &secrets)),
            }
            save(root, "index.json", &index)?;
        }
        Ok(())
    }

    async fn dispatch(&self, root: &Path, call: Call) -> Result<Value> {
        match call {
            Call::ListConnectors => {
                let _gate = self.gate.lock().await;
                let index: Index = load(root, "index.json")?;
                let secrets: Secrets = load(root, "secrets.json")?;
                let mut added: Vec<_> = index.values().cloned().collect();
                for info in &mut added {
                    info.has_secret = secrets.contains_key(&info.name);
                }
                added.sort_by(|a, b| a.title.cmp(&b.title));
                let ids: Vec<_> = index.values().filter_map(|i| i.suggested.clone()).collect();
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
                let mut secrets: Secrets = load(root, "secrets.json")?;
                let (title, description, spec, suggested, read_only, secret, downloaded) = match add
                {
                    ConnectorAdd::Suggested { id } => {
                        if index.values().any(|i| i.suggested.as_deref() == Some(&id)) {
                            bail!("{id} is already added");
                        }
                        let entry = catalog::entries()?
                            .into_iter()
                            .find(|e| e.id == id)
                            .context("no suggested connector with that id")?;
                        let root = root.to_path_buf();
                        tokio::task::spawn_blocking(move || -> Result<_> {
                            let (spec, downloaded) = entry.spec(&root)?;
                            Ok((
                                entry.title,
                                entry.description,
                                spec,
                                Some(entry.id),
                                entry.read_only,
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
                    } => {
                        if title.trim().is_empty() {
                            bail!("a connector needs a title");
                        }
                        match &spec {
                            ConnectorSpec::Http { url, header } => {
                                if !url.starts_with("https://") && !url.starts_with("http://") {
                                    bail!("a connector needs an http or https web address");
                                }
                                if secret.is_some()
                                    && header.as_ref().is_none_or(|h| h.trim().is_empty())
                                {
                                    bail!("a secret needs a header name");
                                }
                            }
                            ConnectorSpec::Command { command, .. } if command.trim().is_empty() => {
                                bail!("a connector needs a command")
                            }
                            _ => {}
                        }
                        (
                            title.trim().to_owned(),
                            String::new(),
                            spec,
                            None,
                            false,
                            secret,
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
                let mut name = base.clone();
                let mut n = 2;
                let reserved = if suggested.is_none() {
                    catalog::entries()?
                        .into_iter()
                        .map(|e| e.id)
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                while index.contains_key(&name) || reserved.contains(&name) {
                    name = format!("{base}-{n}");
                    n += 1;
                }
                let mut info = ConnectorInfo {
                    name: name.clone(),
                    title,
                    description,
                    spec,
                    suggested,
                    read_only,
                    enabled: true,
                    added_ms: crate::journal::now_ms(),
                    has_secret: false,
                    tools: Vec::new(),
                    error: None,
                };
                if let Some(secret) = secret {
                    secrets.insert(name.clone(), secret);
                }
                let client = client::Client::new(info.spec.clone(), secrets.get(&name).cloned());
                let result = client.tools().await;
                let result = match result {
                    Ok(tools) => {
                        info.tools = summaries(&tools, &secrets);
                        // Derived has_secret is not persisted.
                        index.insert(name.clone(), info.clone());
                        save_both(root, &index, &secrets)
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    client.stop().await;
                    if downloaded && let Some(folder) = binary_folder(root, &info) {
                        let _ = std::fs::remove_dir_all(folder);
                    }
                    return Err(error);
                }
                self.clients.lock().await.insert(name.clone(), client);
                info.has_secret = secrets.contains_key(&name);
                Ok(serde_json::to_value(info)?)
            }
            Call::SetConnectorEnabled { name, enabled } => {
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                index
                    .get_mut(&name)
                    .with_context(|| format!("no connector named {name}"))?
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
                let info = index
                    .remove(&name)
                    .with_context(|| format!("no connector named {name}"))?;
                self.stop(&name).await;
                secrets.remove(&name);
                save_both(root, &index, &secrets)?;
                if let Some(folder) = binary_folder(root, &info)
                    && !index.values().any(|i| match &i.spec {
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
                let info = index
                    .get(&name)
                    .with_context(|| format!("no connector named {name}"))?;
                Ok(serde_json::to_value(match &info.suggested {
                    Some(id) => ConnectorAdd::Suggested { id: id.clone() },
                    None => ConnectorAdd::Custom {
                        title: info.title.clone(),
                        spec: info.spec.clone(),
                        secret: None,
                    },
                })?)
            }
            Call::TestConnector { name } => {
                let (client, secrets) = {
                    let _gate = self.gate.lock().await;
                    let index: Index = load(root, "index.json")?;
                    let info = index
                        .get(&name)
                        .cloned()
                        .with_context(|| format!("no connector named {name}"))?;
                    self.stop(&name).await;
                    let secrets = load::<Secrets>(root, "secrets.json")?;
                    (self.connection(&info, &secrets).await, secrets)
                };
                let result = client.tools().await;
                self.refresh(root, &name, &result).await?;
                let _gate = self.gate.lock().await;
                let index: Index = load(root, "index.json")?;
                let mut info = index.get(&name).cloned().context("connector was removed")?;
                info.has_secret = secrets.contains_key(&name);
                if !info.enabled {
                    self.stop(&name).await;
                }
                Ok(serde_json::to_value(info)?)
            }
            Call::ConnectorTools { name: None } => {
                let names = {
                    let _gate = self.gate.lock().await;
                    load::<Index>(root, "index.json")?
                        .into_values()
                        .filter(|i| i.enabled)
                        .map(|i| i.name)
                        .collect::<Vec<_>>()
                };
                let mut connectors = Vec::new();
                for name in names {
                    let (mut info, client) = self.enabled(root, &name).await?;
                    if info.tools.is_empty() && !self.listed.lock().await.contains(&name) {
                        let result = client.tools().await;
                        self.refresh(root, &name, &result).await?;
                        info.tools = summaries(&result?, &load(root, "secrets.json")?);
                    }
                    connectors.push(json!({"name": info.name, "title": info.title, "description": info.description, "read_only": info.read_only,
                        "tools": info.tools.iter().map(|t| json!({"name": t.name, "description": t.description})).collect::<Vec<_>>() }));
                }
                Ok(json!({"connectors": connectors}))
            }
            Call::ConnectorTools { name: Some(name) } => {
                let (info, client) = self.enabled(root, &name).await?;
                let result = client.tools().await;
                self.refresh(root, &name, &result).await?;
                let tools = result?.into_iter().map(|tool| json!({"name": tool["name"], "description": tool["description"], "inputSchema": tool["inputSchema"]})).collect::<Vec<_>>();
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
                let (info, client) = self.enabled(root, &name).await?;
                if read_only && !info.read_only {
                    bail!(
                        "this connector is not marked read_only; use call_connector instead of query_connector"
                    );
                }
                let result = client.call(&tool, arguments).await;
                let _gate = self.gate.lock().await;
                let mut index: Index = load(root, "index.json")?;
                let secrets: Secrets = load(root, "secrets.json")?;
                if let Some(info) = index.get_mut(&name) {
                    info.error = match &result {
                        Err(error) => Some(redact(&format!("{error:#}"), &secrets)),
                        Ok(value) if value["isError"] == true => {
                            Some("The connector tool returned an error".into())
                        }
                        _ => None,
                    };
                    save(root, "index.json", &index)?;
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
        let index: Index = load(root, "index.json")?;
        let info = index
            .get(name)
            .filter(|i| i.enabled)
            .cloned()
            .ok_or_else(|| unknown(name))?;
        let secrets: Secrets = load(root, "secrets.json")?;
        let client = self.connection(&info, &secrets).await;
        Ok((info, client))
    }
}
