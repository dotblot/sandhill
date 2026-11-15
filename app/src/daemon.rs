//! The trial app's connection to sandhilld, through the same `sandhill-link`
//! and `sandhill-proto` crates the Tauri app uses.
//!
//! Those crates run on tokio, and GPUI has its own executor, so the
//! connection lives on a tokio runtime and each call hands back a future
//! that GPUI can await. tokio's channels and join handles work from any
//! executor.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use anyhow::{Context, Result};
use sandhill_link::{Options, Target};
use sandhill_proto::client::Client;
use sandhill_proto::{AppRequest, Call, Event, TerminalEvent};
use serde::de::DeserializeOwned;
use tokio::process::Child;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// What a connection streams: session events and terminal output.
pub struct Streams {
    pub events: UnboundedReceiver<Event>,
    pub terminals: UnboundedReceiver<TerminalEvent>,
    /// What the daemon asks of this window: agents' edits to its documents.
    pub requests: UnboundedReceiver<AppRequest>,
    pub lsp: UnboundedReceiver<sandhill_proto::LspMessage>,
}

/// A call waiting its turn, and where its reply goes.
type Queued = (
    Call,
    tokio::sync::oneshot::Sender<Result<serde_json::Value>>,
);

pub struct Daemon {
    rt: Arc<Runtime>,
    client: Arc<Client>,
    pub(crate) language: Arc<crate::editor::language::transport::Transport>,
    /// Calls that must reach the daemon in the order they were made.
    queue: UnboundedSender<Queued>,
    /// Ports on the daemon's machine reached from here (see
    /// `forward_port`): port there -> port here.
    ports: Arc<std::sync::Mutex<HashMap<u16, u16>>>,
    /// The tasks carrying them, which end with the connection.
    forwards: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// The `sandhilld attach` process; the connection ends with it.
    child: Arc<tokio::sync::Mutex<Child>>,
    /// Cached per connection; hashing bundled payloads never runs on GPUI.
    bundled_build: Arc<OnceLock<Option<bool>>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        for task in self.forwards.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

/// Includes attaches whose project window has already closed. Dropping a GPUI
/// waiter does not cancel the tokio attach task, which may still start a daemon.
static CONNECTING: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Mutex::default);

struct PendingConnection(String);

impl PendingConnection {
    fn new(host: String) -> Self {
        *CONNECTING.lock().unwrap().entry(host.clone()).or_default() += 1;
        Self(host)
    }
}

impl Drop for PendingConnection {
    fn drop(&mut self) {
        let mut connecting = CONNECTING.lock().unwrap();
        if let Some(count) = connecting.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                connecting.remove(&self.0);
            }
        }
    }
}

#[test]
fn pending_attach_blocks_stop_until_every_attach_finishes() {
    let host = "quit-attach-counter.test";
    assert!(!Daemon::connecting(host));
    let first = PendingConnection::new(host.into());
    let second = PendingConnection::new(host.into());
    assert!(Daemon::connecting(host));
    drop(first);
    assert!(Daemon::connecting(host));
    drop(second);
    assert!(!Daemon::connecting(host));
}

/// How to reach servers: the builds of sandhilld (and Ark) to install on
/// them, from `Contents/Resources/daemons` in the Mac app or
/// `$SANDHILL_DAEMON_BUILDS`, as the Tauri app has it. `$SANDHILL_SSH`
/// replaces the ssh program.
fn link_options() -> Options {
    let builds = std::env::var_os("SANDHILL_DAEMON_BUILDS")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            let dir = exe.parent()?.parent()?.join("Resources/daemons");
            dir.is_dir().then_some(dir)
        });
    match &builds {
        Some(dir) => tracing::info!("builds of sandhilld for servers: {}", dir.display()),
        None => tracing::info!("no builds of sandhilld for servers in this app"),
    }
    Options {
        builds,
        ssh: std::env::var_os("SANDHILL_SSH").map(Into::into),
    }
}

impl Daemon {
    pub(crate) fn connecting(host: &str) -> bool {
        CONNECTING.lock().unwrap().contains_key(host)
    }

    /// Connects to the daemon on `target`, starting it if needed. Also
    /// returns the stream of session events.
    pub fn connect(
        rt: Arc<Runtime>,
        target: Target,
    ) -> impl Future<Output = Result<(Daemon, Streams)>> + 'static {
        let pending = PendingConnection::new(match &target {
            Target::Local => sandhill_link::LOCAL.to_string(),
            Target::Ssh(destination) => destination.to_string(),
        });
        let task = rt.spawn(async move {
            let _pending = pending;
            let progress = |message: &str| tracing::info!("{message}");
            let mut conn = sandhill_link::connect(&target, &link_options(), &progress).await?;
            let events = conn.client.take_events().context("no event stream")?;
            let terminals = conn
                .client
                .take_terminal_events()
                .context("no terminal stream")?;
            let requests = conn.client.take_requests().context("no request stream")?;
            let lsp = conn
                .client
                .take_lsp_messages()
                .context("no language stream")?;
            anyhow::Ok((
                conn,
                Streams {
                    events,
                    terminals,
                    requests,
                    lsp,
                },
            ))
        });
        async move {
            let (conn, streams) = task.await??;
            let client = Arc::new(conn.client);
            let (language, lsp) = crate::editor::language::transport::Transport::new(
                client.clone(),
                streams.lsp,
                &rt,
            );
            let streams = Streams { lsp, ..streams };
            let (queue, mut queued) = tokio::sync::mpsc::unbounded_channel::<Queued>();
            let sender = client.clone();
            rt.spawn(async move {
                while let Some((call, reply)) = queued.recv().await {
                    let _ = reply.send(
                        sender
                            .call::<serde_json::Value>(call)
                            .await
                            .map_err(Into::into),
                    );
                }
            });
            Ok((
                Daemon {
                    rt,
                    client,
                    language,
                    queue,
                    ports: Default::default(),
                    forwards: Default::default(),
                    child: Arc::new(tokio::sync::Mutex::new(conn.child)),
                    bundled_build: Default::default(),
                },
                streams,
            ))
        }
    }

    /// Observe EOF without retaining the attach process or stopping its daemon.
    pub(crate) fn closed(&self) -> impl Future<Output = ()> + Send + 'static {
        let client = self.client.clone();
        async move { client.wait_closed().await }
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

    /// Whether this device runs one of the daemon builds shipped by this app.
    pub(crate) fn matches_bundled_build(
        &self,
        build: Option<String>,
    ) -> impl Future<Output = Result<Option<bool>>> + 'static {
        let cache = self.bundled_build.clone();
        let client = self.client.clone();
        let task = self.rt.spawn(async move {
            if let Some(known) = cache.get() {
                return Ok(*known);
            }
            let build = match build {
                Some(build) => build,
                None => client.ping().await?.build,
            };
            tokio::task::spawn_blocking(move || {
                *cache.get_or_init(|| link_options().matches_daemon_build(&build))
            })
            .await
            .map_err(Into::into)
        });
        async move { task.await? }
    }

    /// Runs language transport futures on the connection's tokio reactor.
    pub(crate) fn language_task<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T>> + Send + 'static,
    ) -> impl Future<Output = Result<T>> + 'static {
        let task = self.rt.spawn(work);
        async move { task.await? }
    }

    /// Exact bounded text over `read_bytes`, also supported by older daemons.
    pub fn read_text_bounded(
        &self,
        path: String,
        max_bytes: u64,
    ) -> impl Future<Output = Result<sandhill_proto::FileText>> + 'static {
        let client = self.client.clone();
        let task = self
            .rt
            .spawn(async move { client.read_text_bounded(&path, max_bytes).await });
        async move { Ok(task.await??) }
    }

    /// Like `call`, but gives up after `limit`: for asking kernels
    /// something on the side, where one that doesn't answer mustn't hold
    /// the rest up.
    pub fn call_within<T>(
        &self,
        call: Call,
        limit: std::time::Duration,
    ) -> impl Future<Output = Result<T>> + 'static
    where
        T: DeserializeOwned + Send + 'static,
    {
        let client = self.client.clone();
        let task = self
            .rt
            .spawn(async move { tokio::time::timeout(limit, client.call::<T>(call)).await });
        async move {
            match task.await? {
                Ok(answer) => Ok(answer?),
                Err(_) => Err(anyhow::anyhow!("no answer in {} s", limit.as_secs())),
            }
        }
    }

    /// A stop reply only acknowledges the request. Every link requires the
    /// bridge's `--confirm-exit` contract: successful exit proves the daemon's
    /// exact birth identity is gone, and SSH propagates that status. Legacy
    /// bridges reject that flag before connecting; socket EOF is not proof.
    pub(crate) fn stop_confirmed(&self) -> impl Future<Output = Result<()>> + 'static {
        let client = self.client.clone();
        let child = self.child.clone();
        let task = self.rt.spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                let _: serde_json::Value = client.call(Call::StopDaemon).await?;
                client.wait_closed().await;
                let status = child.lock().await.wait().await?;
                anyhow::ensure!(
                    status.success(),
                    "daemon exit was not confirmed: attach exited {status}"
                );
                anyhow::Ok(())
            })
            .await
            .context("daemon shutdown did not complete within 30 s")?
        });
        async move { task.await? }
    }

    /// Sends one request after those made before it with this method have
    /// been answered: runs, so that Run All runs the cells in order. (Calls
    /// made with `call` go out on tasks of their own and can pass each
    /// other.)
    pub fn call_in_order<T>(&self, call: Call) -> impl Future<Output = Result<T>> + 'static
    where
        T: DeserializeOwned + Send + 'static,
    {
        let (reply, answer) = tokio::sync::oneshot::channel();
        let sent = self.queue.send((call, reply));
        async move {
            sent.map_err(|_| anyhow::anyhow!("the connection has closed"))?;
            let value = answer
                .await
                .map_err(|_| anyhow::anyhow!("the connection has closed"))??;
            Ok(serde_json::from_value(value)?)
        }
    }

    /// Sends calls one after another, in order, without waiting for
    /// replies: for keys typed into a terminal.
    pub fn ordered(
        &self,
        make: impl Fn(String) -> Call + Send + 'static,
    ) -> UnboundedSender<String> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let client = self.client.clone();
        self.rt.spawn(async move {
            while let Some(text) = rx.recv().await {
                if let Err(err) = client.call::<serde_json::Value>(make(text)).await {
                    tracing::warn!("{err}");
                }
            }
        });
        tx
    }

    /// The port on this machine that reaches `port` on the daemon's, for a
    /// server's Shiny app, Quarto preview or page server: a listener on
    /// 127.0.0.1 whose connections are carried over the daemon's
    /// connection, as `ssh -L` would (`sandhill_link::forward`). Each port
    /// is forwarded once per connection.
    pub fn forward_port(&self, port: u16) -> impl Future<Output = Result<u16>> + 'static {
        let known = self.ports.lock().unwrap().get(&port).copied();
        let ports = self.ports.clone();
        let client = self.client.clone();
        let rt = self.rt.clone();
        let opened = known.is_none().then(|| {
            self.rt.spawn(async move {
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .context("cannot open a port on this machine")?;
                let here = listener.local_addr()?.port();
                anyhow::Ok((listener, here))
            })
        });
        // Only the connection holds the tasks' list: once it has gone, a
        // port opened meanwhile isn't wanted.
        let forwards = Arc::downgrade(&self.forwards);
        async move {
            if let Some(here) = known {
                return Ok(here);
            }
            let (listener, here) = opened.expect("opened when not known").await??;
            let Some(forwards) = forwards.upgrade() else {
                anyhow::bail!("the connection has closed");
            };
            {
                let mut ports = ports.lock().unwrap();
                // Asked twice at once: the first one stands.
                if let Some(&first) = ports.get(&port) {
                    return Ok(first);
                }
                ports.insert(port, here);
            }
            let task = rt.spawn(sandhill_link::forward::forward(listener, port, move || {
                Some(client.clone())
            }));
            forwards.lock().unwrap().push(task);
            Ok(here)
        }
    }

    /// Sends a call and only logs a failure.
    pub fn fire(&self, call: Call) {
        let client = self.client.clone();
        self.rt.spawn(async move {
            if let Err(err) = client.call::<serde_json::Value>(call).await {
                tracing::warn!("{err}");
            }
        });
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
    use crate::editor::notebook;
    use crate::editor::shown::{Shown, show};

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
        let (daemon, streams) = rt
            .block_on(Daemon::connect(rt.clone(), Target::Local))
            .unwrap();

        // A port on the daemon's machine, reached through its connection as
        // a server's Shiny app or Quarto preview is.
        {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let echo = rt
                .block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0)))
                .unwrap();
            let there = echo.local_addr().unwrap().port();
            rt.spawn(async move {
                let (mut stream, _) = echo.accept().await.unwrap();
                let mut buf = [0u8; 5];
                stream.read_exact(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
            });
            let here = rt.block_on(daemon.forward_port(there)).unwrap();
            assert_ne!(here, there);
            assert_eq!(rt.block_on(daemon.forward_port(there)).unwrap(), here);
            let echoed = rt
                .block_on(async {
                    tokio::time::timeout(Duration::from_secs(10), async {
                        let mut stream =
                            tokio::net::TcpStream::connect(("127.0.0.1", here)).await?;
                        stream.write_all(b"hello").await?;
                        let mut buf = [0u8; 5];
                        stream.read_exact(&mut buf).await?;
                        anyhow::Ok(buf)
                    })
                    .await
                })
                .unwrap()
                .unwrap();
            assert_eq!(&echoed, b"hello");
        }

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
                let mut events = streams.events;
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
        let _: serde_json::Value = rt
            .block_on(daemon.call(Call::Shutdown {
                session: session.id,
            }))
            .unwrap();
    }
}
