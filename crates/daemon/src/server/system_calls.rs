//! Calls that answer pings, read blobs, save project settings, and report system state and the log.

use super::*;

impl Daemon {
    pub(super) fn ping(&self) -> Result<Value> {
        Ok(serde_json::to_value(PingReply {
            protocol_version: PROTOCOL_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            home: std::env::var("HOME").unwrap_or_default(),
            hostname: hostname(),
            build: crate::build_id().to_string(),
            user: self.user.clone(),
            started_ms: self.started_ms,
            started_by: self.options.started_by.clone(),
            lifecycle: if self.stopping.load(Ordering::Acquire) {
                "stopping"
            } else if self.maintenance.load(Ordering::Acquire) {
                "stopped"
            } else {
                "ready"
            }
            .into(),
        })?)
    }

    pub(super) fn get_blob(&self, hash: String, near: Option<String>) -> Result<Value> {
        let mut bytes = self.journal.blobs().get(&hash);
        if bytes.is_err()
            && let Some(path) = near
                .and_then(|n| files::resolve(&n).ok())
                .and_then(|doc| sidecar::blob_path(&doc, &hash))
        {
            bytes = std::fs::read(path);
        }
        let bytes = bytes.with_context(|| format!("no blob {hash}"))?;
        Ok(serde_json::to_value(Blob {
            hash,
            data: String::from_utf8_lossy(&bytes).into_owned(),
        })?)
    }

    pub(super) async fn save_project_settings(
        &self,
        path: String,
        settings: sandhill_proto::ProjectSettings,
    ) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&path)?);
        if !root.is_dir() {
            anyhow::bail!("{} is not a folder", root.display());
        }
        let machine = settings.chat_history == ChatHistory::Machine;
        let setup = blocking({
            let root = root.clone();
            move || {
                projects::settings::write(&root, &settings)?;
                Ok(projects::setup(&root))
            }
        })
        .await?;
        if machine {
            chats::move_out(&self.terminals, &root)?;
        }
        Ok(serde_json::to_value(setup)?)
    }

    pub(super) async fn system_stats(&self, path: Option<String>) -> Result<Value> {
        let journal = self.journal.clone();
        let jobs: Vec<(String, u32)> = self
            .terminals
            .list()
            .into_iter()
            .filter(|t| t.kind == TERMINAL_JOB && t.running)
            .filter_map(|t| Some((t.id.clone(), self.terminals.pid(&t.id)?)))
            .collect();
        Ok(serde_json::to_value(
            blocking(move || {
                let path = match path {
                    Some(path) => files::resolve(&path)?,
                    None => PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into())),
                };
                let kernels: Vec<(String, u32)> = journal
                    .sessions()?
                    .into_iter()
                    .filter(|s| s.status != SessionStatus::Ended)
                    .filter_map(|s| Some((s.id, s.pid?)))
                    .collect();
                Ok(crate::system::stats(&path, &kernels, &jobs))
            })
            .await?,
        )?)
    }

    pub(super) async fn read_log(&self, bytes: Option<u64>) -> Result<Value> {
        let path = self.state.log_path();
        let tail = blocking(move || crate::logfile::tail(&path, bytes.unwrap_or(LOG_TAIL)))
            .await
            .map_err(|err| match self.options.started_by.as_str() {
                // Then the service manager keeps the log.
                "launchd" | "systemd" => anyhow!(
                    "{err:#}; sandhilld runs under {}, which keeps its log",
                    self.options.started_by
                ),
                _ => err,
            })?;
        Ok(serde_json::to_value(tail)?)
    }
}
