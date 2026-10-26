//! Ports on a host's machine, reached from this Mac for the Viewer: a
//! Shiny app, or the daemon's page server. A server's port gets a port of
//! its own here, on 127.0.0.1, whose connections are carried over the
//! daemon's connection (see `sandhill_link::forward`). This Mac's ports are
//! reached as they are.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::State;

use crate::link::Links;

/// Ports forwarded so far, host and port there -> port here. They stay
/// until the app quits; each carries the host's connection of the moment.
#[derive(Default)]
pub struct Ports {
    forwarded: Mutex<HashMap<(String, u16), u16>>,
}

/// The port on this Mac that reaches `port` on `host`'s machine.
#[tauri::command]
pub async fn forward_port(
    links: State<'_, Arc<Links>>,
    ports: State<'_, Arc<Ports>>,
    host: String,
    port: u16,
) -> Result<u16, String> {
    if host == sandhill_link::LOCAL {
        return Ok(port);
    }
    let key = (host.clone(), port);
    if let Some(&here) = ports.forwarded.lock().unwrap().get(&key) {
        return Ok(here);
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|err| format!("cannot open a port on this Mac: {err}"))?;
    let here = listener.local_addr().map_err(|err| err.to_string())?.port();
    let mut forwarded = ports.forwarded.lock().unwrap();
    // Asked twice at once: the first one stands.
    if let Some(&first) = forwarded.get(&key) {
        return Ok(first);
    }
    forwarded.insert(key, here);
    let links = links.inner().clone();
    tauri::async_runtime::spawn(sandhill_link::forward::forward(listener, port, move || {
        links.client(&host)
    }));
    Ok(here)
}
