//! The trial app's connection to sandhilld, through the same `sandhill-link`
//! and `sandhill-proto` crates the Tauri app uses.
//!
//! Those crates run on tokio, and GPUI has its own executor, so the
//! connection lives on a tokio runtime and each call hands back a future
//! that GPUI can await. tokio's channels and join handles work from any
//! executor.

use std::future::Future;
use std::sync::Arc;

use anyhow::{Context, Result};
use sandhill_link::{Options, Target};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, Event};
use serde::de::DeserializeOwned;
#[cfg(test)]
use serde_json::Value;
use tokio::process::Child;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedReceiver;

pub struct Daemon {
    rt: Arc<Runtime>,
    client: Arc<Client>,
    /// The `sandhilld attach` process; the connection ends with it.
    _child: Child,
}

impl Daemon {
    /// Connects to the daemon on `target`, starting it if needed. Also
    /// returns the stream of session events.
    pub fn connect(
        rt: Arc<Runtime>,
        target: Target,
    ) -> impl Future<Output = Result<(Daemon, UnboundedReceiver<Event>)>> + 'static {
        let task = rt.spawn(async move {
            let progress = |message: &str| tracing::info!("{message}");
            let mut conn = sandhill_link::connect(&target, &Options::default(), &progress).await?;
            let events = conn.client.take_events().context("no event stream")?;
            anyhow::Ok((conn, events))
        });
        async move {
            let (conn, events) = task.await??;
            Ok((
                Daemon {
                    rt,
                    client: Arc::new(conn.client),
                    _child: conn.child,
                },
                events,
            ))
        }
    }

    /// Sends one request; the reply arrives on the returned future.
    pub fn call<T>(&self, call: Call) -> impl Future<Output = Result<T>> + 'static
    where
        T: DeserializeOwned + Send + 'static,
    {
        let client = self.client.clone();
        let task = self.rt.spawn(async move { client.call::<T>(call).await });
        async move { Ok(task.await??) }
    }
}

#[cfg(test)]
mod tests {
    //! Runs a notebook cell the way the window does, against a real daemon
    //! and kernel, without drawing anything.

    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;

    use sandhill_daemon::outputs::OutputBuilder;
    use sandhill_proto::{
        CellRef, EVENT_RUN_FINISHED, KernelspecInfo, RunStarted, SessionInfo, Subscribed,
    };

    use super::*;
    use crate::notebook;
    use crate::shown::{Shown, show};

    /// The workspace's own build of sandhilld (`cargo build -p sandhill-daemon`).
    fn sandhilld() -> Option<PathBuf> {
        let path = std::env::var_os("SANDHILL_DAEMON")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/sandhilld")
            });
        path.exists().then_some(path)
    }

    #[test]
    fn runs_a_notebook_cell_in_a_real_kernel() {
        let Some(exe) = sandhilld() else {
            eprintln!("skipping: build sandhilld first (cargo build -p sandhill-daemon)");
            return;
        };
        let rt = Arc::new(Runtime::new().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let socket = state.join("sandhilld.sock");
        let _serve = rt.block_on(async {
            tokio::process::Command::new(&exe)
                .arg("--state-dir")
                .arg(&state)
                .arg("serve")
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .unwrap()
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: this is the only test that reads these variables.
        unsafe {
            std::env::set_var("SANDHILL_DAEMON", &exe);
            std::env::set_var("SANDHILL_SOCKET", &socket);
        }
        let (daemon, mut events) = rt
            .block_on(Daemon::connect(rt.clone(), Target::Local))
            .unwrap();

        let kernels: Vec<KernelspecInfo> = rt.block_on(daemon.call(Call::ListKernelspecs)).unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("a.qmd").to_string_lossy().into_owned();
        let nb = notebook::parse(&path, "```{python}\nprint('hello')\n6 * 7\n```\n").unwrap();
        let Some(kernel) = notebook::pick_kernel(&nb, &kernels).filter(|k| {
            kernels
                .iter()
                .any(|s| &s.name == k && s.language == "python")
        }) else {
            eprintln!("skipping: no Python kernel is installed");
            return;
        };

        let session: SessionInfo = rt
            .block_on(daemon.call(Call::StartSession {
                kernel,
                cwd: Some(nb.folder().into()),
                project: Some(project.to_string_lossy().into_owned()),
            }))
            .unwrap();
        let _: Subscribed = rt
            .block_on(daemon.call(Call::Subscribe {
                session: session.id.clone(),
                since_seq: None,
            }))
            .unwrap();
        let cell = &nb.cells[0];
        let run: RunStarted = rt
            .block_on(daemon.call(Call::Execute {
                session: session.id.clone(),
                code: cell.source.clone(),
                cell: Some(CellRef {
                    file: path.clone(),
                    cell_id: cell.id.clone(),
                }),
                quiet: false,
                by: None,
            }))
            .unwrap();

        let mut builder = OutputBuilder::new();
        rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(60), async {
                while let Some(event) = events.recv().await {
                    if event.run.as_deref() != Some(&run.run) {
                        continue;
                    }
                    if event.msg_type == EVENT_RUN_FINISHED {
                        assert_eq!(event.content["status"], "ok");
                        break;
                    }
                    builder.push(&event.msg_type, &event.content);
                }
            })
            .await
            .expect("the cell did not finish")
        });
        let shown: Vec<Shown> = builder.finish().iter().map(show).collect();
        assert_eq!(
            shown,
            [Shown::Text("hello\n".into()), Shown::Text("42".into())]
        );
        let _: Value = rt
            .block_on(daemon.call(Call::Shutdown {
                session: session.id,
            }))
            .unwrap();
    }
}
