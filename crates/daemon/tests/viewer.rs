//! The Viewer's side of the daemon against real kernels: R's htmlwidgets
//! and pages, which outputs are pages, the page server, and tunnels that
//! carry its pages (and apps) to another machine. Each test skips (and
//! says so) when its kernel isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{EVENT_RUN_FINISHED, SessionPlots};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener};

const TIMEOUT: Duration = Duration::from_secs(90);
const PAGE: &str = "application/vnd.sandhill.page+json";

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
    state: PathBuf,
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
            state: state.to_path_buf(),
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }

    /// The end of a session's kernel log, to say what a kernel was doing.
    fn kernel_log(&self, session: &str) -> String {
        let log = std::fs::read_to_string(self.state.join(format!("kernels/{session}.log")))
            .unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }
}

async fn kernels(client: &Client) -> Vec<String> {
    client
        .list_kernelspecs()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.name)
        .collect()
}

/// Runs code and returns its display outputs' data.
async fn displays(
    daemon: &TestDaemon,
    client: &mut Client,
    session: &str,
    code: &str,
) -> Vec<Value> {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, None).await.unwrap().run;
    let mut found = Vec::new();
    let mut seen = Vec::new();
    loop {
        let Ok(event) = tokio::time::timeout(TIMEOUT, client.next_event()).await else {
            panic!(
                "timed out waiting for `{code}`, having seen {seen:?}; the kernel's log ends:\n{}",
                daemon.kernel_log(session)
            );
        };
        let event = event.expect("connection closed");
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        let content: String = event.content.to_string().chars().take(300).collect();
        seen.push(format!("{}: {content}", event.msg_type));
        match event.msg_type.as_str() {
            "display_data" | "execute_result" => found.push(event.content["data"].clone()),
            "error" => panic!("the run failed: {}", event.content),
            "stream" => assert!(
                !event.content["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains('\x1b'),
                "the page's name was left in the output: {}",
                event.content
            ),
            EVENT_RUN_FINISHED => return found,
            _ => {}
        }
    }
}

/// The text of a value that may be in the blob store.
async fn text(client: &Client, value: &Value) -> String {
    match value.get("sandhill_blob").and_then(Value::as_str) {
        Some(hash) => client.get_blob(hash).await.unwrap().data,
        None => value.as_str().unwrap_or_default().to_string(),
    }
}

async fn r_has(client: &mut Client, session: &str, package: &str) -> bool {
    let code = format!("cat(requireNamespace('{package}', quietly = TRUE))");
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, &code, None).await.unwrap().run;
    let mut out = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .unwrap()
            .unwrap();
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            out.push_str(event.content["text"].as_str().unwrap_or_default());
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            return out.trim() == "TRUE";
        }
    }
}

#[tokio::test]
async fn r_pages_and_widgets_reach_the_viewer_in_every_r_kernel() {
    // The daemon's warnings, such as a kernel message it couldn't read.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let tmp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&tmp.path().join("state")).await;
    let mut client = daemon.client().await;
    let names = kernels(&client).await;
    let r: Vec<&str> = ["ir", "ark"]
        .into_iter()
        .filter(|k| names.iter().any(|n| n == k))
        .collect();
    if r.is_empty() {
        eprintln!("skipping: no R kernel (IRkernel or Ark) is installed");
        return;
    }
    for kernel in r {
        let session = client.start_session(kernel, None).await.unwrap().id;

        // A page htmltools saves and hands to the viewer, as print() of an
        // htmlwidget does.
        let shown = displays(
            &daemon,
            &mut client,
            &session,
            "htmltools::html_print(htmltools::tags$b('hi'))",
        )
        .await;
        let page = shown
            .iter()
            .find(|d| d.get(PAGE).is_some())
            .unwrap_or_else(|| panic!("{kernel} showed no page: {shown:?}"));
        assert!(page[PAGE]["path"].as_str().unwrap().ends_with("index.html"));
        assert!(
            text(&client, &page["text/html"])
                .await
                .contains("<b>hi</b>")
        );

        if r_has(&mut client, &session, "DT").await {
            // Ark prints a widget through the viewer; IRkernel shows its own HTML.
            let shown = displays(&daemon, &mut client, &session, "DT::datatable(head(iris))").await;
            let html = shown
                .iter()
                .find(|d| d.get("text/html").is_some())
                .unwrap_or_else(|| panic!("{kernel} showed no widget: {shown:?}"));
            let html = text(&client, &html["text/html"]).await;
            assert!(
                html.contains("datatables"),
                "{kernel}: {}",
                &html[..html.len().min(300)]
            );
            if kernel == "ark" {
                // What Ark showed, and the start of its page.
                let seen = format!("{:.1500}\npage: {html:.1500}", format!("{shown:?}"));
                assert_eq!(shown[0][PAGE]["title"], "datatables", "{seen}");
                // Its scripts are inside it, for a notebook to keep.
                assert!(!html.contains("src=\"lib/"), "{seen}");
            }
        } else {
            eprintln!("{kernel}: skipping the DT widget: DT is not installed");
        }

        let pages: SessionPlots = client.session_pages(&session, None).await.unwrap();
        assert!(!pages.plots.is_empty(), "{kernel}: {pages:?}");
        client.shutdown(&session).await.unwrap();
    }
}

#[tokio::test]
async fn the_viewer_takes_pages_but_leaves_tables_to_the_console() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&tmp.path().join("state")).await;
    let mut client = daemon.client().await;
    if !kernels(&client).await.iter().any(|k| k == "python3") {
        eprintln!("skipping: Jupyter kernel \"python3\" is not installed");
        return;
    }
    let session = client.start_session("python3", None).await.unwrap().id;
    let code = "from IPython.display import HTML, display\n\
                display(HTML('<table><tr><td>1</td></tr></table>'))\n\
                display(HTML('<div id=\"x\"></div><script>x.textContent = \"plotly\"</script>'))";
    assert_eq!(
        displays(&daemon, &mut client, &session, code).await.len(),
        2
    );
    let pages = client.session_pages(&session, None).await.unwrap();
    assert_eq!(pages.plots.len(), 1, "{pages:?}");
    assert!(
        pages.plots[0].data["text/html"]
            .as_str()
            .unwrap()
            .contains("<script>")
    );
    client.shutdown(&session).await.unwrap();
}

async fn get(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

#[tokio::test]
async fn pages_reach_another_machine_through_a_tunnel() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let report = root.join("reports");
    std::fs::create_dir_all(report.join("multiqc_data")).unwrap();
    std::fs::write(
        report.join("multiqc_report.html"),
        "<link rel=stylesheet href=\"multiqc_data/a.css\"><h1>MultiQC</h1>",
    )
    .unwrap();
    std::fs::write(report.join("multiqc_data/a.css"), "h1 { color: teal }").unwrap();
    let daemon = TestDaemon::start(&root.join("state")).await;
    let client = Arc::new(daemon.client().await);

    let page = client
        .serve_page(&report.join("multiqc_report.html").to_string_lossy())
        .await
        .unwrap();
    // This machine's end: a port of its own, carried over the client's connection.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let here = listener.local_addr().unwrap().port();
    let carried = client.clone();
    let forward = tokio::spawn(sandhill_link::forward::forward(
        listener,
        page.port,
        move || Some(carried.clone()),
    ));
    let response = get(here, &page.path).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("<h1>MultiQC</h1>"), "{response}");
    let css = get(
        here,
        &page
            .path
            .replace("multiqc_report.html", "multiqc_data/a.css"),
    )
    .await;
    assert!(css.ends_with("h1 { color: teal }"), "{css}");

    // A big page comes through whole, in many pieces.
    let big = format!("<pre>{}</pre>", "ACGT".repeat(300_000));
    let page = client.serve_html(&big).await.unwrap();
    let response = get(here, &page.path).await;
    assert!(response.ends_with(&big), "{} bytes", response.len());
    forward.abort();

    // Nothing listening: the tunnel says so.
    let free = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let closed = free.local_addr().unwrap().port();
    drop(free);
    let err = client.open_tunnel(closed).await.err().unwrap().to_string();
    assert!(err.contains("nothing answers"), "{err}");
}

#[tokio::test]
async fn r_apps_open_in_the_viewer_and_stop_when_interrupted() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(&tmp.path().join("state")).await;
    let names = kernels(&daemon.client().await).await;
    let r: Vec<&str> = ["ir", "ark"]
        .into_iter()
        .filter(|k| names.iter().any(|n| n == k))
        .collect();
    if r.is_empty() {
        eprintln!("skipping: no R kernel (IRkernel or Ark) is installed");
        return;
    }
    for kernel in r {
        let mut client = daemon.client().await;
        let session = client.start_session(kernel, None).await.unwrap().id;

        // browseURL(): a web page goes to the browser, an app's address to the Viewer.
        let shown = displays(
            &daemon,
            &mut client,
            &session,
            "utils::browseURL('https://bioconductor.org/'); utils::browseURL('http://127.0.0.1:9/x')",
        )
        .await;
        assert_eq!(shown.len(), 2, "{kernel}: {shown:?}");
        assert_eq!(shown[0][PAGE]["browser"], true, "{kernel}: {shown:?}");
        assert_eq!(shown[1][PAGE]["url"], "http://127.0.0.1:9/x", "{kernel}");
        assert!(shown[1][PAGE].get("browser").is_none(), "{kernel}");

        if !r_has(&mut client, &session, "shiny").await {
            eprintln!("{kernel}: skipping the Shiny app: shiny is not installed");
            client.shutdown(&session).await.unwrap();
            continue;
        }
        // A Shiny app printed at the top level, as a script's last line is.
        let free = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let code = format!(
            "shiny::shinyApp(shiny::fluidPage(shiny::h1('sandhill cranes')), function(input, output) {{}}, options = list(port = {port}))"
        );
        client.subscribe(&session, None).await.unwrap();
        let run = client.execute(&session, &code, None).await.unwrap().run;
        let url = loop {
            let event = tokio::time::timeout(TIMEOUT, client.next_event())
                .await
                .expect("timed out waiting for the app")
                .expect("connection closed");
            if event.run.as_deref() != Some(run.as_str()) {
                continue;
            }
            match event.msg_type.as_str() {
                "display_data" => {
                    if let Some(url) = event.content["data"][PAGE]["url"].as_str() {
                        break url.to_string();
                    }
                }
                "error" => panic!("{kernel}: the app failed: {}", event.content),
                EVENT_RUN_FINISHED => panic!("{kernel}: the app ended without showing"),
                _ => {}
            }
        };
        assert_eq!(url, format!("http://127.0.0.1:{port}"), "{kernel}");

        // The app answers through a tunnel while the run goes on.
        let mut events = client.take_events().unwrap();
        let client = Arc::new(client);
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let here = listener.local_addr().unwrap().port();
        let carried = client.clone();
        let forward = tokio::spawn(sandhill_link::forward::forward(listener, port, move || {
            Some(carried.clone())
        }));
        let page = get(here, "/").await;
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{kernel}: {page}");
        assert!(page.contains("sandhill cranes"), "{kernel}: {page}");
        forward.abort();

        // Stop: an interrupt ends the app, and its run.
        client.interrupt(&session).await.unwrap();
        loop {
            let event = tokio::time::timeout(TIMEOUT, events.recv())
                .await
                .expect("timed out waiting for the app to stop")
                .expect("connection closed");
            if event.run.as_deref() == Some(run.as_str()) && event.msg_type == EVENT_RUN_FINISHED {
                break;
            }
        }
        let pages = client.session_pages(&session, None).await.unwrap();
        assert!(
            pages
                .plots
                .iter()
                .any(|p| p.data[PAGE]["url"] == url.as_str()),
            "{kernel}: {pages:?}"
        );
        client.shutdown(&session).await.unwrap();
    }
}
