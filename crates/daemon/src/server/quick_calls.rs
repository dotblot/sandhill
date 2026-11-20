//! Explore a Table calls: what is installed, installing and removing what
//! Sandhill installs, session folders and their card, and running
//! templates.

use std::time::Duration;

use sandhill_proto::{
    EVENT_RUN_FINISHED, EnvMode, InstalledKey, NewRuntime, OwnSetup, PackageChange, QuickAgentTool,
    QuickArchiveSelection, QuickCard, QuickInstalling, QuickRequest,
};
use tokio::sync::broadcast::error::RecvError;

use super::*;
use crate::quick::{archive, folder, install, own, python, record, run, setup};
use crate::terminal::Launch;

/// How long one template may run: loading a large table, a big heatmap.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

impl Daemon {
    /// Sandhill's record of what it installed on this device.
    pub(super) fn install_record(&self) -> PathBuf {
        self.state.root().join(record::FILE)
    }

    pub(super) fn own_folder(&self) -> Option<PathBuf> {
        crate::own_env::chosen(&self.install_record())
    }

    pub(super) async fn plan_own_setup(&self, setup: OwnSetup) -> Result<Value> {
        Ok(serde_json::to_value(
            blocking(move || Ok(own::plan(setup))).await?,
        )?)
    }

    pub(super) async fn run_own_setup(&self, setup: OwnSetup) -> Result<Value> {
        let record = self.install_record();
        let executable = self.options.executable.clone();
        self.quick_job(move |log| {
            let plan = own::plan(setup);
            if let Some(refused) = &plan.refused {
                bail!("{refused}");
            }
            install::launch(
                &executable,
                &record,
                install::Job::OwnSetup(plan),
                log,
                "Set up Sandhill's own R and Python",
            )
        })
        .await
    }

    pub(super) async fn set_env_mode(&self, mode: Option<EnvMode>) -> Result<Value> {
        let _guard = self.own_sessions.write().await;
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || own::set_mode(&record, mode)).await?,
        )?)
    }

    pub(super) async fn remove_own_environment(self: &Arc<Self>) -> Result<Value> {
        let folder = PathBuf::from(install::prefs(&self.install_record()).own_folder);
        self.remove_own_folders(vec![folder], Vec::new()).await
    }

    async fn remove_own_folders(
        self: &Arc<Self>,
        folders: Vec<PathBuf>,
        items: Vec<InstalledKey>,
    ) -> Result<Value> {
        // No new kernel can start between shutting these down and removing their files.
        let guard = self.own_sessions.clone().write_owned().await;
        for folder in &folders {
            own::removable(folder)?;
        }
        for session in self.journal.sessions()? {
            let own = crate::lifecycle::recorded_kernel(&session.connection_file)
                .and_then(|identity| identity.program())
                .is_some_and(|program| {
                    folders.iter().any(|folder| {
                        // Linux adds " (deleted)" when a live program no longer has a file.
                        program.starts_with(folder) || crate::own_env::inside(folder, &program)
                    })
                });
            if own {
                self.live(&session.id)?.shutdown().await?;
                self.sessions.lock().unwrap().remove(&session.id);
            }
        }
        let record = self.install_record();
        let executable = self.options.executable.clone();
        let started = self
            .quick_job_started(move |log| {
                install::launch(
                    &executable,
                    &record,
                    install::Job::RemoveOwn(folders, items),
                    log,
                    "Remove Sandhill's own R and Python",
                )
            })
            .await?;
        let id = started.job.id.clone();
        let daemon = Arc::downgrade(self);
        tokio::spawn(async move {
            let _guard = guard;
            while daemon
                .upgrade()
                .is_some_and(|d| d.terminals.info(&id).is_ok_and(|info| info.running))
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        Ok(serde_json::to_value(started)?)
    }

    pub(super) async fn quick_readiness(&self) -> Result<Value> {
        let mut agents = Vec::new();
        for kind in [crate::agents::CLAUDE, crate::agents::CODEX] {
            if let Some(info) = crate::agents::describe(kind).await {
                agents.push(info);
            }
        }
        let record = self.install_record();
        let ready = blocking(move || {
            Ok(setup::readiness(
                &crate::kernelspecs::Search::from_env(),
                &record,
                agents,
            ))
        })
        .await?;
        Ok(serde_json::to_value(ready)?)
    }

    /// Starts an install or removal job whose output is also in a log the
    /// app reads for its progress line.
    async fn quick_job(
        &self,
        make: impl FnOnce(&Path) -> Result<Launch> + Send + 'static,
    ) -> Result<Value> {
        Ok(serde_json::to_value(self.quick_job_started(make).await?)?)
    }

    async fn quick_job_started(
        &self,
        make: impl FnOnce(&Path) -> Result<Launch> + Send + 'static,
    ) -> Result<QuickInstalling> {
        let log = std::env::temp_dir().join(format!(
            "sandhill-quick-install-{}.log",
            uuid::Uuid::new_v4().simple()
        ));
        let at = log.clone();
        let launch = blocking(move || make(&at)).await?;
        let started = self.terminals.start_unmanaged(launch).await?;
        self.keep_awake(&started.id);
        Ok(QuickInstalling {
            job: started,
            log: log.to_string_lossy().into_owned(),
        })
    }

    pub(super) async fn plan_packages(&self, change: PackageChange) -> Result<Value> {
        let own = self.own_folder();
        let plan = blocking(move || {
            Ok(install::plan_packages(
                &crate::kernelspecs::Search::from_env().in_own(own),
                change,
            ))
        })
        .await?;
        Ok(serde_json::to_value(plan)?)
    }

    pub(super) async fn run_packages(&self, change: PackageChange) -> Result<Value> {
        let record = self.install_record();
        let executable = self.options.executable.clone();
        let own = self.own_folder();
        self.quick_job(move |log| {
            let plan =
                install::plan_packages(&crate::kernelspecs::Search::from_env().in_own(own), change);
            if let Some(refused) = &plan.refused {
                bail!("{refused}");
            }
            let title = install::package_title(&plan);
            let launch = install::launch(
                &executable,
                &record,
                install::Job::Packages(plan.clone()),
                log,
                &title,
            )?;
            install::remember(&record, &plan)?;
            Ok(launch)
        })
        .await
    }

    pub(super) async fn plan_runtime(&self, runtime: NewRuntime) -> Result<Value> {
        let plan = blocking(move || {
            Ok(install::plan_runtime(
                &crate::kernelspecs::Search::from_env(),
                runtime,
            ))
        })
        .await?;
        Ok(serde_json::to_value(plan)?)
    }

    pub(super) async fn run_runtime(&self, runtime: NewRuntime) -> Result<Value> {
        let record = self.install_record();
        let executable = self.options.executable.clone();
        self.quick_job(move |log| {
            let plan = install::plan_runtime(&crate::kernelspecs::Search::from_env(), runtime);
            if let Some(refused) = &plan.refused {
                bail!("{refused}");
            }
            let title = install::runtime_title(&plan.runtime);
            install::launch(
                &executable,
                &record,
                install::Job::Runtime(plan),
                log,
                title,
            )
        })
        .await
    }

    pub(super) async fn install_prefs(&self) -> Result<Value> {
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || Ok(install::prefs(&record))).await?,
        )?)
    }

    pub(super) async fn set_install_prefs(&self, without_asking: bool) -> Result<Value> {
        let record = self.install_record();
        let prefs = blocking(move || {
            install::update_prefs(&record, |prefs| prefs.without_asking = without_asking)
        })
        .await?;
        Ok(serde_json::to_value(prefs)?)
    }

    pub(super) async fn remove_installed(
        self: &Arc<Self>,
        items: Vec<InstalledKey>,
    ) -> Result<Value> {
        if items.is_empty() {
            bail!("Choose at least one item to remove.");
        }
        let record = self.install_record();
        if items
            .iter()
            .any(|item| item.kind == record::OWN_ENVIRONMENT)
        {
            let installed = record::read(&record);
            let (own, others): (Vec<_>, Vec<_>) = items
                .into_iter()
                .partition(|item| item.kind == record::OWN_ENVIRONMENT);
            let mut folders = Vec::new();
            for item in own {
                if !installed.iter().any(|row| {
                    row.ours
                        && row.kind == item.kind
                        && row.name == item.name
                        && row.place == item.place
                }) {
                    bail!("That folder isn't recorded as Sandhill's. Sandhill won't remove it.");
                }
                let folder = PathBuf::from(item.place);
                if !folders.contains(&folder) {
                    folders.push(folder);
                }
            }
            return self.remove_own_folders(folders, others).await;
        }
        let executable = self.options.executable.clone();
        self.quick_job(move |log| {
            install::launch(
                &executable,
                &record,
                install::Job::Remove(items),
                log,
                "Remove what Sandhill installed",
            )
        })
        .await
    }

    pub(super) async fn quick_install_agent(&self, kind: String) -> Result<Value> {
        let record = self.install_record();
        let executable = self.options.executable.clone();
        self.quick_job(move |log| setup::install_agent(&executable, &kind, log, &record))
            .await
    }

    pub(super) async fn quick_add_ipykernel(&self, prefix: String) -> Result<Value> {
        let record = self.install_record();
        let self_executable = self.options.executable.clone();
        let own = self.own_folder();
        self.quick_job(move |log| {
            python::add_ipykernel(
                &self_executable,
                &crate::kernelspecs::Search::from_env().in_own(own),
                &prefix,
                log,
                &record,
            )
        })
        .await
    }

    /// Sandhill's storage for sessions not kept yet.
    fn explore_dir(&self) -> PathBuf {
        self.state.root().join("explore")
    }

    pub(super) async fn quick_start(&self, tables: Vec<String>) -> Result<Value> {
        let journal = self.journal.clone();
        let explore = self.explore_dir();
        let made = blocking(move || {
            let made = folder::start(&explore, &tables)?;
            // Sandhill's own storage, holding only links to tables.
            projects::trust::trust(&journal, &explore.to_string_lossy())?;
            Ok(made)
        })
        .await?;
        Ok(Value::String(made.to_string_lossy().into_owned()))
    }

    pub(super) async fn quick_list_archive(&self, path: String) -> Result<Value> {
        Ok(serde_json::to_value(
            blocking(move || archive::list(&path)).await?,
        )?)
    }

    pub(super) async fn quick_open_archives(
        &self,
        project: Option<String>,
        tables: Vec<String>,
        archives: Vec<QuickArchiveSelection>,
        combine: bool,
    ) -> Result<Value> {
        let root = project
            .as_deref()
            .map(projects::normalize)
            .transpose()?
            .map(PathBuf::from);
        if let Some(root) = &root {
            self.trusted(vec![root.clone()], "open tables from an archive")
                .await?;
        }
        let journal = self.journal.clone();
        let explore = self.explore_dir();
        let opened = blocking(move || {
            if root.is_none() {
                // The session stays in Sandhill's own storage.
                std::fs::create_dir_all(&explore)?;
                projects::trust::trust(&journal, &explore.to_string_lossy())?;
            }
            folder::open_archives(&explore, root, &tables, &archives, combine)
        })
        .await?;
        Ok(serde_json::to_value(opened)?)
    }

    pub(super) async fn quick_keep(
        self: &Arc<Self>,
        project: String,
        parent: String,
        name: String,
    ) -> Result<Value> {
        let project = projects::normalize(&project)?;
        let root = PathBuf::from(&project);
        self.save_explore_chats(project, root.clone()).await?;
        let journal = self.journal.clone();
        let kept = blocking(move || {
            let kept = folder::keep(&root, &parent, &name)?;
            // The user asked for it and Sandhill made it; the folder around
            // it keeps its own trust (Huan, 2026-11-19).
            projects::trust::trust(&journal, &kept.to_string_lossy())?;
            Ok(kept)
        })
        .await?;
        Ok(Value::String(kept.to_string_lossy().into_owned()))
    }

    pub(super) async fn quick_discard(self: &Arc<Self>, project: String) -> Result<Value> {
        let explore = self.explore_dir();
        let project = projects::normalize(&project)?;
        let path = PathBuf::from(&project);
        let (explore, root, kept) = blocking(move || {
            let explore = std::fs::canonicalize(explore).context("no explore folder")?;
            let root = std::fs::canonicalize(path).context("no such session")?;
            if root.parent() != Some(explore.as_path()) {
                bail!("only sessions in Sandhill's own storage can be discarded");
            }
            let kept = folder::card(&root)?.kept;
            Ok((explore, root, kept))
        })
        .await?;
        let agents = self.terminals.list().into_iter().filter(|info| {
            info.project.as_deref() == Some(&project) && crate::agents::is_agent(&info.kind)
        });
        if let Some(kept) = kept.map(PathBuf::from).filter(|kept| kept.is_dir()) {
            for agent in agents {
                self.terminals.stop(&agent.id).await?;
                let ended = async {
                    while self
                        .terminals
                        .info(&agent.id)
                        .is_ok_and(|info| info.running)
                    {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                };
                tokio::time::timeout(Duration::from_secs(10), ended)
                    .await
                    .context("The Explore AI helper did not stop.")?;
            }
            self.save_explore_chats(project, kept).await?;
        } else {
            for agent in agents {
                self.terminals.close(&agent.id).await?;
            }
        }
        blocking(move || folder::discard(&explore, &root)).await?;
        Ok(Value::Null)
    }

    async fn save_explore_chats(self: &Arc<Self>, project: String, kept: PathBuf) -> Result<()> {
        let daemon = self.clone();
        blocking(move || {
            chats::save_explore(
                &daemon.terminals,
                &daemon.homes,
                &daemon.machine,
                &project,
                &kept,
            )
        })
        .await
    }

    pub(super) async fn quick_add_tables(
        &self,
        project: String,
        tables: Vec<String>,
    ) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&project)?);
        let names = blocking(move || folder::add_tables(&root, &tables)).await?;
        Ok(serde_json::to_value(names)?)
    }

    pub(super) async fn quick_card(&self, project: String) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&project)?);
        Ok(serde_json::to_value(
            blocking(move || folder::card(&root)).await?,
        )?)
    }

    pub(super) async fn quick_agent(
        &self,
        project: String,
        tool: QuickAgentTool,
        params: Value,
    ) -> Result<Value> {
        let project = projects::normalize(&project)?;
        if !crate::mcp::quick::is_explore(Some(&project)) {
            bail!("Open this session in Explore a Table first.");
        }
        let wait = match tool {
            QuickAgentTool::State | QuickAgentTool::Suggest => Duration::from_secs(20),
            QuickAgentTool::Make => crate::mcp::quick::RUN_WAIT,
        };
        self.windows
            .ask_explore(project, tool.method(), params, wait)
            .await
    }

    /// Saves what the user changed on the card; where the session lives and
    /// its tables' sources stay the daemon's.
    pub(super) async fn save_quick_card(&self, project: String, card: QuickCard) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&project)?);
        blocking(move || {
            let stored = folder::card(&root)?;
            folder::save_card(
                &root,
                &QuickCard {
                    sources: stored.sources,
                    scratch: stored.scratch,
                    kept: stored.kept,
                    ..card
                },
            )?;
            folder::mirror(&root)
        })
        .await?;
        Ok(Value::Null)
    }

    /// Defines the templates quietly, then runs the request as one visible
    /// line, shown as run by Sandhill so its plot reaches Plots.
    pub(super) async fn quick_run(
        &self,
        session: String,
        project: String,
        request: QuickRequest,
    ) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&project)?);
        self.trusted(vec![root.clone()], "explore tables").await?;
        let info = self.session_info(&session)?;
        if !info.language.eq_ignore_ascii_case("r") {
            bail!(
                "Exploring tables runs in R, and this session is {}",
                info.language
            );
        }
        let resolved = run::resolve(&root, &request)?;
        self.quietly_for(
            &session,
            "set up exploring tables",
            Duration::from_secs(60),
            |_| Some(run::define_code()),
        )
        .await?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let code = run::visible_code(&resolved, &token)?;
        let handle = self.live(&session)?;
        let mut events = handle.subscribe();
        let started = handle
            .execute(code, None, false, Some("Sandhill".into()))
            .await?;
        let finished = async {
            loop {
                match events.recv().await {
                    Ok(event) if event.run.as_deref() == Some(started.run.as_str()) => {
                        if event.msg_type == EVENT_RUN_FINISHED {
                            return Ok(());
                        }
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => bail!("the R session ended"),
                }
            }
        };
        match tokio::time::timeout(RUN_TIMEOUT, finished).await {
            Ok(result) => result?,
            Err(_) => bail!("This took too long; the R session may still be busy"),
        }
        let mut stdout = String::new();
        for (msg_type, content) in self.journal.run_events(&started.run, i64::MAX)? {
            match msg_type.as_str() {
                "stream" if content["name"] == "stdout" => {
                    stdout.push_str(content["text"].as_str().unwrap_or(""))
                }
                "error" => {
                    let message = content["evalue"].as_str().unwrap_or("R stopped").trim();
                    bail!("{}", message.strip_prefix("Error: ").unwrap_or(message));
                }
                _ => {}
            }
        }
        let reply = run::reply(&stdout, &token)?;
        let result = run::result(&request, reply)?;
        // Recorded as asked, with names relative to the folder, so steps.R
        // still runs after the folder moves.
        let step = request.clone();
        blocking(move || {
            run::record_step(&root, &step)?;
            folder::mirror(&root)
        })
        .await?;
        Ok(serde_json::to_value(result)?)
    }
}
