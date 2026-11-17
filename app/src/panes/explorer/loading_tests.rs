//! The Explorer against a test-owned daemon and real Python kernel.

use crate::daemon::test_common as common;

use std::path::PathBuf;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext, TestAppContext, WindowOptions};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, EVENT_RUN_FINISHED, RunStarted};
use tokio::runtime::Runtime;

use super::{Block, Explorer};
use crate::daemon::Daemon;

const TIMEOUT: Duration = Duration::from_secs(90);

fn pump_until(cx: &mut TestAppContext, rt: &Runtime, ready: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        cx.run_until_parked();
        if cx.read(&ready) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for the table");
        rt.block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
    }
}

fn finish(rt: &Runtime, client: &mut Client, run: &str) {
    rt.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let event = client.next_event().await.expect("daemon disconnected");
                if event.run.as_deref() == Some(run) && event.msg_type == EVENT_RUN_FINISHED {
                    assert_eq!(event.content["status"], "ok", "{event:?}");
                    break;
                }
            }
        })
        .await
        .expect("kernel did not finish");
    });
}

#[gpui_kit::test]
fn table_refused_while_busy_reloads_after_idle(cx: &mut TestAppContext) {
    let exe = std::env::var_os("SANDHILL_DAEMON")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/sandhilld")
        });
    if !exe.is_file() {
        eprintln!("skipping: build sandhilld first (cargo build -p sandhill-daemon)");
        return;
    }
    // GPUI's deterministic scheduler must be woken on this test's thread.
    let rt = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
    );
    let Ok(spec) = rt.block_on(sandhill_daemon::kernel::find_kernelspec("python3", None)) else {
        eprintln!("skipping: Jupyter kernel python3 is not installed");
        return;
    };
    if !std::process::Command::new(&spec.kernelspec.argv[0])
        .args(["-c", "import ipykernel, pandas"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipping: the python3 kernel needs ipykernel and pandas");
        return;
    }
    // This guard kills this test's kernels even if an assertion fails.
    let dir = common::tempdir().unwrap();
    let socket = dir.path().join("sandhilld.sock");
    let (daemon, mut client, session) = rt.block_on(async {
        let server = tokio::process::Command::new(exe)
            .arg("--state-dir")
            .arg(dir.path())
            .arg("--socket")
            .arg(&socket)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while !socket.exists() {
            assert!(Instant::now() < deadline, "test daemon did not start");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let client = Client::connect_unix(&socket).await.unwrap();
        let session = client.start_session("python3", None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        let app_client = Client::connect_unix(&socket).await.unwrap();
        let daemon = Daemon::from_test_connection(
            rt.clone(),
            sandhill_link::Connection {
                client: app_client,
                child: server,
            },
        );
        (Rc::new(daemon), client, session)
    });
    let run: RunStarted = rt
        .block_on(
            client.call(Call::Execute {
                session: session.clone(),
                code:
                    "import pandas as pd, time\ndf = pd.DataFrame({'a': [1, 2, 3]})\ntime.sleep(2)"
                        .into(),
                cell: None,
                quiet: false,
                by: None,
            }),
        )
        .unwrap();
    let (handle, explorer) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::look::install(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                Explorer::new(
                    daemon.clone(),
                    session.clone(),
                    false,
                    "python".into(),
                    vec!["df".into()],
                    None,
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    pump_until(cx, &rt, |cx| explorer.read(cx).stale);
    cx.read(|cx| {
        assert!(
            explorer
                .read(cx)
                .error
                .as_deref()
                .is_some_and(|e| e.contains(super::BUSY)),
            "a refused request must explain why the table is not loaded"
        );
    });
    finish(&rt, &mut client, &run.run);
    cx.update_window(handle, |_, _, cx| {
        explorer.update(cx, |table, cx| {
            // The busy-start event was missed: the BUSY reply must still
            // make the next idle event retry, even though busy was false.
            assert!(!table.busy);
            table.set_busy(false, cx);
            assert!(
                matches!(table.blocks.get(&0), Some(Block::Loading)),
                "idle must restart a request refused while busy"
            );
        });
    })
    .unwrap();
    pump_until(cx, &rt, |cx| explorer.read(cx).shape.is_some());
    cx.read(|cx| {
        let table = explorer.read(cx);
        assert_eq!(table.shape.as_ref().unwrap().rows, 3);
        assert!(matches!(table.blocks.get(&0), Some(Block::Ready(_))));
        assert!(table.error.is_none());
    });
    let run: RunStarted = rt
        .block_on(client.call(Call::Execute {
            session: session.clone(),
            code: "time.sleep(2)".into(),
            cell: None,
            quiet: false,
            by: None,
        }))
        .unwrap();
    let waiting = cx
        .update_window(handle, |_, window, cx| {
            cx.new(|cx| {
                Explorer::new(
                    daemon.clone(),
                    session.clone(),
                    true,
                    "python".into(),
                    vec!["df".into()],
                    None,
                    window,
                    cx,
                )
            })
        })
        .unwrap();
    cx.read(|cx| {
        let table = waiting.read(cx);
        assert!(table.busy && table.stale);
        assert!(table.blocks.is_empty(), "do not ask a busy kernel for rows");
    });
    finish(&rt, &mut client, &run.run);
    cx.update(|cx| waiting.update(cx, |table, cx| table.set_busy(false, cx)));
    pump_until(cx, &rt, |cx| waiting.read(cx).shape.is_some());
    cx.read(|cx| {
        let table = waiting.read(cx);
        assert_eq!(table.shape.as_ref().unwrap().rows, 3);
        assert!(matches!(table.blocks.get(&0), Some(Block::Ready(_))));
        assert!(table.error.is_none());
    });
    drop(waiting);
    rt.block_on(client.shutdown(&session)).unwrap();
    drop(daemon);
    // Drop the Explorer's last connection so its kill_on_drop daemon stops
    // before the temporary directory's kernel cleanup runs.
    cx.update_window(handle, |_, window, _| window.remove_window())
        .unwrap();
    drop(explorer);
    cx.run_until_parked();
}
