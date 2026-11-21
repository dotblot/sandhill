//! R and Python → Advanced (download sources, the suggested tool), installs
//! that stopped part way, and Open Terminal Here. Every path comes from this
//! daemon's computer, so a server's R and Python page works the same.

use sandhill_proto::{InstallTool, SourcePrefs};

use super::*;
use crate::quick::interrupted::{self, Rerun};
use crate::quick::{environments, install, sources};

impl Daemon {
    pub(super) async fn list_sources(&self, check: bool) -> Result<Value> {
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || Ok(sources::list(&record, check))).await?,
        )?)
    }

    pub(super) async fn set_sources(&self, chosen: SourcePrefs) -> Result<Value> {
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || sources::set(&record, chosen)).await?,
        )?)
    }

    pub(super) async fn set_preferred_tool(
        &self,
        language: String,
        tool: Option<InstallTool>,
    ) -> Result<Value> {
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || install::set_preferred_tool(&record, &language, tool)).await?,
        )?)
    }

    /// Runs the stopped install again through its usual planner, after
    /// removing the new folder it had half made.
    pub(super) async fn finish_interrupted(&self, id: String) -> Result<Value> {
        let record = self.install_record();
        let marker = blocking(move || {
            let marker = interrupted::get(&record, &id)?;
            interrupted::prepare_finish(&record, &marker)?;
            Ok(marker)
        })
        .await?;
        let result = match marker.rerun().cloned() {
            Some(Rerun::Packages(change)) => self.run_packages(change).await,
            Some(Rerun::Runtime(runtime)) => self.run_runtime(runtime).await,
            Some(Rerun::OwnSetup(setup)) => {
                if !setup.copy_from.is_empty() {
                    // The copy table is planned again, as a new set-up would be.
                    self.plan_own_setup(setup.clone()).await?;
                }
                self.run_own_setup(setup).await
            }
            None => Err(anyhow::anyhow!(
                "This install can't be run again. Undo it instead."
            )),
        };
        let record = self.install_record();
        let id = marker.id().to_string();
        let started = result.is_ok();
        blocking(move || interrupted::settle_finish(&record, &id, started)).await?;
        result
    }

    /// Puts back what the stopped install changed: the packages from the
    /// snapshot taken before it, or the new folder it was making removed.
    pub(super) async fn undo_interrupted(&self, id: String) -> Result<Value> {
        let record = self.install_record();
        let marker = id.clone();
        let undo =
            blocking(move || interrupted::undo(&interrupted::get(&record, &marker)?)).await?;
        let record = self.install_record();
        let executable = self.options.executable.clone();
        let started = self
            .quick_job_started(move |log| {
                install::launch(
                    &executable,
                    &record,
                    install::Job::UndoInterrupted(undo),
                    log,
                    "Undo the install that stopped part way",
                )
            })
            .await?;
        // The job now holds what undoes it; the marker goes.
        let record = self.install_record();
        blocking(move || interrupted::clear(&record, &id)).await?;
        Ok(serde_json::to_value(started)?)
    }

    pub(super) async fn env_activation(&self, env: String) -> Result<Value> {
        let record = self.install_record();
        Ok(serde_json::to_value(
            blocking(move || {
                environments::activation(&crate::kernelspecs::Search::from_env(), &record, &env)
            })
            .await?,
        )?)
    }
}
