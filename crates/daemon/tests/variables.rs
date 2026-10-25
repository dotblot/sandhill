//! The Variables pane and the Console, against real kernels: listing a
//! session's variables and their parts, deleting them, and a session's runs
//! for its Console. Each test skips (and says so) when its kernel isn't
//! installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, CellRef, EVENT_RUN_FINISHED, RunStarted, Variable};
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
) {
    client.subscribe(session, None).await.unwrap();
    let started: RunStarted = client
        .call(Call::Execute {
            session: session.into(),
            code: code.into(),
            cell,
            quiet: false,
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

async fn list(client: &Client, session: &str, path: &[&str]) -> Vec<Variable> {
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    tokio::time::timeout(TIMEOUT, client.list_variables(session, &path))
        .await
        .expect("listing timed out")
        .unwrap_or_else(|e| panic!("listing {path:?} failed: {e}"))
        .variables
}

fn find<'a>(variables: &'a [Variable], name: &str) -> &'a Variable {
    variables
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no {name} in {variables:#?}"))
}

fn names(variables: &[Variable]) -> Vec<&str> {
    variables.iter().map(|v| v.name.as_str()).collect()
}

#[tokio::test]
async fn python_variables_are_listed_opened_and_deleted() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    run(
        &mut client,
        &session,
        "import os\nn = 3\nlabel = 'pbmc'\nconfig = {'res': 0.5, 'genes': ['CD3E', 'MS4A1']}\nxs = [1, 2, 3]\ndef qc(adata, min_genes=200): pass\nclass Cells: pass\ncells = Cells()",
        None,
        None,
    )
    .await;

    let all = list(&client, &session, &[]).await;
    assert_eq!(find(&all, "n").kind, "value");
    assert_eq!(find(&all, "n").summary, "3");
    assert_eq!(find(&all, "label").summary, "'pbmc'");
    assert_eq!(find(&all, "qc").kind, "function");
    assert!(
        find(&all, "qc").summary.contains("min_genes=200"),
        "{all:#?}"
    );
    let config = find(&all, "config");
    assert!(config.children && config.kind == "data");
    assert_eq!(find(&all, "cells").class, "Cells");
    assert!(
        !names(&all).contains(&"os") && !names(&all).contains(&"Cells"),
        "modules and classes are left out: {all:#?}"
    );
    // The listing leaves nothing in the session's history.
    let history = client.history(&session, 0, None).await.unwrap();
    assert!(
        history
            .events
            .iter()
            .all(|e| !e.content.to_string().contains("_sandhill")),
        "{:#?}",
        history.events
    );

    let parts = list(&client, &session, &["config"]).await;
    assert_eq!(names(&parts), ["res", "genes"]);
    let genes = list(&client, &session, &["config", "genes"]).await;
    assert_eq!(names(&genes), ["[0]", "[1]"]);
    assert_eq!(genes[1].summary, "'MS4A1'");
    assert!(
        client
            .list_variables(&session, &["nope".into()])
            .await
            .is_err()
    );

    let has_numpy = {
        run(&mut client, &session, "try:\n    import numpy as np\n    big = np.zeros((1000, 100))\nexcept ImportError:\n    pass", None, None).await;
        names(&list(&client, &session, &[]).await).contains(&"big")
    };
    if has_numpy {
        let all = list(&client, &session, &[]).await;
        assert_eq!(find(&all, "big").size, Some(800_000));
        assert!(
            find(&all, "big").summary.starts_with("1,000 × 100"),
            "{all:#?}"
        );
    } else {
        eprintln!("skipping the array part: numpy is not installed");
    }

    let has_pandas = {
        run(&mut client, &session, "try:\n    import pandas as pd\n    obs = pd.DataFrame({'cluster': ['0', '1', '0'], 'n_genes': [812, 1043, 655]})\nexcept ImportError:\n    pass", None, None).await;
        names(&list(&client, &session, &[]).await).contains(&"obs")
    };
    if has_pandas {
        let all = list(&client, &session, &[]).await;
        let obs = find(&all, "obs");
        assert_eq!(obs.class, "DataFrame");
        assert!(
            obs.summary.starts_with("3 × 2; columns: cluster, n_genes"),
            "{obs:?}"
        );
        let columns = list(&client, &session, &["obs"]).await;
        assert_eq!(names(&columns), ["cluster", "n_genes"]);
        assert_eq!(columns[1].class, "Series");
        assert!(columns[1].summary.contains("812, 1043, 655"), "{columns:?}");
    } else {
        eprintln!("skipping the data frame part: pandas is not installed");
    }

    let has_anndata = {
        run(&mut client, &session, "try:\n    import anndata as ad\n    import numpy as np\n    adata = ad.AnnData(np.ones((4, 3), dtype='float32'))\n    adata.obs['group'] = ['a', 'a', 'b', 'b']\n    adata.obsm['X_umap'] = np.zeros((4, 2))\nexcept ImportError:\n    pass", None, None).await;
        names(&list(&client, &session, &[]).await).contains(&"adata")
    };
    if has_anndata {
        let all = list(&client, &session, &[]).await;
        let adata = find(&all, "adata");
        assert!(adata.children);
        assert_eq!(
            adata.summary, "4 obs × 3 vars; obs: group; obsm: X_umap",
            "no layers to name"
        );
        let parts = list(&client, &session, &["adata"]).await;
        assert_eq!(names(&parts), ["X", "obs", "var", "obsm"]);
        let obs = list(&client, &session, &["adata", "obs"]).await;
        assert_eq!(names(&obs), ["group"]);
        let obsm = list(&client, &session, &["adata", "obsm"]).await;
        assert_eq!(names(&obsm), ["X_umap"]);
        assert!(obsm[0].summary.starts_with("4 × 2"), "{obsm:?}");
    } else {
        eprintln!("skipping the AnnData part: anndata is not installed");
    }

    // An output IPython keeps (Out, _) doesn't keep a deleted variable alive.
    run(
        &mut client,
        &session,
        "import weakref\ncells_ref = weakref.ref(cells)",
        None,
        None,
    )
    .await;
    run(&mut client, &session, "cells", None, None).await;
    client
        .delete_variables(
            &session,
            &[
                "cells".into(),
                "big".into(),
                "xs".into(),
                "gone_already".into(),
            ],
        )
        .await
        .unwrap();
    let all = list(&client, &session, &[]).await;
    assert!(
        ["cells", "big", "xs"]
            .iter()
            .all(|name| !names(&all).contains(name)),
        "{all:#?}"
    );
    run(
        &mut client,
        &session,
        "alive = cells_ref() is not None",
        None,
        None,
    )
    .await;
    assert_eq!(
        find(&list(&client, &session, &[]).await, "alive").summary,
        "False"
    );
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn r_variables_are_listed_opened_and_deleted() {
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
    run(
        &mut client,
        &session,
        "res <- 0.5\nmeta <- data.frame(cluster = factor(c('0', '1', '0')), n_genes = c(812L, 1043L, 655L))\nfit <- list(alpha = 1, beta = 'b', 3)\nplot_qc <- function(obj, ...) NULL\nbig <- numeric(1e6)",
        None,
        None,
    )
    .await;

    let all = list(&client, &session, &[]).await;
    assert_eq!(find(&all, "res").kind, "value");
    assert_eq!(find(&all, "res").summary, "0.5");
    let meta = find(&all, "meta");
    assert_eq!(meta.class, "data.frame");
    assert!(meta.children);
    // "×", or "x" where R's locale isn't UTF-8.
    assert!(meta.summary.starts_with("3 "), "{meta:?}");
    assert!(
        meta.summary.ends_with("2; columns: cluster, n_genes"),
        "{meta:?}"
    );
    assert_eq!(find(&all, "plot_qc").kind, "function");
    assert_eq!(find(&all, "plot_qc").summary, "function(obj, ...)");
    assert!(find(&all, "big").size.unwrap() >= 8_000_000);

    let columns = list(&client, &session, &["meta"]).await;
    assert_eq!(names(&columns), ["cluster", "n_genes"]);
    assert_eq!(columns[0].class, "factor");
    assert!(columns[0].summary.contains("2 levels"), "{columns:?}");
    assert!(columns[1].summary.contains("812, 1043, 655"), "{columns:?}");
    let items = list(&client, &session, &["fit"]).await;
    assert_eq!(names(&items), ["alpha", "beta", "[[3]]"]);
    assert_eq!(items[1].summary, "\"b\"");
    assert!(
        client
            .list_variables(&session, &["meta".into(), "nope".into()])
            .await
            .is_err()
    );

    client
        .delete_variables(&session, &["big".into(), "fit".into()])
        .await
        .unwrap();
    let all = list(&client, &session, &[]).await;
    assert!(
        !names(&all).contains(&"big") && !names(&all).contains(&"fit"),
        "{all:#?}"
    );
    assert!(names(&all).contains(&"meta"));
    client.shutdown(&session).await.unwrap();
}

/// The Console lists everything that ran in its session, from wherever:
/// typed, a script's line, a notebook cell or an agent, but not the quiet
/// runs tools use to look around.
#[tokio::test]
async fn the_console_lists_a_sessions_runs() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    let script = CellRef {
        file: dir.path().join("qc.py").to_string_lossy().into_owned(),
        cell_id: "console-0".into(),
    };
    run(&mut client, &session, "x = 40", Some(script.clone()), None).await;
    run(&mut client, &session, "x + 2", None, None).await;
    list(&client, &session, &[]).await;
    run(&mut client, &session, "print(x)", None, Some("Claude")).await;

    let runs = client.session_runs(&session, None).await.unwrap();
    let codes: Vec<&str> = runs.runs.iter().map(|r| r.code.as_str()).collect();
    assert_eq!(codes, ["x = 40", "x + 2", "print(x)"]);
    assert_eq!(runs.runs[0].cell.as_ref(), Some(&script));
    assert_eq!(runs.runs[1].outputs[0]["data"]["text/plain"], "42");
    assert_eq!(runs.runs[2].by.as_deref(), Some("Claude"));
    assert_eq!(runs.runs[2].outputs[0]["text"], "40\n");
    assert!(runs.runs.iter().all(|r| r.finished_ms.is_some()));
    assert!(runs.seq > 0);
    let last = client.session_runs(&session, Some(1)).await.unwrap();
    assert_eq!(last.runs.len(), 1);
    assert_eq!(last.runs[0].code, "print(x)");
    client.shutdown(&session).await.unwrap();
}
