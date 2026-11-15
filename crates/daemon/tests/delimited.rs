//! Exact bounded reads over the real daemon protocol and the GPUI import code
//! against real R/Python kernels. No source-string/echo substitutes for imports.
mod common;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, EVENT_RUN_FINISHED, FileBytes, RunStarted, SessionInfo};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::UnixListener;

#[allow(dead_code)]
#[path = "../../../gpui/src/imports.rs"]
mod imports;

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
}
impl TestDaemon {
    async fn start(path: &Path) -> Self {
        let daemon = Daemon::open(StateDir::new(path)).await.unwrap();
        let socket = path.join("test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        Self {
            _daemon: daemon,
            socket,
        }
    }
    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}

async fn run(client: &mut Client, session: &str, code: String) -> String {
    client.subscribe(session, None).await.unwrap();
    let started: RunStarted = client
        .call(Call::Execute {
            session: session.into(),
            code,
            cell: None,
            quiet: false,
            by: None,
        })
        .await
        .unwrap();
    let status = loop {
        let event = tokio::time::timeout(Duration::from_secs(90), client.next_event())
            .await
            .expect("kernel execution timed out")
            .expect("connection closed");
        if event.run.as_deref() == Some(&started.run) && event.msg_type == EVENT_RUN_FINISHED {
            break event.content["status"].as_str().unwrap().to_string();
        }
    };
    client.unsubscribe(session).await.unwrap();
    status
}

#[tokio::test]
async fn bounded_file_protocol_checks_actual_size_without_starting_a_kernel() {
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let client = daemon.client().await;
    let file = dir.path().join("boundary.csv");
    let path = file.to_string_lossy().into_owned();
    std::fs::write(&file, "x".repeat(5_000_000)).unwrap();
    let stat: FileBytes = client
        .call(Call::ReadBytes {
            path: path.clone(),
            offset: 0,
            limit: Some(0),
        })
        .await
        .unwrap();
    assert_eq!(stat.size, 5_000_000);
    assert!(stat.data.is_empty());
    let exact = client.read_text_bounded(&path, 5_000_000).await.unwrap();
    assert_eq!(exact.text.len(), 5_000_000);
    // Replace the file after the dialog's metadata check. Stale DirEntry/stat
    // information must never authorize an oversized text read.
    std::fs::write(&file, "y".repeat(5_000_001)).unwrap();
    let err = client
        .read_text_bounded(&path, 5_000_000)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("too large"), "{err}");
    let sessions: Vec<SessionInfo> = client.call(Call::ListSessions).await.unwrap();
    assert!(sessions.is_empty());
}

async fn import_with_kernel(kernel: &str, language: &str) {
    if sandhill_daemon::kernel::find_kernelspec(kernel, None)
        .await
        .is_err()
    {
        eprintln!("skipping: {kernel} is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session(kernel, None).await.unwrap().id;
    if language == "python" && run(&mut client, &session, "import pandas".into()).await != "ok" {
        eprintln!("skipping: pandas is not installed for {kernel}");
        client.shutdown(&session).await.unwrap();
        return;
    }
    for (extension, separator) in [("csv", ','), ("tsv", '\t')] {
        let path = dir.path().join(format!("quoted.{extension}"));
        let raw = format!(
            "name{separator}value\r\n\"雪{separator}ice\"{separator}1\r\n\"two\nlines\"{separator}2\r\n"
        );
        std::fs::write(&path, &raw).unwrap();
        let name = format!("imported_{extension}");
        let code = imports::imports_for(language)[0].code(&path.to_string_lossy(), &name);
        let guarded = imports::non_overwriting_code(language, &name, &code);
        assert_eq!(run(&mut client, &session, guarded.clone()).await, "ok");
        let table = client
            .table_rows(&session, std::slice::from_ref(&name), 0, None, &[], &[], "")
            .await
            .unwrap();
        assert_eq!((table.rows, table.width), (2, 2));
        assert_eq!(
            table.values[0],
            vec![json!(format!("雪{separator}ice")), json!(1)]
        );
        assert_eq!(table.values[1], vec![json!("two\nlines"), json!(2)]);
        // Existing target variables are protected in the actual kernel even if
        // they were created after the dialog's preview was drawn.
        std::fs::write(
            &path,
            format!("name{separator}value\nchanged{separator}9\n"),
        )
        .unwrap();
        assert_eq!(run(&mut client, &session, guarded).await, "error");
        let unchanged = client
            .table_rows(&session, &[name], 0, None, &[], &[], "")
            .await
            .unwrap();
        assert_eq!(unchanged.values, table.values);
    }
    client.shutdown(&session).await.unwrap();
    let ended = client
        .call::<RunStarted>(Call::Execute {
            session,
            code: "1".into(),
            cell: None,
            quiet: false,
            by: None,
        })
        .await;
    assert!(
        ended.is_err(),
        "an ended session must not be restarted by import"
    );
}

#[tokio::test]
async fn python_imports_real_quoted_csv_and_tsv_without_replacing_variables() {
    import_with_kernel("python3", "python").await;
}

#[tokio::test]
async fn r_imports_real_quoted_csv_and_tsv_without_replacing_variables() {
    let kernel = if sandhill_daemon::kernel::find_kernelspec("ark", None)
        .await
        .is_ok()
    {
        "ark"
    } else {
        "ir"
    };
    import_with_kernel(kernel, "r").await;
}
