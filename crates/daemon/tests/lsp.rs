//! End-to-end language features with real Python kernels and Astral servers.
mod common;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, LspMessage, SessionInfo};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;

async fn next_matching(
    messages: &mut mpsc::UnboundedReceiver<LspMessage>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let message = messages
                .recv()
                .await
                .expect("LSP connection closed")
                .message;
            assert_ne!(
                message["method"], "sandhill/lspExited",
                "server crashed: {message}"
            );
            if predicate(&message) {
                return message;
            }
        }
    })
    .await
    .expect("language server response timed out")
}

fn labels(message: &Value) -> Vec<&str> {
    message["result"]
        .as_array()
        .or_else(|| message["result"]["items"].as_array())
        .expect("expected completion list")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect()
}

#[tokio::test]
async fn python_language_relay_shares_servers_and_routes_replies_and_diagnostics() {
    let project = common::tempdir().unwrap();
    for name in ["ty", "ruff"] {
        if sandhill_daemon::lsp::find_binary(project.path(), None, name).is_none() {
            eprintln!("skipping Python LSP relay: {name} is not installed");
            return;
        }
    }
    if sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_err()
    {
        eprintln!("skipping Python LSP relay: python3 ipykernel is not installed");
        return;
    }
    let state = common::tempdir().unwrap();
    let daemon = Daemon::open(StateDir::new(state.path())).await.unwrap();
    let (socket, peer) = tokio::io::duplex(1024 * 1024);
    let (reader, writer) = tokio::io::split(peer);
    let server = tokio::spawn(daemon.clone().serve_connection(reader, writer));
    let (reader, writer) = tokio::io::split(socket);
    let mut first = Client::new(reader, writer);
    let mut first_messages = first.take_lsp_messages().unwrap();
    let root = project.path().to_str().unwrap();
    let session: SessionInfo = first
        .call(Call::StartSession {
            kernel: "python3".into(),
            cwd: Some(root.into()),
            project: Some(root.into()),
        })
        .await
        .unwrap();
    let host = first
        .lsp_start(
            root,
            "python",
            Some("notebook".into()),
            Some(session.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        host.capabilities["capabilities"]["positionEncoding"],
        "utf-16"
    );
    first.lsp_send(&host.lsp, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":["utf-16"]}}}})).await.unwrap();
    let initialized = next_matching(&mut first_messages, |message| message["id"] == 1).await;
    assert_eq!(initialized["result"], host.capabilities);
    let file = project.path().join("notebook.sandhill.python.py");
    let uri = format!("file://{}", file.display());
    let text = "# 🐦\nimport os\nos.pa\n";
    first.lsp_send(&host.lsp, json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"python","version":1,"text":text}}})).await.unwrap();

    let (socket, peer) = tokio::io::duplex(1024 * 1024);
    let (reader, writer) = tokio::io::split(peer);
    let second_server = tokio::spawn(daemon.clone().serve_connection(reader, writer));
    let (reader, writer) = tokio::io::split(socket);
    let mut second = Client::new(reader, writer);
    let mut second_messages = second.take_lsp_messages().unwrap();
    let shared = second
        .lsp_start(root, "python", None, Some(session.id.clone()))
        .await
        .unwrap();
    assert_eq!(shared.lsp, host.lsp);
    second.lsp_send(&host.lsp, json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"languageId":"python","version":1,"text":text}}})).await.unwrap();
    let request = json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":2,"character":5}}});
    first.lsp_send(&host.lsp, request.clone()).await.unwrap();
    second.lsp_send(&host.lsp, request).await.unwrap();
    let a = next_matching(&mut first_messages, |message| message["id"] == 2).await;
    let b = next_matching(&mut second_messages, |message| message["id"] == 2).await;
    assert!(
        labels(&a).iter().any(|label| label.starts_with("path")),
        "{a}"
    );
    assert!(
        labels(&b).iter().any(|label| label.starts_with("path")),
        "{b}"
    );
    // Closing one subscriber must not close the other subscriber's document.
    first.lsp_stop(&host.lsp).await.unwrap();
    second.lsp_send(&host.lsp, json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"marker = \"🐦\"; print(sandhill_undefined_name)\n"}]}})).await.unwrap();
    let diagnostic = next_matching(&mut second_messages, |message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["diagnostics"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["code"] == "F821"))
    })
    .await;
    let undefined = diagnostic["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["code"] == "F821")
        .unwrap();
    assert_eq!(undefined["range"]["start"]["line"], 0);
    assert_eq!(undefined["range"]["start"]["character"], 21);
    assert_eq!(undefined["source"], "Ruff");
    second.lsp_stop(&host.lsp).await.unwrap();
    second.shutdown(&session.id).await.unwrap();
    daemon.detach_all();
    drop(first);
    drop(second);
    server.abort();
    second_server.abort();
}

#[tokio::test]
async fn r_language_features_require_an_existing_project_session() {
    let project = common::tempdir().unwrap();
    let state = common::tempdir().unwrap();
    let daemon = Daemon::open(StateDir::new(state.path())).await.unwrap();
    let (socket, peer) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(peer);
    let server = tokio::spawn(daemon.serve_connection(reader, writer));
    let (reader, writer) = tokio::io::split(socket);
    let client = Client::new(reader, writer);
    let error = client
        .lsp_start(project.path().to_str().unwrap(), "r", None, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("ark_session_required"),
        "{error}"
    );
    server.abort();
}
