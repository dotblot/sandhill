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
pub(crate) mod code_actions;
pub(crate) mod composer;
pub(crate) mod composer_view;
pub(crate) mod context_panel;
pub(crate) mod edits;
pub(crate) mod effort_bar;
pub(crate) mod ghost;
pub(crate) mod input_box;
pub(crate) mod notebook_ai;
pub(crate) mod quota;
pub(crate) mod start;
pub(crate) mod table_actions;
pub(crate) mod view;

mod messages;
mod terminal_events;
mod view_switching;

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
use crate::panes::terminal::TerminalView;
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

/// Agents turned off on Welcome's Agents page (Huan, 2026-11-15), or paused
/// in Sandhill (`sandhill_proto::PAUSED_AGENTS`, Huan, 2026-11-18): not
/// offered for new conversations, never asked for models, accounts or
/// quota, and their conversations take no messages.
pub(crate) fn turned_off(kind: &str) -> bool {
    sandhill_proto::agent_paused(kind) || crate::prefs::agent_off(kind)
}

/// The agent a new conversation starts with (Huan, 2026-11-14): Codex, else
/// the first agent turned on.
pub(crate) fn default_kind() -> &'static Kind {
    kind_of("codex")
        .filter(|kind| !turned_off(kind.kind))
        .or_else(|| KINDS.iter().find(|kind| !turned_off(kind.kind)))
        .unwrap_or(&KINDS[0])
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
    /// What was added to the message with @ and #, shown as chips.
    pub(crate) refs: Vec<Value>,
    /// What a new conversation's screen knows of the project.
    pub(crate) start: crate::agents::start::StartContext,
    pub(crate) menu: Option<Menu>,
    /// The context panel's "keep in mind" for Claude Code's compact, made
    /// when the panel first opens.
    pub(crate) compact_focus: Option<Entity<InputState>>,
    /// The conversation writing a summary to start a new one from.
    pub(crate) summarizing: Option<String>,
    /// The height the message text was dragged to; `None` is its own.
    pub(crate) composer_height: Option<f32>,
    pub(crate) popup_triggers: crate::shell::popup::PopupTriggers,
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
    /// Up/Down in the message box: the conversation, how far back
    /// (0 = its newest message) and the unsent text to come back to.
    pub(crate) recall: Option<(String, usize, String)>,
    /// The right-click menu over the conversation's text.
    pub(crate) text_menu: Option<crate::shell::text_menu::TextMenu>,
    /// The ⋯ menu of a piece of code in the conversation.
    pub(crate) code_menu: Option<crate::agents::code_actions::CodeMenu>,
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
            refs: Vec::new(),
            start: Default::default(),
            menu: None,
            compact_focus: None,
            summarizing: None,
            composer_height: crate::prefs::input_height("agent"),
            popup_triggers: crate::shell::popup::PopupTriggers::default(),
            model_tab: None,
            favourites: crate::prefs::favourites(),
            new_choice: Choice::default(),
            browsed_effort: None,
            recall: None,
            text_menu: None,
            code_menu: None,
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
        AgentOptions, KINDS, Show, drives_chat, first_message, kind_of, provider_show,
        supports_terminal, takes_message, terminal_only,
    };
    use serde_json::json;

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
