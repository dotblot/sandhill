//! The AI pane's conversations: Claude Code, Codex, Antigravity,
//! OpenScience and Posit Assistant, each running unmodified under
//! sandhilld. The daemon records each agent's events as chat entries;
//! this keeps them per conversation and sends the user's messages,
//! answers and stops. OpenScience is chat-only; Posit Assistant reports
//! nothing, so its conversations show its own screen. Drawing is in
//! `agents::view`.

pub(crate) mod accounts;
pub(crate) mod cell_ai;
pub(crate) mod chat;
pub(crate) mod composer;
pub(crate) mod composer_view;
pub(crate) mod edits;
pub(crate) mod effort_bar;
pub(crate) mod ghost;
pub(crate) mod input_box;
pub(crate) mod notebook_ai;
pub(crate) mod quota;
pub(crate) mod start;
pub(crate) mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, InputEvent, InputState};
use gpui_kit::*;
use sandhill_daemon::terminal::choices::{Ref, refs_text};
use sandhill_proto::{
    AgentAccount, AgentInfo, AgentOptions, Call, TERMINAL_JOB, TERMINAL_LOGIN, TERMINAL_SHELL,
    TerminalEvent, TerminalInfo, TerminalSubscribed,
};
use serde_json::{Value, json};

use crate::daemon::Daemon;
use crate::terminal::TerminalView;
use crate::workbench::Workbench;
use chat::{Chat, Entry};
use composer::{Mention, ModelChoice};

/// What Claude Code needs between a pasted message and the Enter that sends
/// it, as in the Tauri app.
pub(crate) const SUBMIT_DELAY: Duration = Duration::from_millis(60);
/// The same for Posit Assistant, which drops an Enter that comes 60 ms
/// after the paste.
const POSIT_SUBMIT_DELAY: Duration = Duration::from_millis(300);
/// Answering Claude Code's trust question: how many keys at most, and how
/// long its screen takes to show each.
const TRUST_TRIES: usize = 4;
const TRUST_STEP: Duration = Duration::from_millis(250);
/// A new agent's terminal size, until its view has one.
pub(crate) const START_COLUMNS: u16 = 100;
pub(crate) const START_ROWS: u16 = 30;
/// A bridge handoff must either acknowledge the requested view or leave the
/// conversation usable with an error, including when its process has ended.
const VIEW_SWITCH_TIMEOUT: Duration = Duration::from_secs(45);

/// An agent the pane offers, as the Tauri app's `AGENT_KINDS` describes it.
pub(crate) struct Kind {
    pub kind: &'static str,
    pub name: &'static str,
    /// How the user names it in a sentence: "Claude", "Codex".
    pub short: &'static str,
    pub plan: &'static str,
    pub placeholder: &'static str,
}

pub(crate) const KINDS: [Kind; 7] = [
    Kind {
        kind: "antigravity",
        name: "Antigravity",
        short: "Antigravity",
        plan: "agy headless · your Google sign-in",
        placeholder: "Ask Antigravity…",
    },
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
        kind: "omp",
        name: "Oh My Pi",
        short: "Oh My Pi",
        plan: "omp acp · your own model accounts",
        placeholder: "Ask Oh My Pi…",
    },
    Kind {
        kind: "openscience",
        name: "OpenScience",
        short: "OpenScience",
        plan: "OpenScience · your configured models",
        placeholder: "Ask OpenScience…",
    },
    Kind {
        kind: "pi",
        name: "pi",
        short: "pi",
        plan: "pi RPC · your own model accounts",
        placeholder: "Ask pi…",
    },
    Kind {
        kind: "posit",
        name: "Posit Assistant",
        short: "Posit Assistant",
        plan: "pa in a terminal · your Posit Assistant providers",
        placeholder: "Ask Posit Assistant…",
    },
];

pub(crate) fn kind_of(kind: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.kind == kind)
}

/// Agents turned off for now (Huan, 2026-11-14, after Google suspended the
/// Antigravity account): not offered for new conversations, never asked for
/// models, accounts or quota, and their conversations take no messages.
/// Remove an entry to turn the agent back on.
const TURNED_OFF: [&str; 1] = ["antigravity"];

pub(crate) fn turned_off(kind: &str) -> bool {
    TURNED_OFF.contains(&kind)
}

/// The agent a new conversation starts with (Huan, 2026-11-14).
pub(crate) fn default_kind() -> &'static Kind {
    kind_of("codex").unwrap_or(&KINDS[0])
}

/// Agents whose chat Sandhill drives through a bridge: messages go with
/// `tell_terminal`, not as typing.
pub(crate) fn drives_chat(kind: &str) -> bool {
    matches!(kind, "codex" | "antigravity" | "openscience" | "omp" | "pi")
}

/// Agents that report nothing for a chat (Posit Assistant): their
/// conversations show their own screen, with its own model and mode menus.
pub(crate) fn terminal_only(kind: &str) -> bool {
    kind == "posit"
}

/// OpenScience exposes only its ACP chat, never a native command line.
pub(crate) fn supports_terminal(kind: &str) -> bool {
    kind != "openscience"
}

/// Apply provider view constraints to both requested and restored views.
pub(crate) fn provider_show(kind: &str, show: Show) -> Show {
    match show {
        Show::Chat if terminal_only(kind) => Show::Terminal,
        Show::Terminal if !supports_terminal(kind) => Show::Chat,
        show => show,
    }
}

/// Whether Posit Assistant's screen takes a message: its empty box shows
/// "Enter a prompt", and no workspace trust question is up (keys typed then
/// would answer it).
fn takes_message(screen: &str) -> bool {
    screen.contains("Enter a prompt") && !screen.contains("Trust this workspace?")
}

/// A message for an agent that reports nothing, with what was added to it
/// written out, as the daemon tells the other agents.
fn with_refs(text: &str, refs: &[Value]) -> String {
    let refs: Vec<Ref> = refs
        .iter()
        .filter_map(|r| serde_json::from_value(r.clone()).ok())
        .collect();
    match refs_text(&refs) {
        Some(added) if text.is_empty() => added,
        Some(added) => format!("{text}\n\n{added}"),
        None => text.to_string(),
    }
}

/// Splits a conversation's first message, for starting its agent: what
/// goes to the daemon as its prompt, and what the app types into the
/// agent's screen once it takes messages (Posit Assistant's, with what
/// was added to it, which leaves `options`).
pub(crate) fn first_message(
    kind: &str,
    prompt: Option<String>,
    options: &mut AgentOptions,
) -> (Option<String>, Option<String>) {
    if !terminal_only(kind) {
        return (prompt, None);
    }
    let refs = std::mem::take(&mut options.refs);
    let text = with_refs(prompt.as_deref().unwrap_or_default(), &refs);
    (None, Some(text).filter(|t| !t.is_empty()))
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

/// A missing `view_mode` describes the live bridge, not the daemon that would
/// resume it. Only replace an idle legacy bridge after verifying that daemon.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewPhase {
    Checking,
    Resuming,
    Switching,
}

#[derive(Debug, PartialEq, Eq)]
enum ViewRecovery {
    Resume,
    Busy,
    NoResume,
    OlderService,
    UnknownService,
}

fn legacy_view_recovery(
    state: Option<&str>,
    can_resume: bool,
    bundled_build: Option<bool>,
) -> ViewRecovery {
    if state != Some("idle") {
        return ViewRecovery::Busy;
    }
    match bundled_build {
        Some(false) => ViewRecovery::OlderService,
        None => ViewRecovery::UnknownService,
        Some(true) if !can_resume => ViewRecovery::NoResume,
        Some(true) => ViewRecovery::Resume,
    }
}

/// UI requests are separate from the bridge's authoritative `view_mode`.
#[derive(Clone, Copy)]
pub(crate) struct ViewSwitch {
    pub(crate) target: Show,
    previous: Show,
    generation: u64,
    started: bool,
    phase: ViewPhase,
}

impl ViewSwitch {
    fn begin(
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

    fn observe(&mut self, mode: Option<&str>) -> Option<Show> {
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

pub struct Agents {
    pub(crate) daemon: Rc<Daemon>,
    pub(crate) host: String,
    pub(crate) accounts: Vec<AgentAccount>,
    pub(crate) account_switching: Option<String>,
    /// The window it is in, where its terminals start.
    window: AnyWindowHandle,
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
    /// The conversation whose delete button in the history was clicked
    /// once: the next click deletes it.
    pub(crate) confirm_delete: Option<String>,
    pub(crate) show: Show,
    pub(crate) view_switches: HashMap<String, ViewSwitch>,
    view_generation: u64,
    /// The agent a new conversation starts with.
    pub(crate) new_kind: &'static str,
    pub(crate) composer: Entity<EditorState>,
    /// The hint the message box shows now.
    pub(crate) hint: &'static str,
    pub(crate) chat_scroll: ScrollHandle,
    pub(crate) mention_scroll: ScrollHandle,
    /// Steps opened to show their card.
    pub(crate) open_steps: HashSet<String>,
    /// Requests answered here whose answer hasn't come back yet.
    pub(crate) sent: HashSet<String>,
    /// Conversations the user stopped, until the agent says it is idle.
    stopped: HashSet<String>,
    /// Claude Code conversations whose screen asks whether the user trusts
    /// the folder: the chat shows the question, and holds messages.
    pub(crate) trust_asked: HashSet<String>,
    /// First messages for Posit Assistant, typed once its screen takes them.
    pub(crate) typing: HashMap<String, String>,
    /// A conversation starting: its agent's name.
    pub(crate) starting: Option<&'static str>,
    pub(crate) problem: Option<String>,
    /// Whether notebooks scroll to the cells agents add or change.
    pub(crate) follow_edits: bool,
    /// What was added to the message with @ and #, shown as chips.
    pub(crate) refs: Vec<Value>,
    /// What a new conversation's screen knows of the project.
    pub(crate) start: crate::agents::start::StartContext,
    pub(crate) menu: Option<Menu>,
    pub(crate) popup_triggers: crate::popup::PopupTriggers,
    /// The model menu's tab: an agent's kind, or "fav".
    pub(crate) model_tab: Option<String>,
    pub(crate) favourites: Vec<String>,
    /// What a new conversation starts with.
    pub(crate) new_choice: Choice,
    /// An effort set on another agent's tab of a new conversation's model
    /// menu, by kind; a model then picked on that tab keeps it.
    pub(crate) browsed_effort: Option<(String, String)>,
    /// What each conversation's box shows until the agent reports it.
    pub(crate) picked: HashMap<String, Choice>,
    /// The models driven-chat providers list before a conversation.
    pub(crate) listed: HashMap<String, Vec<Value>>,
    /// Unsent text and chips, by conversation ("new" for a new one).
    pub(crate) drafts: HashMap<String, (String, Vec<Value>)>,
    pub(crate) draft_key: String,
    /// A command typed while the agent waits for an answer, which it would
    /// answer: it goes once the agent stops waiting.
    pub(crate) held: Option<(String, String)>,
    /// Effort changes settle before they go (the bar can be dragged).
    pub(crate) effort_generation: u64,
    pub(crate) effort_bar: crate::agents::effort_bar::Bar,
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
        host: String,
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
                .placeholder(default_kind().placeholder)
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
            window: window.window_handle(),
            daemon,
            host,
            accounts: Vec::new(),
            account_switching: None,
            project,
            installed: Vec::new(),
            terminals: Vec::new(),
            chats: HashMap::new(),
            views: HashMap::new(),
            output_end: HashMap::new(),
            selected: None,
            confirm_delete: None,
            show: Show::Chat,
            view_switches: HashMap::new(),
            view_generation: 0,
            new_kind: default_kind().kind,
            composer,
            hint: default_kind().placeholder,
            chat_scroll: ScrollHandle::new(),
            mention_scroll: ScrollHandle::new(),
            open_steps: HashSet::new(),
            sent: HashSet::new(),
            stopped: HashSet::new(),
            trust_asked: HashSet::new(),
            typing: HashMap::new(),
            starting: None,
            problem: None,
            follow_edits: crate::prefs::follow_agent(),
            refs: Vec::new(),
            start: Default::default(),
            menu: None,
            popup_triggers: crate::popup::PopupTriggers::default(),
            model_tab: None,
            favourites: crate::prefs::favourites(),
            new_choice: Choice::default(),
            browsed_effort: None,
            picked: HashMap::new(),
            listed: HashMap::new(),
            drafts: HashMap::new(),
            draft_key: "new".into(),
            held: None,
            effort_generation: 0,
            effort_bar: Default::default(),
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
        agents.refresh_accounts(cx);
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
                    if agent.path.is_some() && drives_chat(&agent.kind) && !turned_off(&agent.kind)
                    {
                        this.list_models(agent.kind.clone(), cx);
                    }
                }
                this.installed = installed;
                match listed {
                    Ok(all) => {
                        this.problem = None;
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
                        if let Some(t) = this.current() {
                            this.show = provider_show(&t.kind, this.show);
                        }
                        if matches!(this.show, Show::Chat | Show::Terminal)
                            && let Some(t) = this.current()
                            && drives_chat(&t.kind)
                            && supports_terminal(&t.kind)
                            && t.agent.as_ref().is_some_and(|a| {
                                matches!(
                                    a["view_mode"].as_str(),
                                    Some("terminal" | "switching_terminal")
                                )
                            })
                        {
                            this.show = Show::Terminal;
                            if let Some(view) =
                                this.selected.as_ref().and_then(|id| this.views.get(id))
                            {
                                let focus = view.read(cx).focus_handle();
                                let _ = this.window.update(cx, move |_, window, cx| {
                                    window.focus(&focus, cx);
                                });
                            }
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
        let window = self.window;
        let view = cx.new(|cx| TerminalView::new(id.clone(), input, resize, window, cx));
        if terminal.kind == "claude" {
            let id = id.clone();
            cx.observe(&view, move |this, view, cx| {
                let asks = crate::agents::composer::trust_question(&view.read(cx).screen_text(cx))
                    .is_some();
                let changed = if asks {
                    this.trust_asked.insert(id.clone())
                } else {
                    this.trust_asked.remove(&id)
                };
                if changed {
                    cx.notify();
                }
            })
            .detach();
        }
        if terminal_only(&terminal.kind) {
            let id = id.clone();
            cx.observe(&view, move |this, view, cx| {
                if this.typing.contains_key(&id)
                    && takes_message(&view.read(cx).screen_text(cx))
                    && let Some(text) = this.typing.remove(&id)
                {
                    this.type_message(&id, "posit", &text, cx);
                }
            })
            .detach();
        }
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
        if event
            .agent
            .as_ref()
            .is_some_and(|agent| agent["status"]["rate_limits"].is_object())
        {
            self.refresh_accounts(cx);
        }
        if event
            .chat
            .as_ref()
            .is_some_and(|entry| entry.event["kind"] == "account")
        {
            self.refresh_accounts(cx);
        }
        if let Some(data) = &event.data
            && let Some(view) = self.views.get(&id)
        {
            let end = self.output_end.entry(id.clone()).or_default();
            let start = event.offset.unwrap_or(*end);
            let len = decoded_len(data);
            // A new subscription can repeat output already shown.
            if start + len > *end || event.offset.is_none() {
                *end = start + len;
                view.update(cx, |view, cx| view.feed(data, event.replay, cx));
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

            let previous_view = t
                .agent
                .as_ref()
                .and_then(|a| a["view_mode"].as_str())
                .map(str::to_owned);
            let completed = self
                .view_switches
                .get_mut(&id)
                .and_then(|switch| switch.observe(agent["view_mode"].as_str()));
            if completed.is_some() {
                self.view_switches.remove(&id);
                self.problem = None;
            }
            // A failed command line says why once, in the notice the user
            // dismisses, in the report that ends its switch back to chat.
            if matches!(
                previous_view.as_deref(),
                Some("switching_terminal" | "switching_chat")
            ) && agent["view_mode"] == "chat"
                && let Some(error) = agent["view_error"].as_str()
            {
                self.problem = Some(error.to_owned());
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
            if self.selected.as_deref() == Some(&id) {
                self.show = provider_show(&t.kind, self.show);
            }
            if self.selected.as_deref() == Some(&id)
                && matches!(self.show, Show::Chat | Show::Terminal)
                && drives_chat(&t.kind)
                && (completed.is_some()
                    || t.agent.as_ref().and_then(|a| a["view_mode"].as_str())
                        != previous_view.as_deref())
            {
                match t.agent.as_ref().and_then(|a| a["view_mode"].as_str()) {
                    Some("chat") => {
                        self.show = Show::Chat;
                        let composer = self.composer.clone();
                        let _ = self.window.update(cx, move |_, window, cx| {
                            composer.update(cx, |composer, cx| composer.focus(window, cx));
                        });
                    }
                    Some("terminal") => {
                        self.show = Show::Terminal;
                        if let Some(view) = self.views.get(&id) {
                            let focus = view.read(cx).focus_handle();
                            let _ = self.window.update(cx, move |_, window, cx| {
                                window.focus(&focus, cx);
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        self.continue_resumed_view(&id, cx);
        if let Some(exit) = event.exit
            && let Some(t) = self.terminals.iter_mut().find(|t| t.id == id)
            && exit.ended_ms >= t.started_ms
        {
            t.running = false;
            t.exit = Some(exit);
            if self
                .view_switches
                .get(&id)
                .is_some_and(|s| s.phase != ViewPhase::Resuming || self.starting.is_none())
                && let Some(switch) = self.view_switches.remove(&id)
            {
                if self.selected.as_deref() == Some(&id)
                    && matches!(self.show, Show::Chat | Show::Terminal)
                {
                    self.show = switch.previous;
                }
                self.problem = Some(
                    "The agent ended while changing modes. Resume the conversation to continue."
                        .into(),
                );
            }
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

    pub(crate) fn account_host(&self, _: &App) -> String {
        self.host.clone()
    }

    pub(crate) fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        let request = self
            .daemon
            .call::<Vec<AgentAccount>>(Call::ListAgentAccounts {
                refresh: false,
                ids: None,
            });
        cx.spawn(async move |this, cx| {
            if let Ok(accounts) = request.await {
                let _ = this.update(cx, |this, cx| {
                    this.accounts = accounts;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// An explicit account choice always resumes this conversation, never a new one.
    pub(crate) fn switch_account(
        &mut self,
        account: AgentAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.current().cloned().filter(|t| {
            matches!(self.show, Show::Chat | Show::Terminal)
                && t.kind == account.kind
                && (t.running || crate::agents::composer::can_resume(t))
                && t.options.account.as_deref().unwrap_or(&t.kind) != account.id
        }) else {
            return;
        };
        if self.account_switching.is_some() || self.starting.is_some() {
            return;
        }
        if self.working(&t) {
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Stop the current reply and switch to {}?", account.name),
                None,
                &["Stop and Switch", "Cancel"],
                cx,
            );
            self.account_switching = Some(t.id.clone());
            cx.spawn(async move |this, cx| {
                let answer = answer.await;
                let _ = this.update(cx, |this, cx| {
                    this.account_switching = None;
                    if matches!(answer, Ok(0))
                        && let Some(t) = this.terminals.iter().find(|live| live.id == t.id).cloned()
                    {
                        this.restart_on_account(&t, &account, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            self.restart_on_account(&t, &account, cx);
        }
    }

    fn restart_on_account(
        &mut self,
        t: &TerminalInfo,
        account: &AgentAccount,
        cx: &mut Context<Self>,
    ) {
        let mut options = self.resume_options(t);
        options.account = Some(account.id.clone());
        self.account_switching = Some(t.id.clone());
        self.restart(t, options, None, cx);
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
        t.running
            && (t.agent.as_ref().and_then(|a| a["state"].as_str()) == Some("waiting")
                || self.trust_asked.contains(&t.id))
    }

    /// Answers Claude Code's question whether the user trusts the folder,
    /// as its terminal takes it: Esc says no (and ends the conversation);
    /// yes moves the pick to "Yes" (its first choice is "No, exit"), then
    /// Enter. Whatever message started the conversation then goes on.
    pub(crate) fn answer_trust(&mut self, t: &TerminalInfo, trust: bool, cx: &mut Context<Self>) {
        let id = t.id.clone();
        if !trust {
            self.daemon.fire(Call::TerminalInput {
                terminal: id,
                data: "\x1b".into(),
            });
            return;
        }
        let Some(view) = self.view(&id) else {
            return;
        };
        let daemon = self.daemon.clone();
        cx.spawn(async move |_, cx| {
            for _ in 0..TRUST_TRIES {
                let shown = view.read_with(cx, |v, cx| {
                    crate::agents::composer::trust_question(&v.screen_text(cx))
                });
                let key = match shown {
                    None => return,
                    Some(true) => "\r",
                    Some(false) => "\x1b[B",
                };
                daemon.fire(Call::TerminalInput {
                    terminal: id.clone(),
                    data: key.into(),
                });
                if key == "\r" {
                    return;
                }
                cx.background_executor().timer(TRUST_STEP).await;
            }
        })
        .detach();
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
        if self.current().is_some_and(|t| terminal_only(&t.kind)) {
            return self.show(Show::Terminal, window, cx);
        }
        self.chat_scroll.scroll_to_bottom();
        self.show(Show::Chat, window, cx);
    }

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
                    crate::app_window::machine_name(&wb.host()),
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

    /// The switch for notebooks following the agent's edits.
    pub(crate) fn toggle_follow_edits(&mut self, cx: &mut Context<Self>) {
        self.follow_edits = !self.follow_edits;
        crate::prefs::save_follow_agent(self.follow_edits);
        cx.notify();
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
    fn type_message(&self, id: &str, kind: &str, text: &str, cx: &mut Context<Self>) {
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

    /// Conversations waiting for the user.
    pub(crate) fn any_waiting(&self) -> bool {
        self.terminals.iter().any(|t| self.waiting(t))
    }
}

pub(crate) fn account_at_limit(account: &AgentAccount) -> bool {
    account
        .usage
        .as_ref()
        .and_then(|usage| usage["rate_limits"].as_object())
        .is_some_and(|windows| {
            windows.values().any(|window| {
                window["used_percentage"]
                    .as_f64()
                    .is_some_and(|used| used.is_finite() && used >= 100.)
            })
        })
}

fn is_agent(t: &TerminalInfo) -> bool {
    !matches!(
        t.kind.as_str(),
        TERMINAL_SHELL | TERMINAL_JOB | TERMINAL_LOGIN | "preview"
    )
}

/// The model the agent last reported, shortened: "Opus 5.5".
pub(crate) fn model_label(t: &TerminalInfo) -> Option<String> {
    let reported = t
        .agent
        .as_ref()
        .is_some_and(|a| a["status"]["model"].is_string());
    (reported || t.options.model.is_some()).then(|| crate::agents::composer::model_label(t))
}

/// The number of bytes base64 `data` holds.
fn decoded_len(data: &str) -> u64 {
    let padding = data.bytes().rev().take_while(|b| *b == b'=').count();
    (data.len() / 4 * 3).saturating_sub(padding) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        AgentOptions, KINDS, Show, ViewPhase, ViewRecovery, ViewSwitch, drives_chat, first_message,
        kind_of, legacy_view_recovery, provider_show, supports_terminal, takes_message,
        terminal_only,
    };
    use serde_json::json;

    #[test]
    fn view_switch_legacy_recovery_requires_idle_resumable_current_service() {
        assert_eq!(
            legacy_view_recovery(Some("idle"), true, Some(true)),
            ViewRecovery::Resume
        );
        assert_eq!(
            legacy_view_recovery(Some("idle"), true, Some(false)),
            ViewRecovery::OlderService
        );
        assert_eq!(
            legacy_view_recovery(Some("idle"), true, None),
            ViewRecovery::UnknownService
        );
        assert_eq!(
            legacy_view_recovery(Some("idle"), false, Some(true)),
            ViewRecovery::NoResume
        );
        // A freshly started bridge on an old daemon is not upgraded by
        // restarting it, even when its terminal is not marked outdated.
        assert_eq!(
            legacy_view_recovery(Some("idle"), false, Some(false)),
            ViewRecovery::OlderService
        );
        for state in [Some("busy"), Some("waiting"), None] {
            for service in [Some(true), Some(false), None] {
                assert_eq!(
                    legacy_view_recovery(state, true, service),
                    ViewRecovery::Busy
                );
            }
        }
    }

    #[test]
    fn view_switch_recovery_waits_for_new_bridge_before_handoff() {
        let mut switch = ViewSwitch {
            target: Show::Terminal,
            previous: Show::Chat,
            generation: 1,
            started: false,
            phase: ViewPhase::Checking,
        };
        assert!(switch.observe(Some("chat")).is_none());
        switch.phase = ViewPhase::Resuming;
        assert!(switch.observe(Some("chat")).is_none());
        switch.phase = ViewPhase::Switching;
        assert!(switch.observe(Some("chat")).is_none());
        assert!(switch.observe(Some("terminal")) == Some(Show::Terminal));
    }

    #[test]
    fn view_switch_rejects_legacy_handoff_without_blocking_chat() {
        assert!(ViewSwitch::begin(None, Show::Terminal, 1).is_err());
        assert!(ViewSwitch::begin(None, Show::Chat, 1).unwrap().is_none());
        assert!(
            ViewSwitch::begin(Some("chat"), Show::Chat, 1)
                .unwrap()
                .is_none()
        );
        assert!(
            ViewSwitch::begin(Some("terminal"), Show::Terminal, 1)
                .unwrap()
                .is_none()
        );
        assert!(ViewSwitch::begin(Some("switching_terminal"), Show::Chat, 1).is_err());
    }

    #[test]
    fn view_switch_completes_in_both_directions_and_on_bridge_failure() {
        for (previous, target, changing, ready) in [
            ("chat", Show::Terminal, "switching_terminal", "terminal"),
            ("terminal", Show::Chat, "switching_chat", "chat"),
        ] {
            let mut switch = ViewSwitch::begin(Some(previous), target, 1)
                .unwrap()
                .unwrap();
            assert!(switch.observe(None).is_none());
            // An unrelated old report must not acknowledge a new request.
            assert!(switch.observe(Some(previous)).is_none());
            assert!(switch.observe(Some(changing)).is_none());
            assert!(switch.observe(Some(ready)) == Some(target));
            // Failed handoffs report the original mode after switching_*.
            assert!(switch.observe(Some(previous)) == Some(switch.previous));
            // Already-running target: no intermediate report is required.
            let mut switch = ViewSwitch::begin(Some(previous), target, 2)
                .unwrap()
                .unwrap();
            assert!(switch.observe(Some(ready)) == Some(target));
        }
    }

    /// Posit Assistant's screen as `pa` 0.1.0 draws it, trimmed.
    const TRUST: &str = "Posit Assistant\n v0.1.0\n│ Workspace Trust Required │\n│ Trust this workspace? │\n│ Press: t (trust) | r (restricted mode) │\n┌───┐\n│Enter a prompt │\n└───┘";
    const READY: &str = "Posit Assistant\n v0.1.0\n ⚡ Auto · A classifier reviews tool calls\n┌───┐\n│Enter a prompt │\n└───┘";
    const TYPED: &str = "Posit Assistant\n┌───┐\n│Plot the UMAP│\n└───┘";

    #[test]
    fn openscience_is_an_alphabetical_chat_only_provider() {
        let names: Vec<_> = KINDS.iter().map(|kind| kind.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable_by_key(|name| name.to_lowercase());
        assert_eq!(names, sorted);
        let kind = kind_of("openscience").unwrap();
        assert_eq!(kind.name, "OpenScience");
        assert_eq!(kind.short, "OpenScience");
        assert_eq!(kind.plan, "OpenScience · your configured models");
        assert_eq!(kind.placeholder, "Ask OpenScience…");
        assert!(drives_chat(kind.kind));
        assert!(!terminal_only(kind.kind));
        assert!(!supports_terminal(kind.kind));
        assert!(provider_show(kind.kind, Show::Terminal) == Show::Chat);
        assert!(provider_show(kind.kind, Show::Chat) == Show::Chat);
        assert!(provider_show(kind.kind, Show::New) == Show::New);
        assert!(provider_show(kind.kind, Show::History) == Show::History);
        assert!(provider_show("codex", Show::Terminal) == Show::Terminal);
        assert!(provider_show("posit", Show::Chat) == Show::Terminal);
    }

    #[test]
    fn oh_my_pi_and_pi_have_chat_and_command_line() {
        for (kind, name) in [("omp", "Oh My Pi"), ("pi", "pi")] {
            let provider = kind_of(kind).unwrap();
            assert_eq!(provider.name, name);
            assert!(drives_chat(kind));
            assert!(!terminal_only(kind));
            assert!(supports_terminal(kind));
            assert!(provider_show(kind, Show::Terminal) == Show::Terminal);
            assert!(provider_show(kind, Show::Chat) == Show::Chat);
        }
    }

    #[test]
    fn openscience_first_message_keeps_notebook_references_for_the_bridge() {
        let reference = json!({"kind": "chunk", "path": "qc.ipynb", "cell": "load"});
        let mut options = AgentOptions {
            refs: vec![reference.clone()],
            ..Default::default()
        };
        let (prompt, typed) = first_message(
            "openscience",
            Some("Explain this cell".into()),
            &mut options,
        );
        assert_eq!(prompt.as_deref(), Some("Explain this cell"));
        assert_eq!(typed, None);
        assert_eq!(options.refs, vec![reference]);
    }

    #[test]
    fn posit_assistant_takes_a_message_only_at_its_empty_prompt() {
        // Keys typed while it asks about trust would answer the question.
        assert!(!takes_message(TRUST));
        assert!(takes_message(READY));
        assert!(!takes_message(TYPED));
        assert!(!takes_message(""));
    }

    #[test]
    fn posit_assistants_first_message_is_typed_with_what_was_added() {
        let chunk =
            json!({"kind": "chunk", "name": "qc.ipynb › load", "path": "qc.ipynb", "cell": "load"});
        let mut options = AgentOptions {
            refs: vec![chunk.clone()],
            ..Default::default()
        };
        let (prompt, typed) = first_message("posit", Some("Fix this".into()), &mut options);
        assert_eq!(prompt, None);
        assert_eq!(
            typed.as_deref(),
            Some(
                "Fix this\n\nSandhill: the user added these to this message:\n\
                 - the cell `load` of qc.ipynb (get_outputs reads what it showed)"
            )
        );
        assert!(options.refs.is_empty());
        // Started without a message: nothing to type.
        let (prompt, typed) = first_message("posit", Some(String::new()), &mut options);
        assert_eq!((prompt, typed), (None, None));
        // The other agents get theirs as the daemon's prompt, refs and all.
        let mut options = AgentOptions {
            refs: vec![chunk],
            ..Default::default()
        };
        let (prompt, typed) = first_message("claude", Some("Fix this".into()), &mut options);
        assert_eq!((prompt.as_deref(), typed), (Some("Fix this"), None));
        assert_eq!(options.refs.len(), 1);
    }
}

#[cfg(test)]
mod account_tests {
    use super::account_at_limit;
    use sandhill_proto::AgentAccount;
    use serde_json::json;

    #[test]
    fn only_known_exhausted_windows_block_an_account() {
        let mut account: AgentAccount = serde_json::from_value(json!({
            "id":"codex:work", "kind":"codex", "name":"Work", "method":"login"
        }))
        .unwrap();
        assert!(!account_at_limit(&account));
        for (used, expected) in [
            (None, false),
            (Some(99.9), false),
            (Some(100.), true),
            (Some(105.), true),
        ] {
            account.usage = Some(json!({"rate_limits": {
                "primary":{"used_percentage":used},
                "secondary":{"used_percentage":12}
            }}));
            assert_eq!(account_at_limit(&account), expected);
        }
    }
}
