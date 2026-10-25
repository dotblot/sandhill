//! Starting Jupyter kernels and talking to them over ZeroMQ.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use jupyter_protocol::{ConnectionInfo, JupyterMessage, connection_info::Transport};
use jupyter_zmq_client::{
    DealerSendConnection, KernelspecDir, create_client_control_connection,
    create_client_heartbeat_connection, create_client_iopub_connection,
    create_client_shell_connection_with_identity, peer_identity_for_session,
};
use sandhill_proto::KernelspecInfo;
use tokio::process::Child;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::kernelspecs::{self, Search};

/// The installed kernels, and with `project`, those of the environments
/// inside its folder.
pub async fn list_kernelspecs(project: Option<PathBuf>) -> Vec<KernelspecInfo> {
    tokio::task::spawn_blocking(move || {
        kernelspecs::list(&Search::from_env().in_project(project.as_deref()))
    })
    .await
    .unwrap_or_default()
}

pub async fn find_kernelspec(name: &str, project: Option<&Path>) -> Result<KernelspecDir> {
    let wanted = name.to_string();
    let project = project.map(Path::to_path_buf);
    tokio::task::spawn_blocking(move || {
        kernelspecs::find(&Search::from_env().in_project(project.as_deref()), &wanted)
    })
    .await?
    .with_context(|| format!("no Jupyter kernel named {name:?} is installed"))
}

/// A freshly started kernel process.
pub struct Launched {
    pub child: Child,
    pub pid: u32,
    pub connection_info: ConnectionInfo,
    pub connection_file: PathBuf,
}

/// Starts a kernel in its own process group, so it keeps running if the
/// daemon exits or restarts. Its stdout/stderr go to `<session>.log`.
/// `env` (a project's env files) adds to the kernel's environment, but
/// doesn't change what the kernelspec sets, such as the `PATH` that
/// activates its environment.
pub async fn launch(
    mut spec: KernelspecDir,
    cwd: &Path,
    env: &[(String, String)],
    kernels_dir: &Path,
    session_id: &str,
) -> Result<Launched> {
    if !env.is_empty() {
        let vars = spec.kernelspec.env.get_or_insert_with(Default::default);
        for (key, value) in env {
            vars.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let (ports, listeners) = jupyter_zmq_client::peek_ports_with_listeners(ip, 5).await?;
    let connection_info = ConnectionInfo {
        transport: Transport::TCP,
        ip: ip.to_string(),
        stdin_port: ports[0],
        control_port: ports[1],
        hb_port: ports[2],
        shell_port: ports[3],
        iopub_port: ports[4],
        signature_scheme: "hmac-sha256".to_string(),
        key: uuid::Uuid::new_v4().to_string(),
        kernel_name: Some(spec.kernel_name.clone()),
    };

    let connection_file = kernels_dir.join(format!("{session_id}.json"));
    write_private(
        &connection_file,
        serde_json::to_string(&connection_info)?.as_bytes(),
    )?;

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(kernels_dir.join(format!("{session_id}.log")))?;
    let mut command = spec.command(
        &connection_file,
        Some(Stdio::from(log.try_clone()?)),
        Some(Stdio::from(log)),
    )?;
    command.current_dir(cwd).process_group(0);
    let child = command
        .spawn()
        .context("failed to start the kernel process")?;
    // The kernel binds these ports as soon as it starts; release them now.
    drop(listeners);
    let pid = child.id().context("kernel exited immediately")?;
    Ok(Launched {
        child,
        pid,
        connection_info,
        connection_file,
    })
}

pub fn read_connection_file(path: &Path) -> Result<ConnectionInfo> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read connection file {}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// A message from the kernel, tagged with the channel it arrived on.
#[derive(Debug)]
pub enum KernelMsg {
    IoPub(JupyterMessage),
    Shell(JupyterMessage),
    Control(JupyterMessage),
}

/// Sockets to one kernel. Incoming messages are forwarded to the channel
/// passed to [`connect`]; outgoing requests go through `shell` and `control`.
pub struct KernelConnection {
    pub shell: DealerSendConnection,
    pub control: DealerSendConnection,
    readers: Vec<JoinHandle<()>>,
}

impl Drop for KernelConnection {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
    }
}

/// Connects to a running kernel. `client_session` doubles as the ZeroMQ
/// identity of the shell socket: reconnecting with the same value after a
/// daemon restart lets replies to requests sent before the restart reach the
/// new daemon.
pub async fn connect(
    info: &ConnectionInfo,
    client_session: &str,
    tx: mpsc::Sender<KernelMsg>,
) -> Result<KernelConnection> {
    let mut iopub = create_client_iopub_connection(info, "", client_session).await?;
    let identity = peer_identity_for_session(client_session)?;
    let shell =
        create_client_shell_connection_with_identity(info, client_session, identity).await?;
    let control = create_client_control_connection(info, client_session).await?;
    let (shell_send, mut shell_recv) = shell.split();
    let (control_send, mut control_recv) = control.split();

    let iopub_tx = tx.clone();
    let shell_tx = tx.clone();
    let control_tx = tx;
    let readers = vec![
        tokio::spawn(async move {
            while let Ok(msg) = iopub.read().await {
                if iopub_tx.send(KernelMsg::IoPub(msg)).await.is_err() {
                    break;
                }
            }
        }),
        tokio::spawn(async move {
            while let Ok(msg) = shell_recv.read().await {
                if shell_tx.send(KernelMsg::Shell(msg)).await.is_err() {
                    break;
                }
            }
        }),
        tokio::spawn(async move {
            while let Ok(msg) = control_recv.read().await {
                if control_tx.send(KernelMsg::Control(msg)).await.is_err() {
                    break;
                }
            }
        }),
    ];
    Ok(KernelConnection {
        shell: shell_send,
        control: control_send,
        readers,
    })
}

/// True if the kernel answers on its heartbeat socket. This works even while
/// the kernel is busy running code.
pub async fn heartbeat(info: &ConnectionInfo, timeout: std::time::Duration) -> bool {
    let attempt = async {
        let mut hb = create_client_heartbeat_connection(info).await?;
        hb.single_heartbeat().await
    };
    matches!(tokio::time::timeout(timeout, attempt).await, Ok(Ok(())))
}

/// True if a process with this pid exists (and is not a zombie we own).
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill with signal 0 only checks for existence and permissions.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn send_signal(pid: u32, signal: libc::c_int) -> std::io::Result<()> {
    // SAFETY: plain syscall on a pid we started.
    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Kills the kernel's whole process group (the kernel and anything it spawned).
pub fn kill_process_group(pid: u32) {
    // SAFETY: the kernel was started with process_group(0), so its pgid is its pid.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}
