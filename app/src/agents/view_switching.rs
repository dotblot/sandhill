use super::*;

impl ViewSwitch {
    pub(super) fn begin(
        mode: Option<&str>,
        target: Show,
        generation: u64,
    ) -> Result<Option<Self>, &'static str> {
        let previous = match mode {
            Some("chat") => Show::Chat,
            Some("terminal") => Show::Terminal,
            None if target == Show::Chat => return Ok(None),
            Some("switching_chat" | "switching_terminal") => {
                return Err(
                    "This conversation is already changing modes. Please wait for it to finish.",
                );
            }
            _ => {
                return Err(
                    "The agent reported an unrecognized conversation mode. Resume the conversation before changing modes.",
                );
            }
        };
        Ok((previous != target).then_some(Self {
            target,
            previous,
            generation,
            started: false,
            phase: ViewPhase::Switching,
        }))
    }

    pub(super) fn observe(&mut self, mode: Option<&str>) -> Option<Show> {
        if self.phase != ViewPhase::Switching {
            return None;
        }
        let show = match mode {
            Some("switching_chat" | "switching_terminal") => {
                self.started = true;
                return None;
            }
            Some("chat") => Show::Chat,
            Some("terminal") => Show::Terminal,
            _ => return None,
        };
        (show == self.target || self.started).then_some(show)
    }
}

impl Agents {
    pub(crate) fn show(&mut self, show: Show, window: &mut Window, cx: &mut Context<Self>) {
        let show = self
            .current()
            .map_or(show, |t| provider_show(&t.kind, show));
        if matches!(show, Show::Terminal | Show::Chat)
            && let Some(t) = self
                .current()
                .cloned()
                .filter(|t| t.running && drives_chat(&t.kind) && supports_terminal(&t.kind))
        {
            if self.view_switches.contains_key(&t.id) {
                return;
            }
            let mode = t.agent.as_ref().and_then(|a| a["view_mode"].as_str());
            self.view_generation += 1;
            if mode.is_none() && show == Show::Terminal {
                self.prepare_legacy_view(&t, cx);
                return;
            }
            match ViewSwitch::begin(mode, show, self.view_generation) {
                Ok(Some(switch)) => self.switch_view(&t, switch, cx),
                Ok(None) => {}
                Err(message) => {
                    self.problem = Some(message.into());
                    cx.notify();
                    return;
                }
            }
        }
        self.problem = None;
        self.show = show;
        self.menu = None;
        self.swap_draft(window, cx);
        match show {
            Show::Terminal => {
                if let Some(view) = self.selected.as_ref().and_then(|id| self.views.get(id)) {
                    let focus = view.read(cx).focus_handle();
                    window.focus(&focus, cx);
                }
            }
            Show::New | Show::Chat => {
                self.composer.update(cx, |c, cx| c.focus(window, cx));
            }
            Show::History => {}
        }
        cx.notify();
    }

    /// Check the service before ending anything: restarting on an old daemon
    /// would merely create another old bridge.
    fn prepare_legacy_view(&mut self, terminal: &TerminalInfo, cx: &mut Context<Self>) {
        let id = terminal.id.clone();
        let generation = self.view_generation;
        self.view_switches.insert(
            id.clone(),
            ViewSwitch {
                target: Show::Terminal,
                previous: Show::Chat,
                generation,
                started: false,
                phase: ViewPhase::Checking,
            },
        );
        let (build, device) = self
            .workbench
            .upgrade()
            .map_or((None, "this device".into()), |wb| {
                let wb = wb.read(cx);
                (
                    wb.about.as_ref().map(|about| about.build.clone()),
                    crate::shell::app_window::machine_name(&wb.host()),
                )
            });
        let checked = self.daemon.matches_bundled_build(build);
        self.watch_view_switch(id.clone(), generation, cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checked = checked.await;
            this.update(cx, |this, cx| {
                if !this.view_switches.get(&id).is_some_and(|s| s.generation == generation) {
                    return;
                }
                let Some(t) = this.terminals.iter().find(|t| t.id == id).cloned() else {
                    this.view_switches.remove(&id);
                    return;
                };
                let mode = t.agent.as_ref().and_then(|a| a["view_mode"].as_str());
                // A current bridge may have finished starting during the check.
                if mode.is_some() {
                    this.view_switches.remove(&id);
                    match ViewSwitch::begin(mode, Show::Terminal, generation) {
                        Ok(Some(switch)) => this.switch_view(&t, switch, cx),
                        Ok(None) => {
                            this.problem = None;
                        }
                        Err(message) => this.problem = Some(message.into()),
                    }
                    cx.notify();
                    return;
                }
                let state = t.agent.as_ref().and_then(|a| a["state"].as_str());
                let recovery = legacy_view_recovery(
                    state,
                    crate::agents::composer::can_resume(&t),
                    checked.ok().flatten(),
                );
                let message = match recovery {
                    ViewRecovery::Resume if t.running => {
                        if let Some(switch) = this.view_switches.get_mut(&id) {
                            switch.phase = ViewPhase::Resuming;
                        }
                        let options = this.resume_options(&t);
                        this.restart(&t, options, None, cx);
                        return;
                    }
                    ViewRecovery::Resume => "The conversation ended before changing modes. Resume it and try again.".into(),
                    ViewRecovery::Busy => "Wait until this conversation is idle, then try changing modes again.".into(),
                    ViewRecovery::NoResume => "This older agent bridge cannot switch modes, and this conversation has no resumable session. Start a new conversation to use command-line mode.".into(),
                    ViewRecovery::OlderService => format!(
                        "The Sandhill service on {device} is older than this app and cannot switch modes yet. Connect this device from a Sandhill app build that includes server daemons to update it."
                    ),
                    ViewRecovery::UnknownService => format!(
                        "The Sandhill service on {device} cannot switch this conversation's modes, and this app has no matching server daemon to verify an update. Connect this device from a Sandhill app build that includes server daemons to update the service."
                    ),
                };
                this.fail_view_switch(&id, generation, message, cx);
            }).ok();
        }).detach();
    }

    /// RestartTerminal preserves the conversation and options, but its bridge
    /// becomes ready asynchronously. Send the handoff only after its report.
    pub(crate) fn continue_resumed_view(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(pending) = self
            .view_switches
            .get(id)
            .copied()
            .filter(|s| s.phase == ViewPhase::Resuming && self.starting.is_none())
        else {
            return;
        };
        let Some(t) = self.terminals.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        if !t.running {
            self.fail_view_switch(
                id,
                pending.generation,
                "The agent ended while resuming the conversation to change modes.".into(),
                cx,
            );
            return;
        }
        let Some(mode) = t.agent.as_ref().and_then(|a| a["view_mode"].as_str()) else {
            return;
        };
        match ViewSwitch::begin(Some(mode), pending.target, pending.generation) {
            Ok(Some(switch)) => self.switch_view(&t, switch, cx),
            Ok(None) => {
                self.view_switches.remove(id);
                self.problem = None;
            }
            Err(message) => self.fail_view_switch(id, pending.generation, message.into(), cx),
        }
    }

    fn switch_view(&mut self, terminal: &TerminalInfo, switch: ViewSwitch, cx: &mut Context<Self>) {
        let id = terminal.id.clone();
        let generation = switch.generation;
        self.view_switches.insert(id.clone(), switch);
        self.problem = None;
        let requested = self.daemon.call::<Value>(Call::TellTerminal {
            terminal: id.clone(),
            command: json!({
                "kind": "view",
                "mode": if switch.target == Show::Terminal { "terminal" } else { "chat" },
            }),
        });
        let request_id = id.clone();
        cx.spawn(async move |this, cx| {
            if let Err(err) = requested.await {
                this.update(cx, |this, cx| {
                    this.fail_view_switch(
                        &request_id,
                        generation,
                        format!("Could not change conversation mode: {err:#}"),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
        self.watch_view_switch(id, generation, cx);
    }

    fn watch_view_switch(&self, id: String, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(VIEW_SWITCH_TIMEOUT).await;
            this.update(cx, |this, cx| {
                this.fail_view_switch(
                    &id,
                    generation,
                    "The agent did not finish changing modes. You can try again or end and resume the conversation.".into(),
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    fn fail_view_switch(
        &mut self,
        id: &str,
        generation: u64,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let Some(switch) = self.view_switches.get(id).copied() else {
            return;
        };
        if switch.generation != generation {
            return;
        }
        self.view_switches.remove(id);
        if self.selected.as_deref() == Some(id) && matches!(self.show, Show::Chat | Show::Terminal)
        {
            self.show = switch.previous;
        }
        self.problem = Some(message);
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
