//! Ports on a daemon's machine, reached from this one: a listener on
//! `127.0.0.1` here whose connections go through tunnels on the daemon's
//! own connection (see [`Client::open_tunnel`]), as `ssh -L` would, with no
//! second ssh login. The app shows a server's Shiny apps and the Viewer's
//! pages this way.

use std::sync::Arc;

use sandhill_proto::client::{Client, ClientError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The most read from a local connection per message.
const CHUNK: usize = 64 * 1024;

/// Carries each connection to `listener` to `port` on the daemon's machine,
/// through the client `client` returns then (the current one, after a
/// reconnect). Runs until the task is dropped.
pub async fn forward<F>(listener: TcpListener, port: u16, client: F)
where
    F: Fn() -> Option<Arc<Client>> + Send + Sync + 'static,
{
    let client = Arc::new(client);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let client = client.clone();
        tokio::spawn(async move {
            let Some(client) = client() else {
                return;
            };
            if let Err(err) = carry(stream, &client, port).await {
                tracing::debug!("port {port}: {err}");
            }
        });
    }
}

async fn carry(stream: TcpStream, client: &Client, port: u16) -> Result<(), ClientError> {
    let _ = stream.set_nodelay(true);
    let mut tunnel = client.open_tunnel(port).await?;
    let id = tunnel.id;
    let (mut reader, mut writer) = stream.into_split();
    let down = async {
        while let Some(bytes) = tunnel.incoming.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    };
    let up = async {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if client.tunnel_write(id, &buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    // Either side ending ends the connection.
    tokio::select! {
        _ = down => {}
        _ = up => {}
    }
    client.close_tunnel(id).await
}
