//! Ark installed the way the app installs it on a server (in the home
//! folder, not registered with Jupyter) runs R as the `ark` kernel. The
//! daemon runs as a separate process here so it can be given its own home
//! folder.
//!
//! Needs a real Ark: `SANDHILL_TEST_ARK=/path/to/ark`, and R on `PATH`.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::EVENT_RUN_FINISHED;
use sandhill_proto::client::Client;
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");
const TIMEOUT: Duration = Duration::from_secs(90);

#[tokio::test]
async fn the_installed_ark_runs_r() {
    let Some(ark) = std::env::var_os("SANDHILL_TEST_ARK")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    else {
        eprintln!("skipping: set SANDHILL_TEST_ARK to an Ark binary");
        return;
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    if !std::env::split_paths(&path).any(|dir| dir.join("R").is_file()) {
        eprintln!("skipping: R is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let installed = home.join(sandhill_proto::ARK_PATH);
    std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
    std::fs::copy(&ark, &installed).unwrap();

    let state = home.join("state");
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in ["JUPYTER_PATH", "JUPYTER_DATA_DIR", "XDG_DATA_HOME"] {
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
        .find(|k| k.name == "ark")
        .unwrap_or_else(|| panic!("the installed Ark is not listed: {specs:?}"));
    assert_eq!(spec.display_name, "R (Ark)");

    let session = client.start_session("ark", None).await.unwrap().id;
    client.subscribe(&session, None).await.unwrap();
    let run = client
        .execute(&session, "cat(R.version$language, 1 + 1, \"\\n\")", None)
        .await
        .unwrap()
        .run;
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
    assert_eq!(stdout.trim(), "R 2");
}
