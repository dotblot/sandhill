//! Sandhill's MCP tools over Streamable HTTP on 127.0.0.1, for agents that
//! start local MCP servers inside a sandbox without sockets (OpenScience's
//! Seatbelt/bubblewrap policy denies even the daemon's Unix socket). The
//! agent's own unsandboxed process connects here instead. Every request must
//! carry the bearer token, so other local processes cannot use the tools.
//!
//! Only the request/response half of the transport is offered: replies are
//! `application/json`, notifications get `202`, and `GET` (the optional
//! server-to-client stream) is refused with `405`, as the spec allows.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::{Tools, error_reply};

/// Largest request accepted: tool arguments are code and small options.
const MAX_BODY: usize = 8 << 20;
const MAX_HEAD: usize = 64 << 10;

/// Serves `tools` at `http://127.0.0.1:<port>/mcp` until the process ends and
/// returns the port.
pub async fn serve(tools: Arc<Tools>, token: String) -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = Arc::new(format!("Bearer {token}"));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(connection(stream, tools.clone(), token.clone()));
        }
    });
    Ok(port)
}

struct Request {
    method: String,
    path: String,
    authorized: bool,
    keep_alive: bool,
    body: Vec<u8>,
}

async fn connection(stream: TcpStream, tools: Arc<Tools>, token: Arc<String>) {
    let _ = stream.set_nodelay(true);
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    while let Some(request) = read(&mut reader, &token).await {
        let keep = request.keep_alive;
        if answer(&mut writer, request, &tools).await.is_err() || !keep {
            return;
        }
    }
}

async fn read<R: AsyncBufReadExt + Unpin>(reader: &mut R, token: &str) -> Option<Request> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut keep_alive = parts.next() == Some("HTTP/1.1");
    let mut length = 0usize;
    let mut authorized = false;
    let mut total = line.len();
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).await.ok()?;
        total += n;
        if n == 0 || total > MAX_HEAD {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => authorized = value == token,
            "connection" if value.eq_ignore_ascii_case("close") => keep_alive = false,
            "connection" if value.eq_ignore_ascii_case("keep-alive") => keep_alive = true,
            "content-length" => length = value.parse().ok()?,
            _ => {}
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.ok()?;
    Some(Request {
        method,
        path,
        authorized,
        keep_alive,
        body,
    })
}

async fn answer<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    request: Request,
    tools: &Tools,
) -> std::io::Result<()> {
    if request.path.split('?').next() != Some("/mcp") {
        return respond(writer, "404 Not Found", None, &[]).await;
    }
    if !request.authorized {
        return respond(writer, "401 Unauthorized", None, &[]).await;
    }
    if request.method != "POST" {
        return respond(writer, "405 Method Not Allowed", None, &["Allow: POST"]).await;
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        let reply = error_reply(Value::Null, -32700, "parse error");
        return respond(writer, "400 Bad Request", Some(&reply), &[]).await;
    };
    let reply = match message {
        Value::Array(batch) => {
            let mut replies = Vec::new();
            for message in batch {
                replies.extend(one(tools, message).await);
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        message => one(tools, message).await,
    };
    match reply {
        Some(reply) => respond(writer, "200 OK", Some(&reply), &[]).await,
        // Notifications and responses: accepted, nothing to say.
        None => respond(writer, "202 Accepted", None, &[]).await,
    }
}

/// The reply to one JSON-RPC message, if it is a request.
async fn one(tools: &Tools, message: Value) -> Option<Value> {
    let method = message["method"].as_str()?.to_string();
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match tools.handle(&method, params).await {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(err) => error_reply(id, -32601, &format!("{err:#}")),
    })
}

async fn respond<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    status: &str,
    body: Option<&Value>,
    extra: &[&str],
) -> std::io::Result<()> {
    let body = body
        .map(|body| serde_json::to_vec(body).unwrap_or_default())
        .unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    for line in extra {
        head.push_str(line);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await
}
