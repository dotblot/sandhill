//! The Help and Packages panes against real kernels: searching help
//! topics, reading a page and a function's source, and listing installed
//! packages. Each test skips (and says so) when its kernel, or pandas,
//! isn't installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, EVENT_RUN_FINISHED, HelpTopics, RunStarted};
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

/// Runs code and waits for it to finish; returns its stdout.
async fn run(client: &mut Client, session: &str, code: &str) -> String {
    client.subscribe(session, None).await.unwrap();
    let started: RunStarted = client
        .call(Call::Execute {
            session: session.into(),
            code: code.into(),
            cell: None,
            quiet: false,
            by: None,
        })
        .await
        .unwrap();
    let mut out = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() != Some(started.run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            out.push_str(event.content["text"].as_str().unwrap_or(""));
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            break;
        }
    }
    client.unsubscribe(session).await.unwrap();
    out
}

async fn topics(client: &Client, session: &str, query: &str, package: Option<&str>) -> HelpTopics {
    tokio::time::timeout(TIMEOUT, client.help_topics(session, query, package, None))
        .await
        .expect("searching the help timed out")
        .unwrap_or_else(|e| panic!("searching for {query:?} failed: {e}"))
}

/// Asking for help leaves nothing in the session's history.
async fn left_no_trace(client: &Client, session: &str) {
    let history = client.history(session, 0, None).await.unwrap();
    assert!(
        history
            .events
            .iter()
            .all(|e| !e.content.to_string().contains("sandhill")),
        "{:#?}",
        history.events
    );
}

#[tokio::test]
async fn r_help_is_searched_read_and_packages_listed() {
    let kernel = if have_kernel("ark").await {
        "ark"
    } else if have_kernel("ir").await {
        "ir"
    } else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session(kernel, None).await.unwrap().id;
    run(&mut client, &session, "x <- 1").await;

    // An exact name first, from an attached package before others.
    let found = topics(&client, &session, "filter", None).await;
    let first = &found.topics[0];
    assert_eq!(
        (first.topic.as_str(), first.package.as_str()),
        ("filter", "stats")
    );
    assert_eq!(first.title, "Linear Filtering on a Time Series");
    assert!(
        found
            .topics
            .iter()
            .any(|t| t.topic == "Filter" && t.package == "base")
    );
    // Titles are searched too.
    let pca = topics(&client, &session, "principal components", None).await;
    assert!(pca.topics.iter().any(|t| t.topic == "prcomp"), "{pca:?}");
    // A package's own topics, a few at a time.
    let some = tokio::time::timeout(
        TIMEOUT,
        client.help_topics(&session, "", Some("stat"), Some(5)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(some.topics.len(), 5);
    assert!(some.more > 100);
    assert!(some.topics.iter().all(|t| t.package == "stats"));
    let none = topics(&client, &session, "zzzznothinglikethis", None).await;
    assert!(none.topics.is_empty() && none.more == 0);

    let page = client.help_page(&session, "lm", None).await.unwrap();
    assert_eq!(
        (page.package.as_str(), page.title.as_str()),
        ("stats", "Fitting Linear Models")
    );
    assert!(!page.version.is_empty());
    let html = page.html.unwrap();
    assert!(html.contains("<h3>Description</h3>"), "{html}");
    assert!(html.contains("href=\"../../stats/help/"), "{html}");
    assert!(
        !html.contains("<head>") && !html.contains("<body"),
        "{html}"
    );
    let filter = client
        .help_page(&session, "filter", Some("stats"))
        .await
        .unwrap();
    assert_eq!(filter.topic, "filter");
    let missing = client
        .help_page(&session, "zzzznothinglikethis", None)
        .await
        .expect_err("there's no such page")
        .to_string();
    assert!(
        missing.contains("no help for zzzznothinglikethis"),
        "{missing}"
    );

    let source = client.help_source(&session, "Filter", None).await.unwrap();
    assert_eq!(source.package, "base");
    assert!(
        source.text.starts_with("function (f, x)"),
        "{}",
        source.text
    );
    let data = client
        .help_source(&session, "iris", None)
        .await
        .expect_err("a data set has no source")
        .to_string();
    assert!(data.contains("isn't a function"), "{data}");

    let listed = client.list_packages(&session).await.unwrap();
    assert!(!listed.libraries.is_empty());
    let stats = listed.packages.iter().find(|p| p.name == "stats").unwrap();
    assert!(stats.base && stats.attached && stats.loaded, "{stats:?}");
    assert!(!stats.version.is_empty() && stats.title.contains("Stats"));
    let names: Vec<String> = listed
        .packages
        .iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "sorted by name");

    left_no_trace(&client, &session).await;
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn python_help_is_searched_read_and_packages_listed() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    let probe = run(
        &mut client,
        &session,
        "try:\n    import pandas as pd\n    print('PANDAS')\nexcept ImportError:\n    pass\n\ndef qc(x):\n    \"\"\"Keep the good cells.\n\n    More words.\"\"\"\n    return x\n",
    )
    .await;
    if !probe.contains("PANDAS") {
        eprintln!("skipping: pandas is not installed for python3");
        return;
    }

    let found = topics(&client, &session, "read_csv", None).await;
    let first = &found.topics[0];
    assert_eq!(
        (first.topic.as_str(), first.package.as_str()),
        ("pandas.read_csv", "pandas")
    );
    assert!(first.title.to_lowercase().contains("csv"), "{first:?}");
    let in_pandas = topics(&client, &session, "", Some("pandas")).await;
    assert!(in_pandas.topics.iter().all(|t| t.package == "pandas"));
    assert!(in_pandas.more > 0);
    assert!(
        topics(&client, &session, "len", Some("builtins"))
            .await
            .topics[0]
            .topic
            == "builtins.len"
    );

    // The user's own name for a module works, and is shown as the module's.
    let page = client.help_page(&session, "pd.concat", None).await.unwrap();
    assert_eq!(
        (page.topic.as_str(), page.package.as_str()),
        ("pandas.concat", "pandas")
    );
    assert_eq!(page.kind.as_deref(), Some("function"));
    assert!(page.signature.as_deref().unwrap().starts_with('('));
    assert!(page.text.as_deref().unwrap().contains("Concatenate"));
    assert!(!page.version.is_empty() && page.html.is_none());
    let class = client
        .help_page(&session, "pandas.DataFrame", None)
        .await
        .unwrap();
    assert_eq!(class.kind.as_deref(), Some("class"));
    assert!(
        class
            .members
            .iter()
            .any(|m| m.name == "merge" && m.kind == "function"),
        "{:?}",
        class.members
    );
    let module = client.help_page(&session, "json", None).await.unwrap();
    assert_eq!(module.kind.as_deref(), Some("module"));
    assert!(module.members.iter().any(|m| m.name == "dumps"));
    let mine = client.help_page(&session, "qc", None).await.unwrap();
    assert_eq!(mine.title, "Keep the good cells.");
    assert_eq!(mine.signature.as_deref(), Some("(x)"));
    let missing = client
        .help_page(&session, "pd.nothing_like_this", None)
        .await
        .expect_err("there's no such page")
        .to_string();
    assert!(
        missing.contains("no help for pd.nothing_like_this"),
        "{missing}"
    );

    // Code run in the kernel keeps its source, so the user's own functions
    // have it too.
    let source = client.help_source(&session, "qc", None).await.unwrap();
    assert!(source.text.starts_with("def qc(x):"), "{}", source.text);
    let concat = client
        .help_source(&session, "pandas.concat", None)
        .await
        .unwrap();
    assert!(concat.text.contains("def concat"), "{}", concat.text);
    assert!(concat.file.as_deref().unwrap().ends_with(".py"));
    assert!(concat.line.unwrap() > 0);
    let builtin = client
        .help_source(&session, "len", None)
        .await
        .expect_err("a built-in has no Python source")
        .to_string();
    assert!(builtin.contains("no Python source"), "{builtin}");

    let listed = client.list_packages(&session).await.unwrap();
    let pandas = listed
        .packages
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case("pandas"))
        .unwrap();
    assert!(pandas.attached && pandas.loaded, "{pandas:?}");
    assert_eq!(pandas.modules, ["pandas"]);
    assert!(listed.packages.iter().any(|p| !p.loaded));
    assert_eq!(listed.libraries.len(), 1);

    left_no_trace(&client, &session).await;
    client.shutdown(&session).await.unwrap();
}
