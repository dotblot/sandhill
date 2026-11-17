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
    /// there as only looking things up. (Other connectors from the list may
    /// still be queried tool by tool, where the service marks the tool
    /// read-only.)
    #[serde(default)]
    pub read_only: bool,
    /// Reached with a browser sign-in (OAuth) to the user's own account.
    #[serde(default)]
    pub sign_in: bool,
    pub enabled: bool,
    pub added_ms: i64,
    /// A header value is kept for it.
    #[serde(default)]
    pub has_secret: bool,
    /// As of the last time it answered.
    #[serde(default)]
    pub tools: Vec<ConnectorTool>,
    /// Why it last failed, cleared when it answers again. Starts with
    /// [`CONNECTOR_SIGN_IN`] when it needs signing in again.
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
    /// Adding it signs in to the user's own account in the browser.
    #[serde(default)]
    pub sign_in: bool,
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
    /// the value of the spec's `header`. `client_id` is for a sign-in
    /// service that doesn't let Sandhill register itself and gave the user
    /// an id.
    Custom {
        title: String,
        spec: ConnectorSpec,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
}

/// [`crate::Call::AddConnector`] and [`crate::Call::TestConnector`] errors,
/// and [`ConnectorInfo::error`], start with this when the service needs a
/// browser sign-in: the app then runs [`crate::Call::BeginConnectorSignIn`].
pub const CONNECTOR_SIGN_IN: &str = "Sign-in needed:";

/// What a sign-in is for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignInTarget {
    /// Adds the connector once signed in.
    Add { add: ConnectorAdd },
    /// Signs an added connector in again.
    Existing { name: String },
}

/// The answer to [`crate::Call::BeginConnectorSignIn`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectorSignIn {
    /// Names this sign-in in [`crate::Call::FinishConnectorSignIn`]; it is
    /// also OAuth's `state`, which comes back on the redirect.
    pub state: String,
    /// The page to open in the browser.
    pub authorize_url: String,
    /// The service's name for the card: its title, or the address's host.
    pub service: String,
}
