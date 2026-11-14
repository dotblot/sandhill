//! Viewer context uses journaled pages, not kernel evaluation or browser code.
//! Real htmlwidget cases skip with a message when R or its packages are absent.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::blobs::BlobStore;
use sandhill_daemon::journal::{Journal, NewRun, SessionRecord};
use sandhill_daemon::mcp::{Tools, read_only_tools};
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{EVENT_RUN_FINISHED, SessionStatus};
use serde_json::{Value, json};
use tokio::net::UnixListener;

const PAGE: &str = "application/vnd.sandhill.page+json";
const TIMEOUT: Duration = Duration::from_secs(90);
const HTML: &str = r#"<!doctype html><html><head><title>Cells &amp; genes</title>
<style>.private { display: none }</style><script>const SECRET_SCRIPT = 'never return this';</script></head>
<body><h1>Cell counts</h1><p>Visible &amp; useful</p><p hidden>HIDDEN_TEXT</p>
<div style="display: none !important">HIDDEN_STYLE</div><template>TEMPLATE_TEXT</template>
<table><thead><tr><th>Gene</th><th>Count</th></tr></thead><tbody>
<tr><td><b>CD3E</b></td><td>12</td></tr><tr><td>MS4A1</td><td>7</td></tr>
</tbody></table><div class="leaflet html-widget" id="map"></div>
<div class="plotly html-widget" id="plot"></div>
<script type="application/json" data-for="plot">{"x":{"data":[{"type":"scatter","name":"T cells","x":[1,2,3],"y":[12,13,14]}],"layout":{"xaxis":{"title":{"text":"Time"}},"yaxis":{"title":"Count"}}},"evals":[],"jsHooks":{"render":[{"code":"ANOTHER_SECRET_SCRIPT"}]}}</script>
</body></html>"#;

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
    journal: Journal,
}

impl TestDaemon {
    async fn start(root: &Path) -> Self {
        let state = StateDir::new(root);
        let daemon = Daemon::open(state.clone()).await.unwrap();
        let socket = root.join("test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        let journal = Journal::open(&state.db_path(), BlobStore::new(state.blobs_dir())).unwrap();
        Self {
            _daemon: daemon,
            socket,
            journal,
        }
    }

    fn tools(&self, project: Option<&str>) -> Tools {
        Tools::new(self.socket.clone(), project.map(str::to_string))
    }

    fn saved_session(&self, id: &str, project: &str) {
        self.journal
            .insert_session(&SessionRecord {
                id: id.into(),
                kernel: "fixture".into(),
                language: "r".into(),
                cwd: project.into(),
                connection_file: PathBuf::new(),
                client_session: id.into(),
                pid: None,
                interrupt_mode: "signal".into(),
                status: SessionStatus::Ended,
                created_ms: 1,
                ended_ms: Some(2),
                project: Some(project.into()),
                language_version: None,
            })
            .unwrap();
    }

    fn page(&self, session: &str, id: &str, html: &str) {
        self.journal
            .insert_run(id, session, id, "", NewRun::default())
            .unwrap();
        self.journal
            .append(
                session,
                Some(id),
                "display_data",
                json!({
                    "data": {PAGE: {"title": "Fallback"}, "text/html": html},
                    "metadata": {"text/html": {"isolated": true}},
                }),
            )
            .unwrap();
    }
}

async fn viewer(tools: &Tools, args: Value) -> Value {
    let items = tokio::time::timeout(TIMEOUT, tools.call("get_viewer", &args))
        .await
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&items).unwrap().len() <= 64 * 1024);
    assert_eq!(items.len(), 1);
    serde_json::from_str(items[0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn plain_html_returns_text_tables_and_declarative_plots_but_never_scripts() {
    assert!(read_only_tools().iter().any(|name| name == "get_viewer"));
    let temp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(temp.path()).await;
    daemon.saved_session("saved-r", "/project");
    daemon.page("saved-r", "page-1", HTML);
    let summary = viewer(&daemon.tools(Some("/project")), json!({"session": "saved"})).await;
    assert_eq!(summary["title"], "Cells & genes");
    assert!(
        summary["visible_text"]
            .as_str()
            .unwrap()
            .contains("Visible & useful")
    );
    assert_eq!(summary["tables"][0]["columns"], json!(["Gene", "Count"]));
    assert_eq!(
        summary["tables"][0]["rows"],
        json!([["CD3E", "12"], ["MS4A1", "7"]])
    );
    assert!(
        summary["widget_types"]
            .as_array()
            .unwrap()
            .contains(&json!("leaflet"))
    );
    assert_eq!(
        summary["plot_series"][0],
        json!({"type": "scatter", "name": "T cells", "point_count": 3, "axis_titles": {"x": "Time", "y": "Count"}})
    );
    let text = summary.to_string();
    for secret in [
        "SECRET_SCRIPT",
        "ANOTHER_SECRET_SCRIPT",
        "HIDDEN_TEXT",
        "HIDDEN_STYLE",
        "TEMPLATE_TEXT",
        "jsHooks",
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
    assert!(
        summary["limitation"]
            .as_str()
            .unwrap()
            .contains("interaction state")
    );
}

#[tokio::test]
async fn default_selects_latest_page_in_project_not_the_latest_session_activity() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(temp.path()).await;
    for (id, project) in [
        ("first", "/project"),
        ("second", "/project"),
        ("other", "/other"),
    ] {
        daemon.saved_session(id, project);
    }
    daemon.page("first", "first-page", "<title>Older</title><p>one</p>");
    daemon.page(
        "second",
        "second-page",
        "<title>Latest project page</title><p>two</p>",
    );
    daemon
        .journal
        .append(
            "first",
            None,
            "stream",
            json!({"name": "stdout", "text": "new activity"}),
        )
        .unwrap();
    daemon.page("other", "other-page", "<title>Other project</title>");
    let tools = daemon.tools(Some("/project"));
    assert_eq!(
        viewer(&tools, json!({})).await["title"],
        "Latest project page"
    );
    assert_eq!(
        viewer(&tools, json!({"session": "first"})).await["title"],
        "Older"
    );
    assert_eq!(
        viewer(&daemon.tools(Some("/empty")), json!({})).await["title"],
        Value::Null
    );
}

#[tokio::test]
async fn large_blob_pages_remain_valid_json_under_the_total_cap() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(temp.path()).await;
    daemon.saved_session("large", "/project");
    let cell = "\"\\细胞".repeat(150);
    let row = format!("<tr><td>{cell}</td><td>{cell}</td><td>{cell}</td><td>{cell}</td></tr>");
    let table = format!(
        "<table><tr><th>A</th><th>B</th><th>C</th><th>D</th></tr>{}</table>",
        row.repeat(30)
    );
    let html = format!(
        "<title>Large page</title><p>{}</p>{}",
        "细胞 ".repeat(10_000),
        table.repeat(8)
    );
    daemon.page("large", "large-page", &html);
    let summary = viewer(&daemon.tools(None), json!({})).await;
    assert_eq!(summary["title"], "Large page");
    assert_eq!(summary["truncated"], true);
    assert!(
        summary["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["rows"].as_array().unwrap().len() <= 20)
    );
    assert!(summary["visible_text"].as_str().unwrap().len() <= 16 * 1024);
}

#[tokio::test]
async fn python_plotly_html_and_mime_figures_have_series_summaries() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(temp.path()).await;
    daemon.saved_session("python", "/project");
    daemon.page("python", "python-page", r#"<div class="plotly-graph-div" id="p"></div><script>Plotly.newPlot("p", [{"type":"bar","name":"genes","x":["A","B"],"y":[4,8]}], {"xaxis":{"title":{"text":"Gene"}}}, {}); const NEVER_SEND = true;</script>"#);
    let summary = viewer(&daemon.tools(None), json!({})).await;
    assert_eq!(summary["plot_series"][0]["point_count"], 2);
    assert_eq!(summary["plot_series"][0]["axis_titles"]["x"], "Gene");
    assert!(!summary.to_string().contains("NEVER_SEND"));
    daemon
        .journal
        .insert_run("mime", "python", "mime", "", NewRun::default())
        .unwrap();
    daemon.journal.append("python", Some("mime"), "display_data", json!({"data": {
        "application/vnd.plotly.v1+json": {"data": [{"type": "scatter", "name": "typed", "x": {"dtype": "f8", "bdata": "AAAAAAAA8D8AAAAAAAAAQA=="}}], "layout": {}}
    }, "metadata": {}})).unwrap();
    let summary = viewer(&daemon.tools(None), json!({})).await;
    assert_eq!(summary["plot_series"][0]["name"], "typed");
    assert_eq!(summary["plot_series"][0]["point_count"], 2);
}

async fn execute(client: &mut Client, session: &str, code: &str) -> String {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, None).await.unwrap().run;
    let mut text = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("R timed out")
            .unwrap();
        if event.run.as_deref() != Some(&run) {
            continue;
        }
        match event.msg_type.as_str() {
            "error" => panic!("R failed: {}", event.content),
            "stream" => text.push_str(event.content["text"].as_str().unwrap_or_default()),
            EVENT_RUN_FINISHED => return text,
            _ => {}
        }
    }
}

async fn real_widget(package: &str) {
    let temp = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(temp.path()).await;
    let mut client = Client::connect_unix(&daemon.socket).await.unwrap();
    let kernels: Vec<_> = client
        .list_kernelspecs()
        .await
        .unwrap()
        .into_iter()
        .filter(|k| k.language.eq_ignore_ascii_case("r"))
        .collect();
    if kernels.is_empty() {
        eprintln!("skipping {package} Viewer context: no R kernel is installed");
        return;
    }
    for kernel in kernels {
        let session = client.start_session(&kernel.name, None).await.unwrap().id;
        let available = execute(&mut client, &session, &format!("cat(requireNamespace('{package}', quietly=TRUE) && requireNamespace('htmlwidgets', quietly=TRUE))")).await;
        if available.trim() != "TRUE" {
            eprintln!(
                "skipping {} {package} Viewer context: {package}/htmlwidgets is not installed",
                kernel.name
            );
            client.shutdown(&session).await.unwrap();
            continue;
        }
        let widget = if package == "DT" {
            "DT::datatable(data.frame(gene=paste0('gene', 1:30), count=1:30), rownames=FALSE)"
        } else {
            "plotly::layout(plotly::plot_ly(x=1:5, y=2*(1:5), type='scatter', mode='lines', name='counts'), xaxis=list(title='Cell'), yaxis=list(title='Count'))"
        };
        execute(&mut client, &session, &format!("local({{ page <- tempfile(fileext='.html'); htmlwidgets::saveWidget({widget}, page, selfcontained=FALSE); getOption('viewer')(page) }})")).await;
        let summary = viewer(&daemon.tools(None), json!({"session": session})).await;
        assert!(
            summary["widget_types"]
                .as_array()
                .unwrap()
                .contains(&json!(package)),
            "{summary}"
        );
        if package == "DT" {
            let table = summary["tables"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["kind"] == "DT")
                .expect("DT table missing");
            assert_eq!(table["rows"].as_array().unwrap().len(), 20);
            assert_eq!(table["columns"], json!(["gene", "count"]));
            assert_eq!(table["rows"][0], json!(["gene1", "1"]));
        } else {
            assert_eq!(summary["plot_series"][0]["type"], "scatter");
            assert_eq!(summary["plot_series"][0]["name"], "counts");
            assert_eq!(summary["plot_series"][0]["point_count"], 5);
            assert_eq!(
                summary["plot_series"][0]["axis_titles"],
                json!({"x": "Cell", "y": "Count"})
            );
        }
        client.shutdown(&session).await.unwrap();
    }
}

#[tokio::test]
async fn real_r_dt_widget_returns_the_first_twenty_rows() {
    real_widget("DT").await;
}

#[tokio::test]
async fn real_r_plotly_widget_returns_series_and_axes() {
    real_widget("plotly").await;
}
