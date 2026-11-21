//! A real micromamba setup through the daemon, using only temporary state.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, EnvMode, InstallPrefs, KernelspecInfo, OwnSetup, OwnSetupPlan, QuickInstalling,
    QuickReadiness, TerminalInfo,
};
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

fn reachable(url: &str) -> bool {
    std::process::Command::new("/usr/bin/curl")
        .args([
            "-q",
            "--fail",
            "--silent",
            "--location",
            "--range",
            "0-0",
            "--max-time",
            "20",
            "--output",
            "/dev/null",
            url,
        ])
        .status()
        .is_ok_and(|status| status.success())
}

fn files(folder: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

async fn job_done(client: &Client, started: &QuickInstalling) {
    let ended = tokio::time::timeout(Duration::from_secs(900), async {
        loop {
            let terminals: Vec<TerminalInfo> = client.call(Call::ListTerminals).await.unwrap();
            if let Some(done) = terminals
                .into_iter()
                .find(|t| t.id == started.job.id && !t.running)
            {
                break done;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    let log = std::fs::read_to_string(&started.log).unwrap_or_default();
    assert_eq!(
        ended.expect(&log).exit.and_then(|exit| exit.code),
        Some(0),
        "{log}"
    );
}

fn assert_home(home: &Path, expected: &[&str], step: &str) {
    let entries = files(home);
    if entries != expected {
        let output = std::process::Command::new("/usr/bin/find")
            .arg(home)
            .arg("-maxdepth")
            .arg("5")
            .output()
            .unwrap();
        let tree = String::from_utf8_lossy(&output.stdout);
        let leaks = tree
            .lines()
            .filter(|p| !p.starts_with(home.join(".sandhill").to_str().unwrap()))
            .collect::<Vec<_>>();
        panic!("Unexpected files in the home folder after {step}: {leaks:?}");
    }
}

#[tokio::test]
async fn own_python_never_installs_into_home_and_is_removed_with_its_kernel() {
    let tmp = common::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let state = root.join("state");
    let jupyter = root.join("outside-jupyter");
    let outside = jupyter.join("kernels/outside");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("keep.txt"), "personal file").unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join("kernel.json"),
        r#"{"argv":["/bin/sh","{connection_file}"],"display_name":"Outside","language":"sh"}"#,
    )
    .unwrap();
    let _serve = Command::new(DAEMON)
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("SHELL", "/bin/bash")
        .env("TMPDIR", &root)
        .env("JUPYTER_DATA_DIR", &jupyter)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = state.join("sandhilld.sock");
    let client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(client) = Client::connect_unix(&socket).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let folder = home.join(sandhill_proto::OWN_FOLDER);
    let setup = OwnSetup {
        folder: folder.to_string_lossy().into_owned(),
        r: false,
        python: true,
        kits: Vec::new(),
        copy_from: Vec::new(),
    };
    let plan: OwnSetupPlan = client
        .call(Call::PlanOwnSetup {
            setup: setup.clone(),
        })
        .await
        .unwrap();
    if let Some(refused) = plan.refused {
        if !(cfg!(any(target_os = "linux", target_os = "macos"))
            && cfg!(any(target_arch = "x86_64", target_arch = "aarch64")))
        {
            eprintln!("skipping real own setup: {refused}");
            return;
        }
        panic!("{refused}");
    }
    let download = plan.steps[0]
        .command
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .unwrap();
    for url in [
        download,
        "https://conda.anaconda.org/conda-forge/noarch/repodata.json.zst",
        "https://conda.anaconda.org/bioconda/noarch/repodata.json.zst",
    ] {
        if !reachable(url) {
            eprintln!("skipping real own setup: could not reach {url}");
            return;
        }
    }
    let pref_result = client
        .call::<InstallPrefs>(Call::SetEnvMode {
            mode: Some(EnvMode::Own),
        })
        .await;
    assert!(pref_result.is_err());
    let first: Vec<KernelspecInfo> = client.call(Call::ListKernelspecs).await.unwrap();
    assert!(first.iter().any(|spec| spec.name == "outside"));
    let installed: QuickInstalling = client.call(Call::RunOwnSetup { setup }).await.unwrap();
    job_done(&client, &installed).await;
    assert_home(&home, &[".sandhill", "keep.txt"], "setup");
    assert_eq!(files(&home.join(".sandhill")), ["environment"]);
    let ready: QuickReadiness = client.call(Call::QuickReadiness).await.unwrap();
    assert_home(&home, &[".sandhill", "keep.txt"], "readiness");
    assert_eq!(ready.prefs.mode, Some(EnvMode::Own));
    let own = ready.own.unwrap();
    assert!(own.r.is_none());
    assert_eq!(own.kits, ["core"]);
    let python = own.python.unwrap();
    assert!(python.version.starts_with("3.12."));
    let found = ready.python.found.unwrap();
    assert_eq!(found.prefix, python.prefix);
    assert!(Path::new(&found.program).starts_with(&folder));
    assert!(
        ready
            .installed
            .iter()
            .any(|row| row.kind == "own-environment" && row.place == own.folder && row.ours)
    );
    let kernels: Vec<KernelspecInfo> = client.call(Call::ListKernelspecs).await.unwrap();
    assert_eq!(kernels.len(), 1, "{kernels:?}");
    assert_eq!(kernels[0].name, python.kernel);
    let missing = client.start_session("outside", None).await.unwrap_err();
    assert!(missing.to_string().contains("R and Python page"));
    let session = client
        .start_session(&python.kernel, root.to_str())
        .await
        .unwrap();
    assert_home(&home, &[".sandhill", "keep.txt"], "kernel startup");
    let prefs: InstallPrefs = client
        .call(Call::SetEnvMode {
            mode: Some(EnvMode::Existing),
        })
        .await
        .unwrap();
    assert_eq!(prefs.own_folder, own.folder);
    assert!(folder.is_dir());
    let removed: QuickInstalling = client
        .call(Call::RemoveOwnEnvironment)
        .await
        .unwrap_or_else(|e| {
            let log =
                std::fs::read_to_string(state.join("kernels").join(format!("{}.log", session.id)))
                    .unwrap_or_default();
            panic!("{e}\n{log}");
        });
    job_done(&client, &removed).await;
    assert!(!folder.exists());
    assert_home(&home, &["keep.txt"], "removal");
    let ready: QuickReadiness = client.call(Call::QuickReadiness).await.unwrap();
    assert!(ready.own.is_none());
    assert!(ready.prefs.mode.is_none());
    assert!(ready.prefs.own_folder.is_empty());
    assert!(
        !ready
            .installed
            .iter()
            .any(|row| row.kind == "own-environment")
    );
    assert_home(&home, &["keep.txt"], "readiness after removal");
    assert!(
        client
            .execute_quietly(&session.id, "print('still alive')")
            .await
            .is_err()
    );
}
