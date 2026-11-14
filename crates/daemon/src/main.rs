use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sandhill_daemon::lifecycle::ProcessIdentity;
use sandhill_daemon::terminal::host::HostOptions;
use sandhill_daemon::{Daemon, DaemonOptions, StateDir, build_id};
use sandhill_proto::Call;
use sandhill_proto::client::Client;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};

#[derive(Parser)]
#[command(
    name = "sandhilld",
    version,
    about = "Sandhill daemon: kernels and outputs that outlive the app"
)]
struct Cli {
    /// State directory (default: $SANDHILL_STATE_DIR, $XDG_STATE_HOME/sandhill or ~/.local/state/sandhill).
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,
    /// Socket path (default: <state dir>/sandhilld.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon in the foreground (systemd and launchd use this).
    Serve {
        /// What started it, when it can't tell: `attach --start` says `app`.
        #[arg(long, hide = true)]
        started_by: Option<String>,
    },
    /// Bridge stdin/stdout to the daemon's socket. The Mac app runs
    /// `ssh host sandhilld attach --start --confirm-exit` remotely.
    Attach {
        /// Start the daemon in the background if it isn't running.
        #[arg(long)]
        start: bool,
        /// Require successful exit to prove that the daemon process exited,
        /// rather than merely that its socket closed. Required by the app.
        #[arg(long)]
        confirm_exit: bool,
    },
    /// Serve Sandhill's tools to a coding agent over MCP on stdin/stdout.
    /// Claude Code, Codex and Antigravity start this themselves.
    Mcp {
        /// The project folder whose sessions the tools work with (default:
        /// $SANDHILL_PROJECT).
        #[arg(long)]
        project: Option<String>,
    },
    /// Hold one terminal and the program in it (the daemon starts this).
    #[command(hide = true)]
    TerminalHost {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long, default_value_t = 100)]
        cols: u16,
        #[arg(long, default_value_t = 30)]
        rows: u16,
        #[arg(last = true, required = true)]
        program: Vec<std::ffi::OsString>,
    },
    /// Run Codex's conversation for Sandhill's chat, through `codex
    /// app-server` (a Codex conversation's terminal runs this).
    #[command(hide = true)]
    CodexChat {
        /// The `codex` CLI.
        #[arg(long)]
        codex: PathBuf,
        #[arg(long)]
        project: Option<String>,
        /// Model, effort, mode and documents, as JSON.
        #[arg(long, default_value = "{}")]
        options: String,
        /// The first message.
        #[arg(long, allow_hyphen_values = true)]
        prompt: Option<String>,
        /// The thread to resume.
        #[arg(long)]
        thread: Option<String>,
        /// More arguments for `codex app-server`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run Antigravity's conversation for Sandhill's chat, with `agy`'s
    /// headless mode (an Antigravity conversation's terminal runs this).
    #[command(hide = true)]
    AgyChat {
        /// The `agy` CLI.
        #[arg(long)]
        agy: PathBuf,
        #[arg(long)]
        project: Option<String>,
        /// Model, effort, mode and documents, as JSON.
        #[arg(long, default_value = "{}")]
        options: String,
        /// The first message.
        #[arg(long, allow_hyphen_values = true)]
        prompt: Option<String>,
        /// The conversation to go on with.
        #[arg(long)]
        conversation: Option<String>,
    },
    /// Drive an unmodified ACP agent in Sandhill's chat.
    #[command(hide = true)]
    AcpChat {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        program: PathBuf,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "{}")]
        options: String,
        #[arg(long, allow_hyphen_values = true)]
        prompt: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Drive pi's conversation in Sandhill's chat, through `pi --mode rpc`.
    #[command(hide = true)]
    PiChat {
        /// The `pi` CLI.
        #[arg(long)]
        program: PathBuf,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "{}")]
        options: String,
        #[arg(long, allow_hyphen_values = true)]
        prompt: Option<String>,
        /// The session to go on with.
        #[arg(long)]
        session: Option<String>,
    },
    /// Report an agent's state to the terminal it runs in, from its hooks:
    /// `state busy|idle|waiting`, `prompt`, `notification` and `status`
    /// (Claude Code, which sends JSON on stdin), `codex` (Codex's notify
    /// program).
    #[command(hide = true)]
    TerminalEvent {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    // Run by git or ssh as their askpass program, with the prompt as the
    // only argument.
    if let Some(token) = std::env::var_os(sandhill_daemon::git::ASKPASS_TOKEN) {
        return askpass(token);
    }
    let cli = Cli::parse();
    // The daemon serves many clients at once; the helpers it and the agents
    // start (one per terminal, one per hook call) need a single thread.
    let runtime = match cli.command {
        Command::Serve { .. } | Command::Attach { .. } => {
            tokio::runtime::Builder::new_multi_thread()
        }
        _ => tokio::runtime::Builder::new_current_thread(),
    }
    .enable_all()
    .build()?;
    runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    let state = cli
        .state_dir
        .clone()
        .map(StateDir::new)
        .unwrap_or_else(StateDir::default_location);
    let socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
    match cli.command {
        Command::Serve { ref started_by } => {
            let started_by = started_by.clone().unwrap_or_else(service_manager);
            serve(state, socket, started_by).await
        }
        Command::Attach {
            start,
            confirm_exit,
        } => attach(&cli, state, socket, start, confirm_exit).await,
        Command::Mcp { ref project } => {
            // Antigravity starts the server as its settings say, the same
            // for every conversation: what differs comes from the
            // environment Sandhill gives agy.
            let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
            let socket = match (&cli.socket, env("SANDHILL_SOCKET")) {
                (None, Some(socket)) => PathBuf::from(socket),
                _ => socket,
            };
            let project = project
                .clone()
                .or_else(|| env("SANDHILL_PROJECT").map(|p| p.to_string_lossy().into_owned()));
            sandhill_daemon::mcp::serve(socket, project).await
        }
        Command::TerminalHost {
            dir,
            cwd,
            cols,
            rows,
            program,
        } => {
            sandhill_daemon::terminal::host::run(HostOptions {
                dir,
                cwd,
                cols,
                rows,
                program,
            })
            .await
        }
        Command::CodexChat {
            codex,
            project,
            options,
            prompt,
            thread,
            args,
        } => {
            let options: sandhill_proto::AgentOptions =
                serde_json::from_str(&options).context("--options is not valid")?;
            let instructions = sandhill_daemon::agents::instructions(&options);
            let code =
                sandhill_daemon::terminal::codex::run(sandhill_daemon::terminal::codex::Options {
                    codex,
                    args,
                    daemon: socket,
                    project,
                    options,
                    prompt,
                    thread,
                    instructions,
                })
                .await?;
            std::process::exit(code)
        }
        Command::AgyChat {
            agy,
            project,
            options,
            prompt,
            conversation,
        } => {
            let options: sandhill_proto::AgentOptions =
                serde_json::from_str(&options).context("--options is not valid")?;
            let instructions = sandhill_daemon::agents::instructions(&options);
            let code = sandhill_daemon::terminal::antigravity::run(
                sandhill_daemon::terminal::antigravity::Options {
                    agy,
                    executable: std::env::current_exe()?,
                    daemon: socket,
                    project,
                    options,
                    prompt,
                    conversation,
                    instructions,
                },
            )
            .await?;
            std::process::exit(code)
        }
        Command::AcpChat {
            agent,
            program,
            project,
            options,
            prompt,
            session,
        } => {
            let options = serde_json::from_str(&options).context("--options is not valid")?;
            let code =
                sandhill_daemon::terminal::acp::run(sandhill_daemon::terminal::acp::Options {
                    agent,
                    program,
                    project,
                    options,
                    prompt,
                    session,
                    executable: std::env::current_exe()?,
                    daemon: socket,
                })
                .await?;
            std::process::exit(code)
        }
        Command::PiChat {
            program,
            project,
            options,
            prompt,
            session,
        } => {
            let options = serde_json::from_str(&options).context("--options is not valid")?;
            let code =
                sandhill_daemon::terminal::pi::run(sandhill_daemon::terminal::acp::Options {
                    agent: sandhill_daemon::agents::PI.into(),
                    program,
                    project,
                    options,
                    prompt,
                    session,
                    executable: std::env::current_exe()?,
                    daemon: socket,
                })
                .await?;
            std::process::exit(code)
        }
        Command::TerminalEvent { args } => {
            // A hook must never get in the agent's way: report what can be
            // reported and always succeed.
            if let Err(err) = sandhill_daemon::terminal::events::run(&args, &socket).await {
                eprintln!("sandhilld terminal-event: {err:#}");
            }
            Ok(())
        }
    }
}

/// `systemd` or `launchd` when one of them started the daemon, else empty.
fn service_manager() -> String {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    if !var("INVOCATION_ID").is_empty() {
        return "systemd".into();
    }
    // launchd names its jobs here; apps and shells get "application.…" or "0".
    let job = var("XPC_SERVICE_NAME");
    if !job.is_empty() && job != "0" && !job.starts_with("application.") {
        return "launchd".into();
    }
    String::new()
}

async fn serve(state: StateDir, socket: PathBuf, started_by: String) -> Result<()> {
    require_installed(&state)?;
    sandhill_daemon::memory::configure();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        // No colour codes in a log file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    state
        .create()
        .context("cannot create the state directory")?;
    // Serialize socket ownership with concurrent app attaches and service
    // restarts. Never unlink a socket owned by a daemon still starting.
    let owner = try_lock(&state.root().join("daemon.lock"))?
        .context("another sandhilld owns this state directory")?;
    // Keep ownership through runtime teardown, including any still-finishing
    // blocking worker. Only process exit releases this lock to a service.
    static OWNER: std::sync::OnceLock<File> = std::sync::OnceLock::new();
    OWNER
        .set(owner)
        .map_err(|_| anyhow::anyhow!("daemon ownership was already established"))?;
    // Identify this build now, before an update can replace the executable.
    build_id();
    if socket.exists() {
        if UnixStream::connect(&socket).await.is_ok() {
            bail!("sandhilld is already running on {}", socket.display());
        }
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    }
    let options = DaemonOptions {
        // Agents' MCP servers connect to it from their own folders.
        socket: std::path::absolute(&socket).unwrap_or_else(|_| socket.clone()),
        started_by,
        ..DaemonOptions::for_state(&state)
    };
    let daemon = Daemon::open_with(state, options).await?;
    tracing::info!("sandhilld listening on {}", socket.display());

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = daemon.clone().serve(listener) => result?,
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    // Kernels run in their own process groups and keep going; the next
    // daemon reattaches to them.
    tracing::info!("sandhilld stopping; kernels keep running");
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

async fn attach(
    cli: &Cli,
    state: StateDir,
    socket: PathBuf,
    start: bool,
    confirm_exit: bool,
) -> Result<()> {
    let stream = if start {
        require_installed(&state)?;
        state.create()?;
        let _startup = startup_lock(&state).await?;
        let mut connected = None;
        for _ in 0..3 {
            require_installed(&state)?;
            if prepare_daemon(&socket).await? {
                connected = UnixStream::connect(&socket).await.ok();
                if connected.is_some() {
                    break;
                }
            } else {
                // An exiting daemon may have removed its socket while runtime
                // teardown still holds the lifetime lock.
                start_or_wait(cli, &state, &socket).await?;
            }
        }
        connected.context("sandhilld did not become ready; see its log in the state directory")?
    } else {
        UnixStream::connect(&socket)
            .await
            .with_context(|| format!("sandhilld is not running on {}", socket.display()))?
    };
    let peer = peer_identity(&stream)?;
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let daemon_closed = tokio::select! {
        result = tokio::io::copy(&mut stdin, &mut to_daemon) => (false, result),
        result = tokio::io::copy(&mut from_daemon, &mut stdout) => (true, result),
    };
    // Exit right away rather than returning: stdin is read on a thread that
    // can't be cancelled, and shutting the runtime down would wait for the
    // client's next line. The client would never learn the daemon is gone.
    let _ = stdout.flush().await;
    if let Err(error) = daemon_closed.1 {
        eprintln!("attach bridge failed: {error:#}");
        std::process::exit(1);
    }
    if confirm_exit
        && daemon_closed.0
        && let Err(error) = wait_for_exit(&peer).await
    {
        eprintln!("daemon exit was not confirmed: {error:#}");
        std::process::exit(1);
    }
    if confirm_exit && daemon_closed.0 && UnixStream::connect(&socket).await.is_ok() {
        eprintln!(
            "daemon process exited, but another daemon is already listening; stop was not confirmed"
        );
        std::process::exit(1);
    }
    std::process::exit(0)
}

/// A successful connection is not necessarily usable: StopResources latches
/// maintenance. Ask under the daemon's lifecycle gate before reusing/replacing
/// it, then wait for the exact old process to exit before starting another.
async fn prepare_daemon(socket: &Path) -> Result<bool> {
    let stream = match UnixStream::connect(socket).await {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error).context("cannot inspect the running sandhilld"),
    };
    let peer = peer_identity(&stream)?;
    let (read, write) = stream.into_split();
    let client = Client::new(read, write);
    let running = tokio::time::timeout(Duration::from_secs(10), client.ping())
        .await
        .context("the running sandhilld did not answer")??;
    if running.lifecycle == "stopping" {
        wait_for_exit(&peer).await?;
        return Ok(false);
    }
    let ours = build_id();
    if running.lifecycle.is_empty() {
        // A legacy StopDaemon takes its lifecycle read guard, so it waits for
        // already-running destructive maintenance. Restart only the daemon:
        // retained resources and inventory errors do not authorize killing work.
        // Never signal it or clear its maintenance latch.
        let probe = tokio::time::timeout(
            Duration::from_secs(10),
            client.call::<serde_json::Value>(Call::ListRecentProjects),
        )
        .await
        .context("cannot establish legacy daemon readiness")?;
        let stopped = match probe {
            Ok(_) => false,
            Err(sandhill_proto::client::ClientError::Remote(message))
                if message
                    == "resources are stopped for maintenance; reconnect after an explicit daemon restart" =>
            {
                true
            }
            Err(error) => return Err(error).context("legacy daemon is not accepting work"),
        };
        if !stopped && (ours.is_empty() || running.build == ours) {
            return Ok(true);
        }
        let _: serde_json::Value =
            tokio::time::timeout(Duration::from_secs(10), client.call(Call::StopDaemon))
                .await
                .context("legacy daemon did not acknowledge shutdown")??;
    } else {
        let reply = tokio::time::timeout(
            Duration::from_secs(15),
            client.call::<serde_json::Value>(Call::PrepareAttach {
                build: ours.to_string(),
            }),
        )
        .await
        .context("daemon lifecycle is busy; reopening did not interrupt maintenance")?;
        match reply {
            Ok(reply) => match reply["restart"].as_bool() {
                Some(false) => return Ok(true),
                Some(true) => {}
                _ => bail!("daemon did not authorize a usable connection or restart"),
            },
            // Builds before 2026-11-11 refused to reopen after an incomplete
            // Stop All, leaving the device unreachable. PrepareAttach already
            // waited for any active maintenance, so replace only the daemon:
            // StopDaemon leaves the remaining resources running.
            Err(sandhill_proto::client::ClientError::Remote(message))
                if message.starts_with("resource shutdown is incomplete") =>
            {
                let _: serde_json::Value =
                    tokio::time::timeout(Duration::from_secs(10), client.call(Call::StopDaemon))
                        .await
                        .context("stopped daemon did not acknowledge shutdown")??;
            }
            Err(error) => return Err(error.into()),
        }
    }
    tracing::info!(pid = peer.pid, "waiting for replaced daemon to exit");
    wait_for_exit(&peer).await?;
    Ok(false)
}

fn peer_identity(stream: &UnixStream) -> Result<ProcessIdentity> {
    let peer = stream.peer_cred()?;
    if peer.uid() != unsafe { libc::getuid() } {
        bail!("sandhilld runs as another user");
    }
    let pid = u32::try_from(peer.pid().context("cannot identify sandhilld's process")?)?;
    ProcessIdentity::read(pid).context("cannot establish sandhilld's birth identity")
}

async fn wait_for_exit(peer: &ProcessIdentity) -> Result<()> {
    for _ in 0..150 {
        if !peer.alive() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "sandhilld process {} did not exit; no replacement was started",
        peer.pid
    )
}

/// Lock files are never unlinked: waiters must keep referring to the same inode.
fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error.into())
    }
}

async fn startup_lock(state: &StateDir) -> Result<File> {
    for _ in 0..300 {
        if let Some(lock) = try_lock(&state.root().join("attach.lock"))? {
            return Ok(lock);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("another connection is starting sandhilld; try reconnecting after it finishes")
}

async fn start_or_wait(cli: &Cli, state: &StateDir, socket: &Path) -> Result<()> {
    for _ in 0..150 {
        require_installed(state)?;
        if UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
        if let Some(owner) = try_lock(&state.root().join("daemon.lock"))? {
            drop(owner);
            // A service can still win after this observation. Its serve and
            // ours share the lifetime lock, so only one can own the socket.
            start_detached(cli, state)?;
            drop(wait_for_socket(socket).await?);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "sandhilld still owns its state directory without a usable socket; no replacement was started"
    )
}

/// An uninstall tombstone prevents stale clients/services from recreating data.
fn require_installed(state: &StateDir) -> Result<()> {
    let marker = state.root().join("uninstalled");
    if marker.try_exists()? {
        bail!(
            "Sandhill was uninstalled for this state directory. Automatic startup is disabled. \
             To explicitly reinstall, review and remove {} before connecting again.",
            marker.display()
        );
    }
    Ok(())
}

/// Starts `sandhilld serve` in its own session, logging to the state dir.
fn start_detached(cli: &Cli, state: &StateDir) -> Result<()> {
    use std::os::unix::process::CommandExt;
    require_installed(state)?;
    state.create()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.log_path())?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("--state-dir").arg(state.root());
    if let Some(socket) = &cli.socket {
        command.arg("--socket").arg(socket);
    }
    command
        .args(["serve", "--started-by", "app"])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("cannot start sandhilld")?;
    Ok(())
}

async fn wait_for_socket(socket: &Path) -> Result<UnixStream> {
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(socket).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("sandhilld did not start; see its log in the state directory")
}

/// Asks the daemon that ran git for what the user types for the prompt, and
/// prints it for git. Failing (the user cancelled) makes git give up.
fn askpass(token: std::ffi::OsString) -> Result<()> {
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let socket =
        std::env::var_os(sandhill_daemon::git::ASKPASS_SOCKET).context("no daemon to ask")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let answer: String = runtime.block_on(async {
        let client = Client::connect_unix(Path::new(&socket)).await?;
        client
            .call(Call::GitCredential {
                token: token.to_string_lossy().into_owned(),
                prompt,
            })
            .await
    })?;
    println!("{answer}");
    Ok(())
}
