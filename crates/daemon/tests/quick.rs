//! Quick Tools against a real R kernel: a session folder made beside a
//! table and trusted alone, the table loaded and recognised, a volcano plot
//! with its sentence reaching Plots, a made table, a workbook, and the
//! steps recorded. Skips (and says so) without IRkernel or the R packages
//! the templates use.

mod common;

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, Focus, QuickPlot, QuickRequest, QuickResult, QuickSettings, QuickTableKind, SessionInfo,
    TrustedFolder,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

async fn start(state: &Path) -> (Arc<Daemon>, PathBuf) {
    let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    (daemon, socket)
}

/// A DESeq2 table as `write.csv(as.data.frame(res))` writes it: the gene
/// IDs under an empty header. Genes 0-19 go up, 20-29 down.
fn deseq2_csv() -> String {
    let mut text = String::from(
        "\"\",\"baseMean\",\"log2FoldChange\",\"lfcSE\",\"stat\",\"pvalue\",\"padj\"\n",
    );
    for i in 0..200 {
        let (lfc, padj) = match i {
            0..20 => (2.0 + i as f64 / 10., 1e-6 / (i + 1) as f64),
            20..30 => (-1.5 - i as f64 / 20., 1e-4),
            _ => (0.1 * ((i % 7) as f64 - 3.), 0.5),
        };
        writeln!(
            text,
            "\"ENSG{:011}\",{},{lfc},0.2,{},{},{padj}",
            i + 1,
            100 + i * 3,
            lfc / 0.2,
            padj / 2.
        )
        .unwrap();
    }
    text
}

#[tokio::test]
async fn a_dropped_results_table_becomes_a_volcano_plot() {
    if sandhill_daemon::kernel::find_kernelspec("ir", None)
        .await
        .is_err()
    {
        eprintln!("skipping: Jupyter kernel \"ir\" is not installed");
        return;
    }
    let dir = common::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let downloads = dir.path().join("Downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let table = downloads.join("de results.csv");
    std::fs::write(&table, deseq2_csv()).unwrap();
    let (_daemon, socket) = start(&state).await;
    let client = Client::connect_unix(&socket).await.unwrap();

    // Opening the table makes nothing beside it: the session starts in
    // Sandhill's own storage.
    let folder: String = client
        .call(Call::QuickStart {
            tables: vec![table.to_string_lossy().into_owned()],
        })
        .await
        .unwrap();
    assert!(
        folder.starts_with(&state.to_string_lossy().into_owned()),
        "{folder}"
    );
    assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 1);
    // Opened like a project, it still isn't one of the user's recent ones.
    client
        .call::<Value>(Call::OpenProject {
            path: folder.clone(),
        })
        .await
        .unwrap();
    let recent: Vec<sandhill_proto::ProjectInfo> =
        client.call(Call::ListRecentProjects).await.unwrap();
    assert!(recent.is_empty(), "{recent:?}");

    let session: SessionInfo = client
        .call(Call::StartSession {
            kernel: "ir".into(),
            cwd: Some(folder.clone()),
            project: Some(folder.clone()),
        })
        .await
        .unwrap();
    let run = |request: QuickRequest| {
        client.call::<QuickResult>(Call::QuickRun {
            session: session.id.clone(),
            project: folder.clone(),
            request,
        })
    };

    let loaded = run(QuickRequest::Load {
        file: "de results.csv".into(),
        sheet: None,
        name: "de_results".into(),
    })
    .await
    .unwrap();
    let table = loaded.table.expect("the load returns the table");
    assert_eq!(table.rows, 200);
    let found = &table.recognition;
    assert_eq!(found.kind, QuickTableKind::Results);
    assert_eq!(found.fc.as_deref(), Some("log2FoldChange"));
    assert_eq!(found.padj.as_deref(), Some("padj"));
    assert_eq!(found.gene.as_deref(), Some("id"));
    assert_eq!(found.organism.as_deref(), Some("human"));

    let settings = QuickSettings {
        gene: found.gene.clone(),
        fc: found.fc.clone(),
        padj: found.padj.clone(),
        mean: found.mean.clone(),
        ..QuickSettings::default()
    };
    let volcano = run(QuickRequest::Plot {
        plot: QuickPlot::Volcano,
        table: "de_results".into(),
        settings: Box::new(settings.clone()),
        output: None,
    })
    .await;
    let volcano = match volcano {
        Err(err) if format!("{err:#}").contains("Missing R packages") => {
            eprintln!("skipping: {err:#}");
            return;
        }
        other => other.unwrap(),
    };
    assert_eq!(volcano.facts["tested"], Value::from(200));
    assert_eq!(volcano.facts["up"], Value::from(20));
    assert_eq!(volcano.facts["down"], Value::from(10));
    let sentence = volcano.sentence.expect("a volcano says what it shows");
    assert!(
        sentence.starts_with("30 of 200 genes pass the cutoffs"),
        "{sentence}"
    );
    assert!(sentence.contains("doesn't say which group is higher"));

    // Shown as run by Sandhill, so the plot reaches Plots.
    let plots: Value = client
        .call(Call::SessionPlots {
            session: session.id.clone(),
            limit: None,
        })
        .await
        .unwrap();
    assert!(
        plots["plots"]
            .as_array()
            .is_some_and(|p| p.iter().any(|plot| plot["by"] == "Sandhill")),
        "no plot in Plots: {plots}"
    );

    let top = run(QuickRequest::Plot {
        plot: QuickPlot::TopGenes,
        table: "de_results".into(),
        settings: Box::new(settings),
        output: Some("de_results_top_genes".into()),
    })
    .await
    .unwrap();
    assert_eq!(top.output.as_deref(), Some("de_results_top_genes"));
    assert_eq!(top.facts["rows"], Value::from(30));

    // A template's refusal comes back as its plain message.
    let refused = run(QuickRequest::Plot {
        plot: QuickPlot::Ma,
        table: "de_results".into(),
        settings: Box::new(QuickSettings {
            fc: Some("log2FoldChange".into()),
            padj: Some("padj".into()),
            mean: Some("no such column".into()),
            ..QuickSettings::default()
        }),
        output: None,
    })
    .await
    .unwrap_err();
    assert!(
        !format!("{refused:#}").contains("SANDHILL_QUICK"),
        "{refused:#}"
    );

    match run(QuickRequest::Workbook {
        path: "results/de results.xlsx".into(),
        readme: vec![("Comparison".into(), "Not recorded".into())],
        sheets: vec![
            ("de_results".into(), "de_results".into()),
            ("top genes".into(), "de_results_top_genes".into()),
        ],
    })
    .await
    {
        Err(err) if format!("{err:#}").contains("Missing R packages") => {
            eprintln!("skipping the workbook: {err:#}");
        }
        other => {
            other.unwrap();
            assert!(Path::new(&folder).join("results/de results.xlsx").is_file());
        }
    }

    // Outside the session folder is refused before R sees it.
    assert!(
        run(QuickRequest::Load {
            file: "../de results.csv".into(),
            sheet: None,
            name: "x".into(),
        })
        .await
        .is_err()
    );

    let steps = std::fs::read_to_string(Path::new(&folder).join("steps.R")).unwrap();
    assert!(steps.contains("source(\".sandhill/quick.R\")"));
    assert!(steps.contains("\"action\":\"load\""));
    assert!(steps.contains("\"plot\":\"volcano\""));
    assert!(Path::new(&folder).join(".sandhill/quick.R").is_file());

    // Kept where and as the user says, trusted alone, with a copy of the
    // table; later results follow it there.
    let kept: String = client
        .call(Call::QuickKeep {
            project: folder.clone(),
            parent: downloads.to_string_lossy().into_owned(),
            name: "DE for Anna".into(),
        })
        .await
        .unwrap();
    let kept = Path::new(&kept);
    assert_eq!(kept, downloads.join("DE for Anna"));
    assert!(kept.join("de results.csv").is_file());
    assert!(kept.join("steps.R").is_file());
    let trusted: Vec<TrustedFolder> = client.call(Call::ListTrustedFolders).await.unwrap();
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap();
    assert!(
        trusted
            .iter()
            .any(|t| canonical(Path::new(&t.path)) == canonical(kept))
    );
    assert!(
        !trusted
            .iter()
            .any(|t| canonical(Path::new(&t.path)) == canonical(&downloads))
    );
    run(QuickRequest::Plot {
        plot: QuickPlot::UpDown,
        table: "de_results".into(),
        settings: Box::new(QuickSettings {
            fc: Some("log2FoldChange".into()),
            padj: Some("padj".into()),
            ..QuickSettings::default()
        }),
        output: None,
    })
    .await
    .unwrap();
    let steps = std::fs::read_to_string(kept.join("steps.R")).unwrap();
    assert!(steps.contains("\"plot\":\"up_down\""), "{steps}");

    // Done with: the copy in Sandhill's storage goes; the kept folder stays.
    client
        .call::<Value>(Call::QuickDiscard {
            project: folder.clone(),
        })
        .await
        .unwrap();
    assert!(!Path::new(&folder).exists());
    assert!(kept.join("de results.csv").is_file());
}

struct Mcp {
    _child: Child,
    send: ChildStdin,
    receive: Lines<BufReader<ChildStdout>>,
}

impl Mcp {
    fn start(state: &Path, socket: &Path, project: &Path) -> io::Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sandhilld"))
            .arg("--state-dir")
            .arg(state)
            .arg("--socket")
            .arg(socket)
            .arg("mcp")
            .arg("--project")
            .arg(project)
            .env_remove("SANDHILL_TERMINAL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let send = child.stdin.take().unwrap();
        let receive = BufReader::new(child.stdout.take().unwrap()).lines();
        Ok(Self {
            _child: child,
            send,
            receive,
        })
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        self.send
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(120), self.receive.next_line())
            .await
            .expect("MCP timed out")
            .unwrap()
            .expect("MCP exited");
        let message: Value = serde_json::from_str(&response).unwrap();
        assert!(message.get("error").is_none(), "{message}");
        message["result"].clone()
    }

    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }
}

fn tool_json(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn only_explore_tools_reach_the_registered_explore_window() {
    let dir = common::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let source = dir.path().join("results.csv");
    std::fs::write(&source, deseq2_csv()).unwrap();
    let (_daemon, socket) = start(&state).await;
    let client = Client::connect_unix(&socket).await.unwrap();
    let mut normal = Mcp::start(&state, &socket, dir.path()).unwrap();
    let ordinary = normal.rpc("tools/list", json!({})).await;
    assert!(
        !ordinary["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"].as_str().unwrap().starts_with("quick_"))
    );
    assert_eq!(normal.tool("quick_state", json!({})).await["isError"], true);

    let folder: String = client
        .call(Call::QuickStart {
            tables: vec![source.to_string_lossy().into_owned()],
        })
        .await
        .unwrap();
    let mut explore = Mcp::start(&state, &socket, Path::new(&folder)).unwrap();
    let listed = explore.rpc("tools/list", json!({})).await;
    let names: Vec<_> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| {
            tool["name"]
                .as_str()
                .filter(|name| name.starts_with("quick_"))
        })
        .collect();
    assert_eq!(names, ["quick_state", "quick_suggest", "quick_make"]);
    let init = explore
        .rpc("initialize", json!({"protocolVersion": "2025-06-18"}))
        .await;
    assert!(
        init["instructions"]
            .as_str()
            .unwrap()
            .contains("Call quick_state first")
    );

    // A normal project editor can be open at the same path, but cannot
    // act on the live Explore card.
    client.set_focus(&folder, Focus::default()).await.unwrap();
    let closed = explore.tool("quick_state", json!({})).await;
    assert_eq!(closed["isError"], true);
    assert!(
        closed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Open this session in Explore a Table")
    );

    let mut window = Client::connect_unix(&socket).await.unwrap();
    window
        .set_focus(
            &folder,
            Focus {
                explore: true,
                ..Focus::default()
            },
        )
        .await
        .unwrap();
    let routes = [
        ("quick_state", json!({}), json!({"tables": ["results"]})),
        (
            "quick_suggest",
            json!({"organism": "human"}),
            json!({"set": ["organism"], "skipped": []}),
        ),
        (
            "quick_make",
            json!({"plot": "up_down"}),
            json!({"missing": ["the adjusted p-value column"]}),
        ),
    ];
    for (method, params, expected) in routes {
        let (result, ()) = tokio::join!(explore.tool(method, params.clone()), async {
            let request = window.next_request().await.unwrap();
            assert_eq!(request.project, folder);
            assert_eq!(request.method, method);
            assert_eq!(request.params, params);
            window
                .call::<Value>(Call::Answer {
                    id: request.id,
                    result: Some(expected.clone()),
                    error: None,
                })
                .await
                .unwrap();
        });
        assert_eq!(tool_json(&result), expected);
    }
    let (result, ()) = tokio::join!(explore.tool("quick_suggest", json!({})), async {
        let request = window.next_request().await.unwrap();
        window
            .call::<Value>(Call::Answer {
                id: request.id,
                result: None,
                error: Some("Wait for the card to finish loading.".into()),
            })
            .await
            .unwrap();
    });
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Wait for the card to finish loading.")
    );

    // The window can run R on its connection before sending the answer.
    if sandhill_daemon::kernel::find_kernelspec("ir", None)
        .await
        .is_err()
    {
        eprintln!("skipping the quick_make R run: Jupyter kernel \"ir\" is not installed");
        return;
    }
    let session: SessionInfo = window
        .call(Call::StartSession {
            kernel: "ir".into(),
            cwd: Some(folder.clone()),
            project: Some(folder.clone()),
        })
        .await
        .unwrap();
    let _: QuickResult = window
        .call(Call::QuickRun {
            session: session.id.clone(),
            project: folder.clone(),
            request: QuickRequest::Load {
                file: "results.csv".into(),
                sheet: None,
                name: "results".into(),
            },
        })
        .await
        .unwrap();
    let (result, ()) = tokio::join!(
        explore.tool("quick_make", json!({"plot": "up_down", "table": "results"})),
        async {
            let request = window.next_request().await.unwrap();
            assert_eq!(request.method, "quick_make");
            assert_eq!(request.params["table"], "results");
            let made = window
                .call::<QuickResult>(Call::QuickRun {
                    session: session.id.clone(),
                    project: folder.clone(),
                    request: QuickRequest::Plot {
                        plot: QuickPlot::UpDown,
                        table: "results".into(),
                        output: None,
                        settings: Box::new(QuickSettings {
                            fc: Some("log2FoldChange".into()),
                            padj: Some("padj".into()),
                            ..QuickSettings::default()
                        }),
                    },
                })
                .await;
            let (result, error) = match made {
                Ok(result) => (
                    Some(json!({"summary": result.sentence, "numbers": result.facts})),
                    None,
                ),
                Err(err) => (None, Some(err.to_string())),
            };
            window
                .call::<Value>(Call::Answer {
                    id: request.id,
                    result,
                    error,
                })
                .await
                .unwrap();
        }
    );
    window.shutdown(&session.id).await.unwrap();
    if result["isError"] == true
        && result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Missing R packages")
    {
        eprintln!(
            "skipping the quick_make R run: {}",
            result["content"][0]["text"]
        );
        return;
    }
    let made = tool_json(&result);
    assert_eq!(made["numbers"]["up"], 20);
    assert_eq!(made["numbers"]["down"], 10);
    assert!(
        made["summary"]
            .as_str()
            .unwrap()
            .contains("doesn't say which group is higher")
    );
}
