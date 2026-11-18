//! Calls that start and restart terminals and jobs, and sign in agents' accounts.

use super::*;

impl Daemon {
    pub(super) async fn sign_in_agent_account(
        &self,
        id: String,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> Result<Value> {
        // Finished sign-ins leave nothing worth keeping; clear them.
        for ended in self
            .terminals
            .list()
            .into_iter()
            .filter(|t| t.kind == sandhill_proto::TERMINAL_LOGIN && !t.running)
        {
            let _ = self.terminals.remove(&ended.id);
        }
        let mut launch = crate::accounts::Registry::from_env().sign_in(&id)?;
        launch.size = cols.zip(rows);
        Ok(serde_json::to_value(self.terminals.start(launch).await?)?)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_terminal(
        &self,
        kind: String,
        cwd: Option<String>,
        project: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        options: sandhill_proto::AgentOptions,
        prompt: Option<String>,
    ) -> Result<Value> {
        crate::agents::ensure_offered(&kind)?;
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let cwd = match cwd.or_else(|| project.clone()) {
            Some(cwd) => files::resolve(&cwd)?,
            None => PathBuf::from(std::env::var_os("HOME").unwrap_or("/".into())),
        };
        if !cwd.is_dir() {
            anyhow::bail!("{} is not a directory", cwd.display());
        }
        let context = crate::agents::Context {
            executable: &self.options.executable,
            socket: &self.options.socket,
            project: project.as_deref(),
            cwd: &cwd,
            prompt: prompt.as_deref(),
            resume: None,
        };
        let mut launch = crate::agents::launch(&kind, &options, &context).await?;
        launch.size = cols.zip(rows);
        self.name_trust(&mut launch).await?;
        Ok(serde_json::to_value(self.terminals.start(launch).await?)?)
    }

    pub(super) async fn start_job(
        &self,
        job: sandhill_proto::JobSpec,
        cwd: Option<String>,
        project: Option<String>,
    ) -> Result<Value> {
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let cwd = cwd.map(|cwd| files::resolve(&cwd)).transpose()?;
        let launch =
            blocking(move || crate::jobs::launch(&job, cwd.as_deref(), project.as_deref())).await?;
        // A job with no folder given runs in its script's.
        self.trusted(vec![launch.cwd.clone()], "run jobs").await?;
        let started = self.terminals.start(launch).await?;
        self.keep_awake(&started.id);
        Ok(serde_json::to_value(started)?)
    }

    /// `RestartTerminal` of a job.
    pub(super) async fn restart_terminal_job(&self, terminal: String) -> Result<Value> {
        // A job runs again as it was asked to, in the same place.
        let info = self.terminals.info(&terminal)?;
        let job = info.job.unwrap_or_default();
        let cwd = PathBuf::from(&info.cwd);
        let project = info.project;
        let launch =
            blocking(move || crate::jobs::launch(&job, Some(&cwd), project.as_deref())).await?;
        let started = self.terminals.restart(&terminal, launch, &[]).await?;
        self.keep_awake(&started.id);
        Ok(serde_json::to_value(started)?)
    }

    /// `RestartTerminal` of anything but a job: an agent or a shell.
    pub(super) async fn restart_terminal_agent(
        &self,
        terminal: String,
        options: Option<sandhill_proto::AgentOptions>,
        cols: Option<u16>,
        rows: Option<u16>,
        prompt: Option<String>,
    ) -> Result<Value> {
        let info = self.terminals.info(&terminal)?;
        // An old conversation of a paused agent stays readable, not resumable.
        crate::agents::ensure_offered(&info.kind)?;
        // It may have run on another machine, or have to start afresh.
        let resume = chats::prepare(&info, &self.homes, &self.machine)?;
        let prompt = match &resume.note {
            Some(note) => Some(chats::with_note(note, prompt.as_deref())),
            None => prompt,
        };
        let cwd = PathBuf::from(&info.cwd);
        let context = crate::agents::Context {
            executable: &self.options.executable,
            socket: &self.options.socket,
            project: info.project.as_deref(),
            cwd: &cwd,
            prompt: prompt.as_deref(),
            resume: resume.id.as_deref(),
        };
        let options = options.unwrap_or_else(|| info.options.clone());
        let mut launch = crate::agents::launch(&info.kind, &options, &context).await?;
        launch.size = cols.zip(rows);
        self.name_trust(&mut launch).await?;
        let keep = &crate::agents::RESUME_KEYS;
        Ok(serde_json::to_value(
            self.terminals.restart(&terminal, launch, keep).await?,
        )?)
    }
}
