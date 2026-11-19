//! Quick Tools calls: what is installed, installing packages, session
//! folders and their card, and running templates.

use std::time::Duration;

use sandhill_proto::{EVENT_RUN_FINISHED, QuickCard, QuickRequest};
use tokio::sync::broadcast::error::RecvError;

use super::*;
use crate::quick::{folder, run, setup};

/// How long one template may run: loading a large table, a big heatmap.
const RUN_TIMEOUT: Duration = Duration::from_secs(600);

impl Daemon {
    pub(super) async fn quick_readiness(&self) -> Result<Value> {
        let ready =
            blocking(|| Ok(setup::readiness(&crate::kernelspecs::Search::from_env()))).await?;
        Ok(serde_json::to_value(ready)?)
    }

    pub(super) async fn quick_install(&self, r: String, packages: Vec<String>) -> Result<Value> {
        let log = std::env::temp_dir().join(format!(
            "sandhill-quick-install-{}.log",
            uuid::Uuid::new_v4().simple()
        ));
        let at = log.clone();
        let launch = blocking(move || setup::install(&r, &packages, &at)).await?;
        let started = self.terminals.start(launch).await?;
        self.keep_awake(&started.id);
        Ok(serde_json::to_value(sandhill_proto::QuickInstalling {
            job: started,
            log: log.to_string_lossy().into_owned(),
        })?)
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
        &self,
        project: String,
        parent: String,
        name: String,
    ) -> Result<Value> {
        let root = PathBuf::from(projects::normalize(&project)?);
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

    pub(super) async fn quick_discard(&self, project: String) -> Result<Value> {
        let explore = self.explore_dir();
        blocking(move || folder::discard(&explore, Path::new(&project))).await?;
        Ok(Value::Null)
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
