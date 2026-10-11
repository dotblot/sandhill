//! Kernels inside conda environments, found without being registered with
//! Jupyter. The daemon runs as a separate process here so it can be given
//! its own home folder.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::EVENT_RUN_FINISHED;
use sandhill_proto::client::Client;
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");
const TIMEOUT: Duration = Duration::from_secs(90);

/// A Python that has ipykernel installed, if there is one.
fn python_with_ipykernel() -> Option<PathBuf> {
    let output = std::process::Command::new("python3")
        .args(["-c", "import ipykernel, sys; print(sys.executable)"])
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    (output.status.success() && path.is_absolute()).then_some(path)
}

/// Lays out `<prefix>` like a conda environment with ipykernel installed,
/// whose `bin/python` is `python`.
fn conda_env(prefix: &Path, python: &Path) {
    std::fs::create_dir_all(prefix.join("conda-meta")).unwrap();
    std::fs::create_dir_all(prefix.join("bin")).unwrap();
    std::os::unix::fs::symlink(python, prefix.join("bin/python")).unwrap();
    let kernel = prefix.join("share/jupyter/kernels/python3");
    std::fs::create_dir_all(&kernel).unwrap();
    let spec = serde_json::json!({
        "argv": ["python", "-m", "ipykernel_launcher", "-f", "{connection_file}"],
        "display_name": "Python 3 (ipykernel)",
        "language": "python",
    });
    std::fs::write(kernel.join("kernel.json"), spec.to_string()).unwrap();
}

#[tokio::test]
async fn a_conda_environment_runs_its_own_python() {
    let Some(python) = python_with_ipykernel() else {
        eprintln!("skipping: no python3 with ipykernel installed");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    // Python reports its path with symlinks resolved (on macOS the temp
    // folder is under /var, a link to /private/var), so start resolved.
    let home = tmp.path().canonicalize().unwrap();
    let root = home.join("miniconda3");
    std::fs::create_dir_all(root.join("conda-meta")).unwrap();
    let env = root.join("envs/omicverse");
    conda_env(&env, &python);

    let state = home.join("state");
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in [
        "JUPYTER_PATH",
        "JUPYTER_DATA_DIR",
        "XDG_DATA_HOME",
        "CONDA_EXE",
        "CONDA_PREFIX",
        "MAMBA_ROOT_PREFIX",
        "PYENV_ROOT",
    ] {
        command.env_remove(var);
    }
    let _serve = command.spawn().unwrap();
    let socket = state.join("sandhilld.sock");
    let mut client = None;
    for _ in 0..100 {
        if let Ok(c) = Client::connect_unix(&socket).await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut client = client.expect("sandhilld did not start");

    let specs = client.list_kernelspecs().await.unwrap();
    let spec = specs
        .iter()
        .find(|k| k.name == "conda-env-omicverse-py")
        .unwrap_or_else(|| panic!("the environment's kernel is not listed: {specs:?}"));
    assert_eq!(spec.display_name, "Python (omicverse)");
    assert_eq!(spec.env.as_deref(), Some("omicverse"));

    let session = client
        .start_session("conda-env-omicverse-py", None)
        .await
        .unwrap()
        .id;
    client.subscribe(&session, None).await.unwrap();
    let code = "import os, sys\nprint(sys.executable)\nprint(os.environ['CONDA_PREFIX'])\nprint(os.environ['PATH'].split(':')[0])";
    let run = client.execute(&session, code, None).await.unwrap().run;
    let mut stdout = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for output")
            .expect("connection closed");
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            stdout.push_str(event.content["text"].as_str().unwrap());
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            break;
        }
    }
    client.shutdown(&session).await.unwrap();

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [
            env.join("bin/python").to_str().unwrap(),
            env.to_str().unwrap(),
            env.join("bin").to_str().unwrap(),
        ]
    );
}
