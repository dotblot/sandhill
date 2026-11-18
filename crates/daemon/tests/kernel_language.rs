//! Live-kernel completion and Ark's TCP LSP handshake. Missing kernels skip
//! explicitly; no language service or completion response is mocked.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::blobs::BlobStore;
use sandhill_daemon::journal::Journal;
use sandhill_daemon::kernel;
use sandhill_daemon::session::{SessionHandle, Start};
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, EVENT_RUN_FINISHED, RunFinished, SessionInfo};
use serde_json::json;
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

struct TestSession {
    session: SessionHandle,
    journal: Arc<Journal>,
    _dir: common::TempDir,
}

impl TestSession {
    async fn start(name: &str) -> Option<Self> {
        if kernel::find_kernelspec(name, None).await.is_err() {
            eprintln!("skipping: Jupyter kernel {name:?} is not installed");
            return None;
        }
        let dir = common::tempdir().unwrap();
        let kernels = dir.path().join("kernels");
        std::fs::create_dir_all(&kernels).unwrap();
        let journal = Arc::new(
            Journal::open(
                &dir.path().join("journal.sqlite"),
                BlobStore::new(dir.path().join("blobs")),
            )
            .unwrap(),
        );
        let session = SessionHandle::start(
            journal.clone(),
            &kernels,
            name,
            dir.path(),
            Start::default(),
        )
        .await
        .unwrap();
        Some(Self {
            session,
            journal,
            _dir: dir,
        })
    }

    async fn run(&self, code: &str) -> RunFinished {
        let mut events = self.session.subscribe();
        let run = self
            .session
            .execute(code.into(), None, false, None)
            .await
            .unwrap();
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let event = events.recv().await.unwrap();
                if event.run.as_deref() == Some(run.run.as_str())
                    && event.msg_type == EVENT_RUN_FINISHED
                {
                    let finished: RunFinished = serde_json::from_value(event.content).unwrap();
                    assert_eq!(finished.status, "ok");
                    return finished;
                }
            }
        })
        .await
        .expect("kernel execution timed out")
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        // Tests own these kernels; never leave one behind after an assertion.
        if self.session.is_running() {
            for record in self.journal.sessions().unwrap_or_default() {
                if record.id == self.session.id
                    && let Some(pid) = record.pid
                {
                    kernel::kill_process_group(pid);
                }
            }
            self.session.detach();
        }
    }
}

async fn ark_kernel() -> Option<String> {
    for info in kernel::list_kernelspecs(None).await {
        let Ok(spec) = kernel::find_kernelspec(&info.name, None).await else {
            continue;
        };
        if spec.kernelspec.language.eq_ignore_ascii_case("r")
            && spec.kernelspec.argv.first().is_some_and(|program| {
                Path::new(program)
                    .file_name()
                    .is_some_and(|name| name == "ark")
            })
        {
            return Some(info.name);
        }
    }
    eprintln!(
        "skipping: Ark is not installed (register Ark or install it in Sandhill's bin directory)"
    );
    None
}

async fn completion_uses_runtime_without_execution(name: &str, setup: &str, prefix: &str) {
    let Some(test) = TestSession::start(name).await else {
        return;
    };
    let first = test.run(setup).await;
    let before = test
        .journal
        .events_since(&test.session.id, 0, 1000)
        .unwrap();
    // A fresh kernel's first completion loads its completer (jedi in
    // ipykernel: 0.19 s here against 0.01 s after), which can pass the
    // 2 s limit on a busy CI machine (macOS, 2026-11-15 and 2026-11-18).
    // Its answer, or its timeout, is not what this test checks.
    let _ = test
        .session
        .complete(prefix.into(), prefix.chars().count())
        .await;
    // The kernel is busy with it until its idle status, which comes on
    // another channel than its reply.
    let mut tries = 0;
    let reply = loop {
        match test
            .session
            .complete(prefix.into(), prefix.chars().count())
            .await
        {
            Err(err) if err.to_string().starts_with("kernel_busy") && tries < 40 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            reply => break reply.unwrap(),
        }
    };
    assert!(
        reply
            .matches
            .iter()
            .any(|item| item == "sandhill_live_completion"),
        "runtime variable missing from completion: {reply:?}"
    );
    assert_eq!(reply.cursor_start, "# café\n".chars().count());
    assert_eq!(reply.cursor_end, prefix.chars().count());
    assert!(reply.metadata.is_object());
    let after = test
        .journal
        .events_since(&test.session.id, 0, 1000)
        .unwrap();
    assert_eq!(
        after.len(),
        before.len(),
        "completion must not enter the journal"
    );
    let second = test.run("1 + 1").await;
    assert_eq!(
        second.execution_count,
        first.execution_count.map(|count| count + 1)
    );
    test.session.shutdown().await.unwrap();
}

#[tokio::test]
async fn ipykernel_completes_live_variables_without_a_run() {
    completion_uses_runtime_without_execution(
        "python3",
        "sandhill_live_completion = 42",
        "# café\nsandhill_live_comp",
    )
    .await;
}

#[tokio::test]
async fn ipykernel_inspects_live_function_signature_without_a_run() {
    let Some(test) = TestSession::start("python3").await else {
        return;
    };
    let first = test
        .run("def sandhill_signature(alpha, beta=2):\n    return alpha")
        .await;
    let before = test
        .journal
        .events_since(&test.session.id, 0, 1000)
        .unwrap()
        .len();
    // The cursor sits after the callee's name, inside `# café\n…(`.
    let code = "# café\nsandhill_signature(1, ";
    let name_end = "# café\nsandhill_signature".chars().count();
    let reply = test.session.inspect(code.into(), name_end).await.unwrap();
    assert!(reply.found, "{reply:?}");
    // IPython colours its help text; compare the plain characters.
    let raw = reply.data["text/plain"].as_str().unwrap_or_default();
    let mut text = String::new();
    let mut escape = false;
    for ch in raw.chars() {
        match ch {
            '\u{1b}' => escape = true,
            'm' if escape => escape = false,
            _ if escape => {}
            _ => text.push(ch),
        }
    }
    assert!(
        text.contains("Signature: sandhill_signature(alpha, beta=2)"),
        "signature missing from inspection: {raw:?}"
    );
    assert_eq!(
        test.journal
            .events_since(&test.session.id, 0, 1000)
            .unwrap()
            .len(),
        before,
        "inspection must not enter the journal"
    );
    let second = test.run("1 + 1").await;
    assert_eq!(
        second.execution_count,
        first.execution_count.map(|count| count + 1)
    );
    test.session.shutdown().await.unwrap();
}

#[tokio::test]
async fn ark_standard_completion_reply_does_not_create_a_run() {
    let Some(name) = ark_kernel().await else {
        return;
    };
    let Some(test) = TestSession::start(&name).await else {
        return;
    };
    let first = test.run("sandhill_live_completion <- 42").await;
    let before = test
        .journal
        .events_since(&test.session.id, 0, 1000)
        .unwrap()
        .len();
    let code = "sandhill_live_comp";
    let reply = test
        .session
        .complete(code.into(), code.len())
        .await
        .unwrap();
    // Ark currently returns an empty standard complete_reply; actual Ark
    // suggestions are exercised through its LSP below, not invented here.
    assert!(reply.metadata.is_object());
    assert!(reply.cursor_start <= reply.cursor_end);
    assert!(reply.cursor_end <= code.len());
    assert_eq!(
        test.journal
            .events_since(&test.session.id, 0, 1000)
            .unwrap()
            .len(),
        before
    );
    let second = test.run("1 + 1").await;
    assert_eq!(
        second.execution_count,
        first.execution_count.map(|count| count + 1)
    );
    test.session.shutdown().await.unwrap();
}

#[tokio::test]
async fn ipykernel_completion_timeout_does_not_block_the_session_actor() {
    let Some(test) = TestSession::start("python3").await else {
        return;
    };
    test.run(
        r#"import time
sandhill_original_complete = get_ipython().kernel.do_complete
def sandhill_slow_complete(code, cursor_pos):
    time.sleep(3)
    return {'matches': [], 'cursor_start': 0, 'cursor_end': cursor_pos, 'metadata': {}, 'status': 'ok'}
get_ipython().kernel.do_complete = sandhill_slow_complete"#,
    )
    .await;
    let before = test
        .journal
        .events_since(&test.session.id, 0, 1000)
        .unwrap()
        .len();
    let started = tokio::time::Instant::now();
    let error = test
        .session
        .complete("sandhill".into(), 8)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_millis(2800));
    assert_eq!(
        test.journal
            .events_since(&test.session.id, 0, 1000)
            .unwrap()
            .len(),
        before
    );
    // Execution is accepted while the timed-out request is still finishing.
    // Its late shell reply must not complete or consume this later run.
    test.run("get_ipython().kernel.do_complete = sandhill_original_complete")
        .await;
    test.session.shutdown().await.unwrap();
}

async fn completion_rejects_busy_kernel(name: &str, code: &str) {
    let Some(test) = TestSession::start(name).await else {
        return;
    };
    test.run("1 + 1").await;
    let mut events = test.session.subscribe();
    let run = test
        .session
        .execute(code.into(), None, false, None)
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let error = test
        .session
        .complete("sandhill".into(), 8)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "kernel_busy: kernel busy");
    assert!(started.elapsed() < Duration::from_secs(1));
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = events.recv().await.unwrap();
            if event.run.as_deref() == Some(run.run.as_str())
                && event.msg_type == EVENT_RUN_FINISHED
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    test.session.shutdown().await.unwrap();
}

#[tokio::test]
async fn ipykernel_completion_rejects_queued_execution_immediately() {
    completion_rejects_busy_kernel("python3", "import time; time.sleep(3)").await;
}

#[tokio::test]
async fn ark_completion_rejects_queued_execution_immediately() {
    let Some(name) = ark_kernel().await else {
        return;
    };
    completion_rejects_busy_kernel(&name, "Sys.sleep(3)").await;
}

#[tokio::test]
async fn ark_lsp_initializes_and_completes_through_the_daemon_wire() {
    let Some(name) = ark_kernel().await else {
        return;
    };
    let dir = common::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = dir.path().join("state");
    let daemon = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let mut client = Client::connect_unix(&socket).await.unwrap();
    let mut messages = client.take_lsp_messages().unwrap();
    client.open_project(&project).await.unwrap();
    client.trust_folder(&project).await.unwrap();
    let session: SessionInfo = client
        .call(Call::StartSession {
            kernel: name,
            cwd: Some(project.clone()),
            project: Some(project.clone()),
        })
        .await
        .unwrap();
    client.subscribe(&session.id, None).await.unwrap();
    let run = client.execute(&session.id, "1 + 1", None).await.unwrap();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = client.next_event().await.unwrap();
            if event.run.as_deref() == Some(run.run.as_str())
                && event.msg_type == EVENT_RUN_FINISHED
            {
                assert_eq!(event.content["status"], "ok");
                break;
            }
        }
    })
    .await
    .unwrap();
    let started = client
        .lsp_start(
            &project,
            "r",
            Some("script".into()),
            Some(session.id.clone()),
        )
        .await
        .unwrap();
    assert!(started.capabilities["capabilities"].is_object());
    assert_eq!(
        started.capabilities["capabilities"]["positionEncoding"],
        "utf-16"
    );
    client
        .lsp_send(
            &started.lsp,
            json!({
                "jsonrpc": "2.0", "id": 101, "method": "initialize",
                "params": {"capabilities": {}}
            }),
        )
        .await
        .unwrap();
    let initialized = tokio::time::timeout(TIMEOUT, async {
        loop {
            let message = messages.recv().await.unwrap();
            if message.lsp == started.lsp && message.message["id"] == 101 {
                break message.message;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(initialized["result"], started.capabilities);
    client
        .lsp_send(
            &started.lsp,
            json!({
                "jsonrpc": "2.0", "method": "initialized", "params": {}
            }),
        )
        .await
        .unwrap();
    let uri = format!("file://{project}/completion.R");
    let prefix = "sandhill_wire_comp";
    client
        .lsp_send(
            &started.lsp,
            json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": uri, "languageId": "r", "version": 1,
                    "text": format!("# café\nsandhill_wire_completion <- 42\n{prefix}")
                }}
            }),
        )
        .await
        .unwrap();
    client
        .lsp_send(
            &started.lsp,
            json!({
                "jsonrpc": "2.0", "id": 102, "method": "textDocument/completion",
                "params": {
                    "textDocument": {"uri": uri},
                    "position": {"line": 2, "character": prefix.encode_utf16().count()}
                }
            }),
        )
        .await
        .unwrap();
    let completion = tokio::time::timeout(TIMEOUT, async {
        loop {
            let message = messages.recv().await.unwrap();
            if message.lsp == started.lsp && message.message["id"] == 102 {
                break message.message;
            }
        }
    })
    .await
    .unwrap();
    assert!(completion.get("error").is_none(), "{completion}");
    let items = completion["result"]
        .as_array()
        .or_else(|| completion["result"]["items"].as_array())
        .expect("Ark did not return completion items");
    assert!(
        items
            .iter()
            .any(|item| item["label"] == "sandhill_wire_completion"),
        "{completion}"
    );
    client
        .lsp_send(
            &started.lsp,
            json!({
                "jsonrpc": "2.0", "method": "textDocument/didClose",
                "params": {"textDocument": {"uri": uri}}
            }),
        )
        .await
        .unwrap();
    client.lsp_stop(&started.lsp).await.unwrap();
    client.shutdown(&session.id).await.unwrap();
    server.abort();
    daemon.detach_all();
}

#[tokio::test]
async fn irkernel_reports_why_ark_language_service_is_unavailable() {
    let Some(test) = TestSession::start("ir").await else {
        return;
    };
    let error = test.session.start_ark_lsp().await.unwrap_err().to_string();
    assert!(error.contains("requires an Ark session"), "{error}");
    assert!(error.contains("IRkernel"), "{error}");
    test.session.shutdown().await.unwrap();
}
