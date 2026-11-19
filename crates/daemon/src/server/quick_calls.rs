//! Explore a Table calls: what is installed, installing and removing what
//! Sandhill installs, session folders and their card, and running
//! templates.

use std::time::Duration;

use sandhill_proto::{EVENT_RUN_FINISHED, QuickAgentTool, QuickCard, QuickRequest};
use tokio::sync::broadcast::error::RecvError;

use super::*;
use crate::quick::{folder, record, run, setup};
use crate::terminal::Launch;

/// How long one template may run: loading a large table, a big heatmap.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

impl Daemon {
    /// Sandhill's record of what it installed on this device.
    pub(super) fn install_record(&self) -> PathBuf {
        self.state.root().join(record::FILE)
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
        let log = std::env::temp_dir().join(format!(
            "sandhill-quick-install-{}.log",
            uuid::Uuid::new_v4().simple()
        ));
        let at = log.clone();
        let launch = blocking(move || make(&at)).await?;
        let started = self.terminals.start(launch).await?;
        self.keep_awake(&started.id);
        Ok(serde_json::to_value(sandhill_proto::QuickInstalling {
            job: started,
            log: log.to_string_lossy().into_owned(),
        })?)
    }

    pub(super) async fn quick_install(
        &self,
        r: String,
        packages: Vec<String>,
        library: String,
    ) -> Result<Value> {
        let record = self.install_record();
        self.quick_job(move |log| setup::install(&r, &packages, &library, log, &record))
            .await
    }

    pub(super) async fn quick_install_agent(&self, kind: String) -> Result<Value> {
        let record = self.install_record();
        self.quick_job(move |log| setup::install_agent(&kind, log, &record))
            .await
    }

    pub(super) async fn quick_uninstall(&self) -> Result<Value> {
        let record = self.install_record();
        self.quick_job(move |log| record::uninstall(&record, log))
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
