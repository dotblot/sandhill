//! The message box's workings, as the Tauri app's `AgentComposer.tsx` and
//! `AgentsPane.tsx` have them: drafts kept per conversation, the @ and #
//! menus and their chips, attaching files, and the model, effort and mode
//! choices, for a new conversation or the one shown.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use gpui_kit::*;
use sandhill_proto::{
    AgentOptions, Call, FileText, SessionInfo, SessionStatus, TerminalInfo, Variables,
};
use serde_json::{Value, json};

use crate::agents::{
    Agents, Choice, Menu, START_COLUMNS, START_ROWS, SUBMIT_DELAY, Show, drives_chat,
};
use crate::composer::{
    self, ATTACHMENTS, Mention, Mode, ModelChoice, Models, cell_documents, cell_mentions,
    document_cells, filter_cells, filter_mentions, same_ref, typed_mention,
};

/// Effort changes wait this long for the next one (the slider can be dragged).
const EFFORT_SETTLE: Duration = Duration::from_millis(600);
/// Switching Claude Code's mode presses ⇧Tab until its screen shows it.
const MODE_TRIES: usize = 6;
const MODE_STEP: Duration = Duration::from_millis(350);

impl Agents {
    /// The draft the box should hold now: the shown conversation's, or the
    /// new one's.
    pub(crate) fn wanted_draft(&self) -> String {
        match (&self.show, &self.selected) {
            (Show::New, _) | (_, None) => "new".into(),
            (_, Some(id)) => id.clone(),
        }
    }

    /// Keeps the box's text and chips under the conversation they were for
    /// and shows the ones of the conversation shown now.
    pub(crate) fn swap_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.wanted_draft();
        if key == self.draft_key {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        let old = std::mem::replace(&mut self.draft_key, key.clone());
        if text.is_empty() && self.refs.is_empty() {
            self.drafts.remove(&old);
        } else {
            self.drafts
                .insert(old, (text, std::mem::take(&mut self.refs)));
        }
        let (text, refs) = self.drafts.remove(&key).unwrap_or_default();
        self.refs = refs;
        self.menu = None;
        self.composer
            .update(cx, |c, cx| c.set_value(text, window, cx));
    }

    /// The text changed: opens, narrows or closes the @ and # menus.
    pub(crate) fn composer_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (text, caret) = {
            let c = self.composer.read(cx);
            (c.value().to_string(), c.cursor())
        };
        match typed_mention(&text, caret) {
            Some((_, query, sign)) => self.open_mention(sign, query, cx),
            None => {
                if matches!(self.menu, Some(Menu::Mention { .. })) {
                    self.menu = None;
                    cx.notify();
                }
            }
        }
    }

    /// Opens the @ menu (or #), reading what it offers the first time.
    fn open_mention(&mut self, sign: char, query: String, cx: &mut Context<Self>) {
        if let Some(Menu::Mention {
            sign: open,
            query: q,
            active,
            ..
        }) = &mut self.menu
            && *open == sign
        {
            if *q != query {
                *q = query;
                *active = 0;
            }
            cx.notify();
            return;
        }
        self.menu = Some(Menu::Mention {
            sign,
            query,
            items: None,
            active: 0,
        });
        cx.notify();
        if sign == '#' {
            self.load_cells(cx);
        } else {
            self.load_mentions(cx);
        }
    }

    fn loaded(&mut self, sign: char, list: Vec<Mention>, cx: &mut Context<Self>) {
        if let Some(Menu::Mention {
            sign: open, items, ..
        }) = &mut self.menu
            && *open == sign
        {
            *items = Some(list);
            cx.notify();
        }
    }

    /// The project's files and the variables of its running kernels.
    fn load_mentions(&mut self, cx: &mut Context<Self>) {
        let daemon = self.daemon.clone();
        let project = self.project.clone();
        let files = daemon.call::<Vec<String>>(Call::ListFiles {
            path: project.clone(),
            limit: None,
        });
        let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let mut list: Vec<Mention> = files
                .await
                .unwrap_or_default()
                .iter()
                .map(|f| Mention::file(f))
                .collect();
            let sessions = sessions.await.unwrap_or_default();
            for s in sessions.iter().filter(|s| {
                s.status == SessionStatus::Running && s.project.as_deref() == Some(&project)
            }) {
                let listed = daemon.call::<Variables>(Call::ListVariables {
                    session: s.id.clone(),
                    path: Vec::new(),
                });
                if let Ok(vars) = listed.await {
                    list.extend(vars.variables.iter().map(|v| Mention {
                        item: json!({
                            "kind": "variable",
                            "name": v.name,
                            "session": s.id,
                            "language": s.language,
                        }),
                        label: None,
                        detail: None,
                    }));
                }
            }
            this.update(cx, |this, cx| this.loaded('@', list, cx)).ok();
        })
        .detach();
    }

    /// The cells of the documents named with @: as edited in their tab,
    /// else as saved.
    fn load_cells(&mut self, cx: &mut Context<Self>) {
        let mut list = Vec::new();
        let mut unread = Vec::new();
        for path in cell_documents(&self.refs) {
            let full = self.full_path(&path);
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let open = self
                .workbench
                .upgrade()
                .and_then(|w| w.read(cx).open_cells(&full, cx));
            match open {
                Some(cells) => list.extend(cell_mentions(&path, &name, &cells)),
                None => unread.push((path, name, full)),
            }
        }
        if unread.is_empty() {
            return self.loaded('#', list, cx);
        }
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            for (path, name, full) in unread {
                if let Ok(file) = daemon.call::<FileText>(Call::ReadFile { path: full }).await {
                    list.extend(cell_mentions(
                        &path,
                        &name,
                        &document_cells(&path, &file.text),
                    ));
                }
            }
            this.update(cx, |this, cx| this.loaded('#', list, cx)).ok();
        })
        .detach();
    }

    /// A path the agent was given, on the daemon's machine.
    pub(crate) fn full_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{path}", self.project.trim_end_matches('/'))
        }
    }

    /// Opens a file the agent named in the editor, if it is in the project.
    pub(crate) fn open_path(&mut self, path: &str, cx: &mut Context<Self>) {
        let root = format!("{}/", self.project.trim_end_matches('/'));
        let rel = path.strip_prefix(&root).unwrap_or(path);
        if rel.starts_with('/') {
            self.problem = Some(format!("{path} is outside the project"));
            cx.notify();
            return;
        }
        let rel = rel.to_string();
        if let Some(w) = self.workbench.upgrade() {
            w.update(cx, |w, cx| w.open_file(rel, cx));
        }
    }

    /// The open @ or # menu's groups, as shown.
    pub(crate) fn mention_groups(&self) -> Vec<(String, Vec<Mention>)> {
        let Some(Menu::Mention {
            sign,
            query,
            items: Some(items),
            ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let groups = if *sign == '#' {
            filter_cells(items, query)
        } else {
            filter_mentions(items, query)
        };
        groups
            .into_iter()
            .map(|(label, list)| (label, list.into_iter().cloned().collect()))
            .collect()
    }

    fn shown_mentions(&self) -> Vec<Mention> {
        self.mention_groups()
            .into_iter()
            .flat_map(|(_, list)| list)
            .collect()
    }

    /// Arrows in the open @ menu.
    pub(crate) fn move_mention(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.shown_mentions().len();
        if let Some(Menu::Mention { active, .. }) = &mut self.menu
            && n > 0
        {
            *active = if down {
                (*active + 1) % n
            } else {
                (*active + n - 1) % n
            };
            cx.notify();
        }
    }

    /// Adds the picked item (or the highlighted one) as a chip; the "@"
    /// and what was typed after it go.
    pub(crate) fn pick_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = match &self.menu {
            Some(Menu::Mention { active, .. }) => *active,
            _ => return,
        };
        let picked = self
            .shown_mentions()
            .into_iter()
            .nth(index.unwrap_or(active));
        self.close_menu(window, cx);
        if let Some(m) = picked {
            self.add_ref(m.item);
        }
        cx.notify();
    }

    pub(crate) fn add_ref(&mut self, item: Value) {
        if !self.refs.iter().any(|r| same_ref(r, &item)) {
            self.refs.push(item);
        }
    }

    /// Closes the open menu; the @ menu takes what was typed for it along.
    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.menu.take(), Some(Menu::Mention { .. })) {
            self.composer.update(cx, |c, cx| {
                let caret = c.cursor();
                if let Some((at, _, _)) = typed_mention(&c.value(), caret) {
                    c.set_selected_range(at..caret, cx);
                    c.replace("", window, cx);
                }
                c.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(crate) fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let same = self
            .menu
            .as_ref()
            .is_some_and(|m| std::mem::discriminant(m) == std::mem::discriminant(&menu));
        self.close_menu(window, cx);
        if !same {
            if menu == Menu::Model {
                self.model_tab = None;
            }
            self.menu = Some(menu);
        }
        cx.notify();
    }

    pub(crate) fn remove_ref(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.refs.len() {
            self.refs.remove(index);
        }
        cx.notify();
    }

    /// Picks files on this computer and saves them in the project's
    /// attachments folder (kept out of git), where the agent can read them.
    pub(crate) fn attach(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            for path in paths {
                let result = attach_file(&daemon, &folder, &path).await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(item) => this.add_ref(item),
                        Err(err) => {
                            this.problem =
                                Some(format!("Could not attach {}: {err:#}", path.display()))
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The models Codex and Antigravity offer before a conversation.
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

    fn picked_for(&self, t: &TerminalInfo) -> Choice {
        self.picked.get(&t.id).cloned().unwrap_or_default()
    }

    /// The models of an agent, as the box offers them.
    pub(crate) fn models_of(&self, kind: &str) -> Models {
        if let Some(t) = self.box_terminal().filter(|t| t.kind == kind) {
            return composer::running_models(t);
        }
        let chosen = self.new_choice.model.as_ref().and_then(|m| m.id.as_deref());
        match self.listed.get(kind) {
            Some(listed) => composer::listed_models(kind, listed, chosen),
            None => composer::known_models(kind),
        }
    }

    /// The model the box shows.
    pub(crate) fn box_model(&self) -> String {
        match self.box_terminal() {
            Some(t) => self
                .picked_for(t)
                .model
                .map(|m| m.label)
                .unwrap_or_else(|| composer::model_label(t)),
            None => self.new_model().label,
        }
    }

    fn new_model(&self) -> ModelChoice {
        if let Some(m) = &self.new_choice.model {
            return m.clone();
        }
        let models = self.models_of(self.new_kind);
        models.models[models.start.min(models.models.len() - 1)].clone()
    }

    /// The effort the box shows; `None` is the agent's own default.
    pub(crate) fn box_effort(&self) -> Option<String> {
        match self.box_terminal() {
            Some(t) => self.picked_for(t).effort.or_else(|| composer::effort_of(t)),
            None => self.new_choice.effort.clone(),
        }
    }

    pub(crate) fn box_mode(&self) -> &'static Mode {
        match self.box_terminal() {
            Some(t) => self
                .picked_for(t)
                .mode
                .map(composer::mode)
                .unwrap_or_else(|| composer::current_mode(t)),
            None => {
                let mode = composer::mode(self.new_choice.mode.unwrap_or("normal"));
                if composer::has_mode(self.new_kind, mode) {
                    mode
                } else {
                    &composer::MODES[0]
                }
            }
        }
    }

    /// Web search (`"web"`) or session context (`"context"`), on or off.
    pub(crate) fn box_switch(&self, key: &str) -> bool {
        let pick = |c: &Choice| if key == "web" { c.web } else { c.context };
        match self.box_terminal() {
            Some(t) => pick(&self.picked_for(t)).unwrap_or_else(|| composer::switch_on(t, key)),
            None => pick(&self.new_choice).unwrap_or(true),
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
            Some(t) => match self.picked_for(t).model {
                Some(picked) => picked.id == model.id,
                None => {
                    let id = composer::model_id(t);
                    if id.is_some() {
                        model.id == id
                    } else {
                        model.label == composer::model_label(t)
                    }
                }
            },
            None => self.new_model().id == model.id,
        }
    }

    /// A model can be picked: any agent's for a new conversation, else the
    /// conversation's own agent's.
    pub(crate) fn usable(&self, kind: &str) -> bool {
        self.box_terminal().is_none_or(|t| t.kind == kind)
    }

    /// Why the box's choices wait, if they do.
    pub(crate) fn model_note(&self) -> Option<String> {
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
        let Some(t) = self.box_terminal().cloned() else {
            if kind != self.new_kind {
                let Some(k) = crate::agents::kind_of(kind) else {
                    return;
                };
                // Another agent keeps the effort if it has that level.
                let keep = self
                    .new_choice
                    .effort
                    .clone()
                    .filter(|e| self.models_of(k.kind).efforts.contains(e));
                self.new_kind = k.kind;
                self.new_choice.effort = keep;
            }
            self.new_choice.model = Some(model);
            self.menu = None;
            cx.notify();
            return;
        };
        if !self.usable(kind) {
            return;
        }
        let pick = |this: &mut Self, model: ModelChoice| {
            this.picked.entry(t.id.clone()).or_default().model = Some(model);
        };
        if !t.running {
            pick(self, model);
        } else if drives_chat(&t.kind) {
            self.tell(&t, json!({"kind": "set", "model": model.id}));
            pick(self, model);
        } else if let Some(id) = model.id.clone() {
            pick(self, model);
            self.command(&t, format!("/model {id}"), cx);
        } else {
            self.command(&t, "/model".into(), cx);
            self.show(Show::Terminal, window, cx);
        }
        self.menu = None;
        cx.notify();
    }

    pub(crate) fn choose_effort(&mut self, level: String, cx: &mut Context<Self>) {
        let Some(t) = self.box_terminal().cloned() else {
            self.new_choice.effort = Some(level);
            cx.notify();
            return;
        };
        self.picked.entry(t.id.clone()).or_default().effort = Some(level.clone());
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
                    this.tell(&t, json!({"kind": "set", "effort": level}));
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
        let current = self.box_mode();
        if t.kind != "claude" {
            return (mode.id != current.id)
                .then(|| "Change this in the agent's terminal".to_string());
        }
        if self.waiting(t) && mode.id != current.id {
            return Some("Answer Claude first".into());
        }
        if mode.id == "yolo" && current.id != "yolo" && !composer::can_resume(t) {
            return Some("Start a new conversation in YOLO mode".into());
        }
        None
    }

    /// What the mode menu says a mode does here.
    pub(crate) fn mode_text(&self, mode: &Mode) -> String {
        if let Some(note) = self.mode_note(mode) {
            return note;
        }
        let kind = self.box_kind();
        if let Some(t) = self.box_terminal()
            && t.running
            && t.kind == "claude"
            && mode.id == "yolo"
            && self.box_mode().id != "yolo"
        {
            return "Claude Code turns YOLO on only as it starts: it starts again and goes on with this conversation.".into();
        }
        composer::mode_detail(kind, mode).to_string()
    }

    pub(crate) fn choose_mode(&mut self, mode: &'static Mode, cx: &mut Context<Self>) {
        self.menu = None;
        let Some(t) = self.box_terminal().cloned() else {
            self.new_choice.mode = Some(mode.id);
            cx.notify();
            return;
        };
        if self.mode_note(mode).is_some() || self.box_mode().id == mode.id {
            cx.notify();
            return;
        }
        if !t.running {
            self.picked.entry(t.id.clone()).or_default().mode = Some(mode.id);
        } else if t.kind == "claude" && mode.id == "yolo" {
            let mut options = composer::live_options(&t);
            options.permission = Some(mode.permission.into());
            self.restart(&t, options, None, cx);
        } else if drives_chat(&t.kind) {
            self.picked.entry(t.id.clone()).or_default().mode = Some(mode.id);
            self.tell(&t, json!({"kind": "set", "permission": mode.permission}));
        } else {
            self.switch_mode(&t, mode, cx);
        }
        cx.notify();
    }

    /// Switches Claude Code's mode as ⇧Tab does, until its terminal shows
    /// it. Restricted and Ask are Sandhill's, on top of Claude Code's
    /// default mode: the daemon is told which is on.
    fn switch_mode(&mut self, t: &TerminalInfo, mode: &'static Mode, cx: &mut Context<Self>) {
        let Some(view) = self.view(&t.id) else {
            return;
        };
        let t = t.clone();
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            for i in 0..=MODE_TRIES {
                let shown = view.read_with(cx, |v, _| composer::screen_mode(&v.screen_text()));
                let Some(shown) = shown else { break };
                if shown.claude == mode.claude {
                    this.update(cx, |this, cx| {
                        this.picked.entry(t.id.clone()).or_default().mode = Some(mode.id);
                        this.tell(&t, json!({"kind": "set", "permission": mode.permission}));
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
                    "Claude Code didn't switch to {} mode here. Its terminal shows the modes it offers: ⇧Tab switches.",
                    mode.label
                ));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Web search or session context switched in the box.
    pub(crate) fn set_switch(&mut self, key: &'static str, on: bool, cx: &mut Context<Self>) {
        let set = |c: &mut Choice| {
            if key == "web" {
                c.web = Some(on)
            } else {
                c.context = Some(on)
            }
        };
        match self.box_terminal().cloned() {
            None => set(&mut self.new_choice),
            Some(t) => {
                set(self.picked.entry(t.id.clone()).or_default());
                if t.running {
                    self.tell(&t, json!({"kind": "set", key: on}));
                }
            }
        }
        cx.notify();
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
        let key = favourite_key(kind, model);
        if let Some(at) = self.favourites.iter().position(|k| *k == key) {
            self.favourites.remove(at);
        } else {
            self.favourites.push(key);
        }
        crate::prefs::save_favourites(&self.favourites);
        cx.notify();
    }

    /// Types a command into the agent's terminal, then Enter. While the
    /// agent waits for an answer it would answer it, so it waits too.
    pub(crate) fn command(&mut self, t: &TerminalInfo, text: String, cx: &mut Context<Self>) {
        if self.waiting(t) {
            self.held = Some((t.id.clone(), text));
        } else {
            self.type_command(&t.id, text, cx);
        }
    }

    pub(crate) fn type_command(&mut self, id: &str, text: String, cx: &mut Context<Self>) {
        let typed = self.daemon.call_in_order::<Value>(Call::TerminalInput {
            terminal: id.to_string(),
            data: text,
        });
        let enter = Call::TerminalInput {
            terminal: id.to_string(),
            data: "\r".into(),
        };
        let daemon = self.daemon.clone();
        cx.spawn(async move |_, cx| {
            if typed.await.is_ok() {
                cx.background_executor().timer(SUBMIT_DELAY).await;
                daemon.fire(enter);
            }
        })
        .detach();
    }

    /// What a new conversation starts with.
    pub(crate) fn new_options(&self, refs: Vec<Value>) -> AgentOptions {
        let c = &self.new_choice;
        AgentOptions {
            model: self.new_model().id,
            effort: c.effort.clone(),
            permission: Some(self.box_mode_for_new().permission.into()),
            code: c.console.unwrap_or(false).then(|| "console".into()),
            web: (c.web == Some(false)).then_some(false),
            context: (c.context == Some(false)).then_some(false),
            refs,
            ..AgentOptions::default()
        }
    }

    fn box_mode_for_new(&self) -> &'static Mode {
        let mode = composer::mode(self.new_choice.mode.unwrap_or("normal"));
        if composer::has_mode(self.new_kind, mode) {
            mode
        } else {
            &composer::MODES[0]
        }
    }

    /// What an ended conversation goes on with: as it last ran, with what
    /// was picked since.
    pub(crate) fn resume_options(&self, t: &TerminalInfo) -> AgentOptions {
        let mut options = composer::live_options(t);
        let c = self.picked_for(t);
        if let Some(model) = c.model {
            options.model = model.id;
        }
        if let Some(effort) = c.effort {
            options.effort = Some(effort);
        }
        if let Some(mode) = c.mode {
            options.permission = Some(composer::mode(mode).permission.into());
        }
        if let Some(console) = c.console {
            options.code = console.then(|| "console".into());
        }
        if let Some(web) = c.web {
            options.web = (!web).then_some(false);
        }
        if let Some(context) = c.context {
            options.context = (!context).then_some(false);
        }
        options
    }

    /// Starts the agent again on its conversation, with `options`: after it
    /// ended, or to change what it can't while it runs.
    pub(crate) fn restart(
        &mut self,
        t: &TerminalInfo,
        options: AgentOptions,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = t.id.clone();
        let name = crate::agents::kind_of(&t.kind).map_or("the agent", |k| k.name);
        let restarted = self.daemon.call::<TerminalInfo>(Call::RestartTerminal {
            terminal: id.clone(),
            options: Some(options),
            cols: Some(START_COLUMNS),
            rows: Some(START_ROWS),
            prompt,
        });
        self.starting = Some(name);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let restarted = restarted.await;
            this.update(cx, |this, cx| {
                this.starting = None;
                match restarted {
                    Ok(info) => {
                        this.picked.remove(&id);
                        if let Some(view) = this.view(&id) {
                            view.update(cx, |v, cx| v.revive(cx));
                        }
                        if let Some(t) = this.terminals.iter_mut().find(|t| t.id == id) {
                            *t = info;
                        }
                    }
                    Err(err) => {
                        this.problem = Some(format!("Could not start {name} again: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A starred model, as `kind:id`.
pub(crate) fn favourite_key(kind: &str, model: &ModelChoice) -> String {
    format!("{kind}:{}", model.id.as_deref().unwrap_or_default())
}

/// Saves a file from this computer in the project's attachments folder.
async fn attach_file(
    daemon: &crate::daemon::Daemon,
    folder: &str,
    path: &PathBuf,
) -> anyhow::Result<Value> {
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().replace([':', '\\'], "-"))
        .unwrap_or_else(|| "attachment".into());
    daemon
        .call::<Value>(Call::WriteFile {
            path: format!("{folder}/{name}"),
            text: base64::engine::general_purpose::STANDARD.encode(bytes),
            expected_mtime_ms: None,
            base64: true,
            create: false,
        })
        .await?;
    daemon
        .call::<Value>(Call::WriteFile {
            path: format!("{folder}/.gitignore"),
            text: "*\n".into(),
            expected_mtime_ms: None,
            base64: false,
            create: false,
        })
        .await?;
    Ok(json!({"kind": "attach", "name": name, "path": format!("{ATTACHMENTS}/{name}")}))
}
