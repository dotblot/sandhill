//! Connectors (MCP servers) added on a device for every agent (Huan,
//! 2026-11-16; `docs/design/done/connectors-plan.md`). The daemon holds the
//! connections; agents reach them through Sandhill's own tool server
//! (`list_connectors`, `query_connector`, `call_connector`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How a connector is reached.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorSpec {
    /// A web address speaking MCP's Streamable HTTP. `header` names a
    /// header (such as `Authorization` or `X-API-Key`) whose value is kept
    /// as a secret on the device, never sent back.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        header: Option<String>,
    },
    /// A program speaking MCP on stdin and stdout, run as the user on the
    /// device.
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

/// One tool a connector offers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorTool {
    pub name: String,
    /// Its description's first line.
    pub description: String,
    /// The server marks it read-only (`readOnlyHint`).
    #[serde(default)]
    pub read_only: bool,
}

/// A connector added on the device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorInfo {
    /// Its id, unique on the device.
    pub name: String,
    pub title: String,
    pub description: String,
    pub spec: ConnectorSpec,
    /// The entry of Sandhill's list it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested: Option<String>,
    /// Agents may query it without asking: from Sandhill's list and marked
    /// there as only looking things up.
    #[serde(default)]
    pub read_only: bool,
    pub enabled: bool,
    pub added_ms: i64,
    /// A header value is kept for it.
    #[serde(default)]
    pub has_secret: bool,
    /// As of the last time it answered.
    #[serde(default)]
    pub tools: Vec<ConnectorTool>,
    /// Why it last failed, cleared when it answers again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// An entry of Sandhill's list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestedConnector {
    pub id: String,
    pub title: String,
    pub description: String,
    /// Who makes the server.
    pub by: String,
    pub licence: String,
    pub homepage: String,
    /// The page with the databases' own terms.
    pub terms: String,
    /// What Add does, for the card: "Downloads BioMCP 0.9.3 from
    /// github.com/genomoncology/biomcp and runs it on this device", "Runs
    /// `uvx …` on this device", "Calls https://…".
    pub action: String,
    /// Missing on the device and needed (e.g. `uv`), with where to get it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_url: Option<String>,
    pub added: bool,
}

/// The answer to [`crate::Call::ListConnectors`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConnectorList {
    /// By title.
    pub added: Vec<ConnectorInfo>,
    /// In the list's order, added ones too (`added`).
    pub suggested: Vec<SuggestedConnector>,
}

/// What to add with [`crate::Call::AddConnector`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorAdd {
    /// An entry of Sandhill's list.
    Suggested { id: String },
    /// One the user describes. `name` is made safe and unique; `secret` is
    /// the value of the spec's `header`.
    Custom {
        title: String,
        spec: ConnectorSpec,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
    },
}
