//! Exclusive, foreground handoff of a bridge's controlling PTY to a native CLI.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{BufReader, Lines};
use tokio::net::unix::OwnedReadHalf;
use tokio::process::Command;

use super::super::{FromHost, ToHost, report, tell};

/// The caller must release its headless agent before entering and resume it
/// only after this returns. Non-view commands wait until that resume completes.
pub(crate) async fn run(
    command: &mut Command,
    dir: &Path,
    commands: &mut Lines<BufReader<OwnedReadHalf>>,
) -> Result<Vec<Value>> {
    // Let the CLI cancel an active turn before its normal quit command.
    run_quitting(command, dir, commands, &["\x03", "/exit\r"]).await
}

/// [`run`] for a CLI that quits with other keys: `quit` is typed into its
/// terminal, a moment apart, when the user goes back to the chat.
pub(crate) async fn run_quitting(
    command: &mut Command,
    dir: &Path,
    commands: &mut Lines<BufReader<OwnedReadHalf>>,
    quit: &[&str],
) -> Result<Vec<Value>> {
    let terminal = Terminal::capture()?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    // SAFETY: only async-signal-safe operations between fork and exec. The
    // child takes the foreground before it can read, avoiding a SIGTTIN race.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            if libc::tcsetpgrp(0, libc::getpgrp()) < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .context("cannot start the native agent interface")?;
    let pid = child.id().context("native agent has no process id")?;
    let mut queued = Vec::new();
    let mut deadline = None;
    let mut stopping = false;
    let mut terminated = false;
    let mut disconnected = false;
    let mut reaped = false;
    let mut native_failure = None;
    let result: Result<()> = async {
    report(dir, serde_json::json!({"view_mode": "terminal"})).await?;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status.context("cannot wait for the native agent interface")?;
                reaped = true;
                if !status.success() && !stopping && !terminated && !disconnected {
                    native_failure = Some(status);
                }
                break;
            }
            line = commands.next_line(), if !disconnected => {
                match line? {
                    Some(line) => {
                        if let Ok(FromHost::Command { command }) = serde_json::from_str(&line) {
                            if command["kind"] == "view" {
                                if command["mode"] == "chat" && !stopping {
                                    stopping = true;
                                    report(dir, serde_json::json!({"view_mode": "switching_chat"})).await?;
                                    // Write through the master, never to the slave's output.
                                    for (index, keys) in quit.iter().enumerate() {
                                        if index > 0 {
                                            tokio::time::sleep(Duration::from_millis(150)).await;
                                        }
                                        input(dir, keys).await?;
                                    }
                                    deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                                }
                            } else {
                                queued.push(command);
                            }
                        }
                    }
                    None => {
                        disconnected = true;
                        terminated = true;
                        signal(pid, libc::SIGTERM);
                        deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                    }
                }
            }
            _ = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                signal(pid, if terminated { libc::SIGKILL } else { libc::SIGTERM });
                if !terminated {
                    terminated = true;
                    deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                } else {
                    deadline = None;
                }
            }
        }
    }
    Ok(())
    }.await;
    if result.is_err() && !reaped {
        signal(pid, libc::SIGKILL);
        let _ = child.wait().await;
    }
    drop(terminal);
    if let Some(status) = native_failure {
        super::super::add_chat(
            dir,
            serde_json::json!({
                "kind": "error",
                "text": format!("Native agent interface exited ({status})."),
            }),
        )
        .await?;
    }
    result?;
    if disconnected {
        bail!("terminal host disconnected during native agent mode");
    }
    report(dir, serde_json::json!({"view_mode": "switching_chat"})).await?;
    Ok(queued)
}

async fn input(dir: &Path, data: &str) -> Result<()> {
    tell(
        dir,
        &ToHost::Input {
            data: data.to_owned(),
        },
    )
    .await
}

fn signal(pid: u32, signal: libc::c_int) {
    // SAFETY: the child is in its own process group and has not been reaped.
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

struct Terminal {
    group: libc::pid_t,
    settings: libc::termios,
}

impl Terminal {
    fn capture() -> Result<Self> {
        let mut settings = std::mem::MaybeUninit::uninit();
        // SAFETY: stdin is inherited from the bridge's controlling PTY;
        // settings is initialized only on tcgetattr success.
        unsafe {
            let group = libc::tcgetpgrp(0);
            if group < 0 || libc::tcgetattr(0, settings.as_mut_ptr()) < 0 {
                return Err(io::Error::last_os_error())
                    .context("native mode needs a controlling terminal");
            }
            Ok(Self {
                group,
                settings: settings.assume_init(),
            })
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // A background process group must block SIGTTOU while reclaiming its
        // terminal. The mask is thread-local and restored, not a global handler.
        // SAFETY: all pointers refer to initialized stack values of libc types.
        unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            let mut previous: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut mask);
            libc::sigaddset(&mut mask, libc::SIGTTOU);
            let blocked = libc::pthread_sigmask(libc::SIG_BLOCK, &mask, &mut previous) == 0;
            libc::tcsetpgrp(0, self.group);
            // Do not carry a queued quit command into the next native session.
            libc::tcflush(0, libc::TCIFLUSH);
            libc::tcsetattr(0, libc::TCSANOW, &self.settings);
            if blocked {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
            }
        }
    }
}
