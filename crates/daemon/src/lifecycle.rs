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
}

#[cfg(target_os = "linux")]
fn process(pid: u32) -> Option<Process> {
    use std::os::unix::fs::MetadataExt;
    let path = format!("/proc/{pid}");
    let uid = std::fs::metadata(&path).ok()?.uid();
    let text = std::fs::read_to_string(format!("{path}/stat")).ok()?;
    let fields: Vec<&str> = text
        .get(text.rfind(')')? + 2..)?
        .split_whitespace()
        .collect();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(Process {
        identity: ProcessIdentity {
            pid,
            uid,
            birth: format!("{}:{}", boot.trim(), fields.get(19)?),
        },
        parent: fields.get(1)?.parse().ok()?,
        group: fields.get(2)?.parse().ok()?,
        session: Some(fields.get(3)?.parse().ok()?),
        zombie: *fields.first()? == "Z",
    })
}

#[cfg(target_os = "macos")]
fn process(pid: u32) -> Option<Process> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            1, // PROC_PIDTBSDINFO arg != 0 explicitly includes zombies.
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(Process {
        identity: ProcessIdentity {
            pid,
            uid: info.pbi_uid,
            birth: format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        },
        parent: info.pbi_ppid,
        group: info.pbi_pgid,
        // getsid can return ESRCH for a zombie or a process transitioning
        // through exit, even while BSD identity/status remain readable.
        // A missing session must not erase that independent exit evidence.
        session: unsafe { libc::getsid(pid as i32) }.try_into().ok(),
        zombie: info.pbi_status == 5,
    })
}

fn processes() -> Result<HashMap<u32, Process>> {
    #[cfg(target_os = "linux")]
    let pids: Vec<u32> = std::fs::read_dir("/proc")?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    #[cfg(target_os = "macos")]
    let pids: Vec<u32> = {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            bail!("cannot enumerate processes");
        }
        let mut pids = vec![0i32; count as usize + 1024];
        let size = (pids.len() * std::mem::size_of::<i32>()) as i32;
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        if n <= 0 || n as usize >= pids.len() {
            bail!("process enumeration was incomplete");
        }
        pids.truncate(n as usize);
        pids.into_iter().filter_map(|p| p.try_into().ok()).collect()
    };
    Ok(pids
        .into_iter()
        .filter_map(|pid| process(pid).map(|p| (pid, p)))
        .collect())
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

/// Capture children before sending any shutdown request. Kept identities also
/// account for descendants that outlive their original kernel or terminal host.
fn refresh(record: &mut Ownership, all: &mut HashMap<u32, Process>) -> Result<bool> {
    for identity in record.roots.iter().chain(&record.members) {
        // Enumeration and per-process inspection are not an atomic OS
        // snapshot. Resolve known PIDs directly before treating an omitted
        // process as unreadable. This also retains readable zombie status.
        if let std::collections::hash_map::Entry::Vacant(entry) = all.entry(identity.pid) {
            if let Some(current) = process(identity.pid) {
                entry.insert(current);
            } else if crate::kernel::pid_alive(identity.pid) {
                bail!(
                    "cannot inspect live recorded process {}; refusing to assume it stopped",
                    identity.pid
                );
            }
        }
    }
    refresh_with_liveness(record, all, ProcessIdentity::alive)
}

fn refresh_with_liveness(
    record: &mut Ownership,
    all: &HashMap<u32, Process>,
    alive: impl Fn(&ProcessIdentity) -> bool,
) -> Result<bool> {
    let original_len = record.members.len();
    record.members.retain(|id| {
        all.get(&id.pid)
            .is_some_and(|p| p.identity == *id && !p.zombie)
            && alive(id)
    });
    let mut owned: HashSet<u32> = record.members.iter().map(|p| p.pid).collect();
    for root in &record.roots {
        if all
            .get(&root.pid)
            .is_some_and(|p| p.identity == *root && !p.zombie)
            && alive(root)
        {
            owned.insert(root.pid);
        }
    }
    loop {
        let mut added = false;
        for p in all.values() {
            if p.zombie
                || p.identity.uid != unsafe { libc::getuid() }
                || owned.contains(&p.identity.pid)
            {
                continue;
            }
            let grouped = record.roots.iter().any(|root| {
                (owned.contains(&root.pid)
                    && all
                        .get(&root.pid)
                        .is_some_and(|live| live.identity == *root && !live.zombie)
                    || owned.iter().any(|pid| {
                        all.get(pid).is_some_and(|live| {
                            live.group == root.pid || live.session == Some(root.pid)
                        })
                    }))
                    && (p.group == root.pid || p.session == Some(root.pid))
            });
            // Confirm candidate birth identity after the whole snapshot was
            // collected, before it may establish ancestry for another PID.
            if (grouped || owned.contains(&p.parent)) && alive(&p.identity) {
                owned.insert(p.identity.pid);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let mut members: Vec<_> = owned
        .into_iter()
        .filter_map(|pid| all.get(&pid).map(|p| p.identity.clone()))
        .collect();
    members.sort_by_key(|p| p.pid);
    let changed = original_len != record.members.len() || members != record.members;
    record.members = members;
    // A group may have outlived its leader before the first inspection. Do
    // not lose it from inventory, but do not manufacture signal ownership:
    // a historical group number alone could have been reused.
    let mut unverified: Vec<_> = all
        .values()
        .filter(|p| {
            !p.zombie
                && p.identity.uid == unsafe { libc::getuid() }
                && !record.members.contains(&p.identity)
                && record
                    .roots
                    .iter()
                    .any(|root| p.group == root.pid || p.session == Some(root.pid))
        })
        .map(|p| p.identity.clone())
        .collect();
    unverified.sort_by_key(|p| p.pid);
    let changed = changed || unverified != record.unverified;
    record.unverified = unverified;
    Ok(changed)
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
                    all.values().any(|p| {
                        !p.zombie
                            && (p.group == pid
                                || p.session == Some(pid)
                                || p.identity.pid == pid && session.status != SessionStatus::Ended)
                    })
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
                "Stop all resources before clearing saved runs and outputs",
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
                "Saved output blobs, not regenerable cache",
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
        for (id, relative) in component_paths() {
            let path = home.join(relative);
            let manifest = home.join(format!(".local/share/sandhill/install/{id}.json"));
            if !path_exists(&path)? && !path_exists(&manifest)? {
                continue;
            }
            let result = uninstall_one(&home, id, relative, &manifest);
            let mut report = json!({"id":id,"removed":result.is_ok()});
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

fn component_paths() -> [(&'static str, &'static str); 6] {
    [
        ("sandhilld", ".local/bin/sandhilld"),
        ("ark", ".local/share/sandhill/bin/ark"),
        ("ty", sandhill_proto::TY_PATH),
        ("ruff", sandhill_proto::RUFF_PATH),
        ("systemd", ".config/systemd/user/sandhilld.service"),
        (
            "launchd",
            "Library/LaunchAgents/dev.sandhill.sandhilld.plist",
        ),
    ]
}

const AGY_CONFIG: &str = ".gemini/config/mcp_config.json";
const AGY_MANIFEST: &str = ".local/share/sandhill/install/antigravity-mcp.json";

fn antigravity_server_name(home: &Path) -> Result<String> {
    let manifest = home.join(AGY_MANIFEST);
    if !path_exists(&manifest)? {
        return Ok("sandhill".into());
    }
    let value: Value = serde_json::from_slice(&read_safe(&manifest)?)?;
    match value
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("sandhill")
    {
        name @ ("sandhill" | "sandhill-managed") => Ok(name.into()),
        _ => bail!("unknown Sandhill MCP registration name; preserved"),
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

fn antigravity_registration_at(home: &Path, executable: &Path) -> Result<(String, bool)> {
    let path = home.join(AGY_CONFIG);
    let config: Value = if path_exists(&path)? {
        serde_json::from_slice(&read_safe(&path)?)?
    } else {
        json!({})
    };
    let owned_name = antigravity_server_name(home)?;
    for name in ["sandhill", "sandhill-managed"] {
        let Some(entry) = config.get("mcpServers").and_then(|v| v.get(name)) else {
            return Ok((name.into(), false));
        };
        if entry["command"] == executable.to_string_lossy().as_ref()
            && entry["args"] == json!(["mcp"])
            && entry.get("disabled").and_then(Value::as_bool) != Some(true)
        {
            return Ok((name.into(), true));
        }
        if name == owned_name && antigravity_owned(home, entry).is_ok() {
            return Ok((name.into(), false));
        }
    }
    bail!("both Sandhill MCP registration names are unowned or modified; preserved")
}

fn antigravity_entry(home: &Path) -> Result<Option<Value>> {
    let path = home.join(AGY_CONFIG);
    if !path_exists(&path)? {
        return Ok(None);
    }
    let config: Value = serde_json::from_slice(&read_safe(&path)?)?;
    let server = antigravity_server_name(home)?;
    Ok(config
        .get("mcpServers")
        .and_then(|s| s.get(&server))
        .cloned())
}

fn antigravity_owned(home: &Path, entry: &Value) -> Result<()> {
    let manifest: Value = serde_json::from_slice(
        &read_safe(&home.join(AGY_MANIFEST))
            .context("Antigravity MCP entry has no ownership manifest; preserved")?,
    )?;
    if manifest["version"] != 1
        || manifest["component"] != "antigravity-mcp"
        || manifest["path"] != AGY_CONFIG
        || manifest["entry"] != *entry
    {
        bail!("Antigravity MCP registration is unowned or modified; preserved");
    }
    Ok(())
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
    let manifest = home.join(AGY_MANIFEST);
    let parent = SafeDir::ensure(manifest.parent().context("no manifest parent")?)?;
    parent.write_file(
        manifest.file_name().context("no manifest filename")?,
        &serde_json::to_vec_pretty(
            &json!({"version":1,"component":"antigravity-mcp","path":AGY_CONFIG,"server":server,"entry":entry}),
        )?,
    )
}

fn remove_antigravity_mcp_at(home: &Path) -> Result<()> {
    let path = home.join(AGY_CONFIG);
    let server = antigravity_server_name(home)?;
    let before = read_safe(&path)?;
    let mut config: Value = serde_json::from_slice(&before)?;
    let servers = config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .context("no MCP server map")?;
    let entry = servers
        .get(&server)
        .context("Sandhill MCP entry is absent")?;
    antigravity_owned(home, entry)?;
    servers.remove(&server);
    let parent = SafeDir::open(path.parent().context("no MCP config parent")?)?;
    parent.replace_file(
        path.file_name().context("no config filename")?,
        &before,
        &serde_json::to_vec_pretty(&config)?,
    )?;
    remove_safe(&home.join(AGY_MANIFEST))?;
    Ok(())
}

fn uninstall_one(home: &Path, id: &str, relative: &str, manifest: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let bytes = read_safe(manifest)
        .with_context(|| format!("{id}: no readable ownership manifest; preserved"))?;
    let record: Value = serde_json::from_slice(&bytes)?;
    if record["version"] != 1 || record["component"] != id || record["path"] != relative {
        bail!("{id}: invalid ownership manifest; preserved");
    }
    let path = home.join(relative);
    if path_exists(&path)? {
        let parent = SafeDir::open(path.parent().context("no installed file parent")?)?;
        let mut file = parent.open_file(path.file_name().context("no installed filename")?)?;
        let installed = file.metadata()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let digest = sandhill_proto::hex(&hasher.finalize());
        if record["sha256"] != digest {
            bail!("{id}: installed file was modified; preserved");
        }
        // Never stop this running service synchronously: StopDaemon follows the
        // delivered response. Disable its future launch before removing files.
        if id == "systemd" {
            let status = std::process::Command::new("systemctl")
                .args(["--user", "disable", "sandhilld.service"])
                .status()?;
            if !status.success() {
                bail!("could not disable owned systemd service");
            }
            if std::process::Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", "sandhilld.service"])
                .status()?
                .success()
            {
                bail!(
                    "owned systemd service is disabled but still loaded/running; stop it outside the daemon before removing its service file"
                );
            }
        } else if id == "launchd" {
            let target = format!("gui/{}/dev.sandhill.sandhilld", unsafe { libc::getuid() });
            let status = std::process::Command::new("launchctl")
                .args(["disable", &target])
                .status()?;
            if !status.success() {
                bail!("could not disable owned launchd service");
            }
            if std::process::Command::new("launchctl")
                .args(["print", &target])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?
                .success()
            {
                bail!(
                    "owned launchd service is disabled but still loaded; unload it outside the daemon before removing its service file"
                );
            }
        }
        let current = parent
            .open_file(path.file_name().context("no installed filename")?)?
            .metadata()?;
        if (
            installed.dev(),
            installed.ino(),
            installed.len(),
            installed.mtime(),
            installed.mtime_nsec(),
        ) != (
            current.dev(),
            current.ino(),
            current.len(),
            current.mtime(),
            current.mtime_nsec(),
        ) {
            bail!("{id}: installed file changed during removal; preserved");
        }
        parent.remove(path.file_name().context("no installed filename")?)?;
    }
    remove_safe(manifest)?;
    Ok(())
}

// All storage walking and deletion is descriptor-relative, including ancestors.
// A symlink can neither escape the state directory nor race a pathname check.
fn absolute_storage_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // macOS publishes these root-owned aliases for /private. Spell their
    // destinations directly; do not traverse the links. Arbitrary user-created
    // symlinks, including a custom state root, are still rejected by openat.
    #[cfg(target_os = "macos")]
    for (alias, destination) in [
        ("/var", "/private/var"),
        ("/tmp", "/private/tmp"),
        ("/etc", "/private/etc"),
    ] {
        if let Ok(relative) = absolute.strip_prefix(alias) {
            return Ok(Path::new(destination).join(relative));
        }
    }
    Ok(absolute)
}
struct SafeDir(File);
impl SafeDir {
    fn open(path: &Path) -> Result<Self> {
        let absolute = absolute_storage_path(path)?;
        let mut dir = Self(File::open("/")?);
        for component in absolute.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => dir = dir.child(name)?,
                _ => bail!("unsafe storage path {}", path.display()),
            }
        }
        Ok(dir)
    }
    fn ensure(path: &Path) -> Result<Self> {
        let absolute = absolute_storage_path(path)?;
        let mut dir = Self(File::open("/")?);
        for component in absolute.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => {
                    let c = CString::new(name.as_bytes())?;
                    if unsafe { libc::mkdirat(dir.0.as_raw_fd(), c.as_ptr(), 0o700) } < 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    dir = dir.child(name)?;
                }
                _ => bail!("unsafe directory path"),
            }
        }
        Ok(dir)
    }
    fn child(&self, name: &std::ffi::OsStr) -> Result<Self> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(unsafe { File::from_raw_fd(fd) }))
    }
    fn names(&self) -> Result<Vec<OsString>> {
        let fd = unsafe { libc::dup(self.0.as_raw_fd()) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe { libc::close(fd) };
            return Err(std::io::Error::last_os_error().into());
        }
        // dup shares the directory offset; rewind before each independent walk.
        unsafe { libc::rewinddir(stream) };
        let mut names = Vec::new();
        loop {
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            unsafe { *errno = 0 };
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = unsafe { *errno };
                if error != 0 {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::from_raw_os_error(error).into());
                }
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(OsString::from_vec(bytes.to_vec()));
            }
        }
        unsafe { libc::closedir(stream) };
        Ok(names)
    }
    fn stat(&self, name: &std::ffi::OsStr) -> Result<libc::stat> {
        let name = CString::new(name.as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { stat.assume_init() })
    }
    fn open_file(&self, name: &std::ffi::OsStr) -> Result<File> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            bail!("not a regular owned file");
        }
        Ok(file)
    }
    fn read_file(&self, name: &std::ffi::OsStr) -> Result<Vec<u8>> {
        let mut file = self.open_file(name)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn write_file(&self, name: &std::ffi::OsStr, bytes: &[u8]) -> Result<()> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        use std::os::unix::fs::MetadataExt;
        let mut file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            bail!("refusing to truncate linked or special storage");
        }
        file.set_len(0)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    fn replace_file(&self, name: &std::ffi::OsStr, before: &[u8], after: &[u8]) -> Result<()> {
        let temp = OsString::from(format!(".sandhill-{}.tmp", uuid::Uuid::new_v4()));
        self.write_file(&temp, after)?;
        let result = (|| -> Result<()> {
            if self.read_file(name)? != before {
                bail!("configuration changed during removal; preserved");
            }
            let source = CString::new(temp.as_bytes())?;
            let target = CString::new(name.as_bytes())?;
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    target.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.remove(&temp);
        }
        result
    }
    fn measure(&self) -> Result<(u64, bool)> {
        let mut total = 0u64;
        let mut links = false;
        for name in self.names()? {
            let stat = self.stat(&name)?;
            match stat.st_mode & libc::S_IFMT {
                libc::S_IFDIR => {
                    let (bytes, linked) = self.child(&name)?.measure()?;
                    total = total.saturating_add(bytes);
                    links |= linked;
                }
                libc::S_IFREG => total = total.saturating_add(stat.st_size.max(0) as u64),
                _ => links = true,
            }
        }
        Ok((total, links))
    }
    fn remove(&self, name: &std::ffi::OsStr) -> Result<()> {
        let stat = self.stat(name)?;
        let directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
        if directory {
            let dir = self.child(name)?;
            for entry in dir.names()? {
                dir.remove(&entry)?;
            }
        } else if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            bail!("refusing to remove a symlink or special file");
        }
        let name = CString::new(name.as_bytes())?;
        if unsafe {
            libc::unlinkat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

fn read_safe(path: &Path) -> Result<Vec<u8>> {
    SafeDir::open(path.parent().context("no file parent")?)?
        .read_file(path.file_name().context("no filename")?)
}
fn remove_safe(path: &Path) -> Result<()> {
    SafeDir::open(path.parent().context("no file parent")?)?
        .remove(path.file_name().context("no filename")?)
}
fn path_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}
fn storage_entry(id: &str, category: &str, path: &Path, removable: bool) -> Result<Option<Value>> {
    if !path_exists(path)? {
        return Ok(None);
    }
    let measured = (|| -> Result<(u64, bool)> {
        let parent = SafeDir::open(path.parent().context("no parent")?)?;
        let name = path.file_name().context("no filename")?;
        let stat = parent.stat(name)?;
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFDIR => parent.child(name)?.measure(),
            libc::S_IFREG => Ok((
                stat.st_size.max(0) as u64,
                matches!(id, "journal" | "daemon_log") && stat.st_nlink > 1,
            )),
            _ => Ok((0, true)),
        }
    })();
    Ok(Some(match measured {
        Ok((bytes, special)) => {
            let mut value = json!({"id":id,"category":category,"path":path,"bytes":bytes,"removable":removable && !special});
            if special {
                value["reason"] =
                    "Contains symlinks, hard links or special files; never traversed or automatically mutated"
                        .into();
            }
            value
        }
        Err(err) => {
            json!({"id":id,"category":category,"path":path,"bytes":0,"removable":false,"reason":format!("Not safely readable: {err:#}")})
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn symlinks_are_neither_counted_nor_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"private data").unwrap();
        let cache = root.join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("owned"), b"123").unwrap();
        std::os::unix::fs::symlink(&outside, cache.join("escape")).unwrap();
        let entry = storage_entry("cache", "cache", &cache, true)
            .unwrap()
            .unwrap();
        assert_eq!(entry["bytes"], 3);
        assert_eq!(entry["removable"], false);
        assert!(remove_safe(&cache).is_err());
        assert_eq!(
            std::fs::read(outside.join("keep")).unwrap(),
            b"private data"
        );
        assert!(read_safe(&cache.join("escape/keep")).is_err());
    }
    #[test]
    fn ownership_requires_manifest_and_matching_contents() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let relative = ".local/bin/sandhilld";
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"owned").unwrap();
        let manifest = home.join("manifest.json");
        assert!(uninstall_one(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&manifest, serde_json::to_vec(&json!({"version":1,"component":"sandhilld","path":relative,"sha256":sandhill_proto::hex(&Sha256::digest(b"owned"))})).unwrap()).unwrap();
        std::fs::write(&path, b"modified").unwrap();
        assert!(uninstall_one(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&path, b"owned").unwrap();
        uninstall_one(&home, "sandhilld", relative, &manifest).unwrap();
        assert!(!path.exists());
        assert!(!manifest.exists());
    }
    #[test]
    fn reused_pid_does_not_match_owned_process() {
        let live = ProcessIdentity::read(std::process::id()).unwrap();
        let mut stale = live.clone();
        stale.birth.push_str("-old");
        assert!(live.alive());
        assert!(!stale.alive());
        let mut ownership = Ownership {
            roots: vec![stale.clone()],
            members: vec![stale],
            unverified: Vec::new(),
        };
        refresh(&mut ownership, &mut processes().unwrap()).unwrap();
        assert!(ownership.members.is_empty());
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
    #[test]
    fn mcp_removal_preserves_unrelated_servers_and_auth_settings() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let config = home.join(AGY_CONFIG);
        SafeDir::ensure(config.parent().unwrap()).unwrap();
        let entry = json!({"command":"/owned/sandhilld","args":["mcp"]});
        let other = json!({"command":"/user/server","args":["--own"]});
        let document =
            json!({"mcpServers":{"sandhill":entry,"user":other},"auth":{"keep":"unchanged"}});
        std::fs::write(&config, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(remove_antigravity_mcp_at(&home).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&config).unwrap()).unwrap(),
            document
        );
        let manifest = home.join(AGY_MANIFEST);
        SafeDir::ensure(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            &manifest,
            serde_json::to_vec(
                &json!({"version":1,"component":"antigravity-mcp","path":AGY_CONFIG,"entry":entry}),
            )
            .unwrap(),
        )
        .unwrap();
        remove_antigravity_mcp_at(&home).unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert!(after["mcpServers"].get("sandhill").is_none());
        assert_eq!(after["mcpServers"]["user"], other);
        assert_eq!(after["auth"], document["auth"]);
        assert!(!manifest.exists());
    }

    #[test]
    fn tracked_mcp_replacement_preserves_legacy_entry_on_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let config = home.join(AGY_CONFIG);
        SafeDir::ensure(config.parent().unwrap()).unwrap();
        let legacy =
            json!({"command":"/expired/Sandhill.app/Contents/MacOS/sandhilld","args":["mcp"]});
        let current = json!({"command":"/current/sandhilld","args":["mcp"]});
        std::fs::write(
            &config,
            json!({"mcpServers":{"sandhill":legacy}}).to_string(),
        )
        .unwrap();
        assert_eq!(
            antigravity_registration_at(&home, Path::new("/current/sandhilld")).unwrap(),
            ("sandhill-managed".into(), false)
        );
        std::fs::write(
            &config,
            json!({"mcpServers":{"sandhill":legacy,"sandhill-managed":current}}).to_string(),
        )
        .unwrap();
        let manifest = home.join(AGY_MANIFEST);
        SafeDir::ensure(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            &manifest,
            json!({"version":1,"component":"antigravity-mcp",
            "path":AGY_CONFIG,"server":"sandhill-managed","entry":current})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            antigravity_registration_at(&home, Path::new("/upgraded/sandhilld")).unwrap(),
            ("sandhill-managed".into(), false)
        );
        remove_antigravity_mcp_at(&home).unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(after, json!({"mcpServers":{"sandhill":legacy}}));
        assert!(!manifest.exists());
    }
    #[test]
    fn log_truncation_refuses_hardlinked_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("user-data");
        std::fs::write(&original, b"keep").unwrap();
        std::fs::hard_link(&original, root.join("daemon.log")).unwrap();
        assert!(
            SafeDir::open(&root)
                .unwrap()
                .write_file(std::ffi::OsStr::new("daemon.log"), b"")
                .is_err()
        );
        assert_eq!(std::fs::read(&original).unwrap(), b"keep");
    }
    #[test]
    fn known_orphans_remain_owned_but_unobserved_groups_are_only_reported() {
        let root = ProcessIdentity {
            pid: 100,
            uid: unsafe { libc::getuid() },
            birth: "root".into(),
        };
        let child = ProcessIdentity {
            pid: 101,
            uid: root.uid,
            birth: "child".into(),
        };
        let grandchild = ProcessIdentity {
            pid: 102,
            uid: root.uid,
            birth: "grandchild".into(),
        };
        let all = HashMap::from([
            (
                100,
                Process {
                    identity: root.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: true,
                },
            ),
            (
                101,
                Process {
                    identity: child.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: false,
                },
            ),
            (
                102,
                Process {
                    identity: grandchild.clone(),
                    parent: 1,
                    group: 100,
                    session: Some(100),
                    zombie: false,
                },
            ),
        ]);
        let mut known = Ownership {
            roots: vec![root.clone()],
            members: vec![child],
            unverified: vec![],
        };
        refresh_with_liveness(&mut known, &all, |_| true).unwrap();
        assert!(known.members.contains(&grandchild));
        assert!(known.unverified.is_empty());
        let mut unknown = Ownership {
            roots: vec![root],
            members: vec![],
            unverified: vec![],
        };
        refresh_with_liveness(&mut unknown, &all, |_| true).unwrap();
        assert!(unknown.members.is_empty());
        assert_eq!(unknown.unverified.len(), 2);
    }

    #[test]
    fn recorded_live_process_omitted_from_enumeration_is_inspected_directly() {
        let identity = ProcessIdentity::read(std::process::id()).unwrap();
        let mut record = Ownership {
            roots: vec![identity.clone()],
            members: vec![identity.clone()],
            unverified: Vec::new(),
        };
        let mut snapshot = HashMap::new();
        refresh(&mut record, &mut snapshot).unwrap();
        assert_eq!(snapshot[&identity.pid].identity, identity);
        assert!(record.members.contains(&identity));
    }

    #[test]
    fn unreaped_exit_is_not_an_unreadable_live_resource() {
        use std::io::Write;
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let identity = ProcessIdentity::read(child.id()).unwrap();
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        let mut status = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // WNOWAIT observes exit without reaping: kill(pid, 0) still succeeds.
        let waited = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id(),
                status.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        let observed = process(child.id());
        let existed_before_reap = crate::kernel::pid_alive(child.id());
        let mut record = Ownership {
            roots: vec![identity.clone()],
            members: vec![identity],
            unverified: Vec::new(),
        };
        let inspected = refresh(&mut record, &mut HashMap::new());
        // Reap our own child before any assertions, including failure paths.
        let exited = child.wait().unwrap();
        assert_eq!(waited, 0);
        assert!(exited.success());
        assert!(
            existed_before_reap,
            "the regression requires an existing unreaped PID"
        );
        assert!(observed.is_some_and(|process| process.zombie));
        assert!(inspected.is_ok(), "{inspected:?}");
        assert!(record.members.is_empty());
    }
}
