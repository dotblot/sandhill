use super::*;

impl Daemon {
    /// Keeps the Mac awake while a job runs.
    pub(super) fn keep_awake(&self, job: &str) {
        if let Some(pid) = self.terminals.pid(job) {
            crate::jobs::keep_awake(pid);
        }
    }

    pub(super) fn insert(&self, handle: SessionHandle) {
        self.sessions
            .lock()
            .unwrap()
            .insert(handle.id.clone(), Arc::new(handle));
    }

    /// Runs a tool's code quietly in an idle session: the code `code` gives
    /// for the session's language. `what` says what for, in errors.
    pub(super) async fn quietly(
        &self,
        session: &str,
        what: &str,
        code: impl FnOnce(&str) -> Option<String>,
    ) -> Result<variables::Printed> {
        self.quietly_for(session, what, variables::TIMEOUT, code)
            .await
    }

    /// As [`Self::quietly`], waiting up to `timeout`.
    pub(super) async fn quietly_for(
        &self,
        session: &str,
        what: &str,
        timeout: std::time::Duration,
        code: impl FnOnce(&str) -> Option<String>,
    ) -> Result<variables::Printed> {
        let info = self.session_info(session)?;
        let code = code(&info.language)
            .with_context(|| format!("cannot {what} in a {} session", info.language))?;
        if info.pending_runs > 0 {
            bail!("the session is busy running code; try again once it is idle");
        }
        let handle = self.live(session)?;
        let printed = variables::run_quietly(&handle, &self.journal, code, timeout)
            .await
            .with_context(|| format!("could not {what}"))?;
        if let Some(error) = &printed.error {
            bail!("could not {what}: {error}");
        }
        Ok(printed)
    }

    /// Draws a plot again into `save`'s file, or says whether it can.
    pub(super) async fn redraw_plot(
        &self,
        session: &str,
        run: &str,
        image: Option<&str>,
        index: u32,
        save: Option<redraw::Save<'_>>,
    ) -> Result<PlotRedraw> {
        let cannot = |reason: &str| PlotRedraw {
            can: false,
            reason: Some(reason.to_string()),
            png: None,
        };
        let info = self.session_info(session)?;
        if self.live(session).is_err() {
            return Ok(cannot("its session has ended"));
        }
        if self.journal.run_session(run)?.as_deref() != Some(session) {
            return Ok(cannot("it isn't from this session"));
        }
        let events = self.journal.run_events(run, i64::MAX)?;
        let plot = match redraw::find(&events, &info.language, image, index as usize) {
            Ok(plot) => plot,
            Err(reason) => return Ok(cannot(&reason)),
        };
        let Some(save) = save else {
            return Ok(PlotRedraw {
                can: true,
                reason: None,
                png: None,
            });
        };
        if !redraw::FORMATS.contains(&save.format) {
            bail!("a plot can't be saved as {}", save.format);
        }
        let sizes = 0.5..=100.0;
        if !sizes.contains(&save.width)
            || !sizes.contains(&save.height)
            || !(36..=2400).contains(&save.dpi)
        {
            bail!("a plot can't be saved at that size");
        }
        let resolved = files::resolve(save.file)?;
        if let Some(dir) = resolved.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not make the folder for {}", save.file))?;
        }
        let file = resolved.to_string_lossy();
        let save = redraw::Save {
            file: &file,
            ..save
        };
        let printed = self
            .quietly(session, "draw the plot again", |_| {
                Some(redraw::code(&plot, &save))
            })
            .await?;
        if !redraw::saved(&printed.stdout) {
            bail!("could not draw the plot again: nothing was written");
        }
        Ok(PlotRedraw {
            can: true,
            reason: None,
            png: None,
        })
    }

    pub(super) fn live(&self, session: &str) -> Result<Arc<SessionHandle>> {
        self.sessions
            .lock()
            .unwrap()
            .get(session)
            .filter(|s| s.is_running())
            .cloned()
            .ok_or_else(|| anyhow!("no running session {session}"))
    }

    pub(super) fn session_info(&self, id: &str) -> Result<sandhill_proto::SessionInfo> {
        let record = self
            .journal
            .sessions()?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| anyhow!("no session {id}"))?;
        Ok(self.journal.session_info(&record)?)
    }
}
