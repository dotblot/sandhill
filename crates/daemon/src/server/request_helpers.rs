use super::*;

impl Daemon {
    pub(super) async fn agent_accounts(
        &self,
        kinds: Option<&[String]>,
        force: bool,
    ) -> Result<Value> {
        let mut accounts = crate::agents::accounts(kinds, force).await?;
        if let Some(accounts) = accounts.as_array_mut() {
            for terminal in self.terminals.list() {
                if kinds.is_some_and(|kinds| !kinds.contains(&terminal.kind)) {
                    continue;
                }
                if let Some(mut report) = terminal.agent
                    && report.is_object()
                {
                    report["kind"] = json!(terminal.kind);
                    report["feed"] = json!(true);
                    accounts.push(report);
                }
            }
        }
        Ok(accounts)
    }

    /// Runs a git action; prompts for a password go to the window that
    /// asked, on `out`.
    pub(super) async fn git_run(
        &self,
        path: String,
        action: sandhill_proto::GitAction,
        out: &mpsc::UnboundedSender<ServerMessage>,
    ) -> Result<Value> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let asker = Asker {
            out: out.clone(),
            project: path.clone(),
        };
        self.askers.lock().unwrap().insert(token.clone(), asker);
        let askpass = crate::git::Askpass {
            program: self.options.executable.clone(),
            socket: self.options.socket.clone(),
            token: token.clone(),
        };
        let done = crate::git::ASKPASS
            .scope(askpass, crate::git::run_action(&path, action))
            .await;
        self.askers.lock().unwrap().remove(&token);
        Ok(serde_json::to_value(done?)?)
    }

    /// What the user types for a git action's prompt, in its window.
    pub(super) async fn git_credential(&self, token: &str, prompt: String) -> Result<Value> {
        let (out, project) = {
            let askers = self.askers.lock().unwrap();
            let asker = askers.get(token).context("that git action has finished")?;
            (asker.out.clone(), asker.project.clone())
        };
        let secret =
            !prompt.to_ascii_lowercase().starts_with("username") && !prompt.contains("(yes/no");
        let params = json!({"prompt": prompt.trim_end(), "secret": secret});
        match self
            .windows
            .ask_on(&out, project, "git_credential", params, CREDENTIAL_WAIT)
            .await
        {
            Some(Ok(Value::String(text))) => Ok(Value::String(text)),
            Some(Ok(_)) => bail!("the window's answer made no sense"),
            Some(Err(crate::windows::Unanswered::Said(message))) => bail!("{message}"),
            Some(Err(crate::windows::Unanswered::Late)) => bail!("no one answered"),
            Some(Err(crate::windows::Unanswered::Closed)) | None => {
                bail!("the Sandhill window closed")
            }
        }
    }

    pub(super) async fn start_lsp(
        &self,
        project: String,
        language: String,
        session: Option<String>,
        connection: u64,
        out: mpsc::UnboundedSender<ServerMessage>,
    ) -> Result<Value> {
        let project = projects::normalize(&project)?;
        if !Path::new(&project).is_dir() {
            bail!("{project} is not a project directory");
        }
        let language = language.to_ascii_lowercase();
        let records = self.journal.sessions()?;
        let record = if let Some(id) = session {
            let record = records
                .iter()
                .find(|record| record.id == id)
                .context("no such session")?;
            if record.project.as_deref() != Some(project.as_str())
                || record.language.to_ascii_lowercase() != language
            {
                bail!("language session must belong to this project and language");
            }
            Some(record)
        } else {
            records
                .iter()
                .filter(|record| {
                    record.project.as_deref() == Some(project.as_str())
                        && record.language.to_ascii_lowercase() == language
                        && record.status != SessionStatus::Ended
                })
                .max_by_key(|record| record.created_ms)
        };
        let mut interpreter = None;
        let ark = if language == "r" {
            record.map(|record| self.live(&record.id)).transpose()?
        } else {
            None
        };
        if language == "python" {
            let own = self.own_folder();
            let kernel = match record {
                Some(record) => Some(record.kernel.clone()),
                None => {
                    let root = PathBuf::from(&project);
                    let own = own.clone();
                    blocking(move || Ok(projects::setup_scoped(&root, own).python)).await?
                }
            };
            if let Some(kernel) = kernel {
                let spec =
                    crate::kernel::find_scoped(&kernel, Some(Path::new(&project)), own.clone())
                        .await?;
                interpreter = spec
                    .kernelspec
                    .argv
                    .first()
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute());
            }
            if interpreter.is_none() && own.is_none() {
                interpreter = [".venv/bin/python", ".venv/bin/python3", "venv/bin/python"]
                    .into_iter()
                    .map(|relative| Path::new(&project).join(relative))
                    .find(|path| path.is_file());
            }
        }
        let start = crate::lsp::Start {
            project,
            language,
            interpreter,
            ark,
        };
        Ok(serde_json::to_value(
            self.language_servers.start(start, connection, out).await?,
        )?)
    }

    /// Refuses a call that starts something acting on the files of an
    /// untrusted project: a kernel (`.Rprofile`, env files), a terminal, job
    /// or agent (`CLAUDE.md`, hooks), a language server, a Quarto render, or
    /// Git (a repository's configuration can name programs to run).
    pub(super) async fn check_trust(&self, call: &Call) -> Result<()> {
        let paths = |items: [Option<&String>; 2]| -> Vec<PathBuf> {
            items.into_iter().flatten().map(PathBuf::from).collect()
        };
        let (folders, what) = match call {
            Call::StartSession { cwd, project, .. } => {
                (paths([cwd.as_ref(), project.as_ref()]), "start R or Python")
            }
            Call::LspStart { project, .. } => (paths([Some(project), None]), "check code"),
            Call::StartTerminal {
                kind, cwd, project, ..
            } => (
                paths([cwd.as_ref(), project.as_ref()]),
                if kind == sandhill_proto::TERMINAL_SHELL {
                    "use the Terminal"
                } else {
                    "use AI agents"
                },
            ),
            Call::StartJob { cwd, project, .. } => {
                (paths([cwd.as_ref(), project.as_ref()]), "run jobs")
            }
            Call::RestartTerminal { terminal, .. } => match self.terminals.info(terminal) {
                Ok(info) => (vec![PathBuf::from(info.cwd)], "start it again"),
                Err(_) => return Ok(()),
            },
            Call::PreviewDocument { path, project } => {
                (paths([Some(path), project.as_ref()]), "render documents")
            }
            Call::SuggestCell { cwd, .. } => (paths([Some(cwd), None]), "use AI agents"),
            Call::GitStatus { path }
            | Call::GitLog { path, .. }
            | Call::GitShow { path, .. }
            | Call::GitOwnCommits { path }
            | Call::GitDiff { path, .. }
            | Call::GitRun { path, .. }
            | Call::GitWriteMessage { path, .. } => (paths([Some(path), None]), "use Git"),
            _ => return Ok(()),
        };
        self.trusted(folders, what).await
    }

    pub(super) async fn trusted(&self, folders: Vec<PathBuf>, what: &'static str) -> Result<()> {
        let journal = self.journal.clone();
        blocking(move || {
            for folder in &folders {
                projects::trust::check(&journal, folder, what)?;
            }
            Ok(())
        })
        .await
    }

    /// Tells an agent's run the trusted folder it starts in, so an agent that
    /// can be told per run does not ask about the folder again.
    pub(super) async fn name_trust(&self, launch: &mut crate::terminal::Launch) -> Result<()> {
        if launch.kind == sandhill_proto::TERMINAL_SHELL {
            return Ok(());
        }
        let journal = self.journal.clone();
        let cwd = launch.cwd.clone();
        if let Some(folder) =
            blocking(move || projects::trust::trusted_root(&journal, &cwd)).await?
        {
            launch
                .env
                .push((projects::trust::TRUSTED_FOLDER.into(), folder));
        }
        Ok(())
    }
}
