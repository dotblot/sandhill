//! Device-local agent accounts and Welcome's Agents page (Huan,
//! 2026-11-15; `docs/design/done/agents-page-plan.md`). The daemon owns
//! account storage and the agents' own sign-in programs; this view only
//! presents their wire API. Turning agents on or off is this Mac's choice
//! (`prefs::agent_off`), for every device.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    AgentAccount, AgentInfo, Call, TerminalEvent, TerminalInfo, TerminalSubscribed,
};

use crate::daemon::Daemon;
use crate::look::{self, Palette, icon};
use crate::panes::terminal::TerminalView;
use gpui_kit::base::ElementExt as _;

/// Which accounts a listing asks their agents about (sign-in state and
/// usage). Nothing asks on its own: only a Refresh click or a finished
/// sign-in does (Huan, 2026-11-14); otherwise the last answers are shown.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    Nobody,
    Accounts(Vec<String>),
}

impl Ask {
    /// Two waiting listings as one that asks everything either would.
    fn with(self, other: Ask) -> Ask {
        match (self, other) {
            (Ask::Nobody, ask) | (ask, Ask::Nobody) => ask,
            (Ask::Accounts(mut ids), Ask::Accounts(more)) => {
                for id in more {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                Ask::Accounts(ids)
            }
        }
    }
}

#[derive(Clone)]
enum Dialog {
    Add(String),
    Rename(AgentAccount),
    GoogleLogin(AgentAccount),
    Login(AgentAccount),
}

/// Agents whose accounts Sandhill keeps; the others use their own model
/// accounts.
const WITH_ACCOUNTS: [&str; 3] = ["claude", "codex", "antigravity"];
/// Account rows start under the agent's name, past its mark.
const INDENT: f32 = look::PANE_PAD_X + look::ICON_LARGE + look::GAP_12;

pub(crate) struct AccountManager {
    daemon: Rc<Daemon>,
    host: String,
    pub(crate) accounts: Vec<AgentAccount>,
    pub(crate) error: Option<String>,
    sign_in_error: Option<String>,
    pub(crate) refreshing: bool,
    refresh_queued: Option<Ask>,
    accounts_revision: u64,
    dialog: Option<Dialog>,
    dialog_error: Option<String>,
    pending: bool,
    generation: u64,
    focus: FocusHandle,
    name: Option<Entity<InputState>>,
    key: Option<Entity<InputState>>,
    input_subscription: Option<Subscription>,
    login: Option<Entity<TerminalView>>,
    login_id: Option<String>,
    /// The device's agents: installed or not, and their versions.
    agents: Vec<AgentInfo>,
    agents_revision: u64,
    tools_busy: bool,
    tools_error: Option<String>,
    /// The account whose ⋯ menu is open.
    menu: Option<String>,
    /// The account whose row asks "Remove …?", and why removing failed.
    removing: Option<String>,
    remove_error: Option<String>,
    remove_busy: bool,
    /// The agents in the page's order: turned on first, then by name. Set
    /// when the page opens, so a box doesn't move under the pointer when its
    /// switch is clicked (Huan, 2026-11-16).
    order: Vec<&'static crate::agents::Kind>,
    triggers: crate::shell::popup::PopupTriggers,
}

impl AccountManager {
    pub(crate) fn new(daemon: Rc<Daemon>, host: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            daemon,
            host,
            accounts: Vec::new(),
            error: None,
            sign_in_error: None,
            refreshing: false,
            refresh_queued: None,
            accounts_revision: 0,
            dialog: None,
            dialog_error: None,
            pending: false,
            generation: 0,
            focus: cx.focus_handle(),
            name: None,
            key: None,
            input_subscription: None,
            login: None,
            login_id: None,
            agents: Vec::new(),
            agents_revision: 0,
            tools_busy: false,
            tools_error: None,
            menu: None,
            removing: None,
            remove_error: None,
            order: agent_order(),
            remove_busy: false,
            triggers: Default::default(),
        };
        this.refresh(Ask::Nobody, cx);
        this.load_agents(cx);
        this
    }

    /// Reads which agents the device has, and their versions.
    pub(crate) fn load_agents(&mut self, cx: &mut Context<Self>) {
        self.agents_revision += 1;
        let revision = self.agents_revision;
        let listed = self.daemon.call::<Vec<AgentInfo>>(Call::ListAgents);
        cx.spawn(async move |this, cx| {
            if let Ok(agents) = listed.await {
                this.update(cx, |this, cx| {
                    if this.agents_revision == revision {
                        this.agents = agents;
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn set_antigravity_tools(&mut self, allowed: bool, cx: &mut Context<Self>) {
        let kind = "antigravity";
        if self.tools_busy || sandhill_proto::agent_paused(kind) {
            return;
        }
        self.tools_busy = true;
        self.tools_error = None;
        let request = self.daemon.call::<()>(Call::SetAgentTools {
            kind: kind.into(),
            allowed,
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.tools_busy = false;
                match result {
                    Ok(()) => {
                        if let Some(agent) = this.agents.iter_mut().find(|agent| agent.kind == kind)
                        {
                            agent.tools_in_settings = Some(allowed);
                        }
                        this.load_agents(cx);
                    }
                    Err(error) => {
                        this.tools_error =
                            Some(format!("Couldn't change Antigravity's settings: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The page's Refresh: the agents, and every account of an agent
    /// turned on asked about again.
    pub(crate) fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let ids = self
            .accounts
            .iter()
            .filter(|account| !crate::agents::turned_off(&account.kind))
            .map(|account| account.id.clone())
            .collect();
        self.refresh(Ask::Accounts(ids), cx);
        self.load_agents(cx);
    }

    /// Removes an account Sandhill added, after its row asked.
    fn confirm_remove(&mut self, account: AgentAccount, cx: &mut Context<Self>) {
        if account.default || self.remove_busy {
            return;
        }
        self.remove_busy = true;
        self.remove_error = None;
        let requested = self
            .daemon
            .call::<serde_json::Value>(Call::RemoveAgentAccount {
                id: account.id.clone(),
            });
        cx.spawn(async move |this, cx| {
            let result = requested.await;
            this.update(cx, |this, cx| {
                this.remove_busy = false;
                match result {
                    Ok(_) => {
                        crate::prefs::forget_agent_account(&this.host, &account.kind, &account.id);
                        this.accounts.retain(|old| old.id != account.id);
                        this.removing = None;
                        this.refresh(Ask::Nobody, cx);
                    }
                    Err(err) => this.remove_error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn refresh(&mut self, ask: Ask, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_queued = Some(match self.refresh_queued.take() {
                Some(queued) => queued.with(ask),
                None => ask,
            });
            return;
        }
        self.refreshing = true;
        self.error = self.sign_in_error.clone();
        let revision = self.accounts_revision;
        let (refresh, ids) = match ask {
            Ask::Nobody => (false, None),
            Ask::Accounts(ids) => (true, Some(ids)),
        };
        let listed = self
            .daemon
            .call::<Vec<AgentAccount>>(Call::ListAgentAccounts { refresh, ids });
        cx.spawn(async move |this, cx| {
            let result = listed.await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                match result {
                    Ok(accounts) if revision == this.accounts_revision => this.accounts = accounts,
                    Ok(_) => {
                        // A reset returned a newer account while this list was
                        // pending. Keep it visible and read the list again.
                        this.refresh_queued.get_or_insert(Ask::Nobody);
                    }
                    Err(err) => this.error = Some(format!("Could not read accounts: {err:#}")),
                }
                if let Some(ask) = this.refresh_queued.take() {
                    this.refresh(ask, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// After a sign-in ends, ask about the account it was for.
    fn signed_in_ask(&self) -> Ask {
        match &self.dialog {
            Some(Dialog::Login(account) | Dialog::GoogleLogin(account)) => {
                Ask::Accounts(vec![account.id.clone()])
            }
            _ => Ask::Nobody,
        }
    }

    pub(crate) fn apply_account(&mut self, account: AgentAccount, cx: &mut Context<Self>) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|entry| entry.id == account.id)
        {
            *existing = account;
            self.accounts_revision += 1;
            cx.notify();
        }
    }

    pub(crate) fn add(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(kind, "claude" | "codex" | "antigravity") || self.dialog.is_some() {
            return;
        }
        self.begin(Dialog::Add(kind.into()), window, cx);
        self.prepare_name("", window, cx);
        if kind == "antigravity" {
            self.key = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Gemini API key")
                    .masked(true)
            }));
        }
        cx.notify();
    }

    pub(crate) fn sign_in(
        &mut self,
        account: AgentAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some() || account.method == "api_key" {
            return;
        }
        if account.kind == "antigravity" && account.default {
            self.begin(Dialog::GoogleLogin(account), window, cx);
        } else {
            self.begin(Dialog::Login(account.clone()), window, cx);
            self.start_login(account, window, cx);
        }
        cx.notify();
    }

    fn begin(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.dialog = Some(dialog);
        self.dialog_error = None;
        self.pending = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn prepare_name(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Account name");
            input.set_value(value, window, cx);
            input.focus(window, cx);
            input
        });
        self.input_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit(window, cx);
                    }
                }),
            );
        self.name = Some(input);
    }

    fn rename(&mut self, account: AgentAccount, window: &mut Window, cx: &mut Context<Self>) {
        if account.default || self.dialog.is_some() {
            return;
        }
        let name = account.name.clone();
        self.begin(Dialog::Rename(account), window, cx);
        self.prepare_name(&name, window, cx);
    }

    /// The row asks before removing (the interface rules: per-row questions
    /// stay in the row).
    fn ask_remove(&mut self, account: &AgentAccount, cx: &mut Context<Self>) {
        if !account.default {
            self.menu = None;
            self.removing = Some(account.id.clone());
            self.remove_error = None;
            cx.notify();
        }
    }

    fn clear_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.dialog = None;
        self.dialog_error = None;
        self.pending = false;
        self.name = None;
        self.key = None;
        self.input_subscription = None;
        self.login = None;
        self.login_id = None;
        window.blur(cx);
        cx.notify();
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(view) = &self.login else {
            self.clear_dialog(window, cx);
            return;
        };
        let id = view.read(cx).id.clone();
        let stop = self.daemon.call::<serde_json::Value>(Call::StopTerminal {
            terminal: id.clone(),
        });
        let generation = self.generation;
        self.pending = true;
        self.dialog_error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = stop.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = false;
                match result {
                    Ok(_) => {
                        this.daemon.fire(Call::UnsubscribeTerminal { terminal: id });
                        let ask = this.signed_in_ask();
                        this.clear_dialog(window, cx);
                        this.refresh(ask, cx);
                    }
                    Err(err) => {
                        this.dialog_error = Some(format!("Could not stop sign-in: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending {
            return;
        }
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        if let Dialog::GoogleLogin(account) = dialog {
            self.dialog = Some(Dialog::Login(account.clone()));
            self.start_login(account, window, cx);
            return;
        }
        if matches!(dialog, Dialog::Login(_)) {
            return;
        }
        let name = self
            .name
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default();
        let api_key = self
            .key
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string());
        if matches!(dialog, Dialog::Add(_) | Dialog::Rename(_)) && name.is_empty() {
            self.dialog_error = Some("Enter an account name.".into());
            cx.notify();
            return;
        }
        if matches!(&dialog, Dialog::Add(kind) if kind == "antigravity")
            && api_key.as_deref().is_none_or(str::is_empty)
        {
            self.dialog_error = Some("Enter a Gemini API key.".into());
            cx.notify();
            return;
        }
        let call = match &dialog {
            Dialog::Add(kind) => Call::AddAgentAccount {
                kind: kind.clone(),
                name,
                api_key,
            },
            Dialog::Rename(account) => Call::RenameAgentAccount {
                id: account.id.clone(),
                name,
            },
            _ => return,
        };
        let requested = self.daemon.call::<serde_json::Value>(call);
        let generation = self.generation;
        self.pending = true;
        self.dialog_error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = requested.await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = false;
                match result {
                    Ok(value) => {
                        let added = if matches!(dialog, Dialog::Add(_)) {
                            match serde_json::from_value::<AgentAccount>(value) {
                                Ok(account) => Some(account),
                                Err(err) => {
                                    this.dialog_error =
                                        Some(format!("Could not read the added account: {err}"));
                                    this.refresh(Ask::Nobody, cx);
                                    cx.notify();
                                    return;
                                }
                            }
                        } else {
                            None
                        };
                        this.clear_dialog(window, cx);
                        this.refresh(Ask::Nobody, cx);
                        if let Some(account) = added {
                            this.accounts.retain(|old| old.id != account.id);
                            this.accounts.push(account.clone());
                            if account.kind != "antigravity" {
                                this.sign_in(account, window, cx);
                            }
                        }
                    }
                    Err(err) => this.dialog_error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn start_login(&mut self, account: AgentAccount, window: &mut Window, cx: &mut Context<Self>) {
        self.pending = true;
        self.dialog_error = None;
        let generation = self.generation;
        let requested = self.daemon.call::<TerminalInfo>(Call::SignInAgentAccount {
            id: account.id,
            cols: Some(90),
            rows: Some(24),
        });
        let daemon = self.daemon.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = requested.await;
            let orphan = result.as_ref().ok().map(|terminal| terminal.id.clone());
            let updated = this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = false;
                match result {
                    Ok(terminal) => this.follow_login(terminal, window, cx),
                    Err(err) => {
                        this.dialog_error = Some(format!("Could not start sign-in: {err:#}"))
                    }
                }
                cx.notify();
            });
            if updated.is_err()
                && let Some(terminal) = orphan
            {
                daemon.fire(Call::StopTerminal { terminal });
            }
        })
        .detach();
        cx.notify();
    }

    fn follow_login(
        &mut self,
        terminal: TerminalInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if terminal.kind != sandhill_proto::TERMINAL_LOGIN {
            self.dialog_error =
                Some("The device returned a terminal that is not a sign-in program.".into());
            return;
        }
        if !terminal.running {
            let ask = self.signed_in_ask();
            self.clear_dialog(window, cx);
            self.refresh(ask, cx);
            self.sign_in_error = terminal.exit.as_ref().and_then(sign_in_failure);
            self.error = self.sign_in_error.clone();
            return;
        }
        let id = terminal.id;
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
        let view =
            cx.new(|cx| TerminalView::new(id.clone(), input, resize, window.window_handle(), cx));
        view.read(cx).focus_handle().focus(window, cx);
        self.login = Some(view);
        self.login_id = Some(id.clone());
        let subscribed = self
            .daemon
            .call::<TerminalSubscribed>(Call::SubscribeTerminal {
                terminal: id,
                from: Some(0),
                chat_from: None,
            });
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            if let Err(err) = subscribed.await {
                this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.dialog_error = Some(format!("Could not follow sign-in: {err:#}"));
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// The owner forwards its daemon's terminal stream, including replay.
    pub(crate) fn terminal_event(&mut self, event: &TerminalEvent, cx: &mut Context<Self>) {
        let Some(view) = self
            .login
            .as_ref()
            .filter(|view| view.read(cx).id == event.terminal)
            .cloned()
        else {
            return;
        };
        if let Some(data) = &event.data {
            view.update(cx, |view, cx| view.feed(data, event.replay, cx));
        }
        if event.exit.is_some() {
            self.daemon.fire(Call::UnsubscribeTerminal {
                terminal: event.terminal.clone(),
            });
            let ask = self.signed_in_ask();
            self.generation += 1;
            self.dialog = None;
            self.dialog_error = None;
            self.pending = false;
            self.login = None;
            self.login_id = None;
            self.refresh(ask, cx);
            self.sign_in_error = event.exit.as_ref().and_then(sign_in_failure);
            self.error = self.sign_in_error.clone();
            cx.notify();
        }
    }

    pub(crate) fn render_dialogs(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = &self.dialog else {
            return div().into_any_element();
        };
        let p = *Palette::get(cx);
        let (title, action) = match dialog {
            Dialog::Add(kind) => (
                format!("Add {} Account", provider_name(kind)),
                Some("Add Account"),
            ),
            Dialog::Rename(account) => (format!("Rename {}", account.name), Some("Rename")),
            Dialog::GoogleLogin(_) => ("Change Google Login".into(), Some("Continue")),
            Dialog::Login(account) => (format!("Sign In to {}", account.name), None),
        };
        let mut card = v_flex()
            .id("account-dialog-card")
            .occlude()
            .w(px(if matches!(dialog, Dialog::Login(_)) {
                760.
            } else {
                480.
            }))
            .max_w(relative(0.9))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(look::GAP_12))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .text_color(p.text)
            .text_size(px(look::small()))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(title));
        match dialog {
            Dialog::Add(kind) => {
                card =
                    card.child(div().text_color(p.muted).child(
                        "Use an account you own. Accounts and sign-ins stay on this device.",
                    ));
                if let Some(name) = &self.name {
                    card = card.child(Input::new(name).small().disabled(self.pending));
                }
                if kind == "antigravity" {
                    if let Some(key) = &self.key {
                        card = card.child(Input::new(key).small().disabled(self.pending));
                    }
                    card = card.child(div().text_color(p.warn).child("Usage is billed to this Gemini API key, not your Antigravity subscription. The key is kept by Sandhill on this device."));
                } else {
                    card = card.child(
                        div()
                            .text_color(p.muted)
                            .child("Next, sign in using the agent's own command-line program."),
                    );
                }
            }
            Dialog::Rename(_) => {
                if let Some(name) = &self.name {
                    card = card.child(Input::new(name).small().disabled(self.pending));
                }
            }
            Dialog::GoogleLogin(_) => {
                card = card.child(div().text_color(p.warn).child("Every Antigravity conversation on this device changes account."))
                    .child(div().text_color(p.muted).child("In Antigravity's own screen, type /logout, then sign in with the Google account you want to use."));
            }
            Dialog::Login(account) => {
                card = card.child(div().text_color(p.muted).child(match account.kind.as_str() {
                    "claude" => "Finish Claude Code’s setup and sign in, then type /exit.",
                    "antigravity" => "Type /logout, then sign in with the other Google account and exit. Every Antigravity conversation on this device changes account.",
                    _ => "Follow Codex’s sign-in instructions below. This window closes when sign-in finishes.",
                }));
                if let Some(view) = &self.login {
                    card = card.child(
                        div()
                            .h(px(340.))
                            .min_h(px(140.))
                            .w_full()
                            .child(view.clone()),
                    );
                } else if self.pending {
                    card = card.child(div().text_color(p.muted).child("Starting sign-in…"));
                }
            }
        }
        card = card.when_some(self.dialog_error.clone(), |card, error| {
            card.child(div().text_color(p.err).child(error))
        });
        let mut buttons = h_flex().justify_end().gap(px(look::GAP_8)).child(
            Button::new("account-cancel")
                .small()
                .label(if matches!(dialog, Dialog::Login(_)) {
                    "Close"
                } else {
                    "Cancel"
                })
                .disabled(self.pending)
                .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
        );
        if let Some(action) = action {
            buttons = buttons.child(
                Button::new("account-submit")
                    .small()
                    .primary()
                    .label(action)
                    .disabled(self.pending)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            );
        }
        card = card.child(buttons);
        deferred(DialogOverlay {
            content: div()
                .id("account-dialog-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(p.shadow.opacity(0.35))
                .child(card)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                )
                .into_any_element(),
        })
        .with_priority(5)
        .into_any_element()
    }
}

impl Drop for AccountManager {
    fn drop(&mut self) {
        if let Some(terminal) = self.login_id.take() {
            self.daemon.fire(Call::StopTerminal {
                terminal: terminal.clone(),
            });
            self.daemon.fire(Call::UnsubscribeTerminal { terminal });
        }
    }
}

/// Dialogs opened from the status bar must cover the window, not the chip.
#[derive(IntoElement)]
struct DialogOverlay {
    content: AnyElement,
}

impl RenderOnce for DialogOverlay {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let viewport = window.viewport_size();
        anchored()
            .anchor(Anchor::TopLeft)
            .position(point(px(0.), px(0.)))
            .position_mode(AnchoredPositionMode::Window)
            .child(
                div()
                    .relative()
                    .w(viewport.width)
                    .h(viewport.height)
                    .child(self.content),
            )
    }
}

fn provider_name(kind: &str) -> &'static str {
    match kind {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "antigravity" => "Antigravity",
        _ => "Agent",
    }
}

fn sign_in_failure(exit: &sandhill_proto::TerminalExit) -> Option<String> {
    if let Some(signal) = exit.signal {
        Some(format!(
            "Sign-in ended with signal {signal}. Try signing in again."
        ))
    } else {
        exit.code
            .filter(|code| *code != 0)
            .map(|code| format!("Sign-in exited with code {code}. Try signing in again."))
    }
}

fn account_detail(account: &AgentAccount) -> String {
    let state = match account.signed_in {
        Some(true) if account.method == "api_key" => "API key saved",
        Some(true) => "Signed in",
        Some(false) => "Sign in again",
        None => "Sign-in state unavailable",
    };
    let mut parts = vec![state.to_string()];
    if let Some(identity) = &account.identity {
        parts.push(identity.clone());
    }
    let before_usage = parts.len();
    if let Some(windows) = account
        .usage
        .as_ref()
        .and_then(|usage| usage.get("rate_limits"))
        .and_then(|limits| limits.as_object())
    {
        for (key, window) in windows {
            if let Some(used) = window["used_percentage"]
                .as_f64()
                .filter(|used| used.is_finite() && (0.0..=100.).contains(used))
            {
                let label = window["label"].as_str().unwrap_or(match key.as_str() {
                    "five_hour" | "primary" => "5-hour",
                    "seven_day" | "secondary" => "Weekly",
                    _ => key,
                });
                parts.push(format!("{label}: {used:.0}% used (last known)"));
            }
        }
    }
    if parts.len() == before_usage {
        parts.push(
            if account.method == "api_key" {
                "Billed to this Gemini API key"
            } else {
                "Usage unavailable"
            }
            .into(),
        );
    }
    parts.join(" · ")
}

/// What an agent runs, its version and plan ("Codex 0.62.0 · your ChatGPT
/// plan"), or that the device lacks it.
fn agent_detail(kind: &crate::agents::Kind, info: Option<&AgentInfo>, host: &str) -> String {
    if info.is_some_and(|info| info.path.is_none()) {
        return format!(
            "Not installed on {}",
            crate::welcome::start_screen::host_label(host)
        );
    }
    let plan = kind.plan.rsplit(" · ").next().unwrap_or(kind.plan);
    match info
        .and_then(|info| info.version.as_deref())
        .and_then(sandhill_proto::version_in)
    {
        Some(version) => format!("{} {version} · {plan}", kind.name),
        None => format!("{} · {plan}", kind.name),
    }
}

impl AccountManager {
    /// An agent's group: its name, version and switch, then its accounts.
    fn agent_group(
        &self,
        kind: &'static crate::agents::Kind,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = !crate::agents::turned_off(kind.kind);
        let info = self.agents.iter().find(|agent| agent.kind == kind.kind);
        let missing = info.is_some_and(|info| info.path.is_none());
        let mut detail = look::settings_detail(agent_detail(kind, info, &self.host), p);
        if kind.kind == "antigravity" {
            detail = detail.child(div().text_color(p.warn).child(
                "Google suspended an account used with Sandhill; turning it on is your choice.",
            ));
        }
        let install = info
            .filter(|_| missing && on)
            .map(|info| info.install.clone());
        if let Some(command) = install.as_ref() {
            detail = detail.child(div().font_family(look::mono()).child(command.clone()));
        }
        let id = kind.kind;
        let head = h_flex()
            .w_full()
            .min_h(px(look::row()))
            .gap(px(look::GAP_12))
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .child(
                div()
                    .w(px(look::ICON_LARGE))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .when(!on, |d| d.opacity(0.5))
                    .child(crate::agents::view::provider(id).size(px(look::ICON_LARGE))),
            )
            .child(
                look::settings_label(kind.name, Some(detail)).when(!on, |d| d.text_color(p.muted)),
            )
            .children(install.map(|command| {
                Button::new(SharedString::from(format!("agent-install-{id}")))
                    .small()
                    .label("Copy Install Command")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                    })
            }))
            .child(
                gpui_kit::component::switch::Switch::new(SharedString::from(format!(
                    "agent-on-{id}"
                )))
                .small()
                .color(p.accent)
                .checked(on)
                .accessibility_label(kind.name)
                .on_change(cx.listener(move |_, on: &bool, _, cx| {
                    crate::shell::choices::choose(
                        crate::shell::choices::Choice::Agent(id, *on),
                        cx,
                    );
                    cx.notify();
                })),
            )
            .into_any_element();
        let mut rows = vec![head];
        if id == "antigravity"
            && let Some(allowed) = info.and_then(|info| info.tools_in_settings)
        {
            rows.push(
                v_flex()
                    .pl(px(INDENT))
                    .pr(px(look::PANE_PAD_X))
                    .py(px(look::PANE_PAD_Y))
                    .gap(px(look::GAP_4))
                    .child(
                        Checkbox::new("antigravity-tools-consent")
                            .label("Let Antigravity use Sandhill's tools")
                            .checked(allowed)
                            .disabled(self.tools_busy || (missing && !allowed))
                            .on_click(cx.listener(|this, allowed: &bool, _, cx| {
                                this.set_antigravity_tools(*allowed, cx)
                            })),
                    )
                    .child(look::settings_detail(
                        "This adds a Sandhill entry to ~/.gemini/config/mcp_config.json. Untick it to remove just that entry.",
                        p,
                    ))
                    .when_some(self.tools_error.as_ref(), |d, error| {
                        d.child(div().text_color(p.err).whitespace_normal().child(error.clone()))
                    })
                    .into_any_element(),
            );
        }
        if on && WITH_ACCOUNTS.contains(&id) {
            let accounts: Vec<&AgentAccount> = self
                .accounts
                .iter()
                .filter(|account| account.kind == id)
                .collect();
            if accounts.is_empty() {
                rows.push(
                    div()
                        .pl(px(INDENT))
                        .pr(px(look::PANE_PAD_X))
                        .py(px(look::PANE_PAD_Y))
                        .text_color(p.muted)
                        .child(if self.refreshing {
                            "Loading accounts…"
                        } else {
                            "No accounts reported by this device."
                        })
                        .into_any_element(),
                );
            }
            // The account new chats start with: the one picked, else the
            // agent's own login.
            let picked = crate::prefs::agent_account(&self.host, id)
                .filter(|picked| accounts.iter().any(|account| &account.id == picked));
            for account in accounts {
                let for_new = picked
                    .as_deref()
                    .map_or(account.default, |picked| picked == account.id);
                rows.push(self.account_row(account, for_new, p, cx));
            }
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("account-add-{id}")))
                    .min_h(px(look::row()))
                    .pl(px(INDENT))
                    .pr(px(look::PANE_PAD_X))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .text_color(p.accent)
                    .cursor_pointer()
                    .hover(move |d| d.bg(p.ink(look::ROW_HOVER)))
                    .child(icon("add").size(px(look::ICON)))
                    .child("Add Account…")
                    .on_click(cx.listener(move |this, _, window, cx| this.add(id, window, cx)))
                    .into_any_element(),
            );
        }
        // The first row names the agent, so the box has no heading.
        look::settings_box(rows, p).into_any_element()
    }

    /// One account: its name (tagged when new chats start with it), its
    /// sign-in and use, Sign In when needed and a ⋯ menu. Asked to remove,
    /// the row becomes the question.
    fn account_row(
        &self,
        account: &AgentAccount,
        for_new: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.removing.as_deref() == Some(account.id.as_str()) {
            let remove = account.clone();
            return h_flex()
                .w_full()
                .min_h(px(look::row()))
                .gap(px(look::GAP_8))
                .pl(px(INDENT))
                .pr(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .child(look::settings_label(
                    format!("Remove {}?", account.name),
                    Some(match &self.remove_error {
                        Some(error) => look::settings_detail(error.clone(), p).text_color(p.err),
                        None => look::settings_detail(
                            "Its saved sign-in goes; conversation history is kept.",
                            p,
                        ),
                    }),
                ))
                .child(
                    Button::new("account-remove-cancel")
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.removing = None;
                            this.remove_error = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("account-remove-confirm")
                        .small()
                        .danger()
                        .label("Remove")
                        .disabled(self.remove_busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm_remove(remove.clone(), cx)
                        })),
                )
                .into_any_element();
        }
        let kind = account.kind.clone();
        // Antigravity's own login can always be changed (whole device);
        // Claude Code and Codex logins only when missing or expired.
        let sign_in = account.method != "api_key"
            && (kind == "antigravity" || account.signed_in != Some(true));
        let login = account.clone();
        let open = self.menu.as_deref() == Some(account.id.as_str());
        let menu_id = account.id.clone();
        h_flex()
            .w_full()
            .min_h(px(look::row()))
            .gap(px(look::GAP_8))
            .pl(px(INDENT))
            .pr(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(look::GAP_2))
                    .child(
                        h_flex()
                            .gap(px(look::GAP_8))
                            .child(account.name.clone())
                            .when(for_new, |d| {
                                d.child(
                                    div()
                                        .px(px(look::GAP_6))
                                        .rounded_full()
                                        .bg(p.accent_soft)
                                        .text_color(p.accent)
                                        .text_size(px(look::small()))
                                        .child("Default for new chats"),
                                )
                            }),
                    )
                    .child(
                        look::settings_detail(account_detail(account), p)
                            .when(account.signed_in == Some(false), |d| d.text_color(p.warn)),
                    ),
            )
            .when(sign_in, |d| {
                d.child(
                    Button::new(SharedString::from(format!(
                        "account-sign-in-{}",
                        account.id
                    )))
                    .small()
                    .label(if kind == "antigravity" {
                        "Change Google Login…"
                    } else {
                        "Sign In"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.sign_in(login.clone(), window, cx)
                    })),
                )
            })
            // The agent's own login, already the one new chats use, has
            // nothing in the menu.
            .when(!for_new || !account.default, |d| {
                d.child(
                    div()
                        .relative()
                        .flex_none()
                        .child(
                            look::icon_button(
                                SharedString::from(format!("account-menu-{}", account.id)),
                                "more",
                                p,
                                false,
                            )
                            .when(open, |d| d.on_prepaint(self.triggers.track("account-menu")))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.menu = if this.menu.as_deref() == Some(menu_id.as_str()) {
                                        None
                                    } else {
                                        Some(menu_id.clone())
                                    };
                                    cx.notify();
                                },
                            )),
                        )
                        .when(open, |d| {
                            d.child(self.account_menu(account, for_new, p, cx))
                        }),
                )
            })
            .into_any_element()
    }

    /// The ⋯ menu: Use for New Chats, Rename…, Remove….
    fn account_menu(
        &self,
        account: &AgentAccount,
        for_new: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = p.ink(look::ROW_HOVER);
        let item = |id: &'static str, label: &'static str| {
            h_flex()
                .id(id)
                .h(px(look::row()))
                .px(px(look::GAP_8))
                .rounded(px(look::CONTROL_RADIUS))
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .whitespace_nowrap()
                .child(label)
        };
        let mut menu = crate::agents::composer_view::popup(p)
            .min_w(px(180.))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if !this.triggers.contains("account-menu", event.position) {
                    this.menu = None;
                    cx.notify();
                }
            }));
        if !for_new {
            let (host, kind, id) = (self.host.clone(), account.kind.clone(), account.id.clone());
            menu = menu.child(
                item("account-use-new", "Use for New Chats").on_click(cx.listener(
                    move |this, _, _, cx| {
                        crate::prefs::save_agent_account(&host, &kind, &id);
                        this.menu = None;
                        // The status bar's quota panel shows it too.
                        cx.refresh_windows();
                    },
                )),
            );
        }
        if !account.default {
            let rename = account.clone();
            let remove = account.clone();
            menu = menu
                .child(item("account-rename", "Rename…").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.menu = None;
                        this.rename(rename.clone(), window, cx)
                    },
                )))
                .child(
                    item("account-remove", "Remove…")
                        .text_color(p.err)
                        .on_click(cx.listener(move |this, _, _, cx| this.ask_remove(&remove, cx))),
                );
        }
        div()
            .absolute()
            .right_0()
            .top(px(look::control_row() + look::GAP_2))
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopRight)
                        .snap_to_window_with_margin(px(look::PANE_PAD_Y))
                        .child(menu),
                )
                .with_priority(3),
            )
            .into_any_element()
    }
}

impl Render for AccountManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let kinds = self.order.clone();
        v_flex()
            .relative()
            .w_full()
            .gap(px(look::GAP_12))
            .when_some(self.error.clone(), |d, error| {
                d.child(div().text_color(p.err).child(error))
            })
            .children(kinds.into_iter().map(|kind| self.agent_group(kind, &p, cx)))
            .child(self.render_dialogs(cx))
    }
}

/// Turned-on agents first, each group by name. Paused agents are left out:
/// they can't be turned on.
fn agent_order() -> Vec<&'static crate::agents::Kind> {
    let mut kinds: Vec<&'static crate::agents::Kind> = crate::agents::KINDS
        .iter()
        .filter(|kind| !sandhill_proto::agent_paused(kind.kind))
        .collect();
    kinds.sort_by_key(|kind| {
        (
            crate::agents::turned_off(kind.kind),
            kind.name.to_lowercase(),
        )
    });
    kinds
}

#[cfg(test)]
mod tests {
    use super::Ask;

    #[test]
    fn queued_refreshes_keep_every_account_asked_for() {
        let ids = |ids: &[&str]| Ask::Accounts(ids.iter().map(|id| id.to_string()).collect());
        assert_eq!(ids(&["a"]).with(ids(&["b", "a"])), ids(&["a", "b"]));
        assert_eq!(Ask::Nobody.with(ids(&["a"])), ids(&["a"]));
        assert_eq!(ids(&["a"]).with(Ask::Nobody), ids(&["a"]));
        assert_eq!(Ask::Nobody.with(Ask::Nobody), Ask::Nobody);
    }
}
