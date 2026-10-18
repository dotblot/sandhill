//! The Plots pane, against real kernels: the images a session's runs
//! showed, and saving one as a file. Each test skips (and says so) when its
//! kernel isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{BLOB_KEY, Call, CellRef, EVENT_RUN_FINISHED, FileWritten, RunStarted};
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
        let socket = state.join("test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        TestDaemon {
            _daemon: daemon,
            socket,
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}

async fn have_kernel(name: &str) -> bool {
    let found = sandhill_daemon::kernel::find_kernelspec(name).await.is_ok();
    if !found {
        eprintln!("skipping: Jupyter kernel {name:?} is not installed");
    }
    found
}

/// Runs code, marked as `by`'s if given, and waits for it to finish.
async fn run(
    client: &mut Client,
    session: &str,
    code: &str,
    cell: Option<CellRef>,
    by: Option<&str>,
    quiet: bool,
) {
    client.subscribe(session, None).await.unwrap();
    let started: RunStarted = client
        .call(Call::Execute {
            session: session.into(),
            code: code.into(),
            cell,
            quiet,
            by: by.map(Into::into),
        })
        .await
        .unwrap();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() == Some(started.run.as_str())
            && event.msg_type == EVENT_RUN_FINISHED
        {
            break;
        }
    }
    client.unsubscribe(session).await.unwrap();
}

/// A 1 × 1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

#[tokio::test]
async fn a_sessions_plots_come_from_every_kind_of_run() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    let show_png = format!(
        "import base64\nfrom IPython.display import Image, SVG, display\ndisplay(Image(base64.b64decode('{PNG}')))"
    );
    let cell = CellRef {
        file: "/data/pbmc/analysis.qmd".into(),
        cell_id: "umap".into(),
    };
    run(&mut client, &session, &show_png, None, None, false).await;
    run(
        &mut client,
        &session,
        "print('no plot here')",
        None,
        None,
        false,
    )
    .await;
    run(
        &mut client,
        &session,
        "display(SVG('<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"><rect width=\"8\" height=\"8\"/></svg>'))",
        Some(cell.clone()),
        None,
        false,
    )
    .await;
    // Code the user didn't write shows nothing here.
    run(&mut client, &session, &show_png, None, None, true).await;
    run(
        &mut client,
        &session,
        &show_png,
        None,
        Some("Claude"),
        false,
    )
    .await;

    let plots = client.session_plots(&session, None).await.unwrap().plots;
    assert_eq!(plots.len(), 3, "{plots:#?}");
    assert!(
        plots[0].data["image/png"][BLOB_KEY].is_string(),
        "{plots:#?}"
    );
    assert!(plots[0].cell.is_none() && plots[0].by.is_none());
    assert!(plots[0].code.contains("Image("));
    assert!(
        plots[1].data["image/svg+xml"]
            .as_str()
            .is_some_and(|svg| svg.contains("<rect")),
        "small SVGs stay inline: {plots:#?}"
    );
    assert_eq!(plots[1].cell.as_ref(), Some(&cell));
    assert_eq!(plots[2].by.as_deref(), Some("Claude"));
    assert!(plots[0].seq < plots[1].seq && plots[1].seq < plots[2].seq);

    let last = client.session_plots(&session, Some(1)).await.unwrap().plots;
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].run, plots[2].run);

    // Saving one: the blob's base64 goes back as the file's bytes, in a new folder.
    let hash = plots[0].data["image/png"][BLOB_KEY].as_str().unwrap();
    let blob = client.get_blob(hash).await.unwrap();
    let path = dir.path().join("pbmc/figures/umap.png");
    let _: FileWritten = client
        .call(Call::WriteFile {
            path: path.to_string_lossy().into_owned(),
            text: blob.data,
            expected_mtime_ms: None,
            base64: true,
        })
        .await
        .unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(b"\x89PNG"));
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn r_plots_show_up() {
    if !have_kernel("ir").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("ir", None).await.unwrap().id;
    run(&mut client, &session, "x <- 1:10", None, None, false).await;
    run(&mut client, &session, "plot(x, x^2)", None, None, false).await;
    let plots = client.session_plots(&session, None).await.unwrap().plots;
    assert_eq!(plots.len(), 1, "{plots:#?}");
    assert!(
        plots[0].data["image/png"][BLOB_KEY].is_string(),
        "{plots:#?}"
    );
    assert_eq!(plots[0].code, "plot(x, x^2)");
    client.shutdown(&session).await.unwrap();
}
