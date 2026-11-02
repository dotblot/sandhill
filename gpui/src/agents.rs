//! The AI pane's conversations: Claude Code, Codex and Antigravity, each
//! running unmodified in a terminal that sandhilld keeps, as in the Tauri
//! app (`app/src/model/terminals.ts`, `app/src/ui/AgentsPane.tsx`). The
//! daemon records what each agent's hooks report as chat entries; this keeps
//! them per conversation and sends the user's messages, answers and stops.
//! Drawing is in `agents_view`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, InputEvent, InputState};
use gpui_kit::*;
use sandhill_proto::{
    AgentInfo, AgentOptions, Call, TERMINAL_JOB, TERMINAL_SHELL, TerminalEvent, TerminalInfo,
    TerminalSubscribed,
};
use serde_json::{Value, json};

use crate::chat::{self, Chat, Entry};
use crate::composer::{Mention, ModelChoice};
use crate::daemon::Daemon;
use crate::terminal::TerminalView;
use crate::workbench::Workbench;

/// What Claude Code needs between a pasted message and the Enter that sends
/// it, as in the Tauri app.
pub(crate) const SUBMIT_DELAY: Duration = Duration::from_millis(60);
/// A new agent's terminal size, until its view has one.
pub(crate) const START_COLUMNS: u16 = 100;
pub(crate) const START_ROWS: u16 = 30;

/// An agent the pane offers, as the Tauri app's `AGENT_KINDS` describes it.
pub(crate) struct Kind {
    pub kind: &'static str,
    pub name: &'static str,
    /// How the user names it in a sentence: "Claude", "Codex".
    pub short: &'static str,
    pub plan: &'static str,
    pub placeholder: &'static str,
}

pub(crate) const KINDS: [Kind; 3] = [
    Kind {
        kind: "claude",
        name: "Claude Code",
        short: "Claude",
        plan: "Terminal · your Claude plan",
        placeholder: "Ask Claude…",
    },
    Kind {
        kind: "codex",
        name: "Codex",
        short: "Codex",
        plan: "Codex app-server · your ChatGPT plan",
        placeholder: "Ask Codex…",
    },
    Kind {
        kind: "antigravity",
        name: "Antigravity",
        short: "Antigravity",
        plan: "agy headless · your Google sign-in",
        placeholder: "Ask Antigravity…",
    },
];

pub(crate) fn kind_of(kind: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.kind == kind)
}

/// Agents whose chat Sandhill drives through a bridge (Codex,
/// Antigravity): messages go with `tell_terminal`, not as typing.
pub(crate) fn drives_chat(kind: &str) -> bool {
    kind == "codex" || kind == "antigravity"
}

/// A menu open over the message box.
#[derive(Clone, PartialEq)]
pub(crate) enum Menu {
    /// The @ menu (files, variables) or the # menu (cells of the notebooks
    /// named with @); `items` is `None` while it loads.
    Mention {
        sign: char,
        query: String,
        items: Option<Vec<Mention>>,
        active: usize,
    },
    Model,
    Mode,
    Context,
}

/// Choices in the message box that aren't the agent's yet: for a new
/// conversation, or picked while one has ended (they apply as it goes on),
/// or sent and not yet reported back.
#[derive(Clone, Default)]
pub(crate) struct Choice {
    pub model: Option<ModelChoice>,
    pub effort: Option<String>,
    pub mode: Option<&'static str>,
    pub console: Option<bool>,
    pub web: Option<bool>,
    pub context: Option<bool>,
}

/// What the pane shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Show {
    /// The selected conversation as a chat.
    Chat,
    /// The selected conversation's terminal.
    Terminal,
    History,
    /// Picking an agent and typing the first message.
    New,
}

pub struct Agents {
    pub(crate) daemon: Rc<Daemon>,
    pub(crate) project: String,
    /// The agents' CLIs and whether they are installed.
    pub(crate) installed: Vec<AgentInfo>,
    /// This project's agent terminals, oldest first.
    pub(crate) terminals: Vec<TerminalInfo>,
    pub(crate) chats: HashMap<String, Vec<Entry>>,
    pub(crate) views: HashMap<String, Entity<TerminalView>>,
    /// How much of each conversation's output has come, so a new
    /// subscription (after a restart) goes on from there.
    output_end: HashMap<String, u64>,
    pub(crate) selected: Option<String>,
    pub(crate) show: Show,
    /// The agent a new conversation starts with.
    pub(crate) new_kind: &'static str,
    pub(crate) composer: Entity<EditorState>,
    /// The hint the message box shows now.
    pub(crate) hint: &'static str,
    pub(crate) chat_scroll: ScrollHandle,
    /// Steps opened to show their card.
    pub(crate) open_steps: HashSet<String>,
    /// Requests answered here whose answer hasn't come back yet.
    pub(crate) sent: HashSet<String>,
    /// Conversations the user stopped, until the agent says it is idle.
    stopped: HashSet<String>,
    /// A conversation starting: its agent's name.
    pub(crate) starting: Option<&'static str>,
    pub(crate) problem: Option<String>,
    /// What was added to the message with @ and #, shown as chips.
    pub(crate) refs: Vec<Value>,
    pub(crate) menu: Option<Menu>,
    /// The model menu's tab: an agent's kind, or "fav".
    pub(crate) model_tab: Option<String>,
    pub(crate) favourites: Vec<String>,
    /// What a new conversation starts with.
    pub(crate) new_choice: Choice,
    /// What each conversation's box shows until the agent reports it.
    pub(crate) picked: HashMap<String, Choice>,
    /// The models Codex and Antigravity list before a conversation.
    pub(crate) listed: HashMap<String, Vec<Value>>,
    /// Unsent text and chips, by conversation ("new" for a new one).
    pub(crate) drafts: HashMap<String, (String, Vec<Value>)>,
    pub(crate) draft_key: String,
    /// A command typed while the agent waits for an answer, which it would
    /// answer: it goes once the agent stops waiting.
    pub(crate) held: Option<(String, String)>,
    /// Effort changes settle before they go (the slider can be dragged).
    pub(crate) effort_generation: u64,
    pub(crate) workbench: WeakEntity<Workbench>,
    /// Options picked for the agent's questions, by `question_key`.
    pub(crate) picks: HashMap<String, Vec<String>>,
    /// The "Other…" answer to each question.
    pub(crate) others: HashMap<String, Entity<InputState>>,
    /// The conversation menu (the gear) is open.
    pub(crate) gear: bool,
    /// End conversation was clicked once; the second click ends it.
    pub(crate) confirm_end: bool,
    /// The conversation being renamed, and its name field.
    pub(crate) renaming: Option<(String, Entity<InputState>)>,
    pub(crate) _rename_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// A question of a request, as `picks` and `others` keep it.
pub(crate) fn question_key(ask: &str, question: &str) -> String {
    format!("{ask}\u{0}{question}")
}

impl Agents {
    pub fn new(
        daemon: Rc<Daemon>,
        project: String,
        workbench: WeakEntity<Workbench>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .line_number(false)
                .folding(false)
                .soft_wrap(true)
                .submit_on_enter(true)
                .placeholder(KINDS[0].placeholder)
        });
        let subscriptions = vec![cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                InputEvent::Change => this.composer_changed(window, cx),
                _ => {}
            },
        )];
        let mut agents = Agents {
            daemon,
            project,
            installed: Vec::new(),
            terminals: Vec::new(),
            chats: HashMap::new(),
            views: HashMap::new(),
            output_end: HashMap::new(),
            selected: None,
            show: Show::Chat,
            new_kind: KINDS[0].kind,
            composer,
            hint: KINDS[0].placeholder,
            chat_scroll: ScrollHandle::new(),
            open_steps: HashSet::new(),
            sent: HashSet::new(),
            stopped: HashSet::new(),
            starting: None,
            problem: None,
            refs: Vec::new(),
            menu: None,
            model_tab: None,
            favourites: crate::prefs::favourites(),
            new_choice: Choice::default(),
            picked: HashMap::new(),
            listed: HashMap::new(),
            drafts: HashMap::new(),
            draft_key: "new".into(),
            held: None,
            effort_generation: 0,
            workbench,
            picks: HashMap::new(),
            others: HashMap::new(),
            gear: false,
            confirm_end: false,
            renaming: None,
            _rename_subscription: None,
            _subscriptions: subscriptions,
        };
        agents.refresh(cx);
        agents
    }

    /// Lists the installed agents and this project's conversations, and
    /// follows each.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let installed = self.daemon.call::<Vec<AgentInfo>>(Call::ListAgents);
        let listed = self.daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
        cx.spawn(async move |this, cx| {
            let installed = installed.await.unwrap_or_default();
            let listed = listed.await;
            this.update(cx, |this, cx| {
                for agent in &installed {
                    if agent.path.is_some() && drives_chat(&agent.kind) {
                        this.list_models(agent.kind.clone(), cx);
                    }
                }
                this.installed = installed;
                match listed {
                    Ok(all) => {
                        let mut mine: Vec<TerminalInfo> = all
                            .into_iter()
                            .filter(|t| is_agent(t) && t.project.as_deref() == Some(&this.project))
                            .collect();
                        mine.sort_by_key(|t| t.created_ms);
                        for t in &mine {
                            if !this.chats.contains_key(&t.id) {
                                this.follow(t, cx);
                            }
                        }
                        this.terminals = mine;
                        if this.selected.is_none() {
                            // The latest one still running, as the Tauri app opens.
                            this.selected = this
                                .terminals
                                .iter()
                                .rev()
                                .find(|t| t.running)
                                .map(|t| t.id.clone());
                        }
                        if this.terminals.is_empty() {
                            this.show = Show::New;
                        } else if this.selected.is_none() {
                            // None running: the earlier ones.
                            this.show = Show::History;
                        }
                        this.draft_key = this.wanted_draft();
                    }
                    Err(err) => {
                        this.problem = Some(format!("Listing conversations failed: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Subscribes to a conversation's output and chat, from the start.
    fn follow(&mut self, terminal: &TerminalInfo, cx: &mut Context<Self>) {
        let id = terminal.id.clone();
        self.chats.insert(id.clone(), Vec::new());
        let input = self.daemon.ordered({
            let id = id.clone();
            move |data| Call::TerminalInput {
                terminal: id.clone(),
                data,
            }
        });
        let resize = {
            let daemon = self.daemon.clone();
            let id = id.clone();
            Box::new(move |cols, rows| {
                daemon.fire(Call::ResizeTerminal {
                    terminal: id.clone(),
                    cols,
                    rows,
                })
            })
        };
        let view = cx.new(|cx| TerminalView::new(id.clone(), input, resize, cx));
        if !terminal.running {
            view.update(cx, |view, cx| view.exited("ended".into(), cx));
        }
        self.views.insert(id.clone(), view);
        self.subscribe(id, None, Some(0), cx);
    }

    /// Follows a conversation from `from` in its output and `chat_from`
    /// in its chat.
    fn subscribe(
        &mut self,
        id: String,
        from: Option<u64>,
        chat_from: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let subscribed = self
            .daemon
            .call::<TerminalSubscribed>(Call::SubscribeTerminal {
                terminal: id,
                from,
                chat_from,
            });
        cx.spawn(async move |this, cx| {
            if let Err(err) = subscribed.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Following a conversation failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Follows a restarted conversation's new program from where the old
    /// one got to.
    pub(crate) fn follow_again(&mut self, id: &str, cx: &mut Context<Self>) {
        let from = self.output_end.get(id).copied();
        let chat_from = self
            .chats
            .get(id)
            .and_then(|c| c.last())
            .map_or(0, |e| e.seq + 1);
        self.subscribe(id.to_string(), from, Some(chat_from), cx);
    }

    /// Whether this pane follows the terminal.
    pub fn owns(&self, terminal: &str) -> bool {
        self.chats.contains_key(terminal)
    }

    /// Output, chat entries and reports from a conversation's terminal.
    pub fn on_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        let id = event.terminal.clone();
        if let Some(data) = &event.data
            && let Some(view) = self.views.get(&id)
        {
            let end = self.output_end.entry(id.clone()).or_default();
            let start = event.offset.unwrap_or(*end);
            let len = decoded_len(data);
            // A new subscription can repeat output already shown.
            if start + len > *end || event.offset.is_none() {
                *end = start + len;
                view.update(cx, |view, cx| view.feed(data, cx));
            }
        }
        if let Some(entry) = event.chat {
            let entries = self.chats.entry(id.clone()).or_default();
            // A replay after a reconnect repeats what is already here.
            if entries.last().is_none_or(|last| entry.seq > last.seq) {
                if let Some(answered) = entry.event["id"]
                    .as_str()
                    .filter(|_| entry.event["kind"] == "answered")
                {
                    self.sent.remove(answered);
                }
                entries.push(Entry {
                    seq: entry.seq,
                    event: entry.event,
                });
                if self.selected.as_deref() == Some(&id) {
                    self.chat_scroll.scroll_to_bottom();
                }
            }
        }
        if let Some(agent) = &event.agent
            && agent.get("state").is_some_and(|s| s != "waiting")
            && self.held.as_ref().is_some_and(|(t, _)| *t == id)
            && let Some((_, command)) = self.held.take()
        {
            self.type_command(&id, command, cx);
        }
        if let Some(agent) = event.agent
            && let Some(t) = self.terminals.iter_mut().find(|t| t.id == id)
        {
            if agent["state"] == "idle" {
                self.stopped.remove(&id);
            }

            // Reports carry what changed; keep the rest.
            let mut merged = t.agent.take().unwrap_or(json!({}));
            if let (Some(old), Some(new)) = (merged.as_object_mut(), agent.as_object()) {
                for (k, v) in new {
                    old.insert(k.clone(), v.clone());
                }
            } else {
                merged = agent;
            }
            t.agent = Some(merged);
        }
        if let Some(exit) = event.exit
            && let Some(t) = self.terminals.iter_mut().find(|t| t.id == id)
            && exit.ended_ms >= t.started_ms
        {
            t.running = false;
            t.exit = Some(exit);
            if let Some(view) = self.views.get(&id) {
                view.update(cx, |view, cx| view.exited("ended".into(), cx));
            }
        }
        cx.notify();
    }

    pub(crate) fn current(&self) -> Option<&TerminalInfo> {
        let id = self.selected.as_deref()?;
        self.terminals.iter().find(|t| t.id == id)
    }

    pub(crate) fn view(&self, id: &str) -> Option<Entity<TerminalView>> {
        self.views.get(id).cloned()
    }

    pub(crate) fn entries(&self, id: &str) -> &[Entry] {
        self.chats.get(id).map_or(&[], Vec::as_slice)
    }

    /// Busy or waiting for the user, and not just stopped.
    pub(crate) fn working(&self, t: &TerminalInfo) -> bool {
        let state = t.agent.as_ref().and_then(|a| a["state"].as_str());
        t.running && matches!(state, Some("busy" | "waiting")) && !self.stopped.contains(&t.id)
    }

    pub(crate) fn waiting(&self, t: &TerminalInfo) -> bool {
        t.running && t.agent.as_ref().and_then(|a| a["state"].as_str()) == Some("waiting")
    }

    pub(crate) fn chat(&self, t: &TerminalInfo) -> Chat {
        chat::build(self.entries(&t.id), self.working(t))
    }

    /// The conversation's name: the one the user gave, else its first
    /// message, else the agent and its model.
    pub(crate) fn title(&self, t: &TerminalInfo) -> String {
        if let Some(name) = t.name.as_ref().filter(|n| !n.trim().is_empty()) {
            return name.clone();
        }
        let first = self.entries(&t.id).iter().find(|e| {
            e.event["kind"] == "prompt" && e.event["text"].as_str().is_some_and(|s| !s.is_empty())
        });
        match first {
            Some(e) => chat::title_of(e.event["text"].as_str().unwrap_or_default()),
            None => match model_label(t) {
                Some(model) => format!("{} · {model}", t.title),
                None => t.title.clone(),
            },
        }
    }

    pub(crate) fn last_active(&self, t: &TerminalInfo) -> i64 {
        chat::last_active(self.entries(&t.id)).unwrap_or(t.started_ms.max(t.created_ms))
    }

    pub(crate) fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.show = Show::Chat;
        self.chat_scroll.scroll_to_bottom();
        self.swap_draft(window, cx);
        cx.notify();
    }

    pub(crate) fn show(&mut self, show: Show, window: &mut Window, cx: &mut Context<Self>) {
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
    /// or sends it to the one shown.
    pub(crate) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if matches!(self.menu, Some(Menu::Mention { .. })) {
            return self.pick_mention(None, window, cx);
        }
        let refs = std::mem::take(&mut self.refs);
        let sent = match self.show {
            Show::New => {
                let options = self.new_options(refs.clone());
                self.start(self.new_kind, Some(text), options, window, cx);
                true
            }
            _ => match self.current().cloned() {
                Some(t) if t.running => self.send(&t, &text, &refs, cx),
                Some(t) => {
                    let mut options = self.resume_options(&t);
                    options.refs = refs.clone();
                    self.restart(&t, options, Some(text), cx);
                    true
                }
                None => false,
            },
        };
        if sent {
            self.menu = None;
            self.drafts.remove(&self.draft_key);
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
        } else {
            self.refs = refs;
        }
    }

    /// Starts a conversation in the project's folder, with its first
    /// message.
    pub(crate) fn start(
        &mut self,
        kind: &'static str,
        prompt: Option<String>,
        options: AgentOptions,
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
                        this.follow(&info, cx);
                        let id = info.id.clone();
                        this.terminals.push(info);
                        this.new_choice = Choice::default();
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
    /// typed into the agent's terminal (one paste, so line breaks don't
    /// send it early, then Enter).
    fn send(
        &mut self,
        t: &TerminalInfo,
        text: &str,
        refs: &[Value],
        cx: &mut Context<Self>,
    ) -> bool {
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
        } else {
            // The chips go first, so Claude Code's prompt hook finds them.
            if !refs.is_empty() {
                self.tell(t, json!({"kind": "set", "refs": refs}));
            }
            let paste = format!("\x1b[200~{}\x1b[201~", text.replace('\n', "\r"));
            let enter = Call::TerminalInput {
                terminal: t.id.clone(),
                data: "\r".into(),
            };
            let pasted = self.daemon.call_in_order::<Value>(Call::TerminalInput {
                terminal: t.id.clone(),
                data: paste,
            });
            let daemon = self.daemon.clone();
            cx.spawn(async move |_, cx| {
                if pasted.await.is_ok() {
                    cx.background_executor().timer(SUBMIT_DELAY).await;
                    daemon.fire(enter);
                }
            })
            .detach();
        }
        self.problem = None;
        self.chat_scroll.scroll_to_bottom();
        true
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
            if let Err(err) = answered.await {
                this.update(cx, |this, cx| {
                    this.sent.remove(&ask);
                    this.problem = Some(format!("Could not answer: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
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

    /// Conversations waiting for the user.
    pub(crate) fn any_waiting(&self) -> bool {
        self.terminals.iter().any(|t| self.waiting(t))
    }
}

fn is_agent(t: &TerminalInfo) -> bool {
    !matches!(t.kind.as_str(), TERMINAL_SHELL | TERMINAL_JOB | "preview")
}

/// The model the agent last reported, shortened: "Opus 5.5".
pub(crate) fn model_label(t: &TerminalInfo) -> Option<String> {
    let reported = t
        .agent
        .as_ref()
        .is_some_and(|a| a["status"]["model"].is_string());
    (reported || t.options.model.is_some()).then(|| crate::composer::model_label(t))
}

/// The number of bytes base64 `data` holds.
fn decoded_len(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|b| *b == b'=').count();
    (data.len() / 4 * 3).saturating_sub(padding) as u64
}
