use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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
    /// State directory (default: $SANDHILL_STATE_DIR or ~/.sandhill/state).
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
    /// Read an AnnData preview in a separate, memory-limited process.
    #[command(hide = true)]
    DescribeH5ad {
        #[arg(last = true)]
        path: PathBuf,
    },
    /// Apply a confirmed installation plan in a job that can outlive the daemon.
    #[command(hide = true)]
    InstallWorker { record: PathBuf, job: String },
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
    let home = if cli.state_dir.is_none() && std::env::var_os("SANDHILL_STATE_DIR").is_none() {
        std::env::var_os("HOME").map(PathBuf::from)
    } else {
        None
    };
    let default = cli
        .state_dir
        .clone()
        .map(StateDir::new)
        .unwrap_or_else(StateDir::default_location);
    let state = if home.is_some() {
        default.clone().existing_location()
    } else {
        default.clone()
    };
    let socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
    match cli.command {
        Command::DescribeH5ad { path } => sandhill_daemon::h5ad::worker(&path),
        Command::InstallWorker { record, job } => {
            sandhill_daemon::quick::install::worker(&record, &job).await
        }
        Command::Serve { ref started_by } => {
            let started_by = started_by.clone().unwrap_or_else(service_manager);
            let (_startup, state) = if let Some(home) = home.as_deref() {
                let (lock, can_move) = lock_startup(&default, Some(home)).await?;
                let state = if can_move {
                    move_storage(home, &default)?
                } else {
                    default.existing_location()
                };
                (Some(lock), state)
            } else {
                (None, state)
            };
            let socket = cli.socket.unwrap_or_else(|| state.socket_path());
            serve(state, socket, started_by, _startup).await
        }
        Command::Attach {
            start,
            confirm_exit,
        } => attach(&cli, default, home.as_deref(), start, confirm_exit).await,
        Command::Mcp { ref project } => {
            // Antigravity starts the server as its settings say, the same
            // for every conversation: what differs comes from the
            // environment Sandhill gives agy.
            let env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
            if env("SANDHILL_AGY_CHAT").is_some()
                && !sandhill_daemon::lifecycle::antigravity_tools_allowed()
            {
                bail!("Antigravity hasn't been allowed to use Sandhill's tools.");
            }
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
            let instructions =
                sandhill_daemon::agents::instructions_for(&options, project.as_deref());
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

async fn serve(
    state: StateDir,
    socket: PathBuf,
    started_by: String,
    startup: Option<File>,
) -> Result<()> {
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
    drop(startup);
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
    default: StateDir,
    home: Option<&Path>,
    start: bool,
    confirm_exit: bool,
) -> Result<()> {
    let account_home = std::env::var_os("HOME").map(PathBuf::from);
    if start && let Some(home) = &account_home {
        sandhill_daemon::lifecycle::wait_for_removal(home).await?;
    }
    let mut state = if home.is_some() {
        default.clone().existing_location()
    } else {
        default.clone()
    };
    let mut socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
    let stream = if start {
        let (_startup, can_move) = lock_startup(&default, home).await?;
        if home.is_some() {
            state = default.clone().existing_location();
            socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
        }
        let mut connected = None;
        for _ in 0..3 {
            if prepare_daemon(&socket).await? {
                connected = UnixStream::connect(&socket).await.ok();
                if connected.is_some() {
                    break;
                }
            } else {
                // An exiting daemon may have removed its socket while runtime
                // teardown still holds the lifetime lock.
                if wait_for_owner(&state, &socket).await? {
                    if let Some(home) = &account_home {
                        sandhill_daemon::lifecycle::wait_for_removal(home).await?;
                    }
                    if let Some(home) = home.filter(|_| can_move) {
                        state = move_storage(home, &default)?;
                        socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
                    }
                    start_detached(cli, &state)?;
                    drop(wait_for_socket(&socket).await?);
                }
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
        // Reset: a daemon closing its socket as it exits, seen on Linux.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error).context("cannot inspect the running sandhilld"),
    };
    let peer = peer_identity(&stream)?;
    let (read, write) = stream.into_split();
    let client = Client::new(read, write);
    match ask_daemon(&client, &peer).await {
        // The daemon went away while being asked: an older one finishing its
        // own exit. Like one found stopping, wait for that exact process to
        // be gone; the caller then reuses or starts one under its lock.
        Err(error) if went_away(&error) => {
            wait_for_exit(&peer).await?;
            Ok(false)
        }
        asked => asked,
    }
}

/// Whether `error` is the connection to the daemon breaking, rather than
/// the daemon answering.
fn went_away(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<sandhill_proto::client::ClientError>(),
            Some(
                sandhill_proto::client::ClientError::Io(_)
                    | sandhill_proto::client::ClientError::Disconnected
            )
        )
    })
}

async fn ask_daemon(client: &Client, peer: &ProcessIdentity) -> Result<bool> {
    let running = tokio::time::timeout(Duration::from_secs(10), client.ping())
        .await
        .context("the running sandhilld did not answer")??;
    if running.lifecycle == "stopping" {
        wait_for_exit(peer).await?;
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
            Err(sandhill_proto::client::ClientError::Remote(message))
                if message == "daemon shutdown is in progress; reconnect after it exits" =>
            {
                // Uninstall acquired the gate after the ping. Wait for
                // this daemon, then check its finisher before starting one.
            }
            Err(error) => return Err(error.into()),
        }
    }
    tracing::info!(pid = peer.pid, "waiting for replaced daemon to exit");
    wait_for_exit(peer).await?;
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
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .read(true)
        .write(true)
        .open(path)?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.nlink() == 1,
        "The daemon lock is not a regular file"
    );
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

async fn startup_lock(path: &Path) -> Result<File> {
    for _ in 0..300 {
        if let Some(lock) = try_lock(path)? {
            return Ok(lock);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("another connection is starting sandhilld; try reconnecting after it finishes")
}

/// Use a lock outside the movable state directory. An explicit state folder
/// keeps its own lock. If the new home folder can't be prepared, retain the
/// old state and its lock rather than stranding the device.
async fn lock_startup(default: &StateDir, home: Option<&Path>) -> Result<(File, bool)> {
    if let Some(home) = home {
        match StateDir::prepare_home(home).map(|dir| dir.join("attach.lock")) {
            Ok(path) => match startup_lock(&path).await {
                Ok(lock) => return Ok((lock, true)),
                Err(err) if err.downcast_ref::<std::io::Error>().is_none() => return Err(err),
                Err(err) => log_move(
                    &default.clone().existing_location(),
                    &[format!(
                        "Sandhill could not prepare the new folder: {err:#}"
                    )],
                ),
            },
            Err(err) => log_move(
                &default.clone().existing_location(),
                &[format!("Sandhill could not prepare the new folder: {err}")],
            ),
        }
    }
    let state = if home.is_some() {
        default.clone().existing_location()
    } else {
        default.clone()
    };
    state.create()?;
    Ok((
        startup_lock(&state.root().join("attach.lock")).await?,
        false,
    ))
}

fn move_storage(home: &Path, default: &StateDir) -> Result<StateDir> {
    let state = default.clone().existing_location();
    let guard = (|| -> Result<Option<Vec<File>>> {
        let mut roots = vec![state.clone()];
        if let Some(old) = StateDir::legacy_location()
            && old.root() != state.root()
            && old.root().exists()
        {
            roots.push(old);
        }
        let mut locks = Vec::with_capacity(roots.len() + 2);
        for root in roots {
            if !root.root().exists() {
                continue;
            }
            let mut dir = root.root();
            while dir != home {
                anyhow::ensure!(
                    dir.symlink_metadata()?.is_dir(),
                    "{} is not a regular folder",
                    dir.display()
                );
                dir = dir
                    .parent()
                    .context("the state folder is not inside the home folder")?;
            }
            let Some(lock) = try_lock(&root.root().join("daemon.lock"))? else {
                return Ok(None);
            };
            locks.push(lock);
            if root.root() == default.root() {
                continue;
            }
            for name in ["attach.lock", ".install-record.lock"] {
                let path = root.root().join(name);
                if name == ".install-record.lock" && !path.exists() {
                    continue;
                }
                let Some(lock) = try_lock(&path)? else {
                    return Ok(None);
                };
                locks.push(lock);
            }
        }
        Ok(Some(locks))
    })();
    let notes = match guard {
        Ok(Some(_locks)) => StateDir::migrate_home(home),
        Ok(None) => vec!["Sandhill kept its old files because the old daemon or an install job is still using them.".into()],
        Err(err) => vec![format!("Sandhill could not check its old folder: {err:#}")],
    };
    let state = default.clone().existing_location();
    if !notes.is_empty() && !state.root().exists() && state.root() == default.root() {
        state.create()?;
    }
    log_move(&state, &notes);
    Ok(state)
}

fn log_move(state: &StateDir, notes: &[String]) {
    if notes.is_empty() {
        return;
    }
    use std::io::Write as _;
    let log = open_daemon_log(state);
    for note in notes {
        if !matches!(&log, Ok(file) if writeln!(&*file, "{note}").is_ok()) {
            eprintln!("{note}");
        }
    }
}

fn open_daemon_log(state: &StateDir) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state.log_path())?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.nlink() == 1,
        "The daemon log is not a regular file"
    );
    Ok(file)
}

/// An exiting daemon may hold the lifetime lock after its socket closes.
/// Do not move anything until that process is gone.
async fn wait_for_owner(state: &StateDir, socket: &Path) -> Result<bool> {
    for _ in 0..150 {
        if UnixStream::connect(socket).await.is_ok() {
            return Ok(false);
        }
        if !state.root().exists() || try_lock(&state.root().join("daemon.lock"))?.is_some() {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!(
        "sandhilld still owns its state directory without a usable socket; no replacement was started"
    )
}

/// Starts `sandhilld serve` in its own session, logging to the state dir.
fn start_detached(cli: &Cli, state: &StateDir) -> Result<()> {
    use std::os::unix::process::CommandExt;
    state.create()?;
    let log = open_daemon_log(state)?;
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
