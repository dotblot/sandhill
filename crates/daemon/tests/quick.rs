//! Quick Tools against a real R kernel: a session folder made beside a
//! table and trusted alone, the table loaded and recognised, a volcano plot
//! with its sentence reaching Plots, a made table, a workbook, and the
//! steps recorded. Skips (and says so) without IRkernel or the R packages
//! the templates use.

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, QuickPlot, QuickRequest, QuickResult, QuickSettings, QuickTableKind, SessionInfo,
    TrustedFolder,
};
use serde_json::Value;
use tokio::net::UnixListener;

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
