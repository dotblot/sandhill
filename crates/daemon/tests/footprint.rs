//! The executable's actual startup, with a fake home throughout.
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sandhill_daemon::StateDir;
use sandhill_daemon::blobs::BlobStore;
use sandhill_daemon::journal::{Journal, SessionRecord};
use sandhill_proto::client::Client;
use sandhill_proto::{
    ACCOUNTS_PATH, ARK_PATH, BIN_PATH, Call, EVENT_RUN_FINISHED, Footprint, INSTALL_PATH,
    InstalledKey, LogTail, OLD_DAEMON_LAYOUT, SANDHILL_DIR, STATE_PATH, SessionStatus,
    UninstallChoice, UninstallReport,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};

fn legacy(path: &str) -> &str {
    OLD_DAEMON_LAYOUT
        .iter()
        .find(|(_, current)| *current == path)
        .unwrap()
        .0
}

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandhilld"));
    command
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("unused-xdg-state"))
        .env_remove("SANDHILL_STATE_DIR")
        .kill_on_drop(true);
    command
}

async fn open(mut command: Command) -> (Client, Child) {
    command.args(["attach", "--start", "--confirm-exit"]);
    let (client, child) = Client::connect_command(command).await.unwrap();
    tokio::time::timeout(Duration::from_secs(40), client.ping())
        .await
        .unwrap()
        .unwrap();
    (client, child)
}

async fn stop(client: Client, mut child: Child) {
    let _: Value = client.call(Call::StopDaemon).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(20), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

fn journal(state: &StateDir) -> Journal {
    Journal::open(&state.db_path(), BlobStore::new(state.blobs_dir())).unwrap()
}

#[tokio::test]
async fn idle_home_moves_with_history_install_records_and_helpers() {
    let temp = common::tempdir().unwrap();
    let home = temp.path();
    let old = StateDir::new(home.join(legacy(STATE_PATH)));
    let new = StateDir::new(home.join(STATE_PATH));
    old.create().unwrap();
    let connection_file = old.kernels_dir().join("ended.json");
    fs::write(&connection_file, "{}\n").unwrap();
    let prior = journal(&old);
    prior
        .insert_session(&SessionRecord {
            id: "earlier-session".into(),
            kernel: "python3".into(),
            language: "python".into(),
            cwd: home.display().to_string(),
            connection_file,
            client_session: "earlier-client".into(),
            pid: None,
            interrupt_mode: "signal".into(),
            status: SessionStatus::Ended,
            created_ms: 1,
            ended_ms: Some(2),
            project: None,
            language_version: None,
        })
        .unwrap();
    prior
        .append(
            "earlier-session",
            None,
            "stream",
            json!({"name":"stdout", "text":"history survives\n"}),
        )
        .unwrap();
    drop(prior);
    fs::write(
        old.root().join("installed-by-sandhill.tsv"),
        format!(
            "kernel\tsample\t1\t{}/kernels/ended.json\t2026-11-20\tjupyter\tsample\n",
            old.root().display()
        ),
    )
    .unwrap();
    let old_ark = format!("{}/ark", legacy(BIN_PATH));
    let bytes = b"#!/bin/sh\n# example Ark payload\n";
    fs::create_dir_all(home.join(legacy(BIN_PATH))).unwrap();
    fs::write(home.join(&old_ark), bytes).unwrap();
    fs::set_permissions(home.join(&old_ark), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(home.join(legacy(INSTALL_PATH))).unwrap();
    fs::write(home.join(legacy(INSTALL_PATH)).join("ark.json"), json!({
        "version":1, "component":"ark", "path":old_ark, "sha256":sandhill_proto::hex(&Sha256::digest(bytes)),
    }).to_string()).unwrap();
    fs::create_dir_all(home.join(legacy(ACCOUNTS_PATH))).unwrap();
    fs::write(
        home.join(legacy(ACCOUNTS_PATH)).join("example"),
        b"local account data",
    )
    .unwrap();
    // A nearby file that Sandhill did not create must not be removed.
    fs::write(home.join(".local").join("keep"), b"personal").unwrap();

    let (client, bridge) = open(command(home)).await;
    let history = client.history("earlier-session", 0, None).await.unwrap();
    assert_eq!(history.events[0].content["text"], "history survives\n");
    assert_eq!(
        journal(&new).sessions().unwrap()[0].connection_file,
        new.kernels_dir().join("ended.json")
    );
    assert!(
        fs::read_to_string(new.root().join("installed-by-sandhill.tsv"))
            .unwrap()
            .contains(new.kernels_dir().join("ended.json").to_str().unwrap())
    );
    assert_eq!(fs::read(home.join(ARK_PATH)).unwrap(), bytes);
    let record: Value =
        serde_json::from_slice(&fs::read(home.join(INSTALL_PATH).join("ark.json")).unwrap())
            .unwrap();
    assert_eq!(record["path"], ARK_PATH);
    assert_eq!(
        fs::read(home.join(ACCOUNTS_PATH).join("example")).unwrap(),
        b"local account data"
    );
    assert_eq!(
        fs::metadata(home.join(SANDHILL_DIR))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(!home.join("unused-xdg-state").exists());
    for &(old, _) in OLD_DAEMON_LAYOUT {
        assert!(
            !home.join(old).exists(),
            "old directory still present: {old}"
        );
    }
    assert_eq!(fs::read(home.join(".local/keep")).unwrap(), b"personal");
    let log: LogTail = client.call(Call::ReadLog { bytes: None }).await.unwrap();
    assert!(log.text.contains("Moved"), "{log:?}");
    stop(client, bridge).await;
}

#[tokio::test]
async fn uninstall_removes_selected_package_reports_and_exits_before_the_folder_goes() {
    let temp = common::tempdir().unwrap();
    let home = temp.path().canonicalize().unwrap();
    let state = home.join(STATE_PATH);
    let library = home.join("R-library");
    fs::create_dir_all(library.join("sample")).unwrap();
    fs::write(
        library.join("sample/DESCRIPTION"),
        "Package: sample\nVersion: 1.0\n",
    )
    .unwrap();
    let extras = home.join("project/.sandhill");
    fs::create_dir_all(&extras).unwrap();
    fs::write(extras.join("results"), "user data").unwrap();
    let (client, mut bridge) = open(command(&home)).await;
    fs::write(
        state.join("installed-by-sandhill.tsv"),
        format!(
            "r-package\tsample\t1.0\t{}\t2026-11-20\tr\n",
            library.display()
        ),
    )
    .unwrap();
    let footprint: Footprint = client.call(Call::Footprint).await.unwrap();
    assert_eq!(Path::new(&footprint.folder), home.join(SANDHILL_DIR));
    assert!(
        footprint
            .items
            .iter()
            .any(|row| row.path == state.to_str().unwrap())
    );
    assert!(
        footprint
            .installed
            .iter()
            .any(|row| row.name == "sample" && row.ours)
    );

    let report: UninstallReport = client
        .call(Call::RunUninstall {
            choice: UninstallChoice {
                installed: vec![InstalledKey {
                    kind: "r-package".into(),
                    name: "sample".into(),
                    place: library.to_str().unwrap().into(),
                }],
                agents: Vec::new(),
                keep_history: false,
            },
            app_pid: None,
        })
        .await
        .unwrap();
    assert!(report.success, "{report:?}");
    assert_eq!(
        report.removed,
        [library.join("sample").display().to_string()]
    );
    assert!(report.finishing.contains(&footprint.folder));
    assert!(!library.join("sample").exists());
    let status = tokio::time::timeout(Duration::from_secs(20), bridge.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success(), "{status}");
    tokio::time::timeout(Duration::from_secs(10), async {
        while home.join(SANDHILL_DIR).exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the finisher did not remove Sandhill's folder");
    assert_eq!(fs::read(extras.join("results")).unwrap(), b"user data");
    assert!(!home.join("sandhill-uninstall-leftovers.txt").exists());
    assert!(
        !home
            .join("Desktop/sandhill-uninstall-leftovers.txt")
            .exists()
    );
}

#[tokio::test]
async fn old_kernel_keeps_running_and_the_next_idle_start_moves_the_state() {
    let Ok(kernel) = sandhill_daemon::kernel::find_kernelspec("python3", None).await else {
        eprintln!("skipping: Jupyter kernel python3 is not installed");
        return;
    };
    let temp = common::tempdir().unwrap();
    let home = temp.path();
    let jupyter = home.join("jupyter");
    let dir = jupyter.join("kernels/python3");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("kernel.json"),
        serde_json::to_vec(&kernel.kernelspec).unwrap(),
    )
    .unwrap();
    let old = StateDir::new(home.join(legacy(STATE_PATH)));
    let new = StateDir::new(home.join(STATE_PATH));
    let setup = || {
        let mut cmd = command(home);
        cmd.env("JUPYTER_PATH", &jupyter);
        cmd
    };
    let mut before = setup();
    before.env("SANDHILL_STATE_DIR", old.root());
    let (client, bridge) = open(before).await;
    let session = client
        .start_session("python3", Some(home.to_str().unwrap()))
        .await
        .unwrap()
        .id;
    stop(client, bridge).await;

    let (mut client, bridge) = open(setup()).await;
    assert!(old.db_path().exists());
    assert!(!new.root().exists());
    let log: LogTail = client.call(Call::ReadLog { bytes: None }).await.unwrap();
    assert!(log.text.contains("still running"), "{log:?}");
    let storage: Value = client.call(Call::StorageInventory).await.unwrap();
    assert!(
        storage["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == old.root().to_str().unwrap()
                && entry["reason"]
                    .as_str()
                    .is_some_and(|note| note.contains("still running"))),
        "{storage}"
    );
    client.subscribe(&session, None).await.unwrap();
    let run = client
        .execute(&session, "print('still here')", None)
        .await
        .unwrap()
        .run;
    let printed = tokio::time::timeout(Duration::from_secs(20), async {
        let mut printed = false;
        while let Some(event) = client.next_event().await {
            if event.run.as_deref() == Some(run.as_str()) {
                printed |= event.content["text"] == "still here\n";
                if event.msg_type == EVENT_RUN_FINISHED {
                    return printed;
                }
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(printed);
    let pid = journal(&old).sessions().unwrap()[0].pid.unwrap();
    let identity = sandhill_daemon::lifecycle::ProcessIdentity::read(pid).unwrap();
    client.shutdown(&session).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        while identity.alive() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    stop(client, bridge).await;

    let (client, bridge) = open(setup()).await;
    assert!(!old.root().exists());
    assert!(new.db_path().exists());
    let history = client.history(&session, 0, None).await.unwrap();
    assert!(
        history
            .events
            .iter()
            .any(|event| event.content["text"] == "still here\n")
    );
    stop(client, bridge).await;
}

#[tokio::test]
async fn an_explicit_state_directory_does_not_move_the_default_layout() {
    let temp = common::tempdir().unwrap();
    let old = temp.path().join(legacy(STATE_PATH));
    fs::create_dir_all(&old).unwrap();
    let custom = temp.path().join("custom");
    let mut daemon = command(temp.path());
    daemon.env("SANDHILL_STATE_DIR", &custom);
    let (client, bridge) = open(daemon).await;
    assert!(old.is_dir());
    assert!(!temp.path().join(STATE_PATH).exists());
    let log: LogTail = client.call(Call::ReadLog { bytes: None }).await.unwrap();
    assert_eq!(PathBuf::from(log.path), custom.join("sandhilld.log"));
    stop(client, bridge).await;
}
