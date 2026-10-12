use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sandhill_daemon::kernel::{pid_alive, send_signal};
use sandhill_daemon::terminal::host::HostOptions;
use sandhill_daemon::{Daemon, DaemonOptions, StateDir, build_id};
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
    Serve,
    /// Bridge stdin/stdout to the daemon's socket. The Mac app runs
    /// `ssh host sandhilld attach --start` to reach a remote daemon.
    Attach {
        /// Start the daemon in the background if it isn't running.
        #[arg(long)]
        start: bool,
    },
    /// Serve Sandhill's tools to a coding agent over MCP on stdin/stdout.
    /// Claude Code and Codex start this themselves.
    Mcp {
        /// The project folder whose sessions the tools work with.
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
    /// Report an agent's state to the terminal it runs in, from its hooks:
    /// `state busy|idle|waiting`, `notification` and `status` (Claude Code,
    /// which sends JSON on stdin), `codex` (Codex's notify program).
    #[command(hide = true)]
    TerminalEvent {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The daemon serves many clients at once; the helpers it and the agents
    // start (one per terminal, one per hook call) need a single thread.
    let runtime = match cli.command {
        Command::Serve | Command::Attach { .. } => tokio::runtime::Builder::new_multi_thread(),
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
        Command::Serve => serve(state, socket).await,
        Command::Attach { start } => attach(&cli, state, socket, start).await,
        Command::Mcp { ref project } => sandhill_daemon::mcp::serve(socket, project.clone()).await,
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
        Command::TerminalEvent { args } => {
            // A hook must never get in the agent's way: report what can be
            // reported and always succeed.
            if let Err(err) = sandhill_daemon::terminal::events::run(&args).await {
                eprintln!("sandhilld terminal-event: {err:#}");
            }
            Ok(())
        }
    }
}

async fn serve(state: StateDir, socket: PathBuf) -> Result<()> {
    sandhill_daemon::memory::configure();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    state
        .create()
        .context("cannot create the state directory")?;
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

async fn attach(cli: &Cli, state: StateDir, socket: PathBuf, start: bool) -> Result<()> {
    if start && let Err(err) = stop_other_build(&socket).await {
        eprintln!("keeping the running sandhilld: {err:#}");
    }
    let stream = match UnixStream::connect(&socket).await {
        Ok(stream) => stream,
        Err(_) if start => {
            start_detached(cli, &state)?;
            wait_for_socket(&socket).await?
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("sandhilld is not running on {}", socket.display()));
        }
    };
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        _ = tokio::io::copy(&mut stdin, &mut to_daemon) => {}
        _ = tokio::io::copy(&mut from_daemon, &mut stdout) => {}
    }
    // Exit right away rather than returning: stdin is read on a thread that
    // can't be cancelled, and shutting the runtime down would wait for the
    // client's next line. The client would never learn the daemon is gone.
    let _ = stdout.flush().await;
    std::process::exit(0)
}

/// Stops the running daemon if it is another build than this one, so that
/// `--start` starts this build instead. That is how an updated app gets its
/// own daemon. Kernels keep running, and the new daemon reattaches to them.
async fn stop_other_build(socket: &Path) -> Result<()> {
    let Ok(stream) = UnixStream::connect(socket).await else {
        return Ok(());
    };
    let ours = build_id();
    if ours.is_empty() {
        return Ok(());
    }
    let peer = stream.peer_cred()?;
    let (read, write) = stream.into_split();
    let client = Client::new(read, write);
    let running = tokio::time::timeout(Duration::from_secs(10), client.ping())
        .await
        .context("it did not answer")??;
    if running.build == ours {
        return Ok(());
    }
    drop(client);
    let pid = peer.pid().context("cannot tell which process it is")?;
    // SAFETY: getuid has no preconditions.
    if peer.uid() != unsafe { libc::getuid() } {
        bail!("it runs as another user");
    }
    let pid = u32::try_from(pid)?;
    eprintln!(
        "stopping sandhilld {} (pid {pid}), which is another build than this one",
        running.daemon_version
    );
    send_signal(pid, libc::SIGTERM)?;
    // Wait until it is gone: the new daemon can only reconnect to kernels
    // once the old one has let go of them.
    for _ in 0..100 {
        if !pid_alive(pid) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    send_signal(pid, libc::SIGKILL)?;
    for _ in 0..20 {
        if !pid_alive(pid) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("pid {pid} did not stop")
}

/// Starts `sandhilld serve` in its own session, logging to the state dir.
fn start_detached(cli: &Cli, state: &StateDir) -> Result<()> {
    use std::os::unix::process::CommandExt;
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
        .arg("serve")
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
