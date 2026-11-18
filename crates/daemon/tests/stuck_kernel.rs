//! A kernel that won't stop when asked: shutting it down fails and leaves it
//! listed, and Force stop, the user's explicit choice, then ends it. The
//! kernel is ipykernel with a shutdown that hangs. The only test here: it
//! sets `JUPYTER_PATH` for the whole test binary.

mod common;

use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, SessionStatus};
use tokio::net::UnixListener;

const STUCK: &str = r#"
import time
from ipykernel.ipkernel import IPythonKernel
from ipykernel.kernelapp import IPKernelApp

class Stuck(IPythonKernel):
    def do_shutdown(self, restart):
        time.sleep(120)
        return {"status": "ok", "restart": restart}

IPKernelApp.launch_instance(kernel_class=Stuck)
"#;

#[tokio::test]
async fn a_kernel_that_wont_stop_stays_listed_until_forced() {
    let Ok(python) = sandhill_daemon::kernel::find_kernelspec("python3", None).await else {
        eprintln!("skipping: Jupyter kernel \"python3\" is not installed");
        return;
    };
    let dir = common::tempdir().unwrap();
    let spec = dir.path().join("kernels/kernels/stuck");
    std::fs::create_dir_all(&spec).unwrap();
    let argv = [
        python.kernelspec.argv[0].as_str(),
        "-c",
        STUCK,
        "-f",
        "{connection_file}",
    ];
    std::fs::write(
        spec.join("kernel.json"),
        serde_json::json!({"argv": argv, "display_name": "Stuck", "language": "python"})
            .to_string(),
    )
    .unwrap();
    // SAFETY: the only test in this binary, set before anything reads it.
    unsafe { std::env::set_var("JUPYTER_PATH", dir.path().join("kernels")) };

    let state = dir.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let daemon = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon: Arc<Daemon> = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let client = Client::connect_unix(&socket).await.unwrap();
    let info = client.start_session("stuck", None).await.unwrap();

    let err = client.shutdown(&info.id).await.unwrap_err().to_string();
    assert!(err.contains("did not stop gracefully"), "{err}");
    let listed = client.list_sessions().await.unwrap();
    let still = listed.iter().find(|s| s.id == info.id).unwrap();
    assert_eq!(still.status, SessionStatus::Running);
    let pid = info.pid.unwrap();
    assert!(sandhill_daemon::kernel::pid_alive(pid));

    let _: serde_json::Value = client
        .call(Call::ForceStop {
            session: info.id.clone(),
        })
        .await
        .unwrap();
    assert!(!sandhill_daemon::kernel::pid_alive(pid));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let sessions = client.list_sessions().await.unwrap();
            if sessions
                .iter()
                .any(|s| s.id == info.id && s.status == SessionStatus::Ended)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the forced session was never marked ended");
}
