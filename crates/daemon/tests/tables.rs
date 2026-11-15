//! The Data Explorer against real kernels: a table's rows, filtered and
//! sorted in the kernel, its column summaries, and exporting it as CSV.
//! Each test skips (and says so) when its kernel, or pandas, isn't
//! installed.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, CellRef, EVENT_RUN_FINISHED, RunStarted, TableFilter, TableRows, TableSort, TableSummary,
};
use serde_json::{Value, json};
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

async fn rows(
    client: &Client,
    session: &str,
    path: &[&str],
    offset: u64,
    sort: &[TableSort],
    filters: &[TableFilter],
) -> TableRows {
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    tokio::time::timeout(
        TIMEOUT,
        client.table_rows(session, &path, offset, None, sort, filters, ""),
    )
    .await
    .expect("reading the table timed out")
    .unwrap_or_else(|e| panic!("reading {path:?} failed: {e}"))
}

fn filter(column: u32, op: &str, values: &[&str]) -> TableFilter {
    TableFilter {
        column,
        op: op.into(),
        values: values.iter().map(|s| s.to_string()).collect(),
    }
}

fn column(rows: &TableRows, i: usize) -> Vec<Value> {
    rows.values.iter().map(|r| r[i].clone()).collect()
}

/// The same table in either language, and what both must show of it:
/// `text` is what the text column's type may be called, `types` the rest.
async fn check_meta(client: &Client, session: &str, code: &str, text: &[&str], types: [&str; 3]) {
    let all = rows(client, session, &["meta"], 0, &[], &[]).await;
    assert_eq!((all.total, all.rows, all.width), (5, 5, 4));
    assert_eq!(all.code, code);
    assert!(!all.matrix);
    let names: Vec<&str> = all.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["cell", "cluster", "n_genes", "pct"]);
    let kinds: Vec<&str> = all.columns.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["text", "category", "number", "number"]);
    let detail: Vec<&str> = all.columns.iter().map(|c| c.kind_detail.as_str()).collect();
    assert!(text.contains(&detail[0]), "{detail:?}");
    assert_eq!(detail[1..], types);
    assert_eq!(
        all.labels,
        ["AAAC-1", "AAAC-2", "AAAC-3", "AAAC-4", "AAAC-5"]
    );
    assert_eq!(
        column(&all, 2),
        [json!(812), json!(1043), json!(655), Value::Null, json!(990)]
    );
    assert_eq!(
        all.values[1],
        [json!("b\tB"), json!("1"), json!(1043), json!(4.25)]
    );

    // pct < 5 and cluster in 0, 1; most genes first.
    let filters = [filter(3, "<", &["5"]), filter(1, "in", &["0", "1"])];
    let sort = [TableSort {
        column: 2,
        descending: true,
    }];
    let kept = rows(client, session, &["meta"], 0, &sort, &filters).await;
    assert_eq!((kept.total, kept.rows), (5, 3));
    assert_eq!(kept.labels, ["AAAC-2", "AAAC-1", "AAAC-3"]);
    let page = rows(client, session, &["meta"], 1, &sort, &filters).await;
    assert_eq!(page.labels, ["AAAC-1", "AAAC-3"]);
    assert_eq!(page.offset, 1);
    // Missing values sort last either way.
    let up = rows(
        client,
        session,
        &["meta"],
        0,
        &[TableSort {
            column: 2,
            descending: false,
        }],
        &[],
    )
    .await;
    assert_eq!(
        up.labels,
        ["AAAC-3", "AAAC-1", "AAAC-5", "AAAC-2", "AAAC-4"]
    );
    let missing = rows(
        client,
        session,
        &["meta"],
        0,
        &[],
        &[filter(2, "missing", &[])],
    )
    .await;
    assert_eq!(missing.labels, ["AAAC-4"]);
    let text = rows(
        client,
        session,
        &["meta"],
        0,
        &[],
        &[filter(0, "contains", &["b"])],
    )
    .await;
    assert_eq!(text.labels, ["AAAC-2"]);
    let between = rows(
        client,
        session,
        &["meta"],
        0,
        &[],
        &[filter(3, "between", &["1", "4.25"])],
    )
    .await;
    assert_eq!(between.labels, ["AAAC-1", "AAAC-2", "AAAC-4"]);

    let summary = tokio::time::timeout(
        TIMEOUT,
        client.table_summary(session, &["meta".into()], &[], ""),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(summary.rows, 5);
    let genes = &summary.columns[2];
    assert_eq!((genes.missing, genes.distinct), (1, Some(4)));
    assert_eq!(
        (genes.min, genes.max, genes.median),
        (Some(655.0), Some(1043.0), Some(901.0))
    );
    assert_eq!(genes.histogram.iter().sum::<u64>(), 4);
    let clusters = &summary.columns[1];
    let top: Vec<(&str, u64)> = clusters
        .top
        .iter()
        .map(|t| (t.value.as_str(), t.count))
        .collect();
    assert_eq!(top, [("0", 2), ("1", 2), ("2", 1)]);
    let kept = tokio::time::timeout(
        TIMEOUT,
        client.table_summary(session, &["meta".into()], &filters, ""),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(kept.rows, 3);
    assert_eq!(kept.columns[3].max, Some(4.25));

    let dir = common::tempdir().unwrap();
    let file = dir.path().join("out/meta.csv");
    let written = tokio::time::timeout(
        TIMEOUT,
        client.table_export(
            session,
            &["meta".into()],
            &sort,
            &filters,
            file.to_str().unwrap(),
            "",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(written.mtime_ms > 0);
    let csv = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 4, "{csv}");
    assert!(
        lines[1].starts_with("\"AAAC-2\"") || lines[1].starts_with("AAAC-2"),
        "{csv}"
    );

    assert!(
        rows_error(client, session, &["nope"])
            .await
            .contains("no variable named nope")
    );
    // Reading tables leaves nothing in the session's history.
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

/// A table of 1,200 columns, `wide[i, j]` being `i * 1200 + j` from 0:
/// every column is listed, and cells and summaries come a block of columns
/// at a time.
async fn check_wide(client: &Client, session: &str, last: &str) {
    let read = |column_offset: u32, cells_only: bool| {
        client.call::<TableRows>(Call::TableRows {
            session: session.into(),
            path: vec!["wide".into()],
            offset: 0,
            limit: None,
            sort: vec![],
            filters: vec![],
            search: String::new(),
            columns: None,
            column_offset,
            column_limit: None,
            cells_only,
        })
    };
    let first = read(0, false).await.unwrap();
    assert_eq!((first.width, first.columns.len()), (1200, 1200));
    assert_eq!(first.columns[1199].name, last);
    assert_eq!(first.values[0].len(), 100);
    let end = read(1150, true).await.unwrap();
    assert!(end.columns.is_empty());
    assert_eq!(end.column_offset, 1150);
    let want: Vec<Value> = (1150..1200).map(|j| json!(2400 + j)).collect();
    assert_eq!(end.values[2], want);
    // Projection preserves requested order and original identities, for
    // both real R and Python kernels using this shared check.
    let projected: TableRows = client
        .call(Call::TableRows {
            session: session.into(),
            path: vec!["wide".into()],
            offset: 0,
            limit: Some(3),
            sort: vec![],
            filters: vec![],
            search: String::new(),
            columns: Some(vec![1199, 2, 0]),
            column_offset: 0,
            column_limit: None,
            cells_only: false,
        })
        .await
        .unwrap();
    assert_eq!(projected.column_indices, Some(vec![1199, 2, 0]));
    assert_eq!(projected.columns.len(), 3);
    assert_eq!(projected.columns[0].name, last);
    assert_eq!(
        projected.values[2],
        vec![json!(3599), json!(2402), json!(2400)]
    );
    let empty: TableRows = client
        .call(Call::TableRows {
            session: session.into(),
            path: vec!["wide".into()],
            offset: 0,
            limit: Some(1),
            sort: vec![],
            filters: vec![],
            search: String::new(),
            columns: Some(vec![]),
            column_offset: 0,
            column_limit: None,
            cells_only: false,
        })
        .await
        .unwrap();
    assert_eq!(empty.column_indices, Some(vec![]));
    assert!(empty.columns.is_empty());
    assert_eq!(empty.values, vec![Vec::<Value>::new()]);
    let summary: TableSummary = client
        .call(Call::TableSummary {
            session: session.into(),
            path: vec!["wide".into()],
            filters: vec![],
            search: String::new(),
            column_offset: 1190,
            column_limit: None,
        })
        .await
        .unwrap();
    let at: Vec<u32> = summary.columns.iter().map(|c| c.column).collect();
    assert_eq!(at, (1190..1200).collect::<Vec<_>>());
    let last = summary.columns.last().unwrap();
    assert_eq!((last.min, last.max), (Some(1199.0), Some(3599.0)));
}

/// Search is evaluated in the live kernel over full values and every
/// column, including columns omitted from the returned page.
async fn check_search(client: &mut Client, session: &str, language: &str) {
    let keyword = "A\"\\\n[.]";
    let literal = serde_json::to_string(keyword).unwrap();
    let code = if language == "r" {
        format!(
            "searched <- data.frame(name = c('Alpha', 'beta', 'gamma', 'alpha', 'Delta', 'none'), category = factor(c('group', 'ALPHA', 'other', 'other', 'other', 'other')), score = c(42L, 12L, 942L, 24L, 6L, 7L), flag = c(TRUE, FALSE, TRUE, FALSE, TRUE, FALSE), day = as.Date(c('2024-01-02', rep('2025-02-03', 5))), note = c('x', 'x', {literal}, 'x', NA, paste0(strrep('x', 510), 'Needle')), row.names = paste0('R', 1:6))"
        )
    } else {
        format!(
            "searched = pd.DataFrame({{'name': ['Alpha', 'beta', 'gamma', 'alpha', 'Delta', 'none'], 'category': pd.Categorical(['group', 'ALPHA', 'other', 'other', 'other', 'other']), 'score': [42, 12, 942, 24, 6, 7], 'flag': [True, False, True, False, True, False], 'day': pd.to_datetime(['2024-01-02'] + ['2025-02-03'] * 5), 'note': ['x', 'x', {literal}, 'x', None, 'x' * 510 + 'Needle']}}, index=[f'R{{i}}' for i in range(1, 7)])"
        )
    };
    run(client, session, &code, None, None).await;
    let path = vec!["searched".into()];
    for (search, labels) in [
        ("aLpHa", vec!["R1", "R2", "R4"]),
        ("42", vec!["R1", "R3"]),
        ("TRUE", vec!["R1", "R3", "R5"]),
        ("2024-01", vec!["R1"]),
        (keyword, vec!["R3"]),
        ("[.]", vec!["R3"]),
        ("needle", vec!["R6"]),
        ("no match", vec![]),
        ("", vec!["R1", "R2", "R3", "R4", "R5", "R6"]),
    ] {
        let result = client
            .table_rows(session, &path, 0, None, &[], &[], search)
            .await
            .unwrap();
        assert_eq!(result.total, 6);
        assert_eq!(result.rows, labels.len() as u64, "{language}: {search:?}");
        assert_eq!(result.labels, labels, "{language}: {search:?}");
    }
    let filters = vec![filter(2, ">", &["20"])];
    let sort = vec![TableSort {
        column: 2,
        descending: true,
    }];
    let page: TableRows = client
        .call(Call::TableRows {
            session: session.into(),
            path: path.clone(),
            offset: 1,
            limit: Some(1),
            sort: sort.clone(),
            filters: filters.clone(),
            search: "alpha".into(),
            columns: Some(vec![2]),
            column_offset: 0,
            column_limit: None,
            cells_only: false,
        })
        .await
        .unwrap();
    assert_eq!((page.total, page.rows), (6, 2));
    assert_eq!(page.labels, ["R4"]);
    assert_eq!(page.values, vec![vec![json!(24)]]);
    let summary = client
        .table_summary(session, &path, &filters, "alpha")
        .await
        .unwrap();
    assert_eq!(summary.rows, 2);
    assert_eq!(
        (summary.columns[2].min, summary.columns[2].max),
        (Some(24.0), Some(42.0))
    );
    let literal_summary = client
        .table_summary(session, &path, &[], keyword)
        .await
        .unwrap();
    assert_eq!(literal_summary.rows, 1);
    assert_eq!(literal_summary.columns[2].min, Some(942.0));

    let dir = common::tempdir().unwrap();
    let file = dir.path().join("search.csv");
    client
        .table_export(
            session,
            &path,
            &sort,
            &filters,
            file.to_str().unwrap(),
            "alpha",
        )
        .await
        .unwrap();
    let csv = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<_> = csv.lines().collect();
    assert_eq!(lines.len(), 3, "{csv}");
    assert!(
        lines[1].starts_with("\"R1\"") || lines[1].starts_with("R1,"),
        "{csv}"
    );
    assert!(
        lines[2].starts_with("\"R4\"") || lines[2].starts_with("R4,"),
        "{csv}"
    );
    client
        .table_export(session, &path, &[], &[], file.to_str().unwrap(), keyword)
        .await
        .unwrap();
    let csv = std::fs::read_to_string(&file).unwrap();
    assert!(csv.contains("gamma") && csv.contains("942"), "{csv}");
    assert!(!csv.contains("Alpha") && !csv.contains("Delta"), "{csv}");

    // A subsequent request reads the changed live object, not a UI-side cache.
    let update = if language == "r" {
        "searched$name[1] <- 'changed'"
    } else {
        "searched.loc['R1', 'name'] = 'changed'"
    };
    run(client, session, update, None, None).await;
    let changed = client
        .table_rows(session, &path, 0, None, &[], &[], "alpha")
        .await
        .unwrap();
    assert_eq!(changed.labels, ["R2", "R4"]);
    for keyword in ["NA", "nan"] {
        let missing = client
            .table_rows(session, &path, 0, None, &[], &[], keyword)
            .await
            .unwrap();
        assert_eq!(
            missing.rows, 0,
            "{language}: missing values matched {keyword:?}"
        );
    }
    let matrix = client
        .table_rows(session, &["m".into()], 0, None, &[], &[], "6")
        .await
        .unwrap();
    assert_eq!(matrix.rows, 1);
    assert_eq!(matrix.values, vec![vec![json!(2), json!(4), json!(6)]]);
    let empty = if language == "r" {
        "no_columns <- data.frame(row.names = c('R1', 'R2'))"
    } else {
        "no_columns = pd.DataFrame(index=['R1', 'R2'])"
    };
    run(client, session, empty, None, None).await;
    for (search, expected) in [("", 2), ("anything", 0)] {
        let empty = client
            .table_rows(session, &["no_columns".into()], 0, None, &[], &[], search)
            .await
            .unwrap();
        assert_eq!((empty.total, empty.width, empty.rows), (2, 0, expected));
    }
}

async fn rows_error(client: &Client, session: &str, path: &[&str]) -> String {
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    client
        .table_rows(session, &path, 0, None, &[], &[], "")
        .await
        .expect_err("reading should fail")
        .to_string()
}

#[tokio::test]
async fn r_tables_are_read_filtered_sorted_and_exported() {
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
    run(
        &mut client,
        &session,
        "meta <- data.frame(cell = c('a', 'b\tB', 'c', 'd', 'e'), cluster = factor(c('0', '1', '0', '2', '1')), n_genes = c(812L, 1043L, 655L, NA, 990L), pct = c(1.5, 4.25, 0.5, 2, 7.75), row.names = paste0('AAAC-', 1:5))\nm <- matrix(1:6, 2)\nfit <- list(tab = data.frame(x = 1:3))\nwide <- as.data.frame(matrix(seq_len(3600) - 1L, 3, byrow = TRUE))",
        None,
        None,
    )
    .await;
    let all = client
        .list_variables(&session, &[])
        .await
        .unwrap()
        .variables;
    let table = |name: &str| all.iter().find(|v| v.name == name).unwrap().table;
    assert!(table("meta") && table("m") && !table("fit"));
    check_meta(&client, &session, "meta", &["chr"], ["fct", "int", "dbl"]).await;

    let m = rows(&client, &session, &["m"], 0, &[], &[]).await;
    assert!(m.matrix);
    let names: Vec<&str> = m.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["[,1]", "[,2]", "[,3]"]);
    assert_eq!(m.labels, ["1", "2"]);
    assert_eq!(
        m.values,
        [
            [json!(1), json!(3), json!(5)],
            [json!(2), json!(4), json!(6)]
        ]
    );
    let part = rows(&client, &session, &["fit", "tab"], 0, &[], &[]).await;
    assert_eq!((part.code.as_str(), part.rows), ("fit$tab", 3));
    assert!(
        rows_error(&client, &session, &["fit"])
            .await
            .contains("can't be shown as a table")
    );
    check_wide(&client, &session, "V1200").await;
    check_search(&mut client, &session, "r").await;
    client.shutdown(&session).await.unwrap();
}

#[tokio::test]
async fn python_tables_are_read_filtered_sorted_and_exported() {
    if !have_kernel("python3").await {
        return;
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    let session = client.start_session("python3", None).await.unwrap().id;
    run(
        &mut client,
        &session,
        "try:\n    import pandas as pd\n    has_pandas = True\nexcept ImportError:\n    has_pandas = False",
        None,
        None,
    )
    .await;
    let all = client
        .list_variables(&session, &[])
        .await
        .unwrap()
        .variables;
    if all.iter().find(|v| v.name == "has_pandas").unwrap().summary != "True" {
        eprintln!("skipping: pandas is not installed for python3");
        client.shutdown(&session).await.unwrap();
        return;
    }
    run(
        &mut client,
        &session,
        "import numpy as np\nmeta = pd.DataFrame({'cell': ['a', 'b\\tB', 'c', 'd', 'e'], 'cluster': pd.Categorical(['0', '1', '0', '2', '1']), 'n_genes': pd.array([812, 1043, 655, None, 990], dtype='Int64'), 'pct': [1.5, 4.25, 0.5, 2, 7.75]}, index=[f'AAAC-{i}' for i in range(1, 6)])\nm = np.arange(1, 7).reshape(3, 2).T\nfit = {'tab': pd.DataFrame({'x': [1, 2, 3]})}\nwide = pd.DataFrame(np.arange(3600).reshape(3, 1200))",
        None,
        None,
    )
    .await;
    let all = client
        .list_variables(&session, &[])
        .await
        .unwrap()
        .variables;
    let table = |name: &str| all.iter().find(|v| v.name == name).unwrap().table;
    assert!(table("meta") && table("m") && !table("fit"));
    // pandas 3 calls text columns "str"; earlier ones, "object".
    check_meta(
        &client,
        &session,
        "meta",
        &["object", "str"],
        ["category", "Int64", "float64"],
    )
    .await;

    let m = rows(&client, &session, &["m"], 0, &[], &[]).await;
    assert!(m.matrix);
    let names: Vec<&str> = m.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["0", "1", "2"]);
    assert_eq!(m.labels, ["0", "1"]);
    assert_eq!(
        m.values,
        [
            [json!(1), json!(3), json!(5)],
            [json!(2), json!(4), json!(6)]
        ]
    );
    let part = rows(&client, &session, &["fit", "tab"], 0, &[], &[]).await;
    assert_eq!((part.code.as_str(), part.rows), ("fit['tab']", 3));
    assert!(
        rows_error(&client, &session, &["fit"])
            .await
            .contains("can't be shown as a table")
    );
    check_wide(&client, &session, "1199").await;
    check_search(&mut client, &session, "python").await;
    client.shutdown(&session).await.unwrap();
}

/// Runs code and returns the events of its run.
async fn run_events(client: &mut Client, session: &str, code: &str) -> Vec<sandhill_proto::Event> {
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
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() != Some(started.run.as_str()) {
            continue;
        }
        let done = event.msg_type == EVENT_RUN_FINISHED;
        events.push(event);
        if done {
            break;
        }
    }
    client.unsubscribe(session).await.unwrap();
    events
}

#[tokio::test]
async fn view_asks_the_app_to_open_the_data_explorer() {
    let mut kernels = Vec::new();
    for (kernel, code) in [
        ("ark", "df <- data.frame(a = 1:3)\nView(df)"),
        ("ir", "df <- data.frame(a = 1:3)\nView(df)"),
        ("python3", "df = {'a': [1, 2, 3]}\n%view df"),
    ] {
        if have_kernel(kernel).await {
            kernels.push((kernel, code));
        }
    }
    let dir = common::tempdir().unwrap();
    let daemon = TestDaemon::start(dir.path()).await;
    let mut client = daemon.client().await;
    for (kernel, code) in kernels {
        let session = client.start_session(kernel, None).await.unwrap().id;
        let events = run_events(&mut client, &session, code).await;
        let views: Vec<&Value> = events
            .iter()
            .filter(|e| e.msg_type == sandhill_proto::EVENT_VIEW)
            .map(|e| &e.content)
            .collect();
        assert_eq!(views, [&json!({"title": "df"})], "{kernel}: {events:?}");
        // Nothing shows in the output, and the run went well.
        assert!(
            events
                .iter()
                .all(|e| !["display_data", "error"].contains(&e.msg_type.as_str())),
            "{kernel}: {events:?}"
        );
        let finished = events.last().unwrap();
        assert_eq!(finished.content["status"], "ok", "{kernel}: {events:?}");
        // The session goes on as before.
        let next = run_events(&mut client, &session, "1 + 1").await;
        assert!(
            next.iter()
                .any(|e| ["execute_result", "display_data"].contains(&e.msg_type.as_str())),
            "{kernel}: {next:?}"
        );
    }
}
