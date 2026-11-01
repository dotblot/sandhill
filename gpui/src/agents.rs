//! The AI pane's conversations: Claude Code, Codex and Antigravity, each
//! running unmodified in a terminal that sandhilld keeps, as in the Tauri
//! app (`app/src/model/terminals.ts`, `app/src/ui/AgentsPane.tsx`). The
//! daemon records what each agent's hooks report as chat entries; this keeps
//! them per conversation and sends the user's messages, answers and stops.
//! Drawing is in `agents_view`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::*;
use sandhill_proto::{
    AgentInfo, AgentOptions, Call, TERMINAL_JOB, TERMINAL_SHELL, TerminalEvent, TerminalInfo,
    TerminalSubscribed,
};
use serde_json::{Value, json};

use crate::chat::{self, Chat, Entry};
use crate::daemon::Daemon;
use crate::terminal::TerminalView;

/// What Claude Code needs between a pasted message and the Enter that sends
/// it, as in the Tauri app.
const SUBMIT_DELAY: Duration = Duration::from_millis(60);
/// A new agent's terminal size, until its view has one.
const START_COLUMNS: u16 = 100;
const START_ROWS: u16 = 30;

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
    chats: HashMap<String, Vec<Entry>>,
    views: HashMap<String, Entity<TerminalView>>,
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
    _subscriptions: Vec<Subscription>,
}

impl Agents {
    pub fn new(
        daemon: Rc<Daemon>,
        project: String,
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
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(window, cx);
                }
            },
        )];
        let mut agents = Agents {
            daemon,
            project,
            installed: Vec::new(),
            terminals: Vec::new(),
            chats: HashMap::new(),
            views: HashMap::new(),
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
                        }
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
        let subscribed = self
            .daemon
            .call::<TerminalSubscribed>(Call::SubscribeTerminal {
                terminal: id,
                from: None,
                chat_from: Some(0),
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
            view.update(cx, |view, cx| view.feed(data, cx));
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

    pub(crate) fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.show = Show::Chat;
        self.chat_scroll.scroll_to_bottom();
        cx.notify();
    }

    pub(crate) fn show(&mut self, show: Show, window: &mut Window, cx: &mut Context<Self>) {
        self.show = show;
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
        let sent = match self.show {
            Show::New => {
                self.start(self.new_kind, Some(text), cx);
                true
            }
            _ => match self.current().cloned() {
                Some(t) if t.running => self.send(&t, &text, cx),
                _ => false,
            },
        };
        if sent {
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
        }
    }

    /// Starts a conversation in the project's folder, with its first
    /// message.
    pub(crate) fn start(
        &mut self,
        kind: &'static str,
        prompt: Option<String>,
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
            options: AgentOptions::default(),
            prompt,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                this.starting = None;
                match started {
                    Ok(info) => {
                        this.follow(&info, cx);
                        let id = info.id.clone();
                        this.terminals.push(info);
                        this.select(id, cx);
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
    fn send(&mut self, t: &TerminalInfo, text: &str, cx: &mut Context<Self>) -> bool {
        if !self.chat(t).waiting.is_empty() {
            let who = kind_of(&t.kind).map_or("the agent", |k| k.short);
            self.problem = Some(format!("Answer {who}'s question above first."));
            cx.notify();
            return false;
        }
        self.stopped.remove(&t.id);
        if drives_chat(&t.kind) {
            self.tell(t, json!({"kind": "say", "text": text}));
        } else {
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

    fn tell(&self, t: &TerminalInfo, command: Value) {
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
    let status = &t.agent.as_ref()?["status"];
    let model = status["model"].as_str().or(t.options.model.as_deref())?;
    Some(model.trim_start_matches("Claude ").to_string())
}
