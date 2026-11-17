//! A short-lived loopback redirect, never an externally reachable server.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

#[derive(Debug)]
pub(super) struct Callback {
    pub state: String,
    pub code: Option<String>,
    pub error: Option<String>,
}

enum Control {
    Expect { state: String, service: String },
    Cancel,
}

pub(super) struct Loopback {
    pub redirect_uri: String,
    control: Sender<Control>,
}

impl Loopback {
    pub fn bind() -> anyhow::Result<(Self, oneshot::Receiver<anyhow::Result<Callback>>)> {
        Self::with_timeout(Duration::from_secs(10 * 60))
    }

    fn with_timeout(
        timeout: Duration,
    ) -> anyhow::Result<(Self, oneshot::Receiver<anyhow::Result<Callback>>)> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let redirect_uri = format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr()?.port()
        );
        let (control, commands) = mpsc::channel();
        let (send, receive) = oneshot::channel();
        std::thread::Builder::new()
            .name("connector-sign-in".into())
            .spawn(move || {
                let result = listen(&listener, commands, timeout);
                drop(listener);
                let _ = send.send(result);
            })?;
        Ok((
            Self {
                redirect_uri,
                control,
            },
            receive,
        ))
    }

    pub fn expect(&self, state: String, service: String) -> anyhow::Result<()> {
        self.control.send(Control::Expect { state, service })?;
        Ok(())
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Cancel);
    }
}

fn listen(
    listener: &TcpListener,
    commands: Receiver<Control>,
    timeout: Duration,
) -> anyhow::Result<Callback> {
    let deadline = Instant::now() + timeout;
    let mut expected = None;
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                Control::Expect { state, service } => expected = Some((state, service)),
                Control::Cancel => anyhow::bail!("Sign-in cancelled."),
            }
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Sign-in timed out. Try signing in again."
        );
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_millis(250)))?;
                stream.set_write_timeout(Some(Duration::from_millis(250)))?;
                let callback = read_callback(&stream).ok().flatten();
                if let Some(callback) = callback
                    && let Some((state, service)) = &expected
                    && callback.state == *state
                {
                    let message = if let Some(error) = &callback.error {
                        format!("Sign-in failed: {error}")
                    } else {
                        format!(
                            "Signed in to {service}. You can close this tab and go back to Sandhill."
                        )
                    };
                    // Browser acknowledgement only; the daemon still exchanges the code.
                    let _ = respond(&mut stream, "200 OK", &message);
                    return Ok(callback);
                }
                let _ = respond(&mut stream, "404 Not Found", "Not found.");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match commands.recv_timeout(remaining.min(Duration::from_millis(50))) {
                    Ok(Control::Expect { state, service }) => expected = Some((state, service)),
                    Ok(Control::Cancel) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        anyhow::bail!("Sign-in cancelled.")
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_callback(stream: &TcpStream) -> anyhow::Result<Option<Callback>> {
    let mut request = String::new();
    BufReader::new(stream.take(8192)).read_line(&mut request)?;
    let mut parts = request.split_whitespace();
    if parts.next() != Some("GET") {
        return Ok(None);
    }
    let Some(target) = parts.next() else {
        return Ok(None);
    };
    parse_callback(target)
}

fn parse_callback(target: &str) -> anyhow::Result<Option<Callback>> {
    let Some(("/callback", query)) = target.split_once('?') else {
        return Ok(None);
    };
    let mut state = None;
    let mut code = None;
    let mut error = None;
    let mut description = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?;
        let slot = match key.as_str() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            "error_description" => &mut description,
            _ => continue,
        };
        anyhow::ensure!(slot.is_none(), "Duplicate callback parameter.");
        *slot = Some(decode(value)?);
    }
    let Some(state) = state.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let code = code.filter(|s| !s.is_empty());
    let error = error
        .map(|e| match description {
            Some(d) if !d.is_empty() => format!("{e}: {d}"),
            _ => e,
        })
        .or_else(|| {
            code.is_none()
                .then(|| "The service did not return an authorization code.".into())
        });
    Ok(Some(Callback { state, code, error }))
}

fn decode(value: &str) -> anyhow::Result<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes.next().and_then(|b| (b as char).to_digit(16));
                let low = bytes.next().and_then(|b| (b as char).to_digit(16));
                let (Some(high), Some(low)) = (high, low) else {
                    anyhow::bail!("Invalid callback encoding.");
                };
                (high * 16 + low) as u8
            }
            byte => byte,
        });
    }
    Ok(String::from_utf8(decoded)?)
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) -> std::io::Result<()> {
    let mut body = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sandhill sign-in</title></head><body><p>",
    );
    for ch in message.chars() {
        match ch {
            '&' => body.push_str("&amp;"),
            '<' => body.push_str("&lt;"),
            '>' => body.push_str("&gt;"),
            '"' => body.push_str("&quot;"),
            '\'' => body.push_str("&#39;"),
            ch => body.push(ch),
        }
    }
    body.push_str("</p></body></html>");
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::{Loopback, parse_callback};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn request(listener: &Loopback, target: &str) -> String {
        let address = listener
            .redirect_uri
            .strip_prefix("http://")
            .unwrap()
            .strip_suffix("/callback")
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        reply
    }

    #[test]
    fn callback_decodes_and_rejects_ambiguous_queries() {
        let callback = parse_callback("/callback?state=s%2B1&code=a%2Fb+c")
            .unwrap()
            .unwrap();
        assert_eq!(callback.state, "s+1");
        assert_eq!(callback.code.as_deref(), Some("a/b c"));
        let denied =
            parse_callback("/callback?state=s&error=access_denied&error_description=No+thanks%21")
                .unwrap()
                .unwrap();
        assert_eq!(denied.error.as_deref(), Some("access_denied: No thanks!"));
        assert!(parse_callback("/callback?state=s&state=other&code=c").is_err());
        assert!(parse_callback("/callback?state=s&code=%ZZ").is_err());
        assert!(parse_callback("/other?state=s&code=c").unwrap().is_none());
    }

    #[test]
    fn loopback_accepts_only_matching_callback_and_escapes_html() {
        let (listener, receive) = Loopback::bind().unwrap();
        listener
            .expect("right".into(), "A <service>".into())
            .unwrap();
        assert!(request(&listener, "/favicon.ico").starts_with("HTTP/1.1 404"));
        assert!(request(&listener, "/callback?state=wrong&code=no").starts_with("HTTP/1.1 404"));
        let reply = request(&listener, "/callback?state=right&code=yes");
        assert!(reply.contains("Signed in to A &lt;service&gt;."));
        assert_eq!(
            receive.blocking_recv().unwrap().unwrap().code.as_deref(),
            Some("yes")
        );
    }

    #[test]
    fn loopback_reports_denial_cancel_and_timeout() {
        let (listener, receive) = Loopback::bind().unwrap();
        listener.expect("s".into(), "Service".into()).unwrap();
        assert!(
            request(
                &listener,
                "/callback?state=s&error=denied&error_description=No%20access"
            )
            .contains("Sign-in failed: denied: No access")
        );
        assert!(receive.blocking_recv().unwrap().unwrap().error.is_some());
        let (listener, receive) = Loopback::bind().unwrap();
        drop(listener);
        assert!(receive.blocking_recv().unwrap().is_err());
        let (_listener, receive) = Loopback::with_timeout(Duration::from_millis(20)).unwrap();
        assert!(
            receive
                .blocking_recv()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
    }
}
