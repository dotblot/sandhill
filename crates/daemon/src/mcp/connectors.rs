use anyhow::{Context, Result, bail};
use sandhill_proto::Call;
use serde_json::{Value, json};

use super::{Tools, text_item};

pub(crate) const GUIDANCE: &str = "Databases and services the user connected in Sandhill are listed by list_connectors; prefer them to scraping websites. Use query_connector for connectors marked read_only and call_connector for the rest.";

impl Tools {
    pub(super) async fn connector_result(&self, name: &str, args: &Value) -> Result<Value> {
        let required = |key: &str| -> Result<String> {
            args[key]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .with_context(|| format!("`{key}` is required"))
        };
        if name == "call_connector"
            && let Some(dir) = self.terminal_dir.as_deref()
        {
            let report = crate::terminal::choices::report(dir);
            if report["mode"] == "plan" || report["permission"] == "plan" {
                bail!(
                    "Plan mode does not allow call_connector; switch modes to call this connector"
                );
            }
        }
        let call = if name == "list_connectors" {
            let name = match args.get("connector") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
                _ => bail!("`connector` must be a nonempty string"),
            };
            Call::ConnectorTools { name }
        } else {
            let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if !arguments.is_object() {
                bail!("`arguments` must be an object");
            }
            Call::CallConnector {
                name: required("connector")?,
                tool: required("tool")?,
                arguments,
                read_only: name == "query_connector",
            }
        };
        let result = self.client().await?.call::<Value>(call).await.map_err(|error| {
            let error = anyhow::Error::from(error);
            if error.to_string().contains("unknown variant") || error.to_string().contains("method not found") {
                error.context("this daemon has no connectors; update Sandhill on this device and restart this conversation")
            } else { error }
        })?;
        if name == "list_connectors" {
            Ok(json!({"content": [text_item(&serde_json::to_string(&result)?)], "isError": false}))
        } else {
            // Preserve text/image items and the upstream tool-error flag.
            Ok(
                json!({"content": result["content"].as_array().context("connector returned no content items")?,
                "isError": result["isError"].as_bool().unwrap_or(false)}),
            )
        }
    }

    pub(super) async fn connector_tool(&self, name: &str, args: &Value) -> Result<Vec<Value>> {
        let result = self.connector_result(name, args).await?;
        if result["isError"] == true {
            bail!("{}", serde_json::to_string(&result["content"])?);
        }
        Ok(result["content"].as_array().unwrap().clone())
    }
}

pub(super) fn tool_list() -> Vec<Value> {
    let mut tools = vec![json!({
        "name": "list_connectors", "title": "List connected databases and services",
        "description": "List enabled connectors with tool names and short descriptions. Give connector to fetch that connector's full tool descriptions and input schemas before calling one. Prefer connected databases to scraping websites.",
        "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
            "connector": {"type": "string", "description": "An enabled connector name; omit to list connectors."}
        }}, "annotations": {"readOnlyHint": true}
    })];
    for (name, read_only, description) in [
        (
            "query_connector",
            true,
            "Query a connector marked read_only in list_connectors. Read its tool's input schema with list_connectors {connector} first. Custom connectors require call_connector instead.",
        ),
        (
            "call_connector",
            false,
            "Call a tool on a connector the user added. May change data or use an external service. Read its input schema with list_connectors {connector} first. Not allowed in Plan mode.",
        ),
    ] {
        tools.push(json!({"name": name, "description": description,
            "inputSchema": {"type": "object", "additionalProperties": false, "required": ["connector", "tool", "arguments"], "properties": {
                "connector": {"type": "string"}, "tool": {"type": "string"}, "arguments": {"type": "object"}
            }}, "annotations": {"readOnlyHint": read_only}}));
    }
    tools
}
