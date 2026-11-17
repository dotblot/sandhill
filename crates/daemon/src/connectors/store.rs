use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use anyhow::Result;
use sandhill_proto::{ConnectorInfo, ConnectorTool};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::oauth::Tokens;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    #[serde(flatten)]
    pub info: ConnectorInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

// Keep the original string representation for user-supplied header values.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Secret {
    Header(String),
    OAuth(Tokens),
}

impl Secret {
    pub fn sensitive(&self) -> Vec<&str> {
        match self {
            Self::Header(value) => vec![value],
            Self::OAuth(tokens) => std::iter::once(tokens.access_token.as_str())
                .chain(tokens.refresh_token.as_deref())
                .chain(tokens.client_secret.as_deref())
                .collect(),
        }
    }
}

pub(super) type Index = BTreeMap<String, Record>;
pub(super) type Secrets = BTreeMap<String, Secret>;

pub(super) fn load<T: DeserializeOwned + Default>(root: &Path, file: &str) -> Result<T> {
    match std::fs::read(root.join("connectors").join(file)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn save<T: Serialize>(root: &Path, file: &str, value: &T) -> Result<()> {
    let folder = root.join("connectors");
    std::fs::create_dir_all(&folder)?;
    let temp = folder.join(format!(".{file}-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
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

pub(super) fn save_both(root: &Path, index: &Index, secrets: &Secrets) -> Result<()> {
    let previous: Secrets = load(root, "secrets.json")?;
    save(root, "secrets.json", secrets)?;
    if let Err(error) = save(root, "index.json", index) {
        save(root, "secrets.json", &previous)?;
        return Err(error);
    }
    Ok(())
}

pub(super) fn redact(text: &str, secrets: &Secrets) -> String {
    let values = secrets
        .values()
        .flat_map(Secret::sensitive)
        .collect::<Vec<_>>();
    redact_values(text, &values)
}

pub(super) fn redact_values(text: &str, secrets: &[&str]) -> String {
    let mut text = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    text
}

pub(super) fn sanitize_values(value: Value, secrets: &[&str]) -> Value {
    match value {
        Value::String(text) => Value::String(redact_values(&text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| sanitize_values(v, secrets))
                .collect(),
        ),
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(k, v)| (redact_values(&k, secrets), sanitize_values(v, secrets)))
                .collect(),
        ),
        other => other,
    }
}

pub(super) fn summaries(tools: &[Value], secrets: &Secrets) -> Vec<ConnectorTool> {
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
