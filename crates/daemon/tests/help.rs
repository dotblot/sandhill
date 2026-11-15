//! The Help and Packages panes against real kernels: searching help
//! topics, reading a page and a function's source, and listing installed
//! packages. Each test skips (and says so) when its kernel, or pandas,
//! isn't installed.

mod common;

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
    let found = sandhill_daemon::kernel::find_kernelspec(name, None)
        .await
        .is_ok();
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
    let dir = common::tempdir().unwrap();
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
    assert!(none.vignettes.is_empty());
    // A package's vignettes come with its topics, as files the app reads.
    let grid = tokio::time::timeout(
        TIMEOUT,
        client.help_topics(&session, "", Some("grid"), Some(5)),
    )
    .await
    .unwrap()
    .unwrap();
    if grid.vignettes.is_empty() {
        eprintln!("skipping vignettes: this R has none built for grid");
    } else {
        let intro = grid.vignettes.iter().find(|v| v.name == "grid");
        let intro = intro.expect("grid's own vignette");
        assert_eq!(intro.title, "Introduction to grid");
        assert!(std::path::Path::new(&intro.file).is_file(), "{intro:?}");
        assert!(intro.file.ends_with(".pdf") || intro.file.ends_with(".html"));
        assert!(grid.vignettes.iter().all(|v| v.package == "grid"));
        // And a search finds them by title.
        let found = topics(&client, &session, "introduction to grid", None).await;
        assert!(
            found.vignettes.iter().any(|v| v.name == "grid"),
            "{found:?}"
        );
    }

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

/// A stand-in for PyPI's JSON API: every package's latest is 999.0, but
/// `pip` isn't on it.
fn fake_index() -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/pypi", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut request = String::new();
            let _ = BufReader::new(&stream).read_line(&mut request);
            let (status, body) = if request.contains("/pip/json") {
                ("404 Not Found", "{}".to_string())
            } else {
                ("200 OK", r#"{"info": {"version": "999.0"}}"#.to_string())
            };
            let _ = write!(
                &stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

#[tokio::test]
async fn python_updates_are_asked_of_the_index() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    let index = fake_index();
    run(
        &mut client,
        &session,
        &format!("__import__('os').environ['SANDHILL_PYPI_URL'] = {index:?}"),
    )
    .await;
    let listed = client.list_packages(&session).await.unwrap();
    let found = client.package_updates(&session).await.unwrap();
    // A conda environment is asked of conda's channels as well; this test is
    // about the index.
    assert!(found.repos.contains(&index), "{:?}", found.repos);
    assert!(!found.unreached.contains(&index), "{:?}", found.unreached);
    // What pip installed, from an index, is asked about; pip itself isn't on
    // this one.
    let asked: Vec<_> = listed
        .packages
        .iter()
        .filter(|p| p.installer != "conda" && p.name != "pip")
        .collect();
    // Conda may know newer versions of its own; the index's are all 999.0.
    let from_index: Vec<_> = found
        .updates
        .iter()
        .filter(|u| u.available == "999.0")
        .collect();
    assert!(!from_index.is_empty(), "{listed:?}");
    for update in &from_index {
        assert!(
            asked
                .iter()
                .any(|p| p.name == update.name && p.version == update.installed),
            "{update:?}"
        );
    }
    let pips_pip = listed
        .packages
        .iter()
        .any(|p| p.name == "pip" && p.installer != "conda");
    if pips_pip {
        assert!(!found.updates.iter().any(|u| u.name == "pip"));
    }

    // An index that can't be reached says so.
    run(
        &mut client,
        &session,
        "__import__('os').environ['SANDHILL_PYPI_URL'] = 'http://127.0.0.1:9/pypi'",
    )
    .await;
    // Alone it fails the call; beside conda's channels it is named as missed.
    match client.package_updates(&session).await {
        Err(err) => {
            let err = err.to_string();
            assert!(
                err.contains("couldn't reach http://127.0.0.1:9/pypi"),
                "{err}"
            );
        }
        Ok(found) => assert!(
            found
                .unreached
                .iter()
                .any(|r| r.contains("127.0.0.1:9/pypi")),
            "{found:?}"
        ),
    }
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn python_help_is_searched_read_and_packages_listed() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
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

#[tokio::test]
async fn r_updates_come_from_the_repositories() {
    let kernel = if have_kernel("ark").await {
        "ark"
    } else if have_kernel("ir").await {
        "ir"
    } else {
        return;
    };
    // A repository of its own, with a jsonlite (which IRkernel needs) newer than any.
    let repo = common::tempdir().unwrap();
    let contrib = repo.path().join("src/contrib");
    std::fs::create_dir_all(&contrib).unwrap();
    std::fs::write(
        contrib.join("PACKAGES"),
        "Package: jsonlite\nVersion: 999.0.0\n\n",
    )
    .unwrap();
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session(kernel, None).await.unwrap().id;
    let url = format!("file://{}", repo.path().display());
    run(
        &mut client,
        &session,
        &format!("options(repos = c(CRAN = \"{url}\"))"),
    )
    .await;
    let checked = tokio::time::timeout(Duration::from_secs(200), client.package_updates(&session))
        .await
        .expect("checking for updates timed out")
        .unwrap();
    let json = checked.updates.iter().find(|u| u.name == "jsonlite");
    let json = json.unwrap_or_else(|| panic!("{checked:?}"));
    assert_eq!(json.available, "999.0.0");
    assert!(!json.installed.is_empty() && !json.library.is_empty() && !json.bioconductor);
    assert!(checked.repos.contains(&url), "{checked:?}");
    assert!(!checked.unreached.contains(&url), "{checked:?}");
    // A repository that can't be reached says so, rather than "up to date".
    let gone = repo.path().join("gone");
    run(
        &mut client,
        &session,
        &format!("options(repos = c(CRAN = \"file://{}\"))", gone.display()),
    )
    .await;
    let err = client
        .package_updates(&session)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("couldn't reach"), "{err}");
    left_no_trace(&client, &session).await;
    client.shutdown(&session).await.unwrap();
}
