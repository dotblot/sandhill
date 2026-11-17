//! Browser sign-in stays on the device; only a challenge and state leave it.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sandhill_proto::{CONNECTOR_SIGN_IN, ConnectorSignIn, SignInTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use ureq::http::Uri;

use super::client::timeout;

const LIFETIME: Duration = Duration::from_secs(15 * 60);
pub(super) type PendingSignIns = Arc<Mutex<HashMap<String, Pending>>>;

#[derive(Clone)]
pub(super) struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    resource: String,
    scope: Option<String>,
}

pub(super) struct Pending {
    pub target: SignInTarget,
    pub service: String,
    created: Instant,
    redirect_uri: String,
    verifier: String,
    discovery: Discovery,
    client_id: String,
    client_secret: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Tokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    pub resource: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub(super) fn needed(service: &str, again: bool) -> anyhow::Error {
    if again {
        anyhow::anyhow!("{CONNECTOR_SIGN_IN} sign in to {service} again.")
    } else {
        anyhow::anyhow!("{CONNECTOR_SIGN_IN} {service} asks you to sign in.")
    }
}

fn address(value: &str) -> Result<Uri> {
    let uri: Uri = value
        .parse()
        .map_err(|_| anyhow::anyhow!("the service supplied an invalid sign-in address"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.host().is_none()
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
        || value.contains('#')
    {
        bail!("the service supplied an invalid sign-in address");
    }
    Ok(uri)
}

pub(super) fn host(url: &str) -> Result<String> {
    Ok(address(url)?.host().unwrap().to_owned())
}

pub(super) fn validate_redirect(value: &str) -> Result<()> {
    let uri = address(value).map_err(|_| {
        anyhow::anyhow!("sign-in needs an http loopback redirect address with a port")
    })?;
    if uri.scheme_str() != Some("http")
        || !matches!(uri.host(), Some("127.0.0.1" | "localhost"))
        || uri.port_u16().is_none_or(|p| p == 0)
    {
        bail!("sign-in needs an http loopback redirect address with a port");
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout()))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn document(url: &str) -> Result<Value> {
    address(url)?;
    let mut response = agent()
        .get(url)
        .call()
        .map_err(|_| anyhow::anyhow!("could not read the service's sign-in description"))?;
    if !response.status().is_success() {
        bail!("could not read the service's sign-in description");
    }
    serde_json::from_reader(response.body_mut().as_reader())
        .map_err(|_| anyhow::anyhow!("the service returned an invalid sign-in description"))
}

// WWW-Authenticate parameters may be quoted, contain commas or escape quotes.
fn parameter(header: &str, wanted: &str) -> Option<String> {
    let bytes = header.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b',') {
            at += 1;
        }
        let start = at;
        while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
            at += 1;
        }
        let key = &bytes[start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if bytes.get(at) != Some(&b'=') {
            if at == start {
                at += 1;
            }
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = Vec::new();
        if bytes.get(at) == Some(&b'"') {
            at += 1;
            while at < bytes.len() && bytes[at] != b'"' {
                if bytes[at] == b'\\' {
                    at += 1;
                }
                if at < bytes.len() {
                    value.push(bytes[at]);
                    at += 1;
                }
            }
            at += usize::from(at < bytes.len());
        } else {
            while at < bytes.len() && bytes[at] != b',' && !bytes[at].is_ascii_whitespace() {
                value.push(bytes[at]);
                at += 1;
            }
        }
        if key.eq_ignore_ascii_case(wanted.as_bytes()) {
            return String::from_utf8(value).ok().filter(|v| !v.is_empty());
        }
    }
    None
}

fn locations(issuer: &str, name: &str) -> Result<Vec<String>> {
    let uri = address(issuer)?;
    let origin = format!(
        "{}://{}",
        uri.scheme_str().unwrap(),
        uri.authority().unwrap()
    );
    let path = uri.path().trim_end_matches('/');
    Ok(vec![
        format!("{origin}/.well-known/{name}{path}"),
        format!("{origin}{path}/.well-known/{name}"),
    ])
}

fn scope(document: &Value) -> Option<String> {
    document["scopes_supported"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
}

fn metadata(value: &Value, resource: &str, requested_scope: Option<String>) -> Option<Discovery> {
    let authorization_endpoint = value["authorization_endpoint"].as_str()?;
    let token_endpoint = value["token_endpoint"].as_str()?;
    address(authorization_endpoint).ok()?;
    address(token_endpoint).ok()?;
    let registration_endpoint = value["registration_endpoint"].as_str().map(str::to_owned);
    if let Some(url) = &registration_endpoint {
        address(url).ok()?;
    }
    Some(Discovery {
        authorization_endpoint: authorization_endpoint.into(),
        token_endpoint: token_endpoint.into(),
        registration_endpoint,
        resource: resource.into(),
        scope: requested_scope.or_else(|| scope(value)),
    })
}

pub(super) fn discover(url: &str, authenticate: Option<&str>) -> Option<Discovery> {
    let mut candidates = Vec::new();
    if let Some(metadata) = authenticate.and_then(|h| parameter(h, "resource_metadata")) {
        candidates.push(metadata);
    }
    let uri = address(url).ok()?;
    let origin = format!("{}://{}", uri.scheme_str()?, uri.authority()?);
    candidates.push(format!(
        "{origin}/.well-known/oauth-protected-resource{}",
        uri.path().trim_end_matches('/')
    ));
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    for candidate in candidates {
        let Ok(protected) = document(&candidate) else {
            continue;
        };
        let resource = protected["resource"].as_str().unwrap_or(url);
        let requested_scope = authenticate
            .and_then(|h| parameter(h, "scope"))
            .or_else(|| scope(&protected));
        // BioRender points resource_metadata directly at its AS description.
        if let Some(description) = metadata(&protected, resource, requested_scope.clone()) {
            return Some(description);
        }
        let Some(issuer) = protected["authorization_servers"][0].as_str() else {
            continue;
        };
        let Ok(mut places) = locations(issuer, "oauth-authorization-server") else {
            continue;
        };
        if let Ok(mut oidc) = locations(issuer, "openid-configuration") {
            oidc.reverse(); // OIDC appends its suffix to an issuer path.
            places.extend(oidc);
        }
        places.dedup();
        for place in places {
            if let Ok(value) = document(&place) {
                if value["issuer"]
                    .as_str()
                    .is_some_and(|actual| actual != issuer)
                {
                    continue;
                }
                if let Some(description) = metadata(&value, resource, requested_scope.clone()) {
                    return Some(description);
                }
            }
        }
    }
    None
}

fn probe(url: &str) -> Result<Discovery> {
    address(url)?;
    let response = agent().post(url).header("Accept", "application/json, text/event-stream").header("Content-Type", "application/json")
        .send(serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "sandhill", "version": env!("CARGO_PKG_VERSION")}
        }}))?).map_err(|_| anyhow::anyhow!("could not reach the service to sign in"))?;
    let header = response
        .headers()
        .get("WWW-Authenticate")
        .and_then(|v| v.to_str().ok());
    discover(url, header).context("this address does not describe a browser sign-in")
}

pub(super) fn encode(value: &str) -> String {
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(byte as char);
        } else {
            output.push('%');
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 15) as usize] as char);
        }
    }
    output
}

pub(super) async fn begin(
    target: SignInTarget,
    service: String,
    url: String,
    redirect_uri: String,
    typed_id: Option<String>,
) -> Result<(Pending, ConnectorSignIn)> {
    validate_redirect(&redirect_uri)?;
    tokio::task::spawn_blocking(move || {
        let discovery = probe(&url)?;
        let (client_id, client_secret) = if let Some(id) = typed_id.filter(|s| !s.trim().is_empty()) { (id, None) }
        else if let Some(endpoint) = &discovery.registration_endpoint {
            let mut response = agent().post(endpoint).header("Content-Type", "application/json").send(serde_json::to_vec(&json!({
                "client_name": "Sandhill", "redirect_uris": [&redirect_uri], "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"], "token_endpoint_auth_method": "none"
            }))?).map_err(|_| anyhow::anyhow!("could not register Sandhill with this service"))?;
            if !response.status().is_success() { bail!("the service did not let Sandhill register for sign-in"); }
            let registered: Value = serde_json::from_reader(response.body_mut().as_reader()).map_err(|_| anyhow::anyhow!("the service returned an invalid client registration"))?;
            (registered["client_id"].as_str().filter(|id| !id.is_empty()).context("the service returned no client id")?.to_owned(),
                registered["client_secret"].as_str().map(str::to_owned))
        } else { bail!("this service needs a client id; ask its makers for one and add it as a web address with that id"); };
        let mut random = Vec::with_capacity(48);
        for _ in 0..3 { random.extend_from_slice(uuid::Uuid::new_v4().as_bytes()); }
        let verifier = URL_SAFE_NO_PAD.encode(random);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = uuid::Uuid::new_v4().to_string();
        let mut parameters = vec![("response_type", "code"), ("client_id", &client_id), ("redirect_uri", &redirect_uri),
            ("code_challenge", &challenge), ("code_challenge_method", "S256"), ("state", &state), ("resource", &discovery.resource)];
        if let Some(scope) = &discovery.scope { parameters.push(("scope", scope)); }
        let query = parameters.into_iter().map(|(k, v)| format!("{k}={}", encode(v))).collect::<Vec<_>>().join("&");
        let separator = if discovery.authorization_endpoint.contains('?') { '&' } else { '?' };
        let authorize_url = format!("{}{separator}{query}", discovery.authorization_endpoint);
        let result = ConnectorSignIn { state, authorize_url, service: service.clone() };
        Ok((Pending { target, service, created: Instant::now(), redirect_uri, verifier, discovery, client_id, client_secret }, result))
    }).await?
}

pub(super) async fn expire(pending: &PendingSignIns) {
    pending
        .lock()
        .await
        .retain(|_, entry| entry.created.elapsed() < LIFETIME);
}

pub(super) async fn insert(pending: &PendingSignIns, state: String, value: Pending) {
    let mut pending = pending.lock().await;
    pending.retain(|_, entry| entry.created.elapsed() < LIFETIME);
    pending.insert(state, value);
}

pub(super) async fn take(pending: &PendingSignIns, state: &str) -> Result<Pending> {
    let mut pending = pending.lock().await;
    pending.retain(|_, entry| entry.created.elapsed() < LIFETIME);
    pending
        .remove(state)
        .context("this sign-in has expired; sign in again")
}

impl Pending {
    pub fn sensitive(&self) -> Vec<String> {
        let mut values = vec![self.verifier.clone()];
        values.extend(self.client_secret.iter().cloned());
        if let SignInTarget::Add {
            add:
                sandhill_proto::ConnectorAdd::Custom {
                    secret: Some(secret),
                    ..
                },
        } = &self.target
        {
            values.push(secret.clone());
        }
        values
    }

    pub async fn exchange(&self, code: String) -> Result<Tokens> {
        let mut fields = vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("redirect_uri".into(), self.redirect_uri.clone()),
            ("client_id".into(), self.client_id.clone()),
            ("code_verifier".into(), self.verifier.clone()),
            ("resource".into(), self.discovery.resource.clone()),
        ];
        if let Some(secret) = &self.client_secret {
            fields.push(("client_secret".into(), secret.clone()));
        }
        let initial = Tokens {
            access_token: String::new(),
            refresh_token: None,
            expires_at_ms: None,
            token_endpoint: self.discovery.token_endpoint.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            resource: self.discovery.resource.clone(),
            scope: self.discovery.scope.clone(),
        };
        tokio::task::spawn_blocking(move || initial.exchange(fields)).await?
    }
}

impl Tokens {
    pub fn expiring(&self) -> bool {
        self.expires_at_ms
            .is_some_and(|ms| ms <= crate::journal::now_ms().saturating_add(60_000))
    }

    pub async fn refresh(&self) -> Result<Self> {
        let refresh = self
            .refresh_token
            .as_ref()
            .context("the service supplied no refresh token")?;
        let mut fields = vec![
            ("grant_type".into(), "refresh_token".into()),
            ("refresh_token".into(), refresh.clone()),
            ("client_id".into(), self.client_id.clone()),
            ("resource".into(), self.resource.clone()),
        ];
        if let Some(secret) = &self.client_secret {
            fields.push(("client_secret".into(), secret.clone()));
        }
        let tokens = self.clone();
        tokio::task::spawn_blocking(move || tokens.exchange(fields)).await?
    }

    fn exchange(&self, fields: Vec<(String, String)>) -> Result<Self> {
        let mut response = agent()
            .post(&self.token_endpoint)
            .send_form(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .map_err(|_| {
                anyhow::anyhow!("could not exchange sign-in credentials with the service")
            })?;
        if !response.status().is_success() {
            bail!("the service refused the sign-in credentials");
        }
        let value: Value = serde_json::from_reader(response.body_mut().as_reader())
            .map_err(|_| anyhow::anyhow!("the service returned an invalid sign-in response"))?;
        if value["token_type"]
            .as_str()
            .is_some_and(|t| !t.eq_ignore_ascii_case("Bearer"))
        {
            bail!("the service returned an unsupported token type");
        }
        let access_token = value["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .context("the service returned no access token")?
            .to_owned();
        let seconds = value["expires_in"]
            .as_u64()
            .or_else(|| value["expires_in"].as_str().and_then(|s| s.parse().ok()));
        let expires_at_ms = seconds.map(|seconds| {
            crate::journal::now_ms()
                .saturating_add(i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX))
        });
        Ok(Self {
            access_token,
            refresh_token: value["refresh_token"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| self.refresh_token.clone()),
            expires_at_ms,
            scope: value["scope"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| self.scope.clone()),
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sandhill_proto::ConnectorAdd;

    #[test]
    fn authentication_parameters_and_metadata_locations() {
        let header =
            r#"Bearer resource_metadata="https://example.org/meta?x=a,b", scope="read write""#;
        assert_eq!(
            parameter(header, "resource_metadata").as_deref(),
            Some("https://example.org/meta?x=a,b")
        );
        assert_eq!(parameter(header, "scope").as_deref(), Some("read write"));
        assert_eq!(
            locations("https://example.org/tenant", "oauth-authorization-server").unwrap(),
            [
                "https://example.org/.well-known/oauth-authorization-server/tenant",
                "https://example.org/tenant/.well-known/oauth-authorization-server"
            ]
        );
        for url in [
            "https://127.0.0.1:1234/callback",
            "http://example.org:1234/callback",
            "http://localhost/callback",
            "http://localhost:0/callback",
            "http://localhost:1234@evil.org/callback",
        ] {
            assert!(validate_redirect(url).is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn unfinished_sign_ins_expire_and_are_consumed() {
        let pending: PendingSignIns = Arc::default();
        let entry = Pending {
            target: SignInTarget::Add {
                add: ConnectorAdd::Custom {
                    title: "test".into(),
                    spec: sandhill_proto::ConnectorSpec::Http {
                        url: "https://example.org/mcp".into(),
                        header: None,
                    },
                    secret: None,
                    client_id: None,
                },
            },
            service: "test".into(),
            created: Instant::now() - LIFETIME,
            redirect_uri: "http://localhost:1234/callback".into(),
            verifier: "secret".into(),
            discovery: Discovery {
                authorization_endpoint: "https://example.org/authorize".into(),
                token_endpoint: "https://example.org/token".into(),
                registration_endpoint: None,
                resource: "https://example.org/mcp".into(),
                scope: None,
            },
            client_id: "id".into(),
            client_secret: None,
        };
        pending.lock().await.insert("expired".into(), entry);
        let error = match take(&pending, "expired").await {
            Err(error) => error,
            Ok(_) => panic!("expired state was accepted"),
        };
        assert!(error.to_string().contains("expired"));
        assert!(pending.lock().await.is_empty());
    }
}
