//! Explicit, ownership-checked resource and storage management. Ordinary daemon
//! shutdown still detaches: nothing in this module runs from Drop.
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::{Call, SessionStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::server::Daemon;
use crate::terminal::{Paths, ToHost};

mod components;
mod ownership;
mod storage;

use components::{
    AGY_CONFIG, antigravity_entry, antigravity_manifest, antigravity_owned,
    antigravity_registration_at, component_manifest, component_paths, legacy_components,
    remove_antigravity_mcp_at, uninstall_one,
};
use ownership::{process, processes, refresh};
use storage::{SafeDir, path_exists, read_safe, remove_safe, storage_entry};

pub fn allowed_during_maintenance(call: &Call) -> bool {
    matches!(
        call,
        Call::Ping
            | Call::ResourceInventory
            | Call::StopResources { .. }
            | Call::StorageInventory
            | Call::CleanStorage { .. }
            | Call::UninstallComponents { .. }
            | Call::StopDaemon
            | Call::PrepareAttach { .. }
            | Call::ListSessions
            | Call::ReadLog { .. }
    )
}

/// A PID is not ownership. Birth identity and user must still match before it
/// counts as a resource; Linux signaling additionally uses a kernel pidfd.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    uid: u32,
    birth: String,
}

#[derive(Clone)]
struct Process {
    identity: ProcessIdentity,
    parent: u32,
    group: u32,
    session: Option<u32>,
    zombie: bool,
}

impl ProcessIdentity {
    pub fn read(pid: u32) -> Option<Self> {
        process(pid).map(|p| p.identity)
    }

    pub fn alive(&self) -> bool {
        process(self.pid).is_some_and(|p| p.identity == *self && !p.zombie)
    }

    /// The executable of this recorded process, not of a reused PID.
    pub(crate) fn program(&self) -> Option<PathBuf> {
        if !self.alive() {
            return None;
        }
        #[cfg(target_os = "linux")]
        let program = std::fs::read_link(format!("/proc/{}/exe", self.pid)).ok()?;
        #[cfg(target_os = "macos")]
        let program = {
            use std::os::unix::ffi::OsStrExt;
            let mut buffer = [0_u8; 4096];
            // SAFETY: the buffer is writable for the length passed to proc_pidpath.
            let len = unsafe {
                libc::proc_pidpath(
                    self.pid as i32,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                )
            };
            if len <= 0 {
                return None;
            }
            let path = std::ffi::CStr::from_bytes_until_nul(&buffer).ok()?;
            PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes()))
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let program = return None;
        self.alive().then_some(program)
    }

    #[cfg(target_os = "linux")]
    fn signal(&self, signal: i32) -> Result<()> {
        // Open first, then validate birth identity: pidfd pins exactly the
        // process inspected, even if it exits and its numeric PID is reused.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.pid, 0) };
        if fd < 0 {
            if !self.alive() {
                return Ok(());
            }
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = unsafe { File::from_raw_fd(fd as i32) };
        if !self.alive() {
            return Ok(());
        }
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if rc < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn signal(&self, _signal: i32) -> Result<()> {
        if self.alive() {
            bail!(
                "process {} has no retained child handle; this OS cannot signal a saved PID without a reuse race",
                self.pid
            );
        }
        Ok(())
    }

    /// Interrupts this exact process (SIGINT), never whatever holds its PID
    /// later. Linux pins it with a pidfd. Elsewhere the birth identity is
    /// checked right before the signal, so the PID could only be reused in
    /// that instant, where a bare saved PID could be reused any time after
    /// the kernel exits. An interrupt is not a kill: Force stop still
    /// refuses saved PIDs on macOS (see `signal`).
    pub fn interrupt(&self) -> Result<()> {
        anyhow::ensure!(self.alive(), "the kernel process has exited");
        #[cfg(target_os = "linux")]
        return self.signal(libc::SIGINT);
        #[cfg(not(target_os = "linux"))]
        {
            crate::kernel::send_signal(self.pid, libc::SIGINT)?;
            Ok(())
        }
    }
}

/// The kernel process this daemon recorded when it started the kernel.
pub(crate) fn recorded_kernel(connection_file: &Path) -> Option<ProcessIdentity> {
    read_record(&connection_file.with_extension("owner.json"))
        .ok()??
        .roots
        .into_iter()
        .next()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Ownership {
    roots: Vec<ProcessIdentity>,
    #[serde(default)]
    members: Vec<ProcessIdentity>,
    #[serde(default)]
    unverified: Vec<ProcessIdentity>,
}

pub(crate) fn record_kernel(connection_file: &Path, pid: u32) -> Result<()> {
    let root = ProcessIdentity::read(pid).context("cannot establish kernel process identity")?;
    write_record(
        &connection_file.with_extension("owner.json"),
        &Ownership {
            roots: vec![root.clone()],
            members: vec![root],
            unverified: Vec::new(),
        },
    )
}

fn write_record(path: &Path, record: &Ownership) -> Result<()> {
    SafeDir::open(path.parent().context("no ownership parent")?)?.write_file(
        path.file_name().context("no ownership filename")?,
        &serde_json::to_vec(record)?,
    )
}

fn read_record(path: &Path) -> Result<Option<Ownership>> {
    if !path_exists(path)? {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&read_safe(path)?)?))
}

fn ownership_still_live(owner: &mut Ownership, all: &mut HashMap<u32, Process>) -> Result<bool> {
    refresh(owner, all)?;
    Ok(!owner.members.is_empty() || !owner.unverified.is_empty())
}

/// The same ownership and process checks as resource inventory, without
/// opening a writable journal or updating any ownership records.
pub(crate) fn old_resources_alive(
    state: &crate::StateDir,
    sessions: &[crate::journal::SessionRecord],
) -> Result<bool> {
    let mut all = processes()?;
    let mut recorded = HashSet::new();
    for session in sessions {
        let path = session.connection_file.with_extension("owner.json");
        if let Some(mut owner) = read_record(&path)? {
            if ownership_still_live(&mut owner, &mut all)? {
                return Ok(true);
            }
            recorded.insert(path);
        } else if let Some(pid) = session.pid
            && ownership::unrecorded_kernel_alive(pid, session.status == SessionStatus::Ended, &all)
        {
            return Ok(true);
        }
    }
    let dir = state.kernels_dir();
    if path_exists(&dir)? {
        let _safe = SafeDir::open(&dir)?;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".owner.json"))
                && !recorded.contains(&path)
                && let Some(mut owner) = read_record(&path)?
                && ownership_still_live(&mut owner, &mut all)?
            {
                return Ok(true);
            }
        }
    }
    let dir = state.terminals_dir();
    if path_exists(&dir)? {
        let _safe = SafeDir::open(&dir)?;
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            anyhow::ensure!(entry.file_type()?.is_dir(), "unexpected terminal file");
            let paths = Paths::new(entry.path());
            let mut owner =
                read_record(&entry.path().join("lifecycle.json"))?.unwrap_or_else(|| Ownership {
                    roots: Vec::new(),
                    members: Vec::new(),
                    unverified: Vec::new(),
                });
            let host = if path_exists(&paths.host())? {
                Some(serde_json::from_slice::<Value>(&read_safe(&paths.host())?)?)
            } else {
                None
            };
            let mut older_pid_live = false;
            if let Some(host) = host {
                for (identity, pid) in [("host_identity", "host"), ("identity", "pid")] {
                    if host.get(identity).is_none_or(Value::is_null)
                        && !path_exists(&paths.exit())?
                        && let Some(pid) =
                            host[pid].as_u64().and_then(|pid| u32::try_from(pid).ok())
                    {
                        older_pid_live |= ownership::unrecorded_kernel_alive(pid, false, &all);
                    }
                }
                for key in ["host_identity", "identity"] {
                    if let Some(id) = host.get(key).filter(|value| !value.is_null()) {
                        let identity = serde_json::from_value(id.clone())?;
                        if !owner.roots.contains(&identity) {
                            owner.roots.push(identity);
                        }
                    }
                }
            }
            if ownership_still_live(&mut owner, &mut all)?
                || older_pid_live
                || std::os::unix::net::UnixStream::connect(paths.socket()).is_ok()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct Resource {
    value: Value,
    owner: Option<Ownership>,
    terminal: bool,
    raw_id: String,
    host: Option<ProcessIdentity>,
}

impl Daemon {
    /// Called only while holding the lifecycle write gate, so it runs only
    /// after any active cleanup or uninstall. Never clear the maintenance
    /// latch in place: a replacement opens only after this exits.
    pub(crate) fn prepare_attach(&self, build: &str) -> Result<Value> {
        if self.state.root().join("uninstalled").try_exists()? {
            bail!("Sandhill was uninstalled; automatic restart is disabled");
        }
        // After an incomplete Stop All, resources may remain. An explicit
        // reopen still replaces the daemon: the replacement adopts what is
        // left, as after an upgrade, and lists it for another Stop All.
        // Refusing here left no way to reach the device to inspect them.
        let stopped = self.maintenance.load(Ordering::Acquire);
        let restart = stopped || (!build.is_empty() && build != crate::build_id());
        if restart {
            tracing::info!(stopped, "explicit attach accepted daemon replacement");
            self.stopping.store(true, Ordering::Release);
        }
        Ok(json!({"restart":restart}))
    }

    /// The user's explicit Force stop of one kernel after its graceful stop
    /// failed. The session's own child handle kills it when this daemon
    /// started it; a kernel adopted after a restart is killed through its
    /// ownership record, which only Linux can signal without a PID reuse
    /// race (see [`ProcessIdentity`]).
    pub(crate) async fn force_stop_session(&self, id: &str) -> Result<()> {
        let handle = self.sessions.lock().unwrap().get(id).cloned();
        if let Some(handle) = &handle
            && handle.force_stop().await.is_ok()
        {
            self.sessions.lock().unwrap().remove(id);
            return Ok(());
        }
        let session = self
            .journal
            .sessions()?
            .into_iter()
            .find(|s| s.id == id)
            .context("no such session")?;
        let owner_path = session.connection_file.with_extension("owner.json");
        let mut owner = read_record(&owner_path)?.context(
            "this kernel has no ownership record, so Sandhill can't safely kill it; stop it from the Terminal",
        )?;
        let mut all = processes()?;
        if refresh(&mut owner, &mut all)? {
            write_record(&owner_path, &owner)?;
        }
        for process in owner.members.iter().rev() {
            process.signal(libc::SIGKILL)?;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while owner.members.iter().any(ProcessIdentity::alive) {
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "the kernel did not exit after Force stop"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // The session's liveness check notices the exit and ends it.
        self.sessions.lock().unwrap().remove(id);
        Ok(())
    }

    fn resources(&self) -> Result<Vec<Resource>> {
        let mut all = processes()?;
        let mut resources = Vec::new();
        for session in self.journal.sessions()? {
            let owner_path = session.connection_file.with_extension("owner.json");
            let mut owner = read_record(&owner_path)?;
            if let Some(record) = owner.as_mut()
                && refresh(record, &mut all)?
            {
                write_record(&owner_path, record)?;
            }
            let known_live = owner
                .as_ref()
                .is_some_and(|r| !r.members.is_empty() || !r.unverified.is_empty());
            let unknown_live = owner.is_none()
                && session.pid.is_some_and(|pid| {
                    ownership::unrecorded_kernel_alive(
                        pid,
                        session.status == SessionStatus::Ended,
                        &all,
                    )
                });
            if known_live || unknown_live {
                resources.push(Resource {
                    value: json!({"id":format!("kernel:{}",session.id),"kind":"kernel","label":session.kernel,
                        "project":session.project,"state":if owner.as_ref().is_some_and(|r|r.unverified.is_empty()) {"running"} else {"ownership_unverified"}}),
                    owner, terminal: false, raw_id: session.id, host: None,
                });
            }
        }
        // Do not use Terminals::list: its convenience listing skips malformed
        // metadata, which must not turn a failed inspection into 'all stopped'.
        let _safe_terminals = SafeDir::open(&self.state.terminals_dir())?;
        for entry in std::fs::read_dir(self.state.terminals_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                bail!("unexpected terminal entry {}", entry.path().display());
            }
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid terminal id"))?;
            let info = self.terminals.info(&id)?;
            let paths = Paths::new(entry.path());
            let owner_path = entry.path().join("lifecycle.json");
            let host: Value = if path_exists(&paths.host())? {
                serde_json::from_slice(&read_safe(&paths.host())?)?
            } else {
                Value::Null
            };
            let current_host = host
                .get("host_identity")
                .and_then(|value| serde_json::from_value::<ProcessIdentity>(value.clone()).ok());
            let current_roots: Vec<ProcessIdentity> = ["host_identity", "identity"]
                .into_iter()
                .filter_map(|key| serde_json::from_value(host.get(key)?.clone()).ok())
                .collect();
            let mut owner = read_record(&owner_path)?;
            let mut roots_changed = false;
            if !current_roots.is_empty() {
                let record = owner.get_or_insert_with(|| Ownership {
                    roots: Vec::new(),
                    members: Vec::new(),
                    unverified: Vec::new(),
                });
                for root in current_roots {
                    if !record.roots.contains(&root) {
                        record.roots.push(root);
                        roots_changed = true;
                    }
                }
            }
            if let Some(record) = owner.as_mut() {
                let members_changed = refresh(record, &mut all)?;
                if roots_changed || members_changed || !path_exists(&owner_path)? {
                    write_record(&owner_path, record)?;
                }
            }
            if owner
                .as_ref()
                .is_some_and(|r| !r.members.is_empty() || !r.unverified.is_empty())
                || info.running
            {
                resources.push(Resource {
                    value: json!({"id":format!("terminal:{id}"),"kind":info.kind,"label":info.title,"project":info.project,
                        "state":if owner.as_ref().is_some_and(|r|r.unverified.is_empty() && !r.members.is_empty()) {"running"} else {"ownership_unverified"}}),
                    owner, terminal: true, raw_id: id, host: current_host,
                });
            }
        }
        Ok(resources)
    }

    pub(crate) fn resource_inventory(&self) -> Result<Value> {
        match self.resources() {
            Ok(resources) => Ok(
                json!({"resources":resources.into_iter().map(|r|r.value).collect::<Vec<_>>(),"errors":[]}),
            ),
            Err(err) => Ok(json!({"resources":[],"errors":[format!("{err:#}")]})),
        }
    }

    pub(crate) async fn stop_resources(&self, force: bool) -> Result<Value> {
        self.maintenance.store(true, Ordering::Release);
        self.language_servers.stop_all().await;
        let resources = match self.resources() {
            Ok(resources) => resources,
            Err(err) => {
                return Ok(json!({"stopped":false,"remaining":[],"errors":[format!("{err:#}")]}));
            }
        };
        let mut errors = Vec::new();
        let mut legacy_hangups = false;
        for resource in &resources {
            if resource.terminal {
                let path = self.terminals.dir(&resource.raw_id)?.join("host.json");
                let host: Value = read_safe(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok())
                    .unwrap_or(Value::Null);
                if host["management_stop"] != true && force {
                    // Hosts from older builds only know Hangup, the request
                    // behind closing a terminal tab. Their Hangup sends SIGHUP,
                    // then SIGKILL after 3 s, so it is used only on explicit
                    // force. They record no process identity to signal instead.
                    if let Err(err) = self.terminals.send(&resource.raw_id, ToHost::Hangup).await {
                        errors.push(format!("{}: {err:#}", resource.raw_id));
                    }
                    legacy_hangups = true;
                } else if host["management_stop"] != true {
                    errors.push(format!(
                        "{} was started by an older Sandhill and cannot be stopped without force; Force Stop closes it as closing its tab did then (hangup, then kill after 3 s)",
                        resource.raw_id
                    ));
                } else if let Err(err) = self
                    .terminals
                    .send(&resource.raw_id, ToHost::Stop { force })
                    .await
                {
                    // A host may have just exited while descendants remain.
                    if !force {
                        errors.push(format!("{}: {err:#}", resource.raw_id));
                    }
                }
            } else {
                let handle = self.sessions.lock().unwrap().get(&resource.raw_id).cloned();
                if let Some(handle) = handle {
                    let result = if force {
                        handle.force_stop().await
                    } else {
                        handle.shutdown().await
                    };
                    if let Err(err) = result
                        && !force
                    {
                        errors.push(format!("{}: {err:#}", resource.raw_id));
                    }
                }
            }
            // Graceful requests above do not kill anything automatically.
            // Descendants get SIGTERM, or SIGKILL only on explicit force.
            if let Some(owner) = &resource.owner {
                for process in owner.members.iter().rev() {
                    // Let the host drain output, record exit and close its
                    // socket normally. Killing the log holder is not a
                    // graceful terminal stop.
                    if resource.host.as_ref().is_some_and(|host| host == process) {
                        continue;
                    }
                    if let Err(err) =
                        process.signal(if force { libc::SIGKILL } else { libc::SIGTERM })
                    {
                        // macOS retained child/PTY control may already be stopping it.
                        if force {
                            errors.push(format!("{}: {err:#}", resource.raw_id));
                        }
                    }
                }
            }
        }
        if force {
            // Explicit force may also terminate an unresponsive holder, but
            // first give responsive holders time to flush their final output.
            tokio::time::sleep(Duration::from_millis(500)).await;
            for resource in &resources {
                if let Some(host) = resource.host.as_ref()
                    && let Err(err) = host.signal(libc::SIGKILL)
                {
                    errors.push(format!("{} host: {err:#}", resource.raw_id));
                }
            }
        }
        // Older hosts kill their program 3 s after a hangup, then record its exit.
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(if legacy_hangups { 10 } else { 5 });
        loop {
            let remaining = match self.resources() {
                Ok(remaining) => remaining,
                Err(err) => {
                    errors.push(format!("{err:#}"));
                    return Ok(
                        json!({"stopped":false,"remaining":resources.into_iter().map(|r|r.value).collect::<Vec<_>>(),"errors":errors}),
                    );
                }
            };
            if remaining.is_empty() || tokio::time::Instant::now() >= deadline {
                // Stopping is a statement about OS resources, not actor state.
                if remaining.is_empty() {
                    // Finish actors before history can be purged. An actor
                    // whose process exited through pidfd may not have sampled
                    // liveness yet and must not append after SQLite clearing.
                    let handles: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
                    for handle in handles {
                        if handle.is_running() {
                            let _ = handle.shutdown().await;
                        }
                    }
                    errors.clear();
                }
                return Ok(
                    json!({"stopped":remaining.is_empty(),"remaining":remaining.into_iter().map(|r|r.value).collect::<Vec<_>>(),"errors":errors}),
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn storage_entries(&self) -> Result<Vec<Value>> {
        let stopped = self.resources()?.is_empty();
        let root = self.state.root();
        let mut entries = Vec::new();
        for (id, category, path, removable, reason) in [
            (
                "pages",
                "cache",
                root.join("pages"),
                stopped,
                "Stop resources before clearing served output pages",
            ),
            (
                "redraw",
                "cache",
                root.join("redraw"),
                stopped,
                "Stop resources before clearing plot redraws",
            ),
            (
                "daemon_log",
                "logs",
                self.state.log_path(),
                true,
                "Truncated in place without unlinking the daemon's open log",
            ),
            (
                "journal",
                "history",
                self.state.db_path(),
                stopped,
                "Stop all resources before clearing saved runs, outputs and file history",
            ),
            (
                "journal_wal",
                "history",
                root.join("journal.sqlite-wal"),
                false,
                "Managed by the live SQLite connection; never unlinked",
            ),
            (
                "journal_shm",
                "history",
                root.join("journal.sqlite-shm"),
                false,
                "Managed by the live SQLite connection; never unlinked",
            ),
            (
                "blobs",
                "history",
                self.state.blobs_dir(),
                stopped,
                "Saved outputs and file versions, not regenerable cache",
            ),
            (
                "kernels",
                "history",
                self.state.kernels_dir(),
                stopped,
                "Kernel connection files, ownership records and output logs",
            ),
            (
                "terminals",
                "history",
                self.state.terminals_dir(),
                stopped,
                "Terminal and agent conversations; project copies are preserved",
            ),
            (
                "page_key",
                "settings",
                root.join("pages.key"),
                false,
                "Signing key retained with device settings",
            ),
        ] {
            if let Some(mut entry) = storage_entry(id, category, &path, removable)? {
                if !removable {
                    entry["reason"] = reason.into();
                }
                if id == "journal"
                    && let Err(err) = self.journal.check_history_storage()
                {
                    entry["removable"] = false.into();
                    entry["reason"] = format!("{err}").into();
                }
                entries.push(entry);
            }
        }
        let mut projects: HashSet<String> = self
            .journal
            .sessions()?
            .into_iter()
            .filter_map(|s| s.project)
            .collect();
        for terminal in self.terminals.list() {
            if let Some(project) = terminal.project {
                projects.insert(project);
            }
        }
        for (project, _) in self.journal.recent_projects()? {
            projects.insert(project);
        }
        for project in projects {
            let path = PathBuf::from(&project).join(".sandhill");
            if let Some(mut entry) =
                storage_entry(&format!("project:{project}"), "projects", &path, false)?
            {
                entry["reason"] =
                    "Project settings, outputs and conversations are never recursively deleted"
                        .into();
                entries.push(entry);
            }
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for (id, path) in component_paths() {
                if let Some(mut entry) = storage_entry(id, "programs", &home.join(path), false)? {
                    entry["reason"] = "Use confirmed component uninstall; ownership manifest and checksum are required".into();
                    entries.push(entry);
                }
            }
            for (id, path) in legacy_components() {
                if let Some(mut entry) =
                    storage_entry(&format!("old:{id}"), "programs", &home.join(path), false)?
                {
                    entry["reason"] = "Sandhill kept this older install. Removal requires a matching ownership record.".into();
                    entries.push(entry);
                }
            }
            let log = self.state.log_path();
            let latest_move_note = log
                .symlink_metadata()
                .ok()
                .filter(|m| m.is_file())
                .and_then(|_| crate::logfile::tail(&log, 16 * 1024).ok())
                .and_then(|tail| {
                    tail.text
                        .lines()
                        .rev()
                        .find(|line| {
                            [
                                "Sandhill kept its old files because",
                                "Sandhill could not move its old files",
                                "Sandhill could not check its old folder",
                                "Sandhill could not prepare the new folder",
                            ]
                            .iter()
                            .any(|prefix| line.starts_with(prefix))
                        })
                        .map(str::to_owned)
                });
            for &(old, new) in sandhill_proto::OLD_DAEMON_LAYOUT {
                let path = home.join(old);
                if let Some(mut entry) =
                    storage_entry(&format!("old:{old}"), "settings", &path, false)?
                {
                    let reason = if path == root || !home.join(new).exists() {
                        latest_move_note.as_deref().unwrap_or(
                            "Sandhill has not moved this folder yet. See the daemon log for why.",
                        )
                    } else {
                        "Sandhill kept this old folder because the new place already exists."
                    };
                    entry["reason"] = reason.into();
                    entries.push(entry);
                }
            }
            for (id, path) in [
                ("component_manifests", sandhill_proto::INSTALL_PATH),
                ("agent_accounts", sandhill_proto::ACCOUNTS_PATH),
            ] {
                if let Some(entry) = storage_entry(id, "settings", &home.join(path), false)? {
                    entries.push(entry);
                }
            }
            match antigravity_entry(&home) {
                Ok(Some(entry)) => {
                    let owned = antigravity_owned(&home, &entry).is_ok();
                    entries.push(json!({"id":"antigravity-mcp","category":"settings",
                        "path":home.join(AGY_CONFIG),"bytes":serde_json::to_vec(&entry)?.len(),
                        "removable":false,"reason":if owned {"Owned MCP entry; removed only by confirmed uninstall"} else {"Legacy or modified MCP entry; ownership unverified, preserved"}}));
                }
                Ok(None) => {}
                Err(err) => {
                    entries.push(json!({"id":"antigravity-mcp","category":"settings",
                    "path":home.join(AGY_CONFIG),"bytes":0,"removable":false,
                    "reason":format!("Cannot safely inspect provider MCP configuration: {err:#}")}))
                }
            }
        }
        Ok(entries)
    }

    pub(crate) fn storage_inventory(&self) -> Result<Value> {
        match self.storage_entries() {
            Ok(entries) => Ok(json!({"entries":entries,"errors":[]})),
            Err(err) => Ok(json!({"entries":[],"errors":[format!("{err:#}")]})),
        }
    }

    pub(crate) async fn clean_storage(&self, categories: &[String]) -> Result<Value> {
        let selected: HashSet<&str> = categories.iter().map(String::as_str).collect();
        for category in &selected {
            if !matches!(*category, "cache" | "logs" | "history") {
                bail!("category {category:?} cannot be cleaned");
            }
        }
        if !self.resources()?.is_empty() {
            bail!("stop all resources before cleaning storage");
        }
        if selected.is_empty() {
            return Ok(
                json!({"success":true,"results":[],"entries":self.storage_entries()?,"errors":[]}),
            );
        }
        if selected.contains("history") {
            let handles: Vec<_> = self.sessions.lock().unwrap().values().cloned().collect();
            for handle in handles {
                handle.detach_wait().await;
            }
        }
        let entries = self.storage_entries()?;
        let mut results = Vec::new();
        let mut errors = Vec::new();
        let mut journal_cleared = false;
        for entry in &entries {
            let category = entry["category"].as_str().unwrap_or_default();
            if !selected.contains(category) {
                continue;
            }
            let id = entry["id"].as_str().unwrap_or_default();
            // WAL/SHM are included in journal accounting but never individually removed.
            if matches!(id, "journal_wal" | "journal_shm") {
                continue;
            }
            let path = PathBuf::from(entry["path"].as_str().context("storage path missing")?);
            let before = storage_entry(id, category, &path, false)?
                .and_then(|e| e["bytes"].as_u64())
                .unwrap_or(0);
            let result = if entry["removable"] != true {
                Err(anyhow::anyhow!(
                    "{}",
                    entry["reason"].as_str().unwrap_or("protected storage")
                ))
            } else if id == "journal" {
                let _analysis = self.analysis_gate.lock().await;
                let result = self.journal.clear_history().map_err(anyhow::Error::from);
                journal_cleared = result.is_ok();
                if journal_cleared {
                    self.sessions.lock().unwrap().clear();
                }
                result
            } else if id == "daemon_log" {
                SafeDir::open(path.parent().context("no log parent")?)?
                    .write_file(path.file_name().context("no log filename")?, b"")
            } else if category == "history" && !journal_cleared {
                Err(anyhow::anyhow!(
                    "journal history must be cleared successfully first"
                ))
            } else {
                remove_safe(&path)
            };
            let after = storage_entry(id, category, &path, false)?
                .and_then(|e| e["bytes"].as_u64())
                .unwrap_or(0);
            let mut report = json!({"id":id,"category":category,"path":path,"removed":result.is_ok(),"bytes":before.saturating_sub(after)});
            if let Err(err) = result {
                let message = format!("{err:#}");
                report["error"] = message.clone().into();
                errors.push(message);
            }
            results.push(report);
        }
        // Keep fixed state directories present for subsequent explicit reuse.
        self.state.create()?;
        Ok(
            json!({"success":errors.is_empty(),"results":results,"entries":self.storage_entries()?,"errors":errors}),
        )
    }

    pub(crate) async fn uninstall_components(&self, delete_data: bool) -> Result<Value> {
        let stopped = self.stop_resources(false).await?;
        if stopped["stopped"] != true {
            return Ok(
                json!({"success":false,"results":[],"remaining":stopped["remaining"].as_array().unwrap_or(&Vec::new()).iter().map(|r|r["id"].clone()).collect::<Vec<_>>(),"errors":stopped["errors"],"data_preserved":true}),
            );
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")?;
        // Keep this tombstone even when data removal was explicitly requested.
        let parent = SafeDir::open(self.state.root())?;
        parent.write_file(
            std::ffi::OsStr::new("uninstalled"),
            b"Sandhill components were explicitly removed. Reinstall explicitly to reactivate.\n",
        )?;
        let mut results = Vec::new();
        let mut remaining = Vec::new();
        let mut errors = Vec::new();
        for (id, relative) in component_paths()
            .into_iter()
            .map(|(id, path)| (id, std::borrow::Cow::Borrowed(path)))
            .chain(legacy_components().map(|(id, path)| (id, std::borrow::Cow::Owned(path))))
        {
            let manifest = component_manifest(&home, id, &relative);
            if !path_exists(&home.join(relative.as_ref()))? && manifest.is_none() {
                continue;
            }
            let result = manifest
                .with_context(|| {
                    format!("{id}: no matching ownership record for {relative}; left in place")
                })
                .and_then(|manifest| uninstall_one(&home, id, &relative, &manifest));
            let mut report = json!({"id":id,"path":relative.as_ref(),"removed":result.is_ok()});
            if let Err(err) = result {
                let message = format!("{err:#}");
                report["error"] = message.clone().into();
                remaining.push(id.to_string());
                errors.push(message);
            }
            results.push(report);
        }
        let mcp_result = (|| -> Result<bool> {
            if antigravity_entry(&home)?.is_none() {
                return Ok(false);
            }
            remove_antigravity_mcp_at(&home)?;
            Ok(true)
        })();
        match mcp_result {
            Ok(true) => results.push(json!({"id":"antigravity-mcp","removed":true})),
            Ok(false) => {}
            Err(err) => {
                let message = format!("{err:#}");
                results.push(json!({"id":"antigravity-mcp","removed":false,"error":message}));
                remaining.push("antigravity-mcp".into());
                errors.push(message);
            }
        }
        if delete_data {
            let clean = self
                .clean_storage(&["cache".into(), "history".into()])
                .await?;
            for report in clean["results"].as_array().into_iter().flatten() {
                results.push(report.clone());
            }
            for error in clean["errors"].as_array().into_iter().flatten() {
                errors.push(error.as_str().unwrap_or("data removal failed").to_string());
            }
            if clean["success"] != true {
                remaining.push("selected device data".into());
            }
        }
        // This is intentionally not a disconnect; every individual result must
        // reach the client before it chooses StopDaemon.
        Ok(
            json!({"success":remaining.is_empty() && errors.is_empty(),"results":results,"remaining":remaining,"errors":errors,"data_preserved":!delete_data}),
        )
    }
}

/// Preserve legacy/unowned registrations; use a separate tracked name rather
/// than treating an old App path as proof that we may overwrite user settings.
pub(crate) fn antigravity_registration(executable: &Path) -> Result<(String, bool)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    antigravity_registration_at(&home, executable)
}

pub(crate) fn record_antigravity_mcp(server: &str, executable: &Path) -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let config: Value = serde_json::from_slice(&read_safe(&home.join(AGY_CONFIG))?)?;
    let entry = config
        .get("mcpServers")
        .and_then(|v| v.get(server))
        .context("provider MCP config location was not confirmed")?;
    if entry["command"] != executable.to_string_lossy().as_ref() || entry["args"] != json!(["mcp"])
    {
        bail!("provider MCP entry does not match the just-installed command");
    }
    let manifest = antigravity_manifest(&home);
    let parent = SafeDir::ensure(manifest.parent().context("no manifest parent")?)?;
    parent.write_file(
        manifest.file_name().context("no manifest filename")?,
        &serde_json::to_vec_pretty(
            &json!({"version":1,"component":"antigravity-mcp","path":AGY_CONFIG,"server":server,"entry":entry}),
        )?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An adopted kernel is interrupted by identity: a process now holding
    /// its PID is left alone (Huan, 2026-11-18).
    #[test]
    fn interrupt_reaches_only_the_recorded_process() {
        use std::os::unix::process::ExitStatusExt;
        let mut kernel = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let identity = ProcessIdentity::read(kernel.id()).unwrap();
        // `other` holding a PID whose recorded owner was born earlier.
        let mut reused = ProcessIdentity::read(other.id()).unwrap();
        reused.birth.push_str("-earlier");
        let refused = reused.interrupt();
        let sent = identity.interrupt();
        let kernel_exit = kernel.wait().unwrap();
        let other_running = other.try_wait().unwrap().is_none();
        other.kill().unwrap();
        other.wait().unwrap();
        assert!(refused.is_err());
        assert!(other_running, "the process holding the PID was signalled");
        assert!(sent.is_ok(), "{sent:?}");
        assert_eq!(kernel_exit.signal(), Some(libc::SIGINT));
        // Gone (and reaped): an error, no signal to whoever gets the PID.
        assert!(identity.interrupt().is_err());
    }

    #[test]
    fn maintenance_blocks_execution_and_restart() {
        assert!(allowed_during_maintenance(&Call::ResourceInventory));
        assert!(allowed_during_maintenance(&Call::StopResources {
            force: true
        }));
        assert!(!allowed_during_maintenance(&Call::StartSession {
            kernel: "python3".into(),
            cwd: None,
            project: None
        }));
        assert!(!allowed_during_maintenance(&Call::TerminalInput {
            terminal: "a".into(),
            data: "run\n".into()
        }));
    }
}
