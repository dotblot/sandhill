//! File history over the same daemon protocol the editor uses.

mod common;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, FileHistoryLimits, FileText, FileVersionInfo, FileVersionReason, FileWritten,
    NotebookHistoryCell,
};
use serde_json::{Value, json};
use tokio::net::UnixListener;

struct TestDaemon {
    task: tokio::task::JoinHandle<()>,
    client: Client,
    state: PathBuf,
}

impl TestDaemon {
    async fn start(root: &Path, socket_name: &str) -> Self {
        let state = root.join("state");
        let daemon = Daemon::open(StateDir::new(&state)).await.unwrap();
        let socket = root.join(socket_name);
        let listener = UnixListener::bind(&socket).unwrap();
        let task = tokio::spawn(async move { daemon.serve(listener).await.unwrap() });
        let client = Client::connect_unix(socket).await.unwrap();
        Self {
            task,
            client,
            state,
        }
    }

    async fn versions(&self, path: &str) -> Vec<FileVersionInfo> {
        self.client
            .call(Call::FileVersions {
                path: path.to_owned(),
            })
            .await
            .unwrap()
    }

    async fn version(&self, path: &str, id: i64) -> String {
        self.client
            .call(Call::FileVersion {
                path: path.to_owned(),
                id,
            })
            .await
            .unwrap()
    }

    async fn limits(&self, limits: FileHistoryLimits) {
        self.client.call::<()>(limits.into()).await.unwrap();
    }

    async fn stop(self) {
        self.client.call::<Value>(Call::StopDaemon).await.unwrap();
        self.task.await.unwrap();
    }
}

#[tokio::test]
async fn saves_keep_distinct_text_and_opening_catches_external_changes() {
    let tmp = common::tempdir().unwrap();
    let daemon = TestDaemon::start(tmp.path(), "history.sock").await;
    let project = tmp.path().join("atlas");
    std::fs::create_dir(&project).unwrap();
    daemon
        .client
        .open_project(project.to_str().unwrap())
        .await
        .unwrap();
    let file = project.join("qc.py");
    let path = file.to_str().unwrap();
    for text in ["a\n", "a\nb\n", "c\n", "c\n"] {
        daemon.client.write_file(path, text, None).await.unwrap();
    }
    let kept = daemon.versions(path).await;
    assert_eq!(kept.len(), 3);
    assert!(kept.iter().all(|v| v.reason == FileVersionReason::Saved));
    assert_eq!(daemon.version(path, kept[2].id).await, "a\n");
    assert_eq!(daemon.version(path, kept[0].id).await, "c\n");
    assert_eq!((kept[0].added, kept[0].removed), (1, 2));

    std::fs::write(&file, "c\nd\n").unwrap();
    let open = || {
        daemon.client.call::<FileText>(Call::ReadDocument {
            path: path.into(),
            max_bytes: Some(1_000_000),
        })
    };
    assert_eq!(open().await.unwrap().text, "c\nd\n");
    open().await.unwrap();
    let versions = daemon.versions(path).await;
    assert_eq!(versions.len(), 4);
    assert_eq!(versions[0].reason, FileVersionReason::Opened);
    assert_eq!((versions[0].added, versions[0].removed), (1, 0));

    let old_path = project.join("existed.R");
    std::fs::write(&old_path, "original <- TRUE\n").unwrap();
    let old_path = old_path.to_str().unwrap();
    daemon
        .client
        .write_file(old_path, "updated <- TRUE\n", None)
        .await
        .unwrap();
    let versions = daemon.versions(old_path).await;
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[1].reason, FileVersionReason::Opened);
    assert_eq!(
        daemon.version(old_path, versions[1].id).await,
        "original <- TRUE\n"
    );

    let alias = project.join("alias.py");
    std::os::unix::fs::symlink(&file, &alias).unwrap();
    let alias = alias.to_str().unwrap();
    assert_eq!(daemon.versions(alias).await, daemon.versions(path).await);
    let nested = project.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let other_project = nested.to_str().unwrap();
    let other: Vec<FileVersionInfo> = daemon
        .client
        .call_in_project(
            Some(other_project.into()),
            Call::FileVersions { path: alias.into() },
        )
        .await
        .unwrap();
    assert!(other.is_empty());
    let denied = daemon
        .client
        .call_in_project::<String>(
            Some(other_project.into()),
            Call::FileVersion {
                path: alias.into(),
                id: kept[0].id,
            },
        )
        .await;
    assert!(denied.is_err());
    daemon.stop().await;
}

#[tokio::test]
async fn tidy_prunes_age_size_count_and_total_without_losing_shared_text() {
    let tmp = common::tempdir().unwrap();
    let daemon = TestDaemon::start(tmp.path(), "history.sock").await;
    let path = tmp.path().join("qc.py").to_string_lossy().into_owned();
    let other = tmp.path().join("sample.txt").to_string_lossy().into_owned();
    let big = tmp.path().join("big.md").to_string_lossy().into_owned();
    for text in ["expired\n", "shared\n", "newest\n"] {
        daemon.client.write_file(&path, text, None).await.unwrap();
    }
    daemon
        .client
        .write_file(&other, "shared\n", None)
        .await
        .unwrap();
    daemon
        .client
        .write_file(&big, &"a".repeat(3_000_000), None)
        .await
        .unwrap();
    assert!(daemon.versions(&big).await.is_empty());
    {
        let sql = Connection::open(daemon.state.join("journal.sqlite")).unwrap();
        sql.execute("UPDATE file_versions SET time_ms = time_ms - 31 * 86400000 WHERE id = (SELECT min(id) FROM file_versions)", []).unwrap();
    }
    daemon.stop().await;
    let daemon = TestDaemon::start(tmp.path(), "history-restarted.sock").await;
    assert_eq!(daemon.versions(&path).await.len(), 2, "startup tidies age");
    daemon
        .limits(FileHistoryLimits {
            per_file: 1,
            ..Default::default()
        })
        .await;
    assert_eq!(daemon.versions(&path).await.len(), 1);
    let shared = daemon.versions(&other).await;
    assert_eq!(
        daemon.version(&other, shared[0].id).await,
        "shared\n",
        "tidy must preserve a blob another file still references"
    );

    let a = "x".repeat(600_000);
    let b = "y".repeat(600_000);
    daemon.client.write_file(&path, &a, None).await.unwrap();
    daemon.client.write_file(&other, &b, None).await.unwrap();
    daemon
        .limits(FileHistoryLimits {
            per_file: 1,
            total_mb: 1,
            ..Default::default()
        })
        .await;
    assert!(daemon.versions(&path).await.is_empty());
    let kept = daemon.versions(&other).await;
    assert_eq!(kept.len(), 1);
    assert_eq!(daemon.version(&other, kept[0].id).await, b);
    let files: usize = std::fs::read_dir(daemon.state.join("blobs/file-history"))
        .unwrap()
        .map(|p| std::fs::read_dir(p.unwrap().path()).unwrap().count())
        .sum();
    assert_eq!(files, 1, "old text must stop taking space");
    daemon.stop().await;
    let daemon = TestDaemon::start(tmp.path(), "history-limits.sock").await;
    daemon
        .client
        .write_file(&other, "last version\n", None)
        .await
        .unwrap();
    assert_eq!(
        daemon.versions(&other).await.len(),
        2,
        "new entries are tidied on the next interval"
    );
    daemon.stop().await;
    let daemon = TestDaemon::start(tmp.path(), "history-tidy.sock").await;
    assert_eq!(
        daemon.versions(&other).await.len(),
        1,
        "saved per-file limit applies on restart"
    );
    let cleaned: Value = daemon
        .client
        .call(Call::CleanStorage {
            categories: vec!["history".into()],
        })
        .await
        .unwrap();
    assert_eq!(cleaned["success"], true, "{cleaned}");
    assert!(!daemon.state.join("blobs/file-history").exists());
    daemon.stop().await;
    let daemon = TestDaemon::start(tmp.path(), "history-cleaned.sock").await;
    assert!(daemon.versions(&other).await.is_empty());
    daemon.stop().await;
}

#[tokio::test]
async fn notebook_history_ignores_outputs_and_normalizes_source_layout() {
    let tmp = common::tempdir().unwrap();
    let daemon = TestDaemon::start(tmp.path(), "history.sock").await;
    let path = tmp
        .path()
        .join("atlas.ipynb")
        .to_string_lossy()
        .into_owned();
    let original = json!({"nbformat":4,"nbformat_minor":4,"cells":[
        {"cell_type":"code","source":"import scanpy\nadata = None\n","execution_count":1,
         "outputs":[{"output_type":"stream","name":"stdout","text":"do not store me"}]},
        {"cell_type":"markdown","source":"# Sample QC"}
    ]});
    std::fs::write(&path, original.to_string()).unwrap();
    let mut notebook = original;
    notebook["cells"][0]["id"] = json!("cell-1");
    notebook["cells"][1]["id"] = json!("cell-2");
    notebook["cells"][0]["source"] = json!(["import scanpy\n", "adata = None\n"]);
    notebook["cells"][0]["outputs"][0]["text"] = json!("updated output");
    let write = |text: String| {
        daemon.client.call::<FileWritten>(Call::WriteNotebook {
            path: path.clone(),
            text,
            expected_mtime_ms: None,
        })
    };
    write(notebook.to_string()).await.unwrap();
    assert_eq!(
        daemon.versions(&path).await.len(),
        1,
        "formatting, ids and output alone are not a new version"
    );
    notebook["cells"][0]["source"] = json!(["import scanpy\n", "adata = 'changed'\n"]);
    write(notebook.to_string()).await.unwrap();
    let versions = daemon.versions(&path).await;
    assert_eq!(versions.len(), 2);
    assert_eq!((versions[1].added, versions[1].removed), (7, 0));
    assert_eq!((versions[0].added, versions[0].removed), (1, 1));
    for version in &versions {
        let text = daemon.version(&path, version.id).await;
        assert!(!text.contains("outputs"));
        assert!(!text.contains("execution_count"));
        assert!(!text.contains("do not store me") && !text.contains("updated output"));
        let cells: Vec<NotebookHistoryCell> = serde_json::from_str(&text).unwrap();
        assert_eq!(cells[0].source[0], "import scanpy\n");
        assert_eq!(cells[1].source, ["# Sample QC"]);
    }
    daemon.stop().await;
}
