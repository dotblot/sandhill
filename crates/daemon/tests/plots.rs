//! The Plots pane, against real kernels: the images a session's runs
//! showed, and saving one as a file. Each test skips (and says so) when its
//! kernel isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, Call, CellRef, EVENT_RUN_FINISHED, FileWritten, PlotRedraw, RunStarted,
};
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
    let found = sandhill_daemon::kernel::find_kernelspec(name, None)
        .await
        .is_ok();
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
            create: false,
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

/// Asks the daemon to draw a session's plot again: whether it can, with no
/// `file`; else into `file`, `width` × `height` inches at `dpi`.
async fn redraw(
    client: &mut Client,
    session: &str,
    plot: &sandhill_proto::Plot,
    file: Option<&Path>,
    format: &str,
    (width, height, dpi): (f64, f64, u32),
) -> sandhill_proto::client::Result<PlotRedraw> {
    let image = plot.data.as_object().and_then(|data| {
        data.values()
            .find_map(|v| v[BLOB_KEY].as_str().map(str::to_string))
    });
    client
        .call(Call::RedrawPlot {
            session: session.into(),
            run: plot.run.clone(),
            image,
            index: 0,
            file: file.map(|f| f.to_string_lossy().into_owned()),
            format: format.into(),
            width,
            height,
            dpi,
            show: false,
        })
        .await
}

/// A PNG's width and height, from its header.
fn png_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    assert!(
        bytes.starts_with(b"\x89PNG"),
        "{} is not a PNG",
        path.display()
    );
    let at = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
    (at(16), at(20))
}

#[tokio::test]
async fn plots_are_drawn_again_at_a_new_size() {
    let mut kernels = Vec::new();
    for kernel in ["python3", "ark", "ir"] {
        if have_kernel(kernel).await {
            kernels.push(kernel);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for kernel in kernels {
        let session = client.start_session(kernel, None).await.unwrap().id;
        let (plot, other) = if kernel == "python3" {
            (
                "import matplotlib.pyplot as plt\nplt.plot([1, 3, 2])\nplt.show()".to_string(),
                format!(
                    "import base64\nfrom IPython.display import Image, display\ndisplay(Image(base64.b64decode('{PNG}')))"
                ),
            )
        } else {
            ("plot(1:10)".to_string(), String::new())
        };
        run(&mut client, &session, &plot, None, None, false).await;
        let plots = client.session_plots(&session, None).await.unwrap().plots;
        if kernel == "python3" && plots.is_empty() {
            eprintln!("skipping Python: matplotlib is not installed");
            client.shutdown(&session).await.unwrap();
            continue;
        }
        assert_eq!(plots.len(), 1, "{kernel}: {plots:#?}");
        let asked = redraw(&mut client, &session, &plots[0], None, "png", (0.0, 0.0, 0))
            .await
            .unwrap();
        assert!(asked.can, "{kernel}: {asked:?}");

        let png = dir.path().join(format!("{kernel}/figures/plot.png"));
        let saved = redraw(
            &mut client,
            &session,
            &plots[0],
            Some(&png),
            "png",
            (6.0, 4.0, 100),
        )
        .await
        .unwrap();
        assert!(saved.can, "{kernel}: {saved:?}");
        assert_eq!(png_size(&png), (600, 400), "{kernel}");
        // Again, bigger: the plot's own size was put back in between.
        redraw(
            &mut client,
            &session,
            &plots[0],
            Some(&png),
            "png",
            (3.0, 2.0, 300),
        )
        .await
        .unwrap();
        assert_eq!(png_size(&png), (900, 600), "{kernel}");
        let svg = dir.path().join(format!("{kernel}/plot.svg"));
        redraw(
            &mut client,
            &session,
            &plots[0],
            Some(&svg),
            "svg",
            (6.0, 4.0, 100),
        )
        .await
        .unwrap();
        assert!(
            std::fs::read_to_string(&svg).unwrap().contains("<svg"),
            "{kernel}"
        );
        let pdf = dir.path().join(format!("{kernel}/plot.pdf"));
        redraw(
            &mut client,
            &session,
            &plots[0],
            Some(&pdf),
            "pdf",
            (6.0, 4.0, 100),
        )
        .await
        .unwrap();
        assert!(
            std::fs::read(&pdf).unwrap().starts_with(b"%PDF"),
            "{kernel}"
        );
        // Drawn to show at the Plots pane's size, on a Retina screen.
        let shown: PlotRedraw = client
            .call(Call::RedrawPlot {
                session: session.clone(),
                run: plots[0].run.clone(),
                image: None,
                index: 0,
                file: None,
                format: String::new(),
                width: 5.0,
                height: 3.0,
                dpi: 192,
                show: true,
            })
            .await
            .unwrap();
        let shown_png = dir.path().join(format!("{kernel}/shown.png"));
        std::fs::write(
            &shown_png,
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                shown.png.unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(png_size(&shown_png), (960, 576), "{kernel}");
        // The drawing ran out of sight: the session shows no new plots.
        let after = client.session_plots(&session, None).await.unwrap().plots;
        assert_eq!(after.len(), 1, "{kernel}: {after:#?}");

        if kernel == "python3" {
            // A plain image is no figure.
            run(&mut client, &session, &other, None, None, false).await;
            let plots = client.session_plots(&session, None).await.unwrap().plots;
            let asked = redraw(&mut client, &session, &plots[1], None, "png", (0.0, 0.0, 0))
                .await
                .unwrap();
            assert_eq!(
                asked.reason.as_deref(),
                Some("only matplotlib figures can be drawn again in Python")
            );
            // Out of range sizes are refused.
            let err = redraw(
                &mut client,
                &session,
                &plots[0],
                Some(&png),
                "png",
                (600.0, 4.0, 100),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("at that size"), "{err}");
        }
        client.shutdown(&session).await.unwrap();
    }
}

/// The output a notebook shows: the first type of the app's order
/// (`app/src/render/outputs.ts`) the bundle holds.
const SHOWN_ORDER: &[&str] = &[
    "text/html",
    "application/vnd.plotly.v1+json",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "text/markdown",
    "text/latex",
    "application/json",
    "text/plain",
];

/// An R plot shows as an image under its cell and in the Console, before
/// and after the Plots pane draws it again at its own size.
#[tokio::test]
async fn r_plots_show_under_their_cell() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for kernel in ["ark", "ir"] {
        if !have_kernel(kernel).await {
            continue;
        }
        let session = client.start_session(kernel, None).await.unwrap().id;
        let file = dir.path().join(format!("{kernel}.qmd"));
        let cell = CellRef {
            file: file.to_string_lossy().into_owned(),
            cell_id: "plot".into(),
        };
        let check = async |client: &mut Client, when: &str| {
            let cells = client.file_outputs(&cell.file).await.unwrap().cells;
            let runs = client.session_runs(&session, None).await.unwrap().runs;
            let console = &runs.last().expect("no runs").outputs;
            for outputs in [&cells[0].outputs, console] {
                let images: Vec<_> = outputs
                    .iter()
                    .filter(|o| o["output_type"] == "display_data")
                    .collect();
                assert_eq!(images.len(), 1, "{kernel} {when}: {outputs:#?}");
                let data = &images[0]["data"];
                let shown = SHOWN_ORDER.iter().find(|m| !data[**m].is_null());
                assert!(
                    shown.is_some_and(|m| m.starts_with("image/")),
                    "{kernel} {when}: shows {shown:?}: {outputs:#?}"
                );
                let mime = shown.unwrap();
                let blob = match data[*mime][BLOB_KEY].as_str() {
                    Some(hash) => client.get_blob(hash).await.unwrap().data,
                    None => data[*mime].as_str().unwrap_or_default().to_string(),
                };
                if *mime == "image/svg+xml" {
                    assert!(blob.contains("<svg"), "{kernel} {when}");
                } else {
                    let bytes = sandhill_proto::decode_base64(&blob)
                        .unwrap_or_else(|e| panic!("{kernel} {when}: {e}"));
                    assert!(
                        bytes.starts_with(b"\x89PNG"),
                        "{kernel} {when}: {mime} is {} bytes: {outputs:#?}",
                        bytes.len()
                    );
                }
            }
        };
        run(
            &mut client,
            &session,
            "plot(1:10)",
            Some(cell.clone()),
            None,
            false,
        )
        .await;
        check(&mut client, "after running").await;
        let plots = client.session_plots(&session, None).await.unwrap().plots;
        let _: PlotRedraw = client
            .call(Call::RedrawPlot {
                session: session.clone(),
                run: plots[0].run.clone(),
                image: None,
                index: 0,
                file: None,
                format: String::new(),
                width: 5.0,
                height: 3.0,
                dpi: 192,
                show: true,
            })
            .await
            .unwrap();
        check(&mut client, "after the Plots pane drew it").await;
        client.shutdown(&session).await.unwrap();
    }
}
