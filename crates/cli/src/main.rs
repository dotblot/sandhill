//! `sandhill`: a terminal client for sandhilld. Handy for scripting and for
//! testing the daemon before the Mac app exists.

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, EVENT_RUN_FINISHED, EVENT_RUN_STARTED, EVENT_SESSION_ENDED, Event, RunFinished,
};
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "sandhill",
    version,
    about = "Talk to a local or remote sandhilld"
)]
struct Cli {
    /// Daemon socket (default: ~/.local/state/sandhill/sandhilld.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Reach a remote daemon with `ssh HOST sandhilld attach --start`.
    #[arg(long, global = true)]
    ssh: Option<String>,
    /// Path of sandhilld on the remote host.
    #[arg(long, global = true, default_value = "sandhilld")]
    remote_daemon: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check that the daemon answers.
    Ping,
    /// List installed Jupyter kernels.
    Kernels,
    /// Start a kernel session.
    Start {
        kernel: String,
        #[arg(long)]
        cwd: Option<String>,
    },
    /// List sessions.
    Ls,
    /// Run code and print its output. Reads stdin when CODE is `-`.
    Run {
        session: String,
        code: Option<String>,
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
    /// Follow a session's output, optionally replaying from a sequence number.
    Watch {
        session: String,
        #[arg(long)]
        since: Option<i64>,
    },
    /// Print everything journaled for a session and exit.
    History {
        session: String,
        #[arg(long, default_value_t = 0)]
        since: i64,
    },
    Interrupt {
        session: String,
    },
    /// Shut a kernel down.
    Stop {
        session: String,
    },
    /// Write a stored output (for example a plot) to stdout or a file.
    Blob {
        hash: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    if let Err(err) = run(Cli::parse()).await {
        eprintln!("sandhill: {err:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let (mut client, _ssh) = connect(&cli).await?;
    match cli.command {
        Command::Ping => {
            let reply = client.ping().await?;
            println!(
                "sandhilld {} (protocol {})",
                reply.daemon_version, reply.protocol_version
            );
        }
        Command::Kernels => {
            for spec in client.list_kernelspecs().await? {
                println!(
                    "{:<16} {:<10} {}",
                    spec.name, spec.language, spec.display_name
                );
            }
        }
        Command::Start { kernel, cwd } => {
            let info = client.start_session(&kernel, cwd.as_deref()).await?;
            println!("{}", info.id);
        }
        Command::Ls => {
            for s in client.list_sessions().await? {
                let project = s
                    .project
                    .map(|p| format!("  (project {p})"))
                    .unwrap_or_default();
                println!(
                    "{}  {:<10} {:<8} {}{project}",
                    &s.id[..12],
                    s.kernel,
                    format!("{:?}", s.status).to_lowercase(),
                    s.cwd
                );
            }
        }
        Command::Run {
            session,
            code,
            file,
        } => {
            let code = match (code.as_deref(), file) {
                (_, Some(path)) => std::fs::read_to_string(path)?,
                (Some("-"), None) => {
                    let mut text = String::new();
                    std::io::stdin().read_to_string(&mut text)?;
                    text
                }
                (Some(code), None) => code.to_string(),
                (None, None) => bail!("give CODE, `-` for stdin, or --file"),
            };
            let session = resolve(&client, &session).await?;
            let ok = run_code(&mut client, &session, &code).await?;
            if !ok {
                std::process::exit(1);
            }
        }
        Command::Watch { session, since } => {
            let session = resolve(&client, &session).await?;
            client.subscribe(&session, since).await?;
            while let Some(event) = client.next_event().await {
                print_event(&event, true);
            }
        }
        Command::History { session, since } => {
            let session = resolve(&client, &session).await?;
            let mut since = since;
            loop {
                let page = client.history(&session, since, None).await?;
                for event in &page.events {
                    print_event(event, true);
                }
                if page.next_seq == since {
                    break;
                }
                since = page.next_seq;
            }
        }
        Command::Interrupt { session } => {
            let session = resolve(&client, &session).await?;
            client.interrupt(&session).await?;
        }
        Command::Stop { session } => {
            let session = resolve(&client, &session).await?;
            client.shutdown(&session).await?;
        }
        Command::Blob { hash, out } => {
            let blob = client.get_blob(&hash).await?;
            match out {
                Some(path) => std::fs::write(path, blob.data)?,
                None => std::io::stdout().write_all(blob.data.as_bytes())?,
            }
        }
    }
    Ok(())
}

async fn connect(cli: &Cli) -> Result<(Client, Option<tokio::process::Child>)> {
    if let Some(host) = &cli.ssh {
        let mut command = tokio::process::Command::new("ssh");
        command
            .arg("-T")
            .arg(host)
            .arg(&cli.remote_daemon)
            .arg("attach")
            .arg("--start");
        let (client, child) = Client::connect_command(command).await?;
        return Ok((client, Some(child)));
    }
    let socket = match &cli.socket {
        Some(path) => path.clone(),
        None => default_socket(),
    };
    let client = Client::connect_unix(&socket)
        .await
        .with_context(|| format!("cannot reach sandhilld at {}", socket.display()))?;
    Ok((client, None))
}

fn default_socket() -> PathBuf {
    let state = std::env::var_os("SANDHILL_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|d| PathBuf::from(d).join("sandhill")))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state/sandhill"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/sandhill"));
    state.join("sandhilld.sock")
}

/// Accepts a full session id or a unique prefix of one.
async fn resolve(client: &Client, prefix: &str) -> Result<String> {
    let matches: Vec<String> = client
        .list_sessions()
        .await?
        .into_iter()
        .map(|s| s.id)
        .filter(|id| id.starts_with(prefix))
        .collect();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!("no session starts with {prefix:?}"),
        _ => bail!("{prefix:?} matches more than one session"),
    }
}

/// Runs code and prints its output; returns false if it raised an error.
async fn run_code(client: &mut Client, session: &str, code: &str) -> Result<bool> {
    client.subscribe(session, None).await?;
    let run = client.execute(session, code, None).await?.run;
    while let Some(event) = client.next_event().await {
        if event.run.as_deref() != Some(run.as_str()) {
            if event.msg_type == EVENT_SESSION_ENDED {
                bail!("the session ended");
            }
            continue;
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            let finished: RunFinished = serde_json::from_value(event.content)?;
            return Ok(finished.status == "ok");
        }
        print_event(&event, false);
    }
    bail!("lost the connection to sandhilld")
}

fn print_event(event: &Event, verbose: bool) {
    let c = &event.content;
    match event.msg_type.as_str() {
        "stream" => {
            let text = c["text"].as_str().unwrap_or_default();
            if c["name"] == "stderr" {
                eprint!("{text}");
            } else {
                print!("{text}");
            }
        }
        "execute_result" | "display_data" | "update_display_data" => {
            println!("{}", render_bundle(&c["data"]));
        }
        "error" => {
            for line in c["traceback"].as_array().into_iter().flatten() {
                eprintln!("{}", line.as_str().unwrap_or_default());
            }
            if c["traceback"].as_array().is_none_or(|t| t.is_empty()) {
                eprintln!(
                    "{}: {}",
                    c["ename"].as_str().unwrap_or("Error"),
                    c["evalue"]
                );
            }
        }
        EVENT_RUN_STARTED if verbose => {
            println!(
                "\x1b[2m── #{} run {}\x1b[0m",
                event.seq,
                short(event.run.as_deref())
            );
            if let Some(code) = c["code"].as_str() {
                for line in code.lines() {
                    println!("\x1b[2m│ {line}\x1b[0m");
                }
            }
        }
        EVENT_RUN_FINISHED if verbose => {
            println!(
                "\x1b[2m── finished: {}\x1b[0m",
                c["status"].as_str().unwrap_or("?")
            );
        }
        EVENT_SESSION_ENDED => {
            println!("── session ended: {}", c["reason"].as_str().unwrap_or("?"));
        }
        _ => {}
    }
    let _ = std::io::stdout().flush();
}

/// Text for a MIME bundle: `text/plain` when present, otherwise a placeholder
/// naming each MIME type (and blob hash for stored outputs).
fn render_bundle(data: &Value) -> String {
    if let Some(text) = data.get("text/plain").and_then(Value::as_str) {
        return text.to_string();
    }
    let Some(map) = data.as_object() else {
        return String::new();
    };
    map.iter()
        .map(
            |(mime, value)| match value.get(BLOB_KEY).and_then(Value::as_str) {
                Some(hash) => format!(
                    "[{mime}, {} bytes, blob {}]",
                    value["size"],
                    &hash[..12.min(hash.len())]
                ),
                None => format!("[{mime}]"),
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

fn short(id: Option<&str>) -> &str {
    id.map(|id| &id[..8.min(id.len())]).unwrap_or("-")
}
