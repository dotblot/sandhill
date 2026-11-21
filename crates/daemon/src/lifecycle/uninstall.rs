//! The uninstall plan drives both Review and execution. Paths that an open
//! daemon or app might write again go to the detached finisher.
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::process::{Command, Stdio};

use sandhill_proto::{
    Footprint, FootprintItem, FootprintKind, InstalledKey, OLD_DAEMON_LAYOUT, QuickInstalled,
    SANDHILL_DIR, STATE_PATH, SetupStep, UninstallChoice, UninstallPlan, UninstallReport,
};

use super::*;
use crate::agents::shell_quote;
use crate::kernelspecs::Search;
use crate::paths::StateDir;
use crate::quick::{install, record};

mod finisher;

pub(super) use finisher::wait_for_removal;

const MAC_PLACES: &[(&str, &str)] = &[
    ("Library/Caches/dev.sandhill.ide", "Sandhill's macOS cache"),
    (
        "Library/Preferences/dev.sandhill.ide.plist",
        "Sandhill's macOS preferences",
    ),
    (
        "Library/Saved Application State/dev.sandhill.ide.savedState",
        "Sandhill's saved window state",
    ),
    (
        "Library/WebKit/dev.sandhill.ide",
        "Sandhill's web view files",
    ),
    (
        "Library/HTTPStorages/dev.sandhill.ide",
        "Sandhill's web storage",
    ),
    (
        "Library/HTTPStorages/dev.sandhill.ide.binarycookies",
        "Sandhill's web cookies",
    ),
    (
        "Library/Application Support/dev.sandhill.ide",
        "Sandhill's macOS app files",
    ),
];
const STATE_CLEANUP: &[&str] = &[
    "pages",
    "pages.key",
    "redraw",
    "cache",
    "log",
    "logs",
    "sandhilld.log",
    "undo",
    "package-index",
    "copy-package-index",
    "checks",
    "sandhilld.sock",
    "daemon.lock",
    "attach.lock",
    record::FILE,
    ".install-record.lock",
    "install-prefs.json",
    ".install-prefs.lock",
    "uninstalled",
    "connectors",
    "skills",
    "file-history.json",
];
const CHANGED: &str = "Changed since Sandhill installed it. It is yours now.";
const KEYCHAIN: &str = "Keychain: dev.sandhill.agent-account";

struct Device {
    home: PathBuf,
    state: PathBuf,
    mac: bool,
    real_home: bool,
    short_sockets: Option<PathBuf>,
    projects: Vec<PathBuf>,
}

impl Device {
    fn new(state: &StateDir, projects: Vec<PathBuf>) -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is not set.")?;
        let home = fs::canonicalize(home).context("Couldn't find the home folder.")?;
        let state =
            fs::canonicalize(state.root()).context("Couldn't find Sandhill's state folder.")?;
        let explore = state.join("explore");
        let old_explore = home
            .join(crate::paths::legacy_path(STATE_PATH))
            .join("explore");
        let projects = projects
            .into_iter()
            .filter_map(|path| {
                let path = match path.canonicalize() {
                    Ok(path) => path,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => return None,
                    Err(_) => path,
                };
                if (path.starts_with(&explore) || path.starts_with(&old_explore))
                    && crate::quick::folder::card(&path).is_ok_and(|card| card.scratch)
                {
                    // This is a temporary Explore session, not a project the
                    // user saved. Its linked data remains outside Sandhill.
                    return None;
                }
                Some(path)
            })
            .collect();
        let real_home = is_account_home(&home);
        Ok(Self {
            home,
            state,
            mac: cfg!(target_os = "macos"),
            real_home,
            short_sockets: real_home.then(crate::terminal::short_socket_dir),
            projects,
        })
    }

    fn folder(&self) -> PathBuf {
        self.home.join(SANDHILL_DIR)
    }

    fn record(&self) -> PathBuf {
        self.state.join(record::FILE)
    }

    fn overlaps_project(&self, path: &Path) -> bool {
        let real = path.canonicalize().ok();
        self.projects.iter().any(|project| {
            let check = |path: &Path| {
                project.starts_with(path)
                    || (project != &self.home && path.starts_with(project))
                    || path.starts_with(project.join(".sandhill"))
            };
            check(path) || real.as_deref().is_some_and(check)
        })
    }

    fn old_places(&self) -> Vec<(PathBuf, bool)> {
        let mut places: Vec<_> = OLD_DAEMON_LAYOUT
            .iter()
            .map(|(old, new)| {
                let path = self.home.join(old);
                let in_use = path == self.state || !self.home.join(new).exists();
                (path, in_use)
            })
            .collect();
        if !self.state.starts_with(self.folder())
            && !places.iter().any(|(path, _)| *path == self.state)
        {
            places.push((self.state.clone(), true));
        }
        let app = self.home.join(sandhill_proto::APP_PATH);
        if self.mac {
            places.push((
                self.home.join("Library/Application Support/Sandhill"),
                !app.exists(),
            ));
            places.push((
                self.home.join("Library/Caches/Sandhill"),
                !self.home.join(sandhill_proto::CACHE_PATH).exists(),
            ));
        } else {
            for (key, base, dir) in [
                ("XDG_CONFIG_HOME", ".config", "sandhill"),
                ("XDG_DATA_HOME", ".local/share", "dev.sandhill.ide"),
                ("XDG_CACHE_HOME", ".cache", "sandhill"),
            ] {
                let base = std::env::var_os(key)
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .filter(|path| self.real_home || path.starts_with(&self.home))
                    .unwrap_or_else(|| self.home.join(base));
                places.push((base.join(dir), !app.exists()));
            }
        }
        places.push((self.home.join(".local/share/sandhill"), false));
        places
    }

    fn old_programs(&self) -> impl Iterator<Item = (&'static str, String)> {
        legacy_components().chain(component_paths().into_iter().filter_map(|(id, path)| {
            matches!(id, "launchd" | "systemd").then_some((id, path.to_string()))
        }))
    }

    fn footprint(&self) -> Result<Footprint> {
        let record = self.record();
        let (rows, gone) = record::snapshot(&record)?;
        let mut items = Vec::new();
        let mut bytes = 0u64;
        for path in children(&self.folder())? {
            let what = match path.file_name().and_then(|name| name.to_str()) {
                Some("state") => "History, outputs and Sandhill's working files",
                Some("bin") => "Sandhill's programs",
                Some("install") => "Record of Sandhill's own files",
                Some("accounts") => "Accounts added in Sandhill",
                Some("app") => "Sandhill's settings and saved devices",
                Some("cache") => "Saved previews",
                Some("environment") => "Sandhill's own R and Python",
                _ => "Other files in Sandhill's folder",
            };
            if let Some((item, size)) = disk_item(&path, what, FootprintKind::Folder, "")? {
                items.push(item);
                bytes = bytes.saturating_add(size);
            }
        }
        for (path, in_use) in self.old_places() {
            // The old share folder's contents are listed separately below or above.
            if path == self.home.join(".local/share/sandhill") {
                continue;
            }
            if let Some((item, size)) = disk_item(
                &path,
                "Old Sandhill folder",
                FootprintKind::OldPlace,
                if in_use { "Still in use" } else { "Left over" },
            )? {
                items.push(item);
                bytes = bytes.saturating_add(size);
            }
        }
        for (id, relative) in self.old_programs() {
            let path = self.home.join(&relative);
            if let Some((item, size)) = disk_item(
                &path,
                if matches!(id, "launchd" | "systemd") {
                    "Old Sandhill startup file"
                } else {
                    "Old Sandhill program"
                },
                FootprintKind::OldPlace,
                "Left over",
            )? {
                if !items.iter().any(|parent| path.starts_with(&parent.path)) {
                    bytes = bytes.saturating_add(size);
                }
                items.push(item);
            }
        }
        if self.mac {
            for &(relative, what) in MAC_PLACES {
                if let Some((item, size)) =
                    disk_item(&self.home.join(relative), what, FootprintKind::System, "")?
                {
                    items.push(item);
                    bytes = bytes.saturating_add(size);
                }
            }
        }
        if let Some(path) = &self.short_sockets
            && owned_socket_dir(path)
            && let Some((item, size)) = disk_item(
                path,
                "Temporary connections to Sandhill's terminals",
                FootprintKind::System,
                "",
            )?
        {
            items.push(item);
            bytes = bytes.saturating_add(size);
        }
        if self.real_home && self.mac {
            match keychain_accounts() {
                Ok(accounts) if !accounts.is_empty() => items.push(keychain_item(KEYCHAIN, "")),
                Err(err) => items.push(keychain_item(
                    KEYCHAIN,
                    &format!("Couldn't check the Keychain: {err}"),
                )),
                _ => {}
            }
        }
        if let Some(item) = self.antigravity_item()? {
            items.push(item);
        }
        let mut installed = Vec::new();
        let mut agents = Vec::new();
        for row in rows
            .into_iter()
            .filter(|row| !gone.contains(&record::key(row)))
        {
            match row.kind.as_str() {
                record::AGENT => agents.push(row),
                record::AGENT_FOLDER => {
                    let note = if row.ours {
                        "Sandhill made this folder for an extra account."
                    } else {
                        "No longer empty or changed since Sandhill made it."
                    };
                    if let Some((item, size)) = disk_item(
                        Path::new(&row.place),
                        "Shared agent conversation folder",
                        FootprintKind::AgentSetting,
                        note,
                    )? {
                        items.push(item);
                        bytes = bytes.saturating_add(size);
                    }
                    installed.push(row);
                }
                _ => installed.push(row),
            }
        }
        Ok(Footprint {
            folder: display(&self.folder()),
            size_mb: mb(bytes),
            items,
            installed,
            agents,
        })
    }

    fn antigravity_item(&self) -> Result<Option<FootprintItem>> {
        if !path_exists(&antigravity_manifest(&self.home))? {
            return Ok(None);
        }
        let mut item = item(
            &self.home.join(AGY_CONFIG),
            "Sandhill's tool entry in Antigravity",
            FootprintKind::AgentSetting,
            "",
        );
        match antigravity_entry(&self.home) {
            Ok(Some(entry)) => {
                item.size_mb = Some(mb(serde_json::to_vec(&entry)?.len() as u64));
                if antigravity_owned(&self.home, &entry).is_err() {
                    item.note = CHANGED.into();
                }
            }
            Ok(None) => return Ok(None),
            Err(err) => item.note = format!("Couldn't check Antigravity's settings: {err:#}"),
        }
        Ok(Some(item))
    }

    fn work(&self, choice: UninstallChoice) -> Result<Work> {
        let mut work = Work::new(choice);
        work.plan.keep.push(FootprintItem {
            path: String::new(),
            what: "Projects, their .sandhill folders, data and results".into(),
            kind: FootprintKind::Folder,
            size_mb: None,
            note: "Sandhill never removes these.".into(),
        });
        work.plan
            .notes
            .push("Claude Code and Codex sign-ins and settings stay.".into());
        let folder = self.folder();
        if self.overlaps_project(&folder) {
            work.plan.refused = Some(format!(
                "{} overlaps a project. Sandhill won't delete it.",
                folder.display()
            ));
            return Ok(work);
        }
        if path_exists(&folder)? && !fs::symlink_metadata(&folder)?.is_dir() {
            work.plan.refused = Some(format!(
                "{} isn't a regular folder. Sandhill won't delete it.",
                folder.display()
            ));
            return Ok(work);
        }
        let record = self.record();
        let (rows, gone) = record::snapshot(&record)?;
        self.record_actions(&mut work, &rows, &gone)?;
        if let Some(item) = self.antigravity_item()? {
            if item.note.is_empty() {
                let step = format!("Remove Sandhill's entry from {}", shell_quote(&item.path));
                work.add(vec![item], Operation::Antigravity, step);
            } else {
                work.failure(item);
            }
        }
        if self.mac && self.real_home {
            match keychain_accounts() {
                Ok(accounts) => {
                    for id in accounts {
                        let item = keychain_item(&format!("{KEYCHAIN} ({id})"), "");
                        let command = format!(
                            "/usr/bin/security delete-generic-password -s dev.sandhill.agent-account -a {}",
                            shell_quote(&id)
                        );
                        work.add(vec![item], Operation::Keychain(id), command);
                    }
                }
                Err(err) => work.failure(keychain_item(
                    KEYCHAIN,
                    &format!("Couldn't check the Keychain: {err}"),
                )),
            }
            let command = "/usr/bin/defaults delete dev.sandhill.ide";
            work.defer(
                item(
                    &self.home.join(MAC_PLACES[1].0),
                    "Sandhill's macOS preferences",
                    FootprintKind::System,
                    "",
                ),
                finisher::Task::Defaults,
                command.into(),
            );
        }
        if self.mac {
            for &(relative, what) in MAC_PLACES {
                self.defer_path(
                    &mut work,
                    &self.home.join(relative),
                    what,
                    FootprintKind::System,
                    false,
                )?;
            }
        }
        if let Some(path) = &self.short_sockets
            && owned_socket_dir(path)
        {
            use std::os::unix::fs::FileTypeExt as _;
            for terminal in children(&self.state.join("terminals"))? {
                let socket = path.join(terminal.file_name().context("Missing terminal name.")?);
                if socket.symlink_metadata().is_ok_and(|meta| {
                    meta.file_type().is_socket() && meta.uid() == unsafe { libc::getuid() }
                }) {
                    self.defer_path(
                        &mut work,
                        &socket,
                        "A temporary terminal connection",
                        FootprintKind::System,
                        false,
                    )?;
                }
            }
            self.defer_path(
                &mut work,
                path,
                "Temporary connections to Sandhill's terminals",
                FootprintKind::System,
                true,
            )?;
        }
        self.old_actions(&mut work)?;
        if work.plan.choice.keep_history {
            let state = self.home.join(STATE_PATH);
            let old_state = self.home.join(crate::paths::legacy_path(STATE_PATH));
            for history in [&state, &old_state] {
                if !path_exists(history)? {
                    continue;
                }
                let note = if history == &state {
                    "Your history stays in ~/.sandhill/state. Delete that folder to remove it later."
                } else {
                    "Your earlier history is in this folder. Delete it to remove that history later."
                };
                work.plan.keep.push(item(
                    history,
                    "Your Sandhill history and outputs",
                    FootprintKind::Folder,
                    note,
                ));
                for name in STATE_CLEANUP {
                    self.defer_path(
                        &mut work,
                        &history.join(name),
                        "Working files that aren't history",
                        FootprintKind::Folder,
                        false,
                    )?;
                }
                for path in children(&history.join("kernels"))? {
                    if path.extension().is_some_and(|ext| ext == "log") {
                        self.defer_path(
                            &mut work,
                            &path,
                            "Kernel log",
                            FootprintKind::Folder,
                            false,
                        )?;
                    }
                }
            }
            for path in children(&folder)? {
                if path != state && path != self.state {
                    self.defer_path(
                        &mut work,
                        &path,
                        "Files in Sandhill's folder",
                        FootprintKind::Folder,
                        false,
                    )?;
                }
            }
            if !state.exists() && !self.state.starts_with(&folder) {
                self.defer_path(
                    &mut work,
                    &folder,
                    "Sandhill's empty folder",
                    FootprintKind::Folder,
                    true,
                )?;
            }
        } else {
            self.defer_path(
                &mut work,
                &folder,
                "Sandhill's folder, history and outputs",
                FootprintKind::Folder,
                false,
            )?;
        }
        Ok(work)
    }

    fn record_actions(
        &self,
        work: &mut Work,
        rows: &[QuickInstalled],
        gone: &HashSet<InstalledKey>,
    ) -> Result<()> {
        let selected: HashSet<_> = work.plan.choice.installed.iter().cloned().collect();
        let agents: HashSet<_> = work.plan.choice.agents.iter().cloned().collect();
        let managed = self.folder();
        let mut chosen = Vec::new();
        let mut protected = Vec::new();
        for row in rows {
            let id = record::key(row);
            if gone.contains(&id) {
                continue;
            }
            let at = Path::new(&row.place);
            if row.kind == record::AGENT_FOLDER {
                let mut item = item(
                    at,
                    "Shared agent conversation folder",
                    FootprintKind::AgentSetting,
                    "",
                );
                let folder_name = match row.name.as_str() {
                    "claude" => "projects",
                    "codex" => "sessions",
                    _ => "",
                };
                if row.ours
                    && at.file_name().is_some_and(|name| name == folder_name)
                    && !self.overlaps_project(at)
                {
                    let command = format!("/bin/rmdir -- {}", shell_quote(&row.place));
                    work.add(
                        vec![item],
                        Operation::EmptyAgent {
                            path: at.to_path_buf(),
                            identity: row.version.clone(),
                        },
                        command,
                    );
                } else {
                    item.note = "This folder has changed or holds conversations. Sandhill will not remove it.".into();
                    work.plan.keep.push(item);
                }
                continue;
            }
            // Removing Sandhill's folder already removes its own environment and packages.
            if at.starts_with(&managed) || at.starts_with(&self.state) {
                continue;
            }
            let own = row.kind == record::OWN_ENVIRONMENT;
            let picked = if row.kind == record::AGENT {
                agents.contains(&row.name)
            } else {
                own || selected.contains(&id)
            };
            let item = record_item(&self.home, row);
            if !picked
                || !row.ours
                || row.kind == record::RENV_LIBRARY
                || self.overlaps_project(Path::new(&item.path))
            {
                let reason = if !picked {
                    "Not selected."
                } else if row.kind == record::RENV_LIBRARY
                    || self.overlaps_project(Path::new(&item.path))
                {
                    "This belongs to a project."
                } else {
                    CHANGED
                };
                protected.push(row);
                work.keep(item, reason);
            } else {
                chosen.push(row);
            }
        }
        // Removing an entire environment must not remove an unticked or changed record item.
        chosen.retain(|item| {
            let safe = !(record::is_container(item) || item.kind == record::OWN_ENVIRONMENT)
                || !protected
                    .iter()
                    .any(|other| Path::new(&other.place).starts_with(&item.place));
            if !safe {
                work.keep(
                    record_item(&self.home, item),
                    "This folder contains something that will stay.",
                );
            }
            safe
        });
        let containers: Vec<_> = chosen
            .iter()
            .copied()
            .filter(|row| record::is_container(row) || row.kind == record::OWN_ENVIRONMENT)
            .collect();
        chosen.retain(|row| {
            !containers.iter().any(|root| {
                !std::ptr::eq(*row, *root) && Path::new(&row.place).starts_with(&root.place)
            })
        });
        chosen.retain(|row| {
            if row.kind != record::OWN_ENVIRONMENT {
                return true;
            }
            let item = record_item(&self.home, row);
            let command = format!("/bin/rm -rf -- {}", shell_quote(&row.place));
            work.add(
                vec![item],
                Operation::OwnEnvironment(PathBuf::from(&row.place)),
                command,
            );
            false
        });
        let (groups, skipped) = record::removals(&chosen, &Search::from_env());
        for (row, reason) in skipped {
            if reason == "Other packages there need it." {
                work.keep(record_item(&self.home, row), reason);
            } else {
                let mut item = record_item(&self.home, row);
                item.note = reason.into();
                work.failure(item);
            }
        }
        for record::Removal { items, command } in groups {
            let keys = items.iter().map(|row| record::key(row)).collect();
            let items = items
                .iter()
                .map(|row| record_item(&self.home, row))
                .collect();
            work.add(
                items,
                Operation::Records {
                    keys,
                    command: command.clone(),
                },
                command,
            );
        }
        Ok(())
    }

    fn defer_path(
        &self,
        work: &mut Work,
        path: &Path,
        what: &str,
        kind: FootprintKind,
        empty: bool,
    ) -> Result<()> {
        if !path_exists(path)? {
            return Ok(());
        }
        let mut item = item(path, what, kind, "");
        if self.overlaps_project(path) {
            item.note = "This overlaps a project. Sandhill didn't remove it.".into();
            work.failure(item);
            return Ok(());
        }
        match finisher::File::new(path, empty) {
            Ok(file) => {
                let command = file.command();
                work.defer(item, finisher::Task::Remove(file), command);
            }
            Err(err) => {
                item.note = format!("Couldn't safely remove this path: {err:#}");
                work.failure(item);
            }
        }
        Ok(())
    }

    fn old_actions(&self, work: &mut Work) -> Result<()> {
        for (id, relative) in self.old_programs() {
            let path = self.home.join(&relative);
            if !path_exists(&path)? {
                continue;
            }
            let mut item = item(
                &path,
                "Old Sandhill startup file or program",
                FootprintKind::OldPlace,
                "",
            );
            let verified = components::component_manifest(&self.home, id, &relative)
                .context("No matching record for this file.")
                .and_then(|manifest| {
                    let digest = components::owned_component(&self.home, id, &relative, &manifest)?;
                    Ok((digest, manifest))
                });
            let (file, manifest) = match (verified, finisher::File::new(&path, false)) {
                (Ok((digest, manifest)), Ok(file)) if !self.overlaps_project(&path) => {
                    (file.with_checksum(digest), manifest)
                }
                (Err(err), _) | (_, Err(err)) => {
                    item.note = format!("Couldn't verify that this file is Sandhill's: {err:#}");
                    work.failure(item);
                    continue;
                }
                _ => {
                    item.note = "This path overlaps a project.".into();
                    work.failure(item);
                    continue;
                }
            };
            let mut command = file.command();
            let service = if self.real_home {
                finisher::Service::for_platform(id)
            } else {
                None
            };
            if let Some(service) = service {
                command = format!("{}\n{command}", service.command());
            }
            work.defer(item, finisher::Task::Component { file, service }, command);
            if !manifest.starts_with(self.folder()) {
                self.defer_path(
                    work,
                    &manifest,
                    "Record for an old Sandhill file",
                    FootprintKind::OldPlace,
                    false,
                )?;
            }
        }
        for (path, _) in self.old_places() {
            if path == self.home.join(crate::paths::legacy_path(STATE_PATH)) {
                if !work.plan.choice.keep_history {
                    self.defer_path(
                        work,
                        &path,
                        "Sandhill's old state folder",
                        FootprintKind::OldPlace,
                        false,
                    )?;
                }
                continue;
            }
            if path == self.state {
                let mut item = item(&path, "Custom state folder", FootprintKind::OldPlace, "");
                item.note =
                    "Sandhill can't verify what else is in this custom folder. It stays.".into();
                work.failure(item);
                continue;
            }
            if !OLD_DAEMON_LAYOUT
                .iter()
                .any(|(old, _)| self.home.join(old) == path)
                && path != self.home.join(".local/share/sandhill")
            {
                for name in ["gpui.json", "gpui-windows.json", "servers.json", "media"] {
                    self.defer_path(
                        work,
                        &path.join(name),
                        "Old Sandhill settings or cache",
                        FootprintKind::OldPlace,
                        false,
                    )?;
                }
            }
            // Folder containing old account symlinks is Sandhill's named account folder.
            let old_accounts = self
                .home
                .join(crate::paths::legacy_path(sandhill_proto::ACCOUNTS_PATH));
            let empty_only = path != old_accounts;
            self.defer_path(
                work,
                &path,
                "Old Sandhill folder",
                FootprintKind::OldPlace,
                empty_only,
            )?;
        }
        Ok(())
    }
}

struct Work {
    plan: UninstallPlan,
    now: Vec<(Vec<FootprintItem>, Operation)>,
    later: Vec<(String, finisher::Task)>,
    failures: Vec<FootprintItem>,
}

enum Operation {
    Records {
        keys: Vec<InstalledKey>,
        command: String,
    },
    Antigravity,
    EmptyAgent {
        path: PathBuf,
        identity: String,
    },
    Keychain(String),
    OwnEnvironment(PathBuf),
}

impl Work {
    fn new(choice: UninstallChoice) -> Self {
        Self {
            plan: UninstallPlan {
                choice,
                remove: Vec::new(),
                keep: Vec::new(),
                steps: Vec::new(),
                notes: Vec::new(),
                refused: None,
            },
            now: Vec::new(),
            later: Vec::new(),
            failures: Vec::new(),
        }
    }

    fn keep(&mut self, mut item: FootprintItem, note: &str) {
        item.note = note.into();
        self.plan.keep.push(item);
    }

    fn failure(&mut self, item: FootprintItem) {
        self.plan.keep.push(item.clone());
        self.failures.push(item);
    }

    fn add(&mut self, items: Vec<FootprintItem>, operation: Operation, command: String) {
        self.plan.steps.push(SetupStep {
            what: format!(
                "Remove {}",
                items
                    .iter()
                    .map(|item| item.what.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            command,
        });
        self.plan.remove.extend(items.iter().cloned());
        self.now.push((items, operation));
    }

    fn defer(&mut self, item: FootprintItem, task: finisher::Task, command: String) {
        self.plan.steps.push(SetupStep {
            what: format!("Remove {} once Sandhill closes", item.what),
            command,
        });
        self.later.push((item.path.clone(), task));
        if !self.plan.remove.iter().any(|entry| entry.path == item.path) {
            self.plan.remove.push(item);
        }
    }
}

impl Daemon {
    fn uninstall_projects(&self) -> Result<Vec<PathBuf>> {
        let mut projects: HashSet<_> = self
            .journal
            .recent_projects()?
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        projects.extend(
            self.journal
                .sessions()?
                .into_iter()
                .filter_map(|session| session.project),
        );
        projects.extend(
            self.terminals
                .list()
                .into_iter()
                .filter_map(|terminal| terminal.project),
        );
        Ok(projects.into_iter().map(PathBuf::from).collect())
    }

    pub(crate) async fn footprint(&self) -> Result<Footprint> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || Device::new(&state, Vec::new())?.footprint()).await?
    }

    pub(crate) async fn plan_uninstall(&self, choice: UninstallChoice) -> Result<UninstallPlan> {
        let state = self.state.clone();
        let projects = self.uninstall_projects()?;
        tokio::task::spawn_blocking(move || Ok(Device::new(&state, projects)?.work(choice)?.plan))
            .await?
    }

    pub(crate) async fn run_uninstall(
        &self,
        choice: UninstallChoice,
        app_pid: Option<u32>,
    ) -> Result<UninstallReport> {
        let app_pid = finisher::app_pid(app_pid)?;
        let stopped = self.stop_resources(false).await?;
        if stopped["stopped"] != true {
            let details = stopped["remaining"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["id"].as_str())
                .chain(
                    stopped["errors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                )
                .collect::<Vec<_>>()
                .join("; ");
            bail!("Sandhill couldn't stop all work on this device: {details}");
        }
        let state = self.state.clone();
        let projects = self.uninstall_projects()?;
        let (report, finishing) = tokio::task::spawn_blocking(move || {
            Device::new(&state, projects)?.run(choice, app_pid)
        })
        .await??;
        if finishing {
            self.stopping.store(true, Ordering::Release);
        }
        Ok(report)
    }
}

impl Device {
    fn run(
        &self,
        choice: UninstallChoice,
        app_pid: Option<u32>,
    ) -> Result<(UninstallReport, bool)> {
        let record = self.record();
        let _record = record::lock(&record, false)
            .context("An install or removal is still using Sandhill's record.")?;
        let work = self.work(choice)?;
        if let Some(reason) = work.plan.refused {
            bail!("{reason}");
        }
        let mut report = UninstallReport {
            success: false,
            removed: Vec::new(),
            left: work.failures,
            finishing: Vec::new(),
        };
        let mut attempted_records = Vec::new();
        for (mut items, op) in work.now {
            let result = match op {
                Operation::Records { keys, command } => {
                    let result = install::run_command(&command);
                    attempted_records.push((items, keys, result));
                    continue;
                }
                Operation::Antigravity => remove_antigravity_mcp_at(&self.home),
                Operation::EmptyAgent { path, identity } => (|| {
                    if !path_exists(&path)? {
                        return Ok(());
                    }
                    if record::empty_agent_folder_identity(&path).as_deref()
                        != Some(identity.as_str())
                    {
                        bail!("The folder changed or isn't empty.");
                    }
                    Ok(fs::remove_dir(path)?)
                })(),
                Operation::Keychain(id) => delete_keychain_account(&id),
                Operation::OwnEnvironment(path) => crate::quick::own::remove(&record, &path),
            };
            match result {
                Ok(()) => report
                    .removed
                    .extend(items.into_iter().map(|item| item.path)),
                Err(err) => {
                    for item in &mut items {
                        item.note = format!("Couldn't remove it: {err:#}");
                    }
                    report.left.extend(items);
                }
            }
        }
        if !attempted_records.is_empty() {
            let check = record::snapshot(&record);
            for (items, keys, result) in attempted_records {
                for (mut item, key) in items.into_iter().zip(keys) {
                    match &check {
                        Ok((_, gone)) if gone.contains(&key) => {
                            report.removed.push(item.path);
                            continue;
                        }
                        Err(err) => {
                            item.note = format!("Couldn't check whether it was removed: {err:#}")
                        }
                        Ok(_) => {
                            item.note = result.as_ref().err().map_or_else(
                                || "The item is still there after the removal command.".into(),
                                |err| format!("The removal command failed: {err:#}"),
                            )
                        }
                    }
                    report.left.push(item);
                }
            }
        }
        if let Err(err) = finisher::start(&self.home, self.mac, app_pid, &work.later) {
            report.left.push(item(
                &self.folder(),
                "Files to remove after Sandhill closes",
                FootprintKind::Folder,
                &format!("Couldn't start the final removal: {err:#}"),
            ));
            return Ok((report, false));
        }
        for (path, _) in work.later {
            if !report.finishing.contains(&path) {
                report.finishing.push(path);
            }
        }
        report.success = report.left.is_empty();
        Ok((report, true))
    }
}

fn is_account_home(home: &Path) -> bool {
    // HOME may be an isolated test home. Keychain, defaults and service
    // managers use the login account regardless of that environment setting.
    let mut buffer = vec![0u8; 64 * 1024];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found = std::ptr::null_mut();
    // SAFETY: all output parameters point to writable buffers of the given size.
    unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            pwd.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        ) == 0
            && !found.is_null()
            && !(*found).pw_dir.is_null()
            && Path::new(std::ffi::OsStr::from_bytes(
                CStr::from_ptr((*found).pw_dir).to_bytes(),
            ))
            .canonicalize()
            .is_ok_and(|login| login == home)
    }
}

fn owned_socket_dir(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|meta| {
        meta.is_dir() && meta.uid() == unsafe { libc::getuid() } && meta.mode() & 0o077 == 0
    })
}

fn children(path: &Path) -> Result<Vec<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => bail!("{} isn't a regular folder.", path.display()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    }
    // Don't enumerate through a parent symlink.
    SafeDir::open(path)?;
    let mut items = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    items.sort();
    Ok(items)
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn mb(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

fn item(path: &Path, what: &str, kind: FootprintKind, note: &str) -> FootprintItem {
    FootprintItem {
        path: display(path),
        what: what.into(),
        kind,
        size_mb: None,
        note: note.into(),
    }
}

fn disk_item(
    path: &Path,
    what: &str,
    kind: FootprintKind,
    note: &str,
) -> Result<Option<(FootprintItem, u64)>> {
    let Some(storage) = storage_entry("footprint", "", path, false)? else {
        return Ok(None);
    };
    let bytes = storage["bytes"].as_u64().unwrap_or(0);
    let mut item = item(path, what, kind, note);
    if let Some(reason) = storage["reason"]
        .as_str()
        .filter(|reason| reason.starts_with("Not safely readable"))
    {
        if !item.note.is_empty() {
            item.note.push_str(". ");
        }
        item.note.push_str(reason);
    } else {
        item.size_mb = Some(mb(bytes));
    }
    Ok(Some((item, bytes)))
}

fn record_item(home: &Path, row: &QuickInstalled) -> FootprintItem {
    match (row.kind.as_str(), row.name.as_str(), row.place.as_str()) {
        (record::AGENT, "claude", _) => item(
            &home.join(".local/bin/claude"),
            "Claude Code",
            FootprintKind::AgentSetting,
            "",
        ),
        (record::AGENT, "codex", _) => {
            let mut item = item(Path::new("Codex"), "Codex", FootprintKind::AgentSetting, "");
            if let Some(path) = crate::agents::find("codex") {
                item.path = display(&path);
            }
            item
        }
        (record::R_PACKAGE, name, place) => item(
            &Path::new(place).join(name),
            name,
            FootprintKind::Folder,
            "",
        ),
        (_, name, place) => item(Path::new(place), name, FootprintKind::Folder, ""),
    }
}

fn keychain_item(path: &str, note: &str) -> FootprintItem {
    item(
        Path::new(path),
        "API key saved by Sandhill",
        FootprintKind::System,
        note,
    )
}

#[cfg(target_os = "macos")]
fn keychain_accounts() -> Result<Vec<String>> {
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    let hits = match ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(crate::accounts::KEY_SERVICE)
        .load_attributes(true)
        .limit(Limit::All)
        .search()
    {
        Err(err) if err.code() == -25300 => return Ok(Vec::new()),
        result => result?,
    };
    // Search attributes only. No passwords or secret data leave the Keychain.
    let mut ids = Vec::new();
    for hit in hits {
        let id = hit
            .simplify_dict()
            .and_then(|mut attrs| attrs.remove("acct"))
            .context("Couldn't identify a Sandhill Keychain entry.")?;
        ids.push(id);
    }
    ids.sort();
    Ok(ids)
}

#[cfg(not(target_os = "macos"))]
fn keychain_accounts() -> Result<Vec<String>> {
    Ok(Vec::new())
}

fn delete_keychain_account(id: &str) -> Result<()> {
    let output = Command::new("/usr/bin/security")
        .args([
            "delete-generic-password",
            "-s",
            "dev.sandhill.agent-account",
            "-a",
            id,
        ])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(home: &Path) -> Device {
        let home = home.canonicalize().unwrap();
        let state = home.join(STATE_PATH);
        fs::create_dir_all(&state).unwrap();
        Device {
            short_sockets: Some(home.join("socket-folder")),
            home,
            state,
            mac: true,
            real_home: false,
            projects: Vec::new(),
        }
    }

    fn write(path: &Path, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn footprint_lists_each_kind_and_only_recorded_agent_folders() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let device = fixture(tmp.path());
        let at = |relative: &str| device.home.join(relative);
        let data = vec![0; 2 * 1024 * 1024];
        write(&device.state.join("journal.sqlite"), data);
        for area in ["bin", "install", "accounts", "app", "cache", "environment"] {
            write(&device.folder().join(area).join("example"), b"x");
        }
        for &(path, _) in MAC_PLACES {
            write(&at(path), b"system");
        }
        let sockets = device.short_sockets.as_ref().unwrap();
        fs::create_dir(sockets).unwrap();
        fs::set_permissions(sockets, fs::Permissions::from_mode(0o700)).unwrap();
        let old = OLD_DAEMON_LAYOUT[0].0;
        write(&at(old).join("journal.sqlite"), b"earlier");
        write(&at(sandhill_proto::OLD_DAEMON_PATH), b"older daemon");
        let startup = "Library/LaunchAgents/dev.sandhill.sandhilld.plist";
        write(&at(startup), b"old service");
        let entry = json!({"command":"/Sandhill/sandhilld","args":["mcp"]});
        write(
            &at(AGY_CONFIG),
            json!({"mcpServers":{"sandhill":entry}}).to_string(),
        );
        write(
            &antigravity_manifest(&device.home),
            json!({"version":1,"component":"antigravity-mcp","path":AGY_CONFIG,"entry":entry})
                .to_string(),
        );
        let projects = at(".claude/projects");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(at(".codex/sessions")).unwrap();
        record::record_agent_folder(&device.record(), "claude", &projects).unwrap();

        let footprint = device.footprint().unwrap();
        let check = |path: &Path, kind| {
            let row = footprint
                .items
                .iter()
                .find(|row| row.path == display(path))
                .unwrap();
            assert_eq!(row.kind, kind);
            assert!(row.size_mb.is_some());
        };
        check(&device.state, FootprintKind::Folder);
        check(&at(old), FootprintKind::OldPlace);
        check(
            &at(sandhill_proto::OLD_DAEMON_PATH),
            FootprintKind::OldPlace,
        );
        check(&at(startup), FootprintKind::OldPlace);
        check(sockets, FootprintKind::System);
        check(&at(MAC_PLACES[0].0), FootprintKind::System);
        check(&at(AGY_CONFIG), FootprintKind::AgentSetting);
        check(&projects, FootprintKind::AgentSetting);
        assert!(
            !footprint
                .items
                .iter()
                .any(|row| row.path.ends_with(".codex/sessions"))
        );
        assert!(footprint.size_mb >= 2);
        assert_eq!(footprint.installed[0].kind, record::AGENT_FOLDER);
    }

    #[test]
    fn uninstall_plan_respects_history_choices_and_preserves_changed_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let device = fixture(tmp.path());
        for path in [
            "journal.sqlite",
            "undo/one",
            "package-index/index",
            "log/job",
            "cache/item",
        ] {
            write(&device.state.join(path), b"Sandhill");
        }
        write(&device.folder().join("environment/R"), b"R");
        let library = device.home.join("R library");
        for (name, current) in [("selected", "1"), ("unticked", "1"), ("changed", "2")] {
            write(
                &library.join(name).join("DESCRIPTION"),
                format!("Package: {name}\nVersion: {current}\n"),
            );
        }
        write(
            &device.record(),
            ["selected", "unticked", "changed"]
                .map(|name| {
                    format!(
                        "r-package\t{name}\t1\t{}\t2026-11-20\tr\n",
                        library.display()
                    )
                })
                .concat(),
        );
        let key = |name: &str| InstalledKey {
            name: name.into(),
            kind: record::R_PACKAGE.into(),
            place: display(&library),
        };
        let choice = UninstallChoice {
            installed: vec![key("selected"), key("changed")],
            agents: Vec::new(),
            keep_history: true,
        };
        let plan = device.work(choice.clone()).unwrap().plan;
        let removed = |path: &Path| plan.remove.iter().any(|row| row.path == display(path));
        assert!(removed(&library.join("selected")));
        assert!(removed(&device.state.join("undo")));
        assert!(removed(&device.state.join("package-index")));
        assert!(removed(&device.state.join("log")));
        assert!(removed(&device.state.join("cache")));
        assert!(removed(&device.folder().join("environment")));
        assert!(!removed(&device.state));
        assert!(!removed(&device.state.join("journal.sqlite")));
        assert!(plan.keep.iter().any(
            |row| row.path == display(&library.join("unticked")) && row.note == "Not selected."
        ));
        assert!(
            plan.keep
                .iter()
                .any(|row| row.path == display(&library.join("changed")) && row.note == CHANGED)
        );
        assert!(plan.keep.iter().any(
            |row| row.path == display(&device.state) && row.note.contains("~/.sandhill/state")
        ));
        assert!(
            plan.steps
                .iter()
                .any(|step| step.command.contains("R library/selected"))
        );

        let plan = device
            .work(UninstallChoice {
                keep_history: false,
                ..choice
            })
            .unwrap()
            .plan;
        assert!(
            plan.remove
                .iter()
                .any(|row| row.path == display(&device.folder()))
        );
        assert!(
            !plan
                .keep
                .iter()
                .any(|row| row.path == display(&device.state))
        );
    }
}
