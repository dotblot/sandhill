use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sandhill_daemon::{Daemon, StateDir};
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let state = cli
        .state_dir
        .clone()
        .map(StateDir::new)
        .unwrap_or_else(StateDir::default_location);
    let socket = cli.socket.clone().unwrap_or_else(|| state.socket_path());
    match cli.command {
        Command::Serve => serve(state, socket).await,
        Command::Attach { start } => attach(&cli, state, socket, start).await,
    }
}

async fn serve(state: StateDir, socket: PathBuf) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    state
        .create()
        .context("cannot create the state directory")?;
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
    let daemon = Daemon::open(state).await?;
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
    Ok(())
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

async fn wait_for_socket(socket: &std::path::Path) -> Result<UnixStream> {
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(socket).await {
            return Ok(stream);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("sandhilld did not start; see its log in the state directory")
}
