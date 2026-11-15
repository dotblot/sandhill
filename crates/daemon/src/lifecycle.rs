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
    AGY_CONFIG, AGY_MANIFEST, antigravity_entry, antigravity_owned, antigravity_registration_at,
    component_paths, remove_antigravity_mcp_at, uninstall_one,
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
    let manifest = home.join(AGY_MANIFEST);
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
