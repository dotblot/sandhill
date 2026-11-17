//! OAuth fixture shared by the connector integration tests.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sandhill_proto::{ConnectorAdd, ConnectorSpec};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct Authorization {
    challenge: String,
    redirect: String,
    resource: String,
    client_id: String,
}

#[derive(Default)]
struct State {
    codes: HashMap<String, Authorization>,
    access: String,
    refresh: String,
    sensitive: Vec<String>,
    registrations: Vec<Value>,
    grants: usize,
    counter: usize,
}

pub struct OAuth {
    pub address: String,
    stopped: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    state: Arc<Mutex<State>>,
    pub refreshes: Arc<AtomicUsize>,
    pub fail_refresh: Arc<AtomicBool>,
    pub revoked: Arc<AtomicBool>,
    pub fail_tools: Arc<AtomicBool>,
}

impl OAuth {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let stopped = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State::default()));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let fail_refresh = Arc::new(AtomicBool::new(false));
        let revoked = Arc::new(AtomicBool::new(false));
        let fail_tools = Arc::new(AtomicBool::new(false));
        let (stop, data, base, count, fail, revoke, tools) = (
            stopped.clone(),
            state.clone(),
            address.clone(),
            refreshes.clone(),
            fail_refresh.clone(),
            revoked.clone(),
            fail_tools.clone(),
        );
        let thread = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut reader = BufReader::new(socket.try_clone().unwrap());
                        let mut first = String::new();
                        if reader.read_line(&mut first).unwrap() == 0 {
                            continue;
                        }
                        let target = first.split_whitespace().nth(1).unwrap();
                        let path = target.split('?').next().unwrap();
                        let mut length = 0;
                        let mut headers = String::new();
                        loop {
                            let mut line = String::new();
                            reader.read_line(&mut line).unwrap();
                            if line == "\r\n" {
                                break;
                            }
                            if let Some(value) =
                                line.to_ascii_lowercase().strip_prefix("content-length:")
                            {
                                length = value.trim().parse::<usize>().unwrap();
                            }
                            headers.push_str(&line);
                        }
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        let mut state = data.lock().unwrap();
                        let as_metadata = |issuer: &str, registration: bool| {
                            let mut value = json!({"issuer": format!("{base}{issuer}"), "authorization_endpoint": format!("{base}/authorize"), "token_endpoint": format!("{base}/token"), "scopes_supported": ["read"]});
                            if registration {
                                value["registration_endpoint"] = json!(format!("{base}/register"));
                            }
                            value
                        };
                        let response = match path {
                            "/resource" | "/.well-known/oauth-protected-resource" => {
                                json!({"resource": format!("{base}/mcp"), "authorization_servers": [format!("{base}/issuer")], "scopes_supported": ["read"]})
                            }
                            "/.well-known/oauth-protected-resource/fallback" => {
                                json!({"resource": format!("{base}/fallback"), "authorization_servers": [format!("{base}/issuer")], "scopes_supported": ["read"]})
                            }
                            "/typed-resource" => {
                                json!({"resource": format!("{base}/typed"), "authorization_servers": [format!("{base}/typed-issuer")]})
                            }
                            "/oidc-resource" => {
                                json!({"resource": format!("{base}/oidc"), "authorization_servers": [format!("{base}/oidc-issuer")]})
                            }
                            "/.well-known/oauth-authorization-server/issuer" => {
                                as_metadata("/issuer", true)
                            }
                            "/.well-known/oauth-authorization-server/typed-issuer" => {
                                as_metadata("/typed-issuer", false)
                            }
                            "/oidc-issuer/.well-known/openid-configuration" => {
                                as_metadata("/oidc-issuer", true)
                            }
                            "/direct-meta" => as_metadata("/issuer", true),
                            "/register" => {
                                let registration: Value = serde_json::from_slice(&body).unwrap();
                                assert_eq!(registration["client_name"], "Sandhill");
                                assert_eq!(
                                    registration["grant_types"],
                                    json!(["authorization_code", "refresh_token"])
                                );
                                assert_eq!(registration["response_types"], json!(["code"]));
                                assert_eq!(registration["token_endpoint_auth_method"], "none");
                                state.registrations.push(registration);
                                json!({"client_id":"registered-client", "client_secret":"private-registration-secret"})
                            }
                            "/authorize" => {
                                let parameters = form(target.split_once('?').unwrap().1);
                                assert_eq!(parameters.get("response_type").unwrap(), "code");
                                assert_eq!(
                                    parameters.get("code_challenge_method").unwrap(),
                                    "S256"
                                );
                                assert!(parameters.get("state").is_some_and(|s| !s.is_empty()));
                                assert!(
                                    parameters
                                        .get("code_challenge")
                                        .is_some_and(|s| s.len() == 43)
                                );
                                assert!(parameters.get("resource").unwrap().starts_with(&base));
                                let code = format!("private-code-{}", state.counter);
                                state.counter += 1;
                                state.codes.insert(
                                    code.clone(),
                                    Authorization {
                                        challenge: parameters["code_challenge"].clone(),
                                        redirect: parameters["redirect_uri"].clone(),
                                        resource: parameters["resource"].clone(),
                                        client_id: parameters["client_id"].clone(),
                                    },
                                );
                                json!({"code":code, "state":parameters["state"]})
                            }
                            "/token" => {
                                let fields = form(std::str::from_utf8(&body).unwrap());
                                if fields["client_id"] == "registered-client" {
                                    assert!(
                                        fields
                                            .get("client_secret")
                                            .is_some_and(|s| s == "private-registration-secret")
                                    );
                                }
                                let expiring = if fields["grant_type"] == "authorization_code" {
                                    let code = state.codes.remove(&fields["code"]).unwrap();
                                    let challenge = URL_SAFE_NO_PAD
                                        .encode(Sha256::digest(fields["code_verifier"].as_bytes()));
                                    assert!(
                                        challenge == code.challenge,
                                        "PKCE must match the authorize request"
                                    );
                                    assert!(fields["code_verifier"].len() == 64);
                                    assert!(fields["resource"] == code.resource);
                                    assert!(fields["redirect_uri"] == code.redirect);
                                    assert!(fields["client_id"] == code.client_id);
                                    state.sensitive = vec![
                                        fields["code"].clone(),
                                        fields["code_verifier"].clone(),
                                        "private-registration-secret".into(),
                                    ];
                                    state.grants += 1;
                                    state.grants == 1
                                } else {
                                    assert_eq!(fields["grant_type"], "refresh_token");
                                    count.fetch_add(1, Ordering::AcqRel);
                                    if fail.load(Ordering::Acquire) {
                                        reply(
                                            &mut socket,
                                            400,
                                            json!({"error":"invalid_grant"}),
                                            None,
                                        );
                                        continue;
                                    }
                                    assert!(fields["refresh_token"] == state.refresh);
                                    false
                                };
                                state.access = format!("private-access-{}", state.counter);
                                state.refresh = format!("private-refresh-{}", state.counter);
                                state.counter += 1;
                                let access = state.access.clone();
                                let refresh = state.refresh.clone();
                                state.sensitive.extend([access.clone(), refresh.clone()]);
                                json!({"access_token":access, "refresh_token":refresh, "token_type":"Bearer", "expires_in":if expiring {30} else {3600}, "scope":"read write"})
                            }
                            "/mcp" | "/fallback" | "/direct" | "/typed" | "/oidc"
                            | "/root-fallback" => {
                                let bearer = format!("authorization: bearer {}", state.access);
                                let revoked_now = revoke.swap(false, Ordering::AcqRel);
                                if revoked_now {
                                    state.access = "revoked-access".into();
                                }
                                if state.access.is_empty()
                                    || !headers.to_ascii_lowercase().contains(&bearer)
                                    || revoked_now
                                {
                                    let authenticate = match path {
                                        "/direct" => Some(format!(
                                            "Bearer resource_metadata=\"{base}/direct-meta\""
                                        )),
                                        "/typed" => Some(format!(
                                            "Bearer resource_metadata=\"{base}/typed-resource\""
                                        )),
                                        "/oidc" => Some(format!(
                                            "Bearer resource_metadata=\"{base}/oidc-resource\""
                                        )),
                                        "/fallback" => Some("Bearer scope=\"read\"".into()),
                                        "/root-fallback" => None,
                                        _ => Some(format!(
                                            "Bearer resource_metadata=\"{base}/resource\", scope=\"read write\""
                                        )),
                                    };
                                    reply(&mut socket, 401, json!({}), authenticate.as_deref());
                                    continue;
                                }
                                let request: Value = serde_json::from_slice(&body).unwrap();
                                let method = request["method"].as_str().unwrap();
                                if method == "notifications/initialized" {
                                    reply(&mut socket, 202, Value::Null, None);
                                    continue;
                                }
                                if method == "tools/list" && tools.load(Ordering::Acquire) {
                                    reply(&mut socket, 500, json!({}), None);
                                    continue;
                                }
                                let sensitive = state.sensitive.join(" ");
                                let result = match method {
                                    "initialize" => {
                                        json!({"protocolVersion":"2025-06-18", "capabilities":{}, "serverInfo":{"name":"oauth-fixture", "version":"1"}})
                                    }
                                    "tools/list" => {
                                        json!({"tools":[{"name":"lookup", "description":format!("Read a gene {sensitive}"), "inputSchema":{"type":"object"}, "annotations":{"readOnlyHint":true}}]})
                                    }
                                    _ => {
                                        json!({"content":[{"type":"text", "text":format!("TP53 {sensitive}")}]})
                                    }
                                };
                                json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                            }
                            _ => {
                                reply(&mut socket, 404, json!({}), None);
                                continue;
                            }
                        };
                        reply(&mut socket, 200, response, None);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            address,
            stopped,
            thread: Some(thread),
            state,
            refreshes,
            fail_refresh,
            revoked,
            fail_tools,
        }
    }

    pub fn add(&self, path: &str, client_id: Option<&str>) -> ConnectorAdd {
        ConnectorAdd::Custom {
            title: "Account database".into(),
            spec: ConnectorSpec::Http {
                url: format!("{}{path}", self.address),
                header: None,
            },
            secret: None,
            client_id: client_id.map(str::to_owned),
        }
    }

    pub async fn browser(&self, url: String) -> Value {
        tokio::task::spawn_blocking(move || {
            let mut response = ureq::get(&url).call().unwrap();
            serde_json::from_reader(response.body_mut().as_reader()).unwrap()
        })
        .await
        .unwrap()
    }

    pub fn sensitive(&self) -> Vec<String> {
        self.state.lock().unwrap().sensitive.clone()
    }
    pub fn registrations(&self) -> usize {
        self.state.lock().unwrap().registrations.len()
    }
}

impl Drop for OAuth {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn reply(socket: &mut TcpStream, status: u16, body: Value, authenticate: Option<&str>) {
    let body = if status == 202 {
        String::new()
    } else {
        body.to_string()
    };
    let header = authenticate
        .map(|value| format!("WWW-Authenticate: {value}\r\n"))
        .unwrap_or_default();
    write!(socket, "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

fn form(value: &str) -> HashMap<String, String> {
    fn decode(value: &str) -> String {
        let mut result = Vec::new();
        let bytes = value.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            match bytes[at] {
                b'%' => {
                    result.push(u8::from_str_radix(&value[at + 1..at + 3], 16).unwrap());
                    at += 3;
                }
                b'+' => {
                    result.push(b' ');
                    at += 1;
                }
                byte => {
                    result.push(byte);
                    at += 1;
                }
            }
        }
        String::from_utf8(result).unwrap()
    }
    value
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(key), decode(value))
        })
        .collect()
}
