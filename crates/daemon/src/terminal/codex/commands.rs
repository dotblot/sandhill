use super::*;

impl Bridge {
    /// What the user did in the chat.
    pub(super) async fn command(self: Arc<Self>, command: Value) {
        match command["kind"].as_str().unwrap_or("") {
            "say" => {
                if let Some(text) = command["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    self.say(text.to_string(), true).await;
                }
            }
            "interrupt" => self.interrupt().await,
            "set" => {
                let refused = {
                    let mut state = self.state.lock().unwrap();
                    let text = |key: &str| command[key].as_str().map(str::to_string);
                    if command.get("model").is_some() {
                        state.model = text("model");
                        let label = state
                            .models
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|m| Some(&m["id"]) == command.get("model"))
                            .map(|m| m["label"].clone());
                        let id = state.model.clone().unwrap_or(state.default_model.clone());
                        state.status.insert("model_id".into(), json!(id));
                        state
                            .status
                            .insert("model".into(), label.unwrap_or(json!(id)));
                    }
                    if command.get("effort").is_some() {
                        state.effort = text("effort");
                    }
                    let wanted = text("permission");
                    let allowed = crate::quick::own_lock::codex_mode_allowed(
                        wanted.as_deref(),
                        std::env::var(crate::quick::own_lock::HOST_LOCK)
                            .ok()
                            .as_deref(),
                    );
                    if command.get("permission").is_some() && allowed {
                        state.permission = wanted;
                    }
                    command.get("permission").is_some() && !allowed
                };
                if refused {
                    self.chat(
                        json!({"kind": "error", "text": crate::quick::own_lock::CODEX_SWITCH}),
                    )
                    .await;
                }
                self.report(json!({})).await;
            }
            "compact" => {
                let thread = self.state.lock().unwrap().thread.clone();
                if let Err(err) = self
                    .rpc
                    .call("thread/compact/start", json!({"threadId": thread}))
                    .await
                {
                    self.chat(json!({"kind": "error", "text": format!("Codex couldn't compact the conversation: {err:#}")}))
                        .await;
                }
            }
            "undo" => self.undo().await,
            "login" => self.login(command["method"].as_str() == Some("code")).await,
            _ => {}
        }
    }

    async fn undo(self: &Arc<Self>) {
        let target = {
            let mut state = self.state.lock().unwrap();
            let target = state
                .undo_target()
                .map(|(thread, turn)| (thread.to_owned(), turn.to_owned()));
            if target.is_ok() {
                // Reserve the conversation, as say does while starting a turn:
                // queued messages must not start during the history replacement.
                state.turn = Some(String::new());
            }
            target
        };
        let (thread, turn) = match target {
            Ok(target) => target,
            Err(err) => {
                self.chat(json!({"kind": "error", "text": err.to_string()}))
                    .await;
                return;
            }
        };
        match self
            .rpc
            .call(
                "thread/revert",
                json!({"threadId": thread, "beforeTurnId": turn}),
            )
            .await
        {
            // Notifications may update context during the revert, but only a
            // subsequent usage notification is authoritative for the new counts.
            Ok(_) => {
                let event = {
                    let mut state = self.state.lock().unwrap();
                    state.turn = None;
                    state.reverted(&turn)
                };
                self.chat(event).await;
            }
            Err(err) => {
                self.state.lock().unwrap().turn = None;
                self.chat(json!({"kind": "error", "text": format!("Codex couldn't undo the last exchange: {err:#}")})).await;
            }
        }
        self.report(json!({})).await;
        self.next();
    }

    /// Starts a turn with the user's message, or keeps it for the next turn
    /// while one runs. `shown`: the message shows in the chat.
    pub(super) async fn say(self: Arc<Self>, text: String, shown: bool) {
        let at_ms = now_ms();
        let params = {
            let mut state = self.state.lock().unwrap();
            if state.paused || state.turn.is_some() || state.waiting > 0 {
                state.queued.push(text);
                return;
            }
            // Taken now, so a second message waits for this one's turn.
            state.turn = Some(String::new());
            state.plan = None;
            state.errored = false;
            let mut params = items::mode_settings(state.permission.as_deref());
            params["threadId"] = json!(state.thread);
            if let Some(model) = &state.model {
                params["model"] = json!(model);
            }
            if let Some(effort) = &state.effort {
                params["effort"] = json!(effort);
            }
            let planning = state.permission.as_deref() == Some("plan");
            state.planned |= planning;
            if state.planned {
                let model = state.model.clone().unwrap_or(state.default_model.clone());
                params["collaborationMode"] = json!({
                    "mode": if planning { "plan" } else { "default" },
                    "settings": {"model": model, "reasoning_effort": state.effort, "developer_instructions": null},
                });
            }
            params
        };
        let prompt = if shown {
            self.prompt().await
        } else {
            Default::default()
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.paused {
                state.turn = None;
                state.queued.push(text);
                return;
            }
        }
        let mut input = turn_input(&text, &prompt.refs);
        if let Some(context) = &prompt.context {
            input.push(json!({"type": "text", "text": context, "text_elements": []}));
        }
        if prompt.report.as_object().is_some_and(|r| !r.is_empty()) {
            self.report(prompt.report.clone()).await;
        }
        let place = prompt.place;
        let mut params = params;
        params["input"] = json!(input);
        if shown {
            println!("\n› {text}");
        }
        self.report(json!({"state": "busy"})).await;
        match self.rpc.call("turn/start", params).await {
            Ok(started) => {
                let turn = started["turn"]["id"].as_str().unwrap_or("").to_string();
                {
                    let mut state = self.state.lock().unwrap();
                    // A fast turn can complete before its start reply is handled.
                    // Do not resurrect it while returning from native mode.
                    if state.turn.as_deref() == Some("") && !state.history.contains(&turn) {
                        state.turn = Some(turn.clone());
                    }
                    if shown {
                        state.shown.push(turn.clone());
                    }
                }
                if shown {
                    self.chat(json!({
                        "kind": "prompt",
                        "turn": turn,
                        "text": text,
                        "place": place,
                        "at_ms": at_ms,
                    }))
                    .await;
                }
            }
            Err(err) => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.turn = None;
                    state.stopping = false;
                }
                let turn = uuid::Uuid::new_v4().to_string();
                if shown {
                    self.chat(
                        json!({"kind": "prompt", "turn": turn, "text": text, "at_ms": at_ms}),
                    )
                    .await;
                }
                self.chat(json!({"kind": "error", "turn": turn, "text": format!("{err:#}")}))
                    .await;
                self.report(json!({})).await;
                self.chat(json!({"kind": "stop", "turn": turn})).await;
            }
        }
    }

    /// What goes with the user's message, as Claude Code's prompt hook
    /// gives it: the text for the agent, and the refs for the chat.
    async fn prompt(&self) -> crate::mcp::Prompt {
        let tools = crate::mcp::Tools::new(self.daemon.clone(), self.project.clone())
            .in_terminal(Some(self.dir.clone()));
        tools.prompt(REFS_TIMEOUT).await
    }

    pub(super) async fn interrupt(&self) {
        let (thread, turn) = {
            let mut state = self.state.lock().unwrap();
            state.queued.clear();
            if state.turn.is_some() && !state.started {
                // Starting: stopped once Codex has started it.
                state.stopping = true;
                return;
            }
            (state.thread.clone(), state.turn.clone())
        };
        if let Some(turn) = turn {
            let _ = self
                .rpc
                .call(
                    "turn/interrupt",
                    json!({"threadId": thread, "turnId": turn}),
                )
                .await;
        }
    }

    /// Signs Codex in with ChatGPT: in the browser, or with a code on
    /// another device (for a server). Codex keeps the login; the app only
    /// shows where to go.
    async fn login(&self, code: bool) {
        let params = if code {
            json!({"type": "chatgptDeviceCode"})
        } else {
            json!({"type": "chatgpt"})
        };
        match self.rpc.call("account/login/start", params).await {
            Ok(started) => {
                let login = json!({
                    "id": started["loginId"],
                    "url": started.get("authUrl").or(started.get("verificationUrl")),
                    "code": started["userCode"],
                });
                self.report(json!({"login": login})).await;
            }
            Err(err) => {
                self.report(json!({"login": {"error": format!("{err:#}")}}))
                    .await
            }
        }
    }

    /// Starts the messages that came while the turn ran.
    pub(super) fn next(self: &Arc<Self>) {
        self.spawn(self.clone().drain_queued());
    }

    pub(super) async fn drain_queued(self: Arc<Self>) {
        let queued = {
            let mut state = self.state.lock().unwrap();
            if state.paused {
                return;
            }
            std::mem::take(&mut state.queued)
        };
        if !queued.is_empty() {
            self.say(queued.join("\n\n"), true).await;
        }
    }

    /// Asks the user whether to carry out the plan Codex made in plan mode,
    /// as Claude Code's plan card does; yes switches to Normal and carries
    /// it out.
    pub(super) async fn decide_plan(self: Arc<Self>, turn: String, plan: String) {
        let id = short_id();
        let event = json!({
            "kind": "ask",
            "turn": turn,
            "id": id,
            "tool": "ExitPlanMode",
            "input": {"plan": plan},
            "at_ms": now_ms(),
        });
        self.state.lock().unwrap().waiting += 1;
        self.report(json!({})).await;
        let answer = super::ask(&self.dir, &id, event).await.ok().flatten();
        let carry_out = answer.as_ref().is_some_and(|a| a["behavior"] == "allow");
        {
            let mut state = self.state.lock().unwrap();
            state.waiting -= 1;
            if carry_out {
                state.permission = Some("supervised".into());
            }
        }
        self.report(json!({})).await;
        self.chat(json!({"kind": "stop", "turn": turn})).await;
        if carry_out {
            self.spawn(self.clone().say(CARRY_OUT.to_string(), false));
        } else {
            self.next();
        }
    }
}

/// Codex's native skill input accompanies, rather than replaces, the text.
fn turn_input(text: &str, refs: &[super::super::choices::Ref]) -> Vec<Value> {
    let mut input = vec![json!({"type": "text", "text": text, "text_elements": []})];
    input.extend(
        refs.iter()
            .filter(|r| r.kind == "skill")
            .filter_map(|skill| {
                Some(json!({"type": "skill", "name": skill.name, "path": skill.path.as_deref()?}))
            }),
    );
    input
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_refs_are_native_codex_user_input() {
        let refs: Vec<super::super::super::choices::Ref> = serde_json::from_value(json!([
            {"kind": "file", "name": "data.csv", "path": "data.csv"},
            {"kind": "skill", "name": "scan", "path": "/skills/scan/SKILL.md"},
            {"kind": "skill", "name": "missing-path"}
        ]))
        .unwrap();
        assert_eq!(
            turn_input("Inspect data", &refs),
            vec![
                json!({"type": "text", "text": "Inspect data", "text_elements": []}),
                json!({"type": "skill", "name": "scan", "path": "/skills/scan/SKILL.md"})
            ]
        );
    }

    #[test]
    fn undo_uses_the_completed_turn_id_and_writes_a_revert_entry() {
        let mut state = State {
            thread: "thread-1".into(),
            completed: vec!["turn-1".into(), "turn-2".into()],
            ..State::default()
        };
        state.history.extend(state.completed.iter().cloned());
        assert_eq!(state.undo_target().unwrap(), ("thread-1", "turn-2"));
        state.turn = Some("turn-3".into());
        assert!(state.undo_target().is_err());
        state.turn = None;
        assert_eq!(
            state.reverted("turn-2"),
            json!({"kind": "revert", "turn": "turn-2"})
        );
        assert!(!state.history.contains("turn-2"));
        assert_eq!(state.undo_target().unwrap(), ("thread-1", "turn-1"));
        state.reverted("turn-1");
        assert!(state.undo_target().is_err());
    }
}
