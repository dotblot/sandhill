//! Device-local account metadata. Agent logins are owned exclusively by their CLI.
//! Only Sandhill's own Gemini API keys are read here; no agent credential is opened.

use std::collections::BTreeMap;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use sandhill_proto::{AgentAccount, AgentOptions, TERMINAL_LOGIN, TerminalInfo};
use serde_json::{Value, json};

use crate::agents::{ANTIGRAVITY, CLAUDE, CODEX};
use crate::chats::Homes;
use crate::terminal::Launch;

const KINDS: [&str; 3] = [CLAUDE, CODEX, ANTIGRAVITY];
static LOCK: Mutex<()> = Mutex::new(());
#[cfg(target_os = "macos")]
pub(crate) const KEY_SERVICE: &str = "dev.sandhill.agent-account";

pub struct Registry {
    root: PathBuf,
    homes: Homes,
    record: Option<PathBuf>,
}

impl Registry {
    pub fn new(root: PathBuf, homes: Homes) -> Self {
        Self {
            root,
            homes,
            record: None,
        }
    }

    pub fn from_env() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        Self::new(
            crate::paths::installed_path(&home, sandhill_proto::ACCOUNTS_PATH, ""),
            Homes::from_env(),
        )
        .with_state(
            crate::paths::StateDir::default_location()
                .existing_location()
                .root(),
        )
    }

    pub(crate) fn with_state(mut self, state: &Path) -> Self {
        self.record = Some(state.join(crate::quick::record::FILE));
        self
    }

    fn folder(&self, id: &str) -> Result<PathBuf> {
        let (kind, slug) = id
            .split_once(':')
            .context("the default account has no Sandhill folder")?;
        if !KINDS.contains(&kind)
            || slug.is_empty()
            || !slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            bail!("invalid agent account id");
        }
        Ok(self.root.join(kind).join(slug))
    }

    fn metadata(&self) -> Result<Vec<AgentAccount>> {
        let mut accounts: Vec<_> = KINDS
            .iter()
            .map(|kind| AgentAccount {
                id: (*kind).into(),
                kind: (*kind).into(),
                name: "Default".into(),
                method: "login".into(),
                default: true,
                signed_in: None,
                identity: None,
                usage: None,
            })
            .collect();
        for kind in KINDS {
            let entries = match std::fs::read_dir(self.root.join(kind)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let file = entry.path().join("account.json");
                let account: AgentAccount = serde_json::from_slice(&std::fs::read(file)?)?;
                if account.default
                    || account.kind != kind
                    || self.folder(&account.id)? != entry.path()
                {
                    bail!("invalid account metadata");
                }
                accounts.push(account);
            }
        }
        let snapshots = self.snapshots()?;
        for account in &mut accounts {
            if let Some(snapshot) = snapshots.get(&account.id) {
                apply_snapshot(account, snapshot);
            }
            if account.method == "api_key" {
                account.usage = None;
            }
        }
        Ok(accounts)
    }

    pub fn account(&self, id: &str) -> Result<AgentAccount> {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        self.metadata()?
            .into_iter()
            .find(|a| a.id == id)
            .context("no such agent account")
    }

    pub fn add(&self, kind: &str, name: &str, api_key: Option<&str>) -> Result<AgentAccount> {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !KINDS.contains(&kind) {
            bail!("this agent does not support extra accounts");
        }
        let name = checked_name(name)?;
        if kind == ANTIGRAVITY {
            if api_key.is_none_or(|key| key.trim().is_empty()) {
                bail!("a Gemini API key is required");
            }
        } else if api_key.is_some() {
            bail!("this agent signs in through its own CLI");
        }
        let slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "account" } else { slug };
        let id = format!(
            "{kind}:{slug}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let folder = self.folder(&id)?;
        private_dir(&folder)?;
        let account = AgentAccount {
            id,
            kind: kind.into(),
            name: name.into(),
            method: if kind == ANTIGRAVITY {
                "api_key"
            } else {
                "login"
            }
            .into(),
            default: false,
            signed_in: (kind == ANTIGRAVITY).then_some(true),
            identity: None,
            usage: None,
        };
        let result = (|| {
            self.link_shared(&account)?;
            crate::files::write_atomic(
                &folder.join("account.json"),
                &serde_json::to_vec_pretty(&account)?,
            )?;
            if let Some(key) = api_key {
                self.store_key(&account.id, key)?;
            }
            Ok(account)
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(folder);
        }
        result
    }

    fn link_shared(&self, account: &AgentAccount) -> Result<()> {
        let (source, required, links): (&Path, &str, &[&str]) = match account.kind.as_str() {
            CLAUDE => (
                &self.homes.claude,
                "projects",
                &[
                    "projects",
                    "settings.json",
                    "CLAUDE.md",
                    "agents",
                    "commands",
                    "skills",
                    "plugins",
                    "todos",
                ],
            ),
            CODEX => (
                &self.homes.codex,
                "sessions",
                &[
                    "sessions",
                    "archived_sessions",
                    "config.toml",
                    "AGENTS.md",
                    "prompts",
                    "skills",
                ],
            ),
            _ => return Ok(()),
        };
        // Only make the shared conversation folder when it isn't there yet.
        let shared = source.join(required);
        std::fs::create_dir_all(source)?;
        match std::fs::create_dir(&shared) {
            Ok(()) => {
                if let Some(record) = &self.record
                    && let Err(err) =
                        crate::quick::record::record_agent_folder(record, &account.kind, &shared)
                {
                    let _ = std::fs::remove_dir(&shared);
                    return Err(err);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && shared.is_dir() => {}
            Err(err) => return Err(err.into()),
        }
        let destination = self.folder(&account.id)?;
        for name in links {
            let target = source.join(name);
            let link = destination.join(name);
            if target.exists() && std::fs::symlink_metadata(&link).is_err() {
                std::os::unix::fs::symlink(target, link)?;
            }
        }
        Ok(())
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<AgentAccount> {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut account = self
            .metadata()?
            .into_iter()
            .find(|a| a.id == id)
            .context("no such agent account")?;
        if account.default {
            bail!("the default account cannot be renamed");
        }
        account.name = checked_name(name)?.into();
        crate::files::write_atomic(
            &self.folder(id)?.join("account.json"),
            &serde_json::to_vec_pretty(&account)?,
        )?;
        Ok(account)
    }

    pub fn remove(&self, id: &str, terminals: &[TerminalInfo]) -> Result<()> {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let account = self
            .metadata()?
            .into_iter()
            .find(|a| a.id == id)
            .context("no such agent account")?;
        if account.default {
            bail!("the default account cannot be removed");
        }
        if terminals
            .iter()
            .any(|t| t.running && t.options.account.as_deref() == Some(id))
        {
            bail!("this account is in use by a running conversation or sign-in");
        }
        if account.method == "api_key" {
            self.delete_key(id)?;
        }
        // remove_dir_all does not follow symlinks: only our folder is removed.
        std::fs::remove_dir_all(self.folder(id)?)?;
        let _snapshot_lock = self.lock_snapshots()?;
        let mut snapshots = self.snapshots()?;
        snapshots.remove(id);
        self.save_snapshots(&snapshots)
    }

    pub fn environment(&self, kind: &str, id: Option<&str>) -> Result<Vec<(String, String)>> {
        if !KINDS.contains(&kind) {
            if id.is_some() {
                bail!("this agent does not support account selection");
            }
            return Ok(Vec::new());
        }
        let account = self.account(id.unwrap_or(kind))?;
        if account.kind != kind {
            bail!("the account belongs to another agent");
        }
        let mut env = vec![("SANDHILL_ACCOUNT_ID".into(), account.id.clone())];
        if !account.default {
            self.link_shared(&account)?;
            let folder = self.folder(&account.id)?.to_string_lossy().into_owned();
            match kind {
                CLAUDE => env.push(("CLAUDE_CONFIG_DIR".into(), folder)),
                CODEX => env.extend([
                    ("CODEX_HOME".into(), folder),
                    ("SANDHILL_CODEX_FILE_AUTH".into(), "1".into()),
                ]),
                ANTIGRAVITY => env.extend([
                    (
                        "GEMINI_API_KEY".into(),
                        self.key(&account.id)?
                            .context("this account has no API key")?,
                    ),
                    ("SANDHILL_AGY_API_KEY".into(), "1".into()),
                ]),
                _ => unreachable!(),
            }
        }
        Ok(env)
    }

    pub fn sign_in(&self, id: &str) -> Result<Launch> {
        let account = self.account(id)?;
        crate::agents::ensure_offered(&account.kind)?;
        if account.method == "api_key" {
            bail!("API-key accounts do not need sign-in");
        }
        let program = crate::agents::find(if account.kind == ANTIGRAVITY {
            "agy"
        } else {
            &account.kind
        })
        .context("the agent CLI is not installed")?;
        let mut env = self.environment(&account.kind, Some(id))?;
        if let Some(path) = crate::agents::path_for(&program)? {
            env.push(("PATH".into(), path));
        }
        let args = match account.kind.as_str() {
            // The full first-run screen includes theme/onboarding as well as the CLI's login.
            CLAUDE => Vec::new(),
            CODEX => {
                let mut args = Vec::new();
                if !account.default {
                    args.extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
                }
                args.push("login".into());
                if std::env::var_os("SSH_CONNECTION").is_some()
                    || std::env::var_os("SSH_CLIENT").is_some()
                    || std::env::var_os("SSH_TTY").is_some()
                {
                    args.push("--device-auth".into());
                }
                args
            }
            ANTIGRAVITY => Vec::new(),
            _ => unreachable!(),
        };
        Ok(Launch {
            kind: TERMINAL_LOGIN.into(),
            title: format!("Sign in — {}", account.name),
            program,
            args,
            env,
            cwd: PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into())),
            project: None,
            options: AgentOptions {
                account: Some(id.into()),
                ..AgentOptions::default()
            },
            size: None,
            job: None,
        })
    }

    /// With `refresh`, asks the agents about each account, or only about
    /// `ids`; the others keep what was last known.
    pub async fn list(&self, refresh: bool, ids: Option<&[String]>) -> Result<Vec<AgentAccount>> {
        let mut accounts = {
            let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            self.metadata()?
        };
        if refresh {
            for account in &mut accounts {
                // A paused agent is never asked: it keeps what was last known.
                if ids.is_some_and(|ids| !ids.contains(&account.id))
                    || sandhill_proto::agent_paused(&account.kind)
                {
                    continue;
                }
                if account.method == "api_key" {
                    if let Ok(key) = self.key(&account.id) {
                        account.signed_in = Some(key.is_some());
                        self.remember(&account.id, &json!({"signed_in": key.is_some()}))?;
                    }
                    continue;
                }
                if let Ok(snapshot) = self.query(account).await {
                    self.remember(&account.id, &snapshot)?;
                    apply_snapshot(account, &snapshot);
                }
            }
        }
        Ok(accounts)
    }

    async fn query(&self, account: &AgentAccount) -> Result<Value> {
        let folder = (!account.default)
            .then(|| self.folder(&account.id))
            .transpose()?;
        match account.kind.as_str() {
            CLAUDE => {
                let program = crate::agents::find("claude").context("Claude is not installed")?;
                let identity =
                    crate::agents::claude_account_in(&program, folder.as_deref()).await?;
                Ok(json!({"account": identity, "observed_at_ms": crate::journal::now_ms()}))
            }
            CODEX => {
                let program = crate::agents::find("codex").context("Codex is not installed")?;
                crate::terminal::codex::read_account_in(&program, folder.as_deref()).await
            }
            ANTIGRAVITY => {
                let program = crate::agents::find("agy").context("Antigravity is not installed")?;
                let mut report = crate::terminal::antigravity::usage::read_usage(&program, None)
                    .await?
                    .report;
                report["signed_in"] = json!(true);
                Ok(report)
            }
            _ => unreachable!(),
        }
    }

    /// Uses one of a Codex account's banked resets now (the one `credit`
    /// names, else Codex picks), then reads the account again. Returns
    /// `{outcome, account}`.
    pub async fn use_reset(&self, id: &str, credit: Option<&str>) -> Result<Value> {
        let mut account = self.account(id)?;
        if account.kind != CODEX {
            bail!("only Codex reports banked resets that Sandhill can use");
        }
        let folder = (!account.default)
            .then(|| self.folder(&account.id))
            .transpose()?;
        let program = crate::agents::find("codex").context("Codex is not installed")?;
        let outcome =
            crate::terminal::codex::use_reset_credit(&program, folder.as_deref(), credit).await?;
        if let Ok(snapshot) = self.query(&account).await {
            self.remember(&account.id, &snapshot)?;
            apply_snapshot(&mut account, &snapshot);
        }
        Ok(json!({"outcome": outcome, "account": account}))
    }

    fn snapshots(&self) -> Result<BTreeMap<String, Value>> {
        match std::fs::read(self.root.join("snapshots.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn save_snapshots(&self, snapshots: &BTreeMap<String, Value>) -> Result<()> {
        private_dir(&self.root)?;
        crate::files::write_atomic(
            &self.root.join("snapshots.json"),
            &serde_json::to_vec(snapshots)?,
        )?;
        Ok(())
    }

    fn lock_snapshots(&self) -> Result<std::fs::File> {
        use std::os::fd::AsRawFd;
        private_dir(&self.root)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.root.join(".lock"))?;
        // SAFETY: file owns a valid descriptor; flock is released when it closes.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(file)
    }

    pub fn remember(&self, id: &str, snapshot: &Value) -> Result<()> {
        let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _snapshot_lock = self.lock_snapshots()?;
        let mut snapshots = self.snapshots()?;
        let previous = snapshots.entry(id.into()).or_insert_with(|| json!({}));
        let identity = snapshot
            .get("account")
            .or_else(|| snapshot["status"].get("quota_account"));
        if let Some(signed_in) = snapshot.get("signed_in") {
            previous["signed_in"] = signed_in.clone();
        }
        if let Some(identity) = identity {
            previous["account"] = identity.clone();
        }
        if snapshot["status"]["rate_limits"].is_object() {
            let observed = snapshot["status"]
                .get("quota_observed_at_ms")
                .or_else(|| snapshot.get("observed_at_ms"))
                .and_then(Value::as_i64)
                .unwrap_or_else(crate::journal::now_ms);
            if previous["observed_at_ms"]
                .as_i64()
                .is_none_or(|old| observed >= old)
            {
                previous["status"] = json!({"rate_limits": snapshot["status"]["rate_limits"]});
                if let Some(credits) = snapshot["status"].get("reset_credits") {
                    previous["status"]["reset_credits"] = credits.clone();
                }
                previous["observed_at_ms"] = json!(observed);
            }
        }
        self.save_snapshots(&snapshots)
    }

    #[cfg(target_os = "macos")]
    fn store_key(&self, id: &str, key: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(KEY_SERVICE, id, key.as_bytes())
            .context("cannot save Sandhill API key")
    }
    #[cfg(target_os = "macos")]
    fn key(&self, id: &str) -> Result<Option<String>> {
        use security_framework::passwords::{PasswordOptions, generic_password};
        match generic_password(PasswordOptions::new_generic_password(KEY_SERVICE, id)) {
            Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(error).context("cannot read Sandhill API key"),
        }
    }
    #[cfg(target_os = "macos")]
    fn delete_key(&self, id: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password(KEY_SERVICE, id) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(error) => Err(error).context("cannot remove Sandhill API key"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    fn store_key(&self, id: &str, key: &str) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.folder(id)?.join("api-key"))?;
        file.write_all(key.as_bytes())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    fn key(&self, id: &str) -> Result<Option<String>> {
        match std::fs::read_to_string(self.folder(id)?.join("api-key")) {
            Ok(key) => Ok(Some(key)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    fn delete_key(&self, _id: &str) -> Result<()> {
        Ok(())
    }
}

fn checked_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.len() > 120 || name.chars().any(char::is_control) {
        bail!("account names must contain 1–120 bytes and no control characters");
    }
    Ok(name)
}

fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    Ok(())
}

fn apply_snapshot(account: &mut AgentAccount, snapshot: &Value) {
    if let Some(signed_in) = snapshot["signed_in"].as_bool() {
        account.signed_in = Some(signed_in);
    }
    if let Some(identity) = snapshot.get("account") {
        account.signed_in = Some(!identity.is_null());
        account.identity = ["email", "name", "id"]
            .iter()
            .find_map(|key| identity[key].as_str().map(str::to_owned));
    }
    if snapshot["status"]["rate_limits"].is_object() {
        account.usage = Some(
            json!({"rate_limits": snapshot["status"]["rate_limits"], "observed_at_ms": snapshot["observed_at_ms"]}),
        );
    }
    if let (Some(usage), Some(credits)) = (
        account.usage.as_mut(),
        snapshot["status"].get("reset_credits"),
    ) {
        usage["reset_credits"] = credits.clone();
    }
}

/// Called by each persistent terminal host, so usage survives daemon restarts.
pub(crate) fn observe(kind: &str, report: &Value) {
    if !KINDS.contains(&kind) {
        return;
    }
    let id = std::env::var("SANDHILL_ACCOUNT_ID").unwrap_or_else(|_| kind.into());
    if kind == ANTIGRAVITY && id != ANTIGRAVITY {
        return;
    }
    // Do not persist arbitrary tool reports, only the public identity and rate-limit fields.
    if (report.get("account").is_some() || report["status"]["rate_limits"].is_object())
        && let Err(error) = Registry::from_env().remember(&id, report)
    {
        tracing::debug!("cannot remember agent account usage: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::{Registry, apply_snapshot};
    use crate::chats::Homes;
    use serde_json::json;

    #[tokio::test]
    async fn observations_stay_with_the_selected_account_and_keep_newest_usage() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let registry = Registry::new(
            root.join("accounts"),
            Homes {
                claude: root.join("claude"),
                codex: root.join("codex"),
            },
        );
        let first = registry.add("claude", "Personal", None).unwrap();
        let second = registry.add("claude", "Work", None).unwrap();
        registry.remember(&first.id, &json!({"status": {
            "quota_account": {"email": "personal@example.test"},
            "quota_observed_at_ms": 20, "rate_limits": {"five_hour": {"used_percentage": 75}}
        }})).unwrap();
        registry
            .remember(
                &first.id,
                &json!({"status": {
                    "quota_observed_at_ms": 10, "rate_limits": {"five_hour": {"used_percentage": 1}}
                }}),
            )
            .unwrap();
        registry
            .remember(&second.id, &json!({"account": null}))
            .unwrap();
        let accounts = registry.list(false, None).await.unwrap();
        let personal = accounts.iter().find(|a| a.id == first.id).unwrap();
        assert_eq!(personal.identity.as_deref(), Some("personal@example.test"));
        assert_eq!(personal.signed_in, Some(true));
        assert_eq!(
            personal.usage.as_ref().unwrap()["rate_limits"]["five_hour"]["used_percentage"],
            75
        );
        assert_eq!(personal.usage.as_ref().unwrap()["observed_at_ms"], 20);
        let work = accounts.iter().find(|a| a.id == second.id).unwrap();
        assert_eq!(work.signed_in, Some(false));
        assert!(work.usage.is_none());
        assert!(
            accounts
                .iter()
                .find(|a| a.id == "claude")
                .unwrap()
                .usage
                .is_none()
        );
    }

    #[test]
    fn identity_only_refresh_preserves_observed_usage() {
        let mut account = sandhill_proto::AgentAccount {
            id: "claude".into(),
            kind: "claude".into(),
            name: "Default".into(),
            method: "login".into(),
            default: true,
            signed_in: None,
            identity: None,
            usage: Some(json!({"rate_limits": {}, "observed_at_ms": 10})),
        };
        apply_snapshot(
            &mut account,
            &json!({"account": {"email": "owner@example.test"}}),
        );
        assert_eq!(account.identity.as_deref(), Some("owner@example.test"));
        assert_eq!(account.usage.as_ref().unwrap()["observed_at_ms"], 10);
    }
}
