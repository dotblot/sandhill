use super::*;

impl Agents {
    /// The models driven-chat providers offer before a conversation.
    pub(crate) fn list_models(&mut self, kind: String, cx: &mut Context<Self>) {
        let listed = self
            .daemon
            .call::<Vec<Value>>(Call::AgentModels { kind: kind.clone() });
        cx.spawn(async move |this, cx| {
            if let Ok(models) = listed.await {
                this.update(cx, |this, cx| {
                    this.listed.insert(kind, models);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The conversation the box is for, unless it is for a new one.
    pub(crate) fn box_terminal(&self) -> Option<&TerminalInfo> {
        if self.show == Show::New {
            None
        } else {
            self.current()
        }
    }

    pub(crate) fn box_kind(&self) -> &str {
        self.box_terminal()
            .map_or(self.new_kind, |t| t.kind.as_str())
    }

    pub(super) fn picked_for(&self, t: &TerminalInfo) -> Choice {
        self.picked.get(&t.id).cloned().unwrap_or_default()
    }

    /// The models of an agent, as the box offers them.
    pub(crate) fn models_of(&self, kind: &str) -> Models {
        if let Some(t) = self.box_terminal().filter(|t| t.kind == kind) {
            let chosen = self
                .picked
                .get(&t.id)
                .and_then(|c| c.model.as_ref())
                .and_then(|m| m.id.as_deref());
            if t.agent
                .as_ref()
                .and_then(|a| a["models"].as_array())
                .is_none_or(Vec::is_empty)
                && let Some(listed) = self.listed.get(kind)
            {
                let id = composer::model_id(t);
                return composer::listed_models(kind, listed, chosen.or(id.as_deref()));
            }
            return composer::running_models(t, chosen);
        }
        let chosen = (kind == self.new_kind)
            .then_some(&self.new_choice)
            .and_then(|c| c.model.as_ref())
            .and_then(|m| m.id.as_deref());
        match self.listed.get(kind) {
            Some(listed) => composer::listed_models(kind, listed, chosen),
            None => composer::known_models(kind),
        }
    }

    /// The model the box shows.
    pub(crate) fn box_model(&self) -> String {
        let models = self.models_of(self.box_kind());
        match self.box_terminal() {
            Some(t) => {
                let picked = self.picked_for(t).model;
                let id = picked
                    .as_ref()
                    .and_then(|m| m.id.clone())
                    .or_else(|| composer::model_id(t));
                if let Some(id) = id.as_deref()
                    && models.variant_effort(Some(id)).is_some()
                {
                    return models.choice(id).label;
                }
                picked
                    .map(|m| m.label)
                    .unwrap_or_else(|| composer::model_label(t))
            }
            None => {
                let model = self.new_model();
                model
                    .id
                    .as_deref()
                    .map_or(model.label.clone(), |id| models.choice(id).label)
            }
        }
    }

    pub(super) fn new_model(&self) -> ModelChoice {
        if let Some(m) = &self.new_choice.model {
            return m.clone();
        }
        let models = self.models_of(self.new_kind);
        models.models[models.start.min(models.models.len() - 1)].clone()
    }

    /// The effort the box shows; `None` is the agent's own default.
    pub(crate) fn box_effort(&self) -> Option<String> {
        let id = match self.box_terminal() {
            Some(t) => self
                .picked_for(t)
                .model
                .and_then(|m| m.id)
                .or_else(|| composer::model_id(t)),
            None => self.new_model().id,
        };
        if let Some(level) = self
            .models_of(self.box_kind())
            .variant_effort(id.as_deref())
        {
            return Some(level.to_string());
        }
        match self.box_terminal() {
            Some(t) => self.picked_for(t).effort.or_else(|| composer::effort_of(t)),
            None => self.new_choice.effort.clone(),
        }
    }

    /// The agent whose thinking effort the model menu shows: the open tab's,
    /// or with Favourites the box's own.
    pub(crate) fn effort_kind(&self) -> String {
        let tab = self.open_model_tab();
        if tab == "fav" {
            self.box_kind().to_string()
        } else {
            tab
        }
    }

    /// The effort the menu shows for `kind`; `None` is the agent's own
    /// default. Another agent's tab (new conversations only) shows the level
    /// set there, else the one a model picked there would keep.
    pub(crate) fn effort_shown(&self, kind: &str) -> Option<String> {
        if kind == self.box_kind() {
            return self.box_effort();
        }
        match &self.browsed_effort {
            Some((k, level)) if k == kind => Some(level.clone()),
            _ => self.new_choice.effort.clone(),
        }
    }

    /// The mode the box shows; `None` when the agent has no modes.
    pub(crate) fn box_mode(&self) -> Option<&'static Mode> {
        match self.box_terminal() {
            Some(t) => match self.picked_for(t).mode {
                Some(picked) => composer::mode(&t.kind, Some(picked)),
                None => composer::current_mode(t),
            },
            None => composer::mode(self.new_kind, self.new_choice.mode),
        }
    }

    pub(crate) fn box_console(&self) -> bool {
        match self.box_terminal() {
            Some(t) => self
                .picked_for(t)
                .console
                .unwrap_or_else(|| composer::code_to_console(t)),
            None => self.new_choice.console.unwrap_or(false),
        }
    }

    pub(crate) fn is_model(&self, kind: &str, model: &ModelChoice) -> bool {
        if kind != self.box_kind() {
            return false;
        }
        match self.box_terminal() {
            Some(t) => {
                let picked = self.picked_for(t).model.and_then(|m| m.id);
                let id = picked.or_else(|| composer::model_id(t));
                self.models_of(kind)
                    .same_model(model.id.as_deref(), id.as_deref())
            }
            None => self
                .models_of(kind)
                .same_model(self.new_model().id.as_deref(), model.id.as_deref()),
        }
    }

    /// A model can be picked: any agent's for a new conversation, else the
    /// conversation's own agent's.
    pub(crate) fn usable(&self, kind: &str) -> bool {
        self.box_terminal().is_none_or(|t| t.kind == kind)
    }

    /// Why the box's choices wait, if they do.
    pub(crate) fn model_note(&self) -> Option<String> {
        if self.box_terminal().is_none() {
            let name = crate::agents::kind_of(self.new_kind).map_or(self.new_kind, |k| k.name);
            if self.installed.is_empty() {
                return Some(format!("Checking {name} availability…"));
            }
            if !self
                .installed
                .iter()
                .any(|a| a.kind == self.new_kind && a.path.is_some())
            {
                return Some(format!(
                    "{name} is not installed. Choose another provider or install its CLI."
                ));
            }
            if self.new_kind == "antigravity" && self.new_model().id.is_none() {
                return Some(
                    "Waiting for Antigravity's model list. Refresh agents if it does not load."
                        .into(),
                );
            }
        }
        let t = self.box_terminal()?;
        let who = crate::agents::kind_of(&t.kind).map_or("the agent", |k| k.short);
        if !t.running {
            return Some(format!("Applies as {who} goes on with your next message."));
        }
        if drives_chat(&t.kind) {
            return self
                .working(t)
                .then(|| "Changes apply from your next message.".to_string());
        }
        self.waiting(t)
            .then(|| format!("Changes here wait until you answer {who}."))
    }

    pub(crate) fn choose_model(
        &mut self,
        kind: &str,
        model: ModelChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == "antigravity" && model.id.is_none() {
            return;
        }
        self.effort_generation += 1;
        let encoded = self
            .models_of(kind)
            .variant_effort(model.id.as_deref())
            .is_some();
        let Some(t) = self.box_terminal().cloned() else {
            let Some(k) = crate::agents::kind_of(kind) else {
                return;
            };
            self.new_kind = k.kind;
            self.new_choice.model = Some(model);
            let carried = match self.browsed_effort.take() {
                Some((k, level)) if k == kind => Some(level),
                _ => self.new_choice.effort.take(),
            };
            self.new_choice.effort =
                carried.filter(|e| !encoded && self.models_of(kind).efforts.contains(e));
            self.menu = None;
            self.problem = None;
            cx.notify();
            return;
        };
        if !self.usable(kind) {
            return;
        }
        let pick = |this: &mut Self, model: ModelChoice| {
            let choice = this.picked.entry(t.id.clone()).or_default();
            choice.model = Some(model);
            if encoded {
                choice.effort = None;
            }
        };
        if !t.running {
            pick(self, model);
        } else if drives_chat(&t.kind) {
            self.tell(
                &t,
                if encoded {
                    json!({"kind": "set", "model": model.id, "effort": null})
                } else {
                    json!({"kind": "set", "model": model.id})
                },
            );
            pick(self, model);
        } else if let Some(id) = model.id.clone() {
            pick(self, model);
            self.command(&t, format!("/model {id}"), cx);
        } else {
            self.command(&t, "/model".into(), cx);
            self.show(Show::Terminal, window, cx);
        }
        self.menu = None;
        self.problem = None;
        cx.notify();
    }

    pub(crate) fn choose_effort(&mut self, kind: &str, level: String, cx: &mut Context<Self>) {
        let models = self.models_of(kind);
        if !models.efforts.contains(&level) {
            return;
        }
        if kind != self.box_kind() {
            if self.box_terminal().is_none() {
                self.browsed_effort = Some((kind.to_string(), level));
                cx.notify();
            }
            return;
        }
        self.problem = None;
        let current = match self.box_terminal() {
            Some(t) => self
                .picked_for(t)
                .model
                .and_then(|m| m.id)
                .or_else(|| composer::model_id(t)),
            None => self.new_model().id,
        };
        let variant = models.effort_model(current.as_deref(), &level);
        let Some(t) = self.box_terminal().cloned() else {
            if let Some(model) = variant {
                self.new_choice.model = Some(model);
                self.new_choice.effort = None;
                cx.notify();
                return;
            }
            self.new_choice.effort = Some(level);
            cx.notify();
            return;
        };
        let choice = self.picked.entry(t.id.clone()).or_default();
        if let Some(model) = &variant {
            choice.model = Some(model.clone());
            choice.effort = None;
        } else {
            choice.effort = Some(level.clone());
        }
        cx.notify();
        if !t.running {
            return;
        }
        self.effort_generation += 1;
        let generation = self.effort_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EFFORT_SETTLE).await;
            this.update(cx, |this, cx| {
                if this.effort_generation != generation {
                    return;
                }
                if drives_chat(&t.kind) {
                    this.tell(
                        &t,
                        if let Some(model) = &variant {
                            json!({"kind": "set", "model": model.id, "effort": null})
                        } else {
                            json!({"kind": "set", "effort": level})
                        },
                    );
                } else {
                    this.command(&t, format!("/effort {level}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Why a mode can't be picked now, if it can't.
    pub(crate) fn mode_note(&self, mode: &Mode) -> Option<String> {
        let t = self.box_terminal()?;
        if !t.running || drives_chat(&t.kind) {
            return None;
        }
        let other = self.box_mode().map(|m| m.permission) != Some(mode.permission);
        if t.kind != "claude" {
            return other.then(|| "Change this in the agent's terminal".to_string());
        }
        if self.waiting(t) && other {
            return Some("Answer Claude first".into());
        }
        if mode.permission == "full" && other && !composer::can_resume(t) {
            return Some(format!("Start a new conversation in {}", mode.label));
        }
        None
    }

    /// What the mode menu says a mode does here.
    pub(crate) fn mode_text(&self, mode: &Mode) -> String {
        if let Some(note) = self.mode_note(mode) {
            return note;
        }
        if let Some(t) = self.box_terminal()
            && t.running
            && t.kind == "claude"
            && mode.permission == "full"
            && self.box_mode().map(|m| m.permission) != Some("full")
        {
            return format!(
                "Claude Code turns {} on only as it starts: it starts again and goes on with this conversation.",
                mode.label
            );
        }
        mode.detail.to_string()
    }

    pub(crate) fn choose_mode(&mut self, mode: &'static Mode, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(t) = self.box_terminal().cloned() else {
            self.new_choice.mode = Some(mode.permission);
            self.problem = None;
            cx.notify();
            return;
        };
        if self.mode_note(mode).is_some()
            || self.box_mode().map(|m| m.permission) == Some(mode.permission)
        {
            cx.notify();
            return;
        }
        self.problem = None;
        if !t.running {
            self.picked.entry(t.id.clone()).or_default().mode = Some(mode.permission);
        } else if t.kind == "claude" && mode.permission == "full" {
            let mut options = composer::live_options(&t);
            options.permission = Some(mode.permission.into());
            self.restart(&t, options, None, cx);
        } else if drives_chat(&t.kind) {
            self.picked.entry(t.id.clone()).or_default().mode = Some(mode.permission);
            self.tell(&t, json!({"kind": "set", "permission": mode.permission}));
        } else {
            self.switch_mode(&t, mode, cx);
        }
        cx.notify();
    }

    /// Switches Claude Code's mode as ⇧Tab does, until its terminal shows
    /// it; then the daemon is told which is on.
    fn switch_mode(&mut self, t: &TerminalInfo, mode: &'static Mode, cx: &mut Context<Self>) {
        let Some(view) = self.view(&t.id) else {
            return;
        };
        let t = t.clone();
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            for i in 0..=MODE_TRIES {
                let shown = view.read_with(cx, |v, cx| composer::screen_mode(&v.screen_text(cx)));
                let Some(shown) = shown else { break };
                if shown == mode.permission {
                    this.update(cx, |this, cx| {
                        this.picked.entry(t.id.clone()).or_default().mode = Some(mode.permission);
                        this.tell(&t, json!({"kind": "set", "permission": mode.permission}));
                        this.problem = None;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                if i == MODE_TRIES {
                    break;
                }
                daemon.fire(Call::TerminalInput {
                    terminal: t.id.clone(),
                    data: "\x1b[Z".into(),
                });
                cx.background_executor().timer(MODE_STEP).await;
            }
            this.update(cx, |this, cx| {
                this.problem = Some(format!(
                    "Claude Code didn't switch to {} here. Its terminal shows the modes it offers: ⇧Tab switches.",
                    mode.label
                ));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Where the agent's code goes: the focus file, or the Console.
    pub(crate) fn set_console(&mut self, console: bool, cx: &mut Context<Self>) {
        match self.box_terminal().cloned() {
            None => self.new_choice.console = Some(console),
            Some(t) => {
                self.picked.entry(t.id.clone()).or_default().console = Some(console);
                if t.running {
                    let code = if console {
                        json!("console")
                    } else {
                        Value::Null
                    };
                    self.tell(&t, json!({"kind": "set", "code": code}));
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn compact(&mut self, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(t) = self.box_terminal().cloned() {
            if drives_chat(&t.kind) {
                self.tell(&t, json!({"kind": "compact"}));
            } else {
                self.command(&t, "/compact".into(), cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn star(&mut self, kind: &str, model: &ModelChoice, cx: &mut Context<Self>) {
        let models = self.models_of(kind);
        let matching = |key: &str| {
            key.split_once(':').is_some_and(|(k, id)| {
                k == kind && models.same_model(Some(id), model.id.as_deref())
            })
        };
        if self.favourites.iter().any(|key| matching(key)) {
            self.favourites.retain(|key| !matching(key));
        } else {
            self.favourites.push(favourite_key(kind, model));
        }
        crate::prefs::save_favourites(&self.favourites);
        cx.notify();
    }
}
