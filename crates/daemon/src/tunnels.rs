//! Tunnels: TCP connections to ports on the daemon's machine that a client
//! carries over its own connection to the daemon. The app shows a Shiny
//! app or the Viewer's pages on a server this way, with no ports forwarded
//! by ssh. Only `127.0.0.1` is reached. A tunnel belongs to the client
//! connection that opened it and ends with it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use sandhill_proto::{ServerMessage, TunnelData};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The most read from a connection per message.
const CHUNK: usize = 64 * 1024;

struct Open {
    /// Bytes from the client, for the connection.
    to_port: mpsc::UnboundedSender<Vec<u8>>,
    task: AbortHandle,
}

/// One client connection's tunnels.
pub struct Tunnels {
    out: mpsc::UnboundedSender<ServerMessage>,
    open: Arc<Mutex<HashMap<u64, Open>>>,
}

impl Tunnels {
    pub fn new(out: mpsc::UnboundedSender<ServerMessage>) -> Tunnels {
        Tunnels {
            out,
            open: Default::default(),
        }
    }

    /// Opens tunnel `id` to `port`, answering request `request` once
    /// connected. Bytes written before then wait for the connection.
    pub fn open(&self, request: u64, id: u64, port: u16) {
        let (to_port, mut from_client) = mpsc::unbounded_channel::<Vec<u8>>();
        let out = self.out.clone();
        let open = self.open.clone();
        // Held until the tunnel is listed, so a failed connection can't
        // take it off the list first.
        let mut listed = self.open.lock().unwrap();
        let task = tokio::spawn(async move {
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
                    .await;
            let mut stream = match connected {
                Ok(Ok(stream)) => stream,
                Ok(Err(err)) => {
                    open.lock().unwrap().remove(&id);
                    let error = format!("nothing answers on port {port}: {err}");
                    let _ = out.send(crate::server::response(
                        request,
                        Err(anyhow::anyhow!(error)),
                    ));
                    return;
                }
                Err(_) => {
                    open.lock().unwrap().remove(&id);
                    let error = format!("port {port} didn't answer in time");
                    let _ = out.send(crate::server::response(
                        request,
                        Err(anyhow::anyhow!(error)),
                    ));
                    return;
                }
            };
            let _ = stream.set_nodelay(true);
            let _ = out.send(crate::server::response(request, Ok(Value::Null)));
            let mut buf = vec![0u8; CHUNK];
            loop {
                tokio::select! {
                    read = stream.read(&mut buf) => {
                        let data = match read {
                            Ok(0) | Err(_) => break,
                            Ok(n) => base64::engine::general_purpose::STANDARD.encode(&buf[..n]),
                        };
                        let message = TunnelData { id, data: Some(data), closed: false };
                        if out.send(ServerMessage::Tunnel { tunnel: message }).is_err() {
                            return;
                        }
                    }
                    bytes = from_client.recv() => match bytes {
                        Some(bytes) => {
                            if stream.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                        // The client closed it.
                        None => return,
                    },
                }
            }
            open.lock().unwrap().remove(&id);
            let message = TunnelData {
                id,
                data: None,
                closed: true,
            };
            let _ = out.send(ServerMessage::Tunnel { tunnel: message });
        });
        listed.insert(
            id,
            Open {
                to_port,
                task: task.abort_handle(),
            },
        );
    }

    /// Bytes for a tunnel, base64. Bytes for one that has ended are
    /// dropped: the client was told it closed.
    pub fn write(&self, id: u64, data: &str) {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return;
        };
        if let Some(open) = self.open.lock().unwrap().get(&id) {
            let _ = open.to_port.send(bytes);
        }
    }

    pub fn close(&self, id: u64) {
        // Dropping the sender ends the task once it has written what came before.
        self.open.lock().unwrap().remove(&id);
    }
}

impl Drop for Tunnels {
    fn drop(&mut self) {
        for (_, open) in self.open.lock().unwrap().drain() {
            open.task.abort();
        }
    }
}
