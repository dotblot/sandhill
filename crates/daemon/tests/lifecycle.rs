//! Resource management uses real task-owned terminals/kernels and isolated
//! state. No uninstall call targets the real HOME or service manager.
mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::terminal::{Launch, Terminals};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{AgentOptions, Call};
use serde_json::{Value, json};
use tokio::net::UnixListener;

struct Fixture {
    _temp: common::TempDir,
    root: PathBuf,
    daemon: Arc<Daemon>,
    client: Client,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new() -> Self {
        let temp = common::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = StateDir::new(root.join("state"));
        let daemon = Daemon::open_with(
            state.clone(),
            DaemonOptions {
                executable: env!("CARGO_BIN_EXE_sandhilld").into(),
                socket: state.socket_path(),
                started_by: String::new(),
            },
        )
        .await
        .unwrap();
        let listener = UnixListener::bind(state.socket_path()).unwrap();
        let server = tokio::spawn(daemon.clone().serve(listener));
        let client = Client::connect_unix(&state.socket_path()).await.unwrap();
        Self {
            _temp: temp,
            root,
            daemon,
            client,
            server,
        }
    }
    fn terminals(&self) -> Terminals {
        Terminals::new(
            self.daemon.state().terminals_dir(),
            env!("CARGO_BIN_EXE_sandhilld").into(),
        )
    }
    async fn launch(&self, kind: &str, script: &str) -> sandhill_proto::TerminalInfo {
        self.terminals()
            .start(Launch {
                kind: kind.into(),
                title: format!("isolated {kind}"),
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                env: vec![],
                cwd: self.root.clone(),
                project: Some(self.root.to_string_lossy().into_owned()),
                options: AgentOptions::default(),
                size: Some((80, 24)),
                job: None,
            })
            .await
            .unwrap()
    }
    async fn call(&self, call: Call) -> Value {
        tokio::time::timeout(Duration::from_secs(40), self.client.call::<Value>(call))
            .await
            .unwrap()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        self.daemon.detach_all();
    }
}

#[tokio::test]
async fn all_terminal_kinds_are_inventoried_and_graceful_stop_checks_hosts() {
    let fixture = Fixture::new().await;
    for kind in [
        "shell",
        "job",
        "preview",
        "claude",
        "codex",
        "antigravity",
        "posit",
    ] {
        fixture.launch(kind, "exec sleep 120").await;
    }
    let inventory = fixture.call(Call::ResourceInventory).await;
    let resources = inventory["resources"].as_array().unwrap();
    let count = resources.len();
    let kinds: Vec<_> = resources
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_string())
        .collect();
    let stopped = fixture.call(Call::StopResources { force: false }).await;
    // Always clean task-owned processes before asserting the graceful result.
    let forced = fixture.call(Call::StopResources { force: true }).await;
    assert_eq!(count, 7, "{inventory}");
    assert!(kinds.contains(&"job".into()) && kinds.contains(&"antigravity".into()));
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(forced["stopped"], true, "{forced}");
    assert!(fixture.terminals().list().iter().all(|t| !t.running));
}

#[tokio::test]
async fn restarted_terminal_is_reinventoried_and_stopped_after_prior_inventory() {
    let fixture = Fixture::new().await;
    let first = fixture.launch("shell", "exec sleep 120").await;
    let before = fixture.call(Call::ResourceInventory).await;
    let terminals = fixture.terminals();
    terminals
        .restart(
            &first.id,
            Launch {
                kind: "shell".into(),
                title: "restarted isolated shell".into(),
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "exec sleep 120".into()],
                env: vec![],
                cwd: fixture.root.clone(),
                project: Some(fixture.root.to_string_lossy().into_owned()),
                options: AgentOptions::default(),
                size: Some((80, 24)),
                job: None,
            },
            &[],
        )
        .await
        .unwrap();
    let after = fixture.call(Call::ResourceInventory).await;
    let stopped = fixture.call(Call::StopResources { force: false }).await;
    let forced = fixture.call(Call::StopResources { force: true }).await;
    assert_eq!(before["resources"].as_array().unwrap().len(), 1, "{before}");
    assert!(
        after["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| resource["id"] == format!("terminal:{}", first.id)),
        "{after}"
    );
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(forced["stopped"], true, "{forced}");
    assert!(!terminals.info(&first.id).unwrap().running);
}

#[tokio::test]
async fn hardlinked_journal_is_reported_protected_and_history_is_not_purged() {
    let fixture = Fixture::new().await;
    let state = fixture.daemon.state();
    std::fs::hard_link(state.db_path(), fixture.root.join("linked-backup.sqlite")).unwrap();
    let inventory = fixture.call(Call::StorageInventory).await;
    let journal = inventory["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "journal")
        .unwrap();
    assert_eq!(journal["removable"], false, "{journal}");
    let cleaned = fixture
        .call(Call::CleanStorage {
            categories: vec!["history".into()],
        })
        .await;
    assert_eq!(cleaned["success"], false, "{cleaned}");
    assert!(state.db_path().exists());
    assert!(fixture.root.join("linked-backup.sqlite").exists());
}

#[tokio::test]
async fn graceful_stop_never_kills_a_hup_and_term_resistant_job() {
    let fixture = Fixture::new().await;
    let ready = fixture.root.join("ready");
    fixture
        .launch(
            "job",
            "trap '' HUP TERM; echo ready > ready; while :; do sleep 1; done",
        )
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let graceful = fixture.call(Call::StopResources { force: false }).await;
    let force = fixture.call(Call::StopResources { force: true }).await;
    assert_eq!(graceful["stopped"], false, "{graceful}");
    assert!(!graceful["remaining"].as_array().unwrap().is_empty());
    assert_eq!(force["stopped"], true, "{force}");
    assert_eq!(fixture.client.ping().await.unwrap().lifecycle, "stopped");
}

/// An incomplete Stop All must not lock the device: an explicit reopen
/// replaces the daemon and leaves what is still running alone.
#[tokio::test]
async fn reopen_after_incomplete_stop_replaces_daemon_and_keeps_remaining_work() {
    let fixture = Fixture::new().await;
    let ready = fixture.root.join("ready");
    let job = fixture
        .launch(
            "job",
            "trap '' HUP TERM; echo ready > ready; while :; do sleep 1; done",
        )
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let dir = fixture.daemon.state().terminals_dir().join(&job.id);
    let host: Value =
        serde_json::from_slice(&std::fs::read(dir.join("host.json")).unwrap()).unwrap();
    let program = host["pid"].as_i64().unwrap() as i32;

    let graceful = fixture.call(Call::StopResources { force: false }).await;
    let reopen = fixture
        .client
        .call::<Value>(Call::PrepareAttach {
            build: sandhill_daemon::build_id().to_string(),
        })
        .await;
    let lifecycle = fixture.client.ping().await.unwrap().lifecycle;
    let survived = unsafe { libc::kill(program, 0) } == 0;
    // The daemon is now replacing itself; clean the task-owned job directly.
    unsafe { libc::kill(-program, libc::SIGKILL) };

    assert_eq!(graceful["stopped"], false, "{graceful}");
    assert_eq!(reopen.unwrap()["restart"], true);
    assert_eq!(lifecycle, "stopping");
    assert!(survived, "reopening must not stop remaining work");
}

/// Hosts started by builds before managed stops recorded only their pids
/// (`{"host":…,"pid":…}`). Stop All must refuse them; explicit force must
/// close them through the Hangup request those hosts understand.
#[tokio::test]
async fn legacy_terminal_host_is_closed_only_by_explicit_force() {
    let fixture = Fixture::new().await;
    let shell = fixture.launch("shell", "exec sleep 120").await;
    let dir = fixture.daemon.state().terminals_dir().join(&shell.id);
    let host: Value =
        serde_json::from_slice(&std::fs::read(dir.join("host.json")).unwrap()).unwrap();
    let legacy = json!({"host": host["host"], "pid": host["pid"]});
    std::fs::write(dir.join("host.json"), legacy.to_string()).unwrap();
    let _ = std::fs::remove_file(dir.join("lifecycle.json"));
    let program = host["pid"].as_i64().unwrap() as i32;

    let graceful = fixture.call(Call::StopResources { force: false }).await;
    let program_survived = unsafe { libc::kill(program, 0) } == 0;
    let force = fixture.call(Call::StopResources { force: true }).await;

    assert_eq!(graceful["stopped"], false, "{graceful}");
    assert!(
        graceful["errors"][0]
            .as_str()
            .is_some_and(|e| e.contains("older Sandhill")),
        "{graceful}"
    );
    assert!(
        program_survived,
        "Stop All must not end a legacy host's program"
    );
    assert_eq!(force["stopped"], true, "{force}");
    assert!(!fixture.terminals().info(&shell.id).unwrap().running);
}

#[tokio::test]
async fn cache_cleanup_preserves_history_and_project_data_then_history_uses_live_db() {
    let fixture = Fixture::new().await;
    let state = fixture.daemon.state();
    std::fs::create_dir_all(state.root().join("pages/a")).unwrap();
    std::fs::write(state.root().join("pages/a/index.html"), b"cached").unwrap();
    std::fs::write(state.blobs_dir().join("retained"), b"saved output").unwrap();
    let project = fixture.root.join("project");
    std::fs::create_dir_all(project.join(".sandhill/outputs")).unwrap();
    std::fs::write(project.join(".sandhill/outputs/user"), b"never delete").unwrap();
    fixture
        .call(Call::OpenProject {
            path: project.to_string_lossy().into_owned(),
        })
        .await;
    let before = fixture.call(Call::StorageInventory).await;
    assert!(
        before["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["category"] == "projects" && e["removable"] == false)
    );
    let cache = fixture
        .call(Call::CleanStorage {
            categories: vec!["cache".into()],
        })
        .await;
    assert_eq!(cache["success"], true, "{cache}");
    assert!(!state.root().join("pages/a/index.html").exists());
    assert!(state.blobs_dir().join("retained").exists());
    // Incidental cache cleanup must not put the daemon into Stop All mode.
    fixture
        .call(Call::OpenProject {
            path: project.to_string_lossy().into_owned(),
        })
        .await;
    let history = fixture
        .call(Call::CleanStorage {
            categories: vec!["history".into()],
        })
        .await;
    assert_eq!(history["success"], true, "{history}");
    assert!(
        state.db_path().exists(),
        "the live journal is never unlinked"
    );
    assert!(project.join(".sandhill/outputs/user").exists());
    assert!(!state.blobs_dir().join("retained").exists());
    let sessions = fixture.call(Call::ListSessions).await;
    assert_eq!(sessions, json!([]));
}

#[tokio::test]
async fn real_kernel_explicit_stop_confirms_os_exit() {
    if sandhill_daemon::kernel::find_kernelspec("python3", None)
        .await
        .is_err()
    {
        eprintln!("skipping: python3 kernel is not installed");
        return;
    }
    let fixture = Fixture::new().await;
    let session = fixture
        .client
        .start_session("python3", Some(fixture.root.to_str().unwrap()))
        .await
        .unwrap();
    let inventory = fixture.call(Call::ResourceInventory).await;
    let inventoried = inventory["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == format!("kernel:{}", session.id));
    let stopped = fixture.call(Call::StopResources { force: false }).await;
    let force = fixture.call(Call::StopResources { force: true }).await;
    assert!(inventoried, "{inventory}");
    assert_eq!(stopped["stopped"], true, "{stopped}");
    assert_eq!(force["stopped"], true, "{force}");
    assert!(!sandhill_daemon::kernel::pid_alive(session.pid.unwrap()));
}
