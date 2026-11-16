use super::*;

impl Agents {
    pub(crate) fn choose_new_kind(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        if self.new_kind != kind {
            self.new_choice.model = None;
        }
        self.new_kind = kind;
        cx.notify();
    }

    /// The message box's hint, for the agent it goes to.
    pub(crate) fn placeholder(&self) -> &'static str {
        let kind = match self.show {
            Show::New => Some(self.new_kind),
            _ => self.current().map(|t| t.kind.as_str()),
        };
        kind.and_then(kind_of)
            .map_or("Ask the agent…", |k| k.placeholder)
    }

    /// Enter in the message box: starts a conversation with the message,
    /// or sends it to the one shown. Posit Assistant also starts without
    /// one, as its own screen takes messages.
    pub(crate) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && !(self.show == Show::New && terminal_only(self.new_kind)) {
            return;
        }
        if matches!(self.menu, Some(Menu::Mention { .. })) {
            return self.pick_mention(None, window, cx);
        }
        let refs = std::mem::take(&mut self.refs);
        let sent = self.say(text, refs.clone(), window, cx);
        if sent {
            self.menu = None;
            self.drafts.remove(&self.draft_key);
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
        } else {
            self.refs = refs;
        }
    }

    /// Sends `text`, with `refs`, to the conversation shown, or starts one
    /// with it; whether it went.
    fn say(
        &mut self,
        text: String,
        refs: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let kind = match self.show {
            Show::New => Some(self.new_kind),
            _ => self.current().map(|t| t.kind.as_str()),
        };
        if let Some(off) = kind.filter(|kind| turned_off(kind)).and_then(kind_of) {
            self.problem = Some(format!("{} is turned off for now.", off.name));
            cx.notify();
            return false;
        }
        match self.show {
            Show::New => {
                if let Some(reason) = self.model_note() {
                    self.problem = Some(reason);
                    cx.notify();
                    return false;
                }
                let options = self.new_options(refs);
                self.start(self.new_kind, Some(text), options, window, cx);
                true
            }
            _ => match self.current().cloned() {
                Some(t) if t.running => self.send(&t, &text, &refs, cx),
                Some(t) => {
                    let mut options = self.resume_options(&t);
                    options.refs = refs;
                    self.restart(&t, options, Some(text), cx);
                    true
                }
                None => false,
            },
        }
    }

    /// A notebook asks the agent about one of its cells (its Fix and
    /// Explain buttons): the message names the cell with #, so the agent
    /// is told what the cell last showed.
    pub(crate) fn ask_about_cell(
        &mut self,
        text: String,
        cell: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show != Show::New && self.current().is_none() {
            self.show = Show::New;
        }
        self.say(text, vec![cell], window, cx);
        cx.notify();
    }

    /// Notebook shortcuts always start a fresh conversation using the
    /// agent/model currently selected in the composer.
    pub(crate) fn start_notebook_conversation(
        &mut self,
        action: crate::agents::notebook_ai::NotebookAction,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.starting.is_some() {
            return;
        }
        let current = (self.show != Show::New)
            .then(|| self.current().cloned())
            .flatten();
        let (kind, options) = if let Some(current) = current {
            let Some(kind) = kind_of(&current.kind) else {
                return;
            };
            (kind.kind, self.resume_options(&current))
        } else {
            if let Some(reason) = self.model_note() {
                self.problem = Some(reason);
                cx.notify();
                return;
            }
            (self.new_kind, self.new_options(Vec::new()))
        };
        let prompt = action.prompt(&path);
        let options = action.options(options, &path);
        self.start(kind, Some(prompt), options, window, cx);
    }

    /// The switch for notebooks following the agent's edits, in every project.
    pub(crate) fn toggle_follow_edits(&mut self, cx: &mut Context<Self>) {
        crate::shell::choices::choose(
            crate::shell::choices::Choice::FollowAgent(!self.follow_edits),
            cx,
        );
    }

    /// Starts a conversation in the project's folder, with its first
    /// message.
    pub(crate) fn start(
        &mut self,
        kind: &'static str,
        prompt: Option<String>,
        mut options: AgentOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = kind_of(kind).map_or(kind, |k| k.name);
        if self
            .installed
            .iter()
            .find(|a| a.kind == kind)
            .is_some_and(|a| a.path.is_none())
        {
            self.problem = Some(format!("{name} isn't installed on this machine."));
            cx.notify();
            return;
        }
        options.account = crate::prefs::agent_account(&self.host, kind);
        let (prompt, first) = first_message(kind, prompt, &mut options);
        self.starting = Some(name);
        self.problem = None;
        let started = self.daemon.call::<TerminalInfo>(Call::StartTerminal {
            kind: kind.into(),
            cwd: Some(self.project.clone()),
            project: Some(self.project.clone()),
            cols: Some(START_COLUMNS),
            rows: Some(START_ROWS),
            options,
            prompt,
        });
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let started = started.await;
            this.update_in(cx, |this, window, cx| {
                this.starting = None;
                match started {
                    Ok(info) => {
                        let id = info.id.clone();
                        if let Some(first) = first {
                            this.typing.insert(id.clone(), first);
                        }
                        this.follow(&info, cx);
                        this.terminals.push(info);
                        this.new_choice = Choice::default();
                        this.browsed_effort = None;
                        this.select(id, window, cx);
                    }
                    Err(err) => this.problem = Some(format!("Could not start {name}: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Sends a message: to the bridge of an agent Sandhill drives, or as if
    /// typed into the agent's terminal.
    fn send(
        &mut self,
        t: &TerminalInfo,
        text: &str,
        refs: &[Value],
        cx: &mut Context<Self>,
    ) -> bool {
        if self.trust_asked.contains(&t.id) {
            self.problem = Some("Answer Claude Code's question first.".into());
            cx.notify();
            return false;
        }
        if !self.chat(t).waiting.is_empty() {
            let who = kind_of(&t.kind).map_or("the agent", |k| k.short);
            self.problem = Some(format!("Answer {who}'s question above first."));
            cx.notify();
            return false;
        }
        self.stopped.remove(&t.id);
        if drives_chat(&t.kind) {
            if refs.is_empty() {
                self.tell(t, json!({"kind": "say", "text": text}));
            } else {
                self.tell(t, json!({"kind": "say", "text": text, "refs": refs}));
            }
        } else if terminal_only(&t.kind) {
            self.type_message(&t.id, &t.kind, &with_refs(text, refs), cx);
        } else {
            // The chips go first, so Claude Code's prompt hook finds them.
            if !refs.is_empty() {
                self.tell(t, json!({"kind": "set", "refs": refs}));
            }
            self.type_message(&t.id, &t.kind, text, cx);
        }
        self.problem = None;
        self.chat_scroll.scroll_to_bottom();
        true
    }

    /// Types a message into the agent's terminal: one paste, so line breaks
    /// don't send it early, then Enter.
    pub(super) fn type_message(&self, id: &str, kind: &str, text: &str, cx: &mut Context<Self>) {
        let (text, delay) = if terminal_only(kind) {
            (text.to_string(), POSIT_SUBMIT_DELAY)
        } else {
            (text.replace('\n', "\r"), SUBMIT_DELAY)
        };
        let enter = Call::TerminalInput {
            terminal: id.to_string(),
            data: "\r".into(),
        };
        let pasted = self.daemon.call_in_order::<Value>(Call::TerminalInput {
            terminal: id.to_string(),
            data: format!("\x1b[200~{text}\x1b[201~"),
        });
        let daemon = self.daemon.clone();
        cx.spawn(async move |_, cx| {
            if pasted.await.is_ok() {
                cx.background_executor().timer(delay).await;
                daemon.fire(enter);
            }
        })
        .detach();
    }

    /// Stops what the agent is doing, as Esc does.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else {
            return;
        };
        if drives_chat(&t.kind) {
            self.tell(&t, json!({"kind": "interrupt"}));
        } else {
            self.daemon.fire(Call::TerminalInput {
                terminal: t.id.clone(),
                data: "\x1b".into(),
            });
        }
        self.stopped.insert(t.id);
        self.problem = None;
        cx.notify();
    }

    pub(crate) fn tell(&self, t: &TerminalInfo, command: Value) {
        self.daemon.fire(Call::TellTerminal {
            terminal: t.id.clone(),
            command,
        });
    }

    /// Answers what the agent asked: `{behavior, scope?, answers?}`.
    pub(crate) fn answer(&mut self, ask: &str, answer: Value, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else {
            return;
        };
        self.sent.insert(ask.to_string());
        let answered = self.daemon.call::<Value>(Call::AnswerTerminal {
            terminal: t.id,
            id: ask.to_string(),
            answer,
        });
        let ask = ask.to_string();
        cx.spawn(async move |this, cx| {
            let answered = answered.await;
            this.update(cx, |this, cx| {
                this.problem = match answered {
                    Ok(_) => None,
                    Err(err) => {
                        this.sent.remove(&ask);
                        Some(format!("Could not answer: {err:#}"))
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Gives each question waiting in the conversation shown an "Other…"
    /// field.
    pub(crate) fn question_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else {
            return;
        };
        for ask in self.chat(&t).waiting {
            for q in chat::ask_view(&ask, None, &t.kind).questions {
                let key = question_key(&ask.id, &q.question);
                if self.others.contains_key(&key) {
                    continue;
                }
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Other…"));
                self._subscriptions
                    .push(cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()));
                self.others.insert(key, input);
            }
        }
    }

    /// Picks an option of a question, or (with several allowed) toggles it.
    pub(crate) fn pick_option(
        &mut self,
        key: String,
        label: String,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        let picked = self.picks.entry(key).or_default();
        if !multi {
            *picked = vec![label];
        } else if let Some(at) = picked.iter().position(|l| *l == label) {
            picked.remove(at);
        } else {
            picked.push(label);
        }
        cx.notify();
    }

    /// A question's answer: the options picked and the words typed.
    pub(crate) fn answer_of(&self, key: &str, cx: &App) -> String {
        let mut parts = self.picks.get(key).cloned().unwrap_or_default();
        if let Some(other) = self.others.get(key) {
            let text = other.read(cx).value().trim().to_string();
            if !text.is_empty() {
                parts.push(text);
            }
        }
        parts.join(", ")
    }

    pub(crate) fn toggle_step(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.open_steps.remove(key) {
            self.open_steps.insert(key.to_string());
        }
        cx.notify();
    }
}
