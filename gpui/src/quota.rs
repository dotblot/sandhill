//! Host-local agent accounts and quotas. Sign-ins stay in the agent's own CLI.
//! Each provider has its own status chip and account popup; hidden providers
//! remain reachable through the full account list in the same popup frame.
//! Nothing asks an agent for its quota on its own: the chips show what the
//! last Refresh click saved on the device, until its window resets
//! (Huan, 2026-11-14).

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{AgentAccount, Call, TerminalEvent};
use serde::Deserialize;
use serde_json::Value;

use crate::agent_accounts::{AccountManager, Ask};
use crate::agents::{Agents, Show};
use crate::daemon::Daemon;
use crate::look::{self, Palette};

/// OpenScience, Oh My Pi and pi use the user's own model accounts and keys,
/// and Posit Assistant manages its own models: none has a quota to show.
fn quota_providers() -> impl Iterator<Item = &'static crate::agents::Kind> {
    crate::agents::KINDS
        .iter()
        .filter(|provider| !matches!(provider.kind, "openscience" | "omp" | "pi" | "posit"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuotaPopup {
    All,
    Provider(&'static str),
}

pub(crate) struct QuotaMonitor {
    daemon: Rc<Daemon>,
    host: String,
    manager: Entity<AccountManager>,
    agents: Option<Entity<Agents>>,
    agents_observation: Option<Subscription>,
    popup_triggers: crate::popup::PopupTriggers,
    quota_open: Option<QuotaPopup>,
    enabled: BTreeMap<String, bool>,
    reset_confirm: Option<ResetConfirmation>,
    reset_pending: Option<String>,
    reset_notices: BTreeMap<String, (String, bool)>,
}

#[derive(Default)]
struct Account {
    observed_at: i64,
    windows: BTreeMap<String, QuotaWindow>,
}

#[derive(Clone)]
struct QuotaWindow {
    label: String,
    ring_label: SharedString,
    used: Option<f32>,
    reset_ms: Option<i64>,
    requires_reset: bool,
    observed_ms: i64,
}

#[derive(Clone)]
struct ResetConfirmation {
    account: String,
    credit: String,
    expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct ResetResponse {
    outcome: String,
    account: AgentAccount,
}

#[derive(Debug, PartialEq, Eq)]
enum RemainingLevel {
    Ok,
    Warn,
    Err,
}

fn remaining_level(remaining: f32) -> RemainingLevel {
    if remaining >= 30. {
        RemainingLevel::Ok
    } else if remaining >= 10. {
        RemainingLevel::Warn
    } else {
        RemainingLevel::Err
    }
}

fn remaining_color(remaining: f32, p: &Palette) -> Hsla {
    match remaining_level(remaining) {
        RemainingLevel::Ok => p.ok,
        RemainingLevel::Warn => p.warn,
        RemainingLevel::Err => p.err,
    }
}

fn timestamp(value: &Value) -> Option<i64> {
    if let Some(seconds) = value.as_i64() {
        return seconds.checked_mul(1000);
    }
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn window_label(minutes: Option<u64>, fallback: &str) -> String {
    match minutes {
        Some(10080) => "7d".into(),
        Some(minutes) if minutes > 0 && minutes % 1440 == 0 => format!("{}d", minutes / 1440),
        Some(minutes) if minutes > 0 && minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) if minutes > 0 => format!("{minutes}m"),
        _ => fallback.into(),
    }
}

/// CLI display names retain their model group in the popup, not in the ring.
fn compact_window_label(window: &str) -> String {
    match window.trim() {
        "hourly" => "1h".into(),
        "daily" => "1d".into(),
        "weekly" => "7d".into(),
        "monthly" => "1mo".into(),
        window => {
            let split = window.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
            let (amount, unit) = window.split_at(split);
            let Ok(amount) = amount.parse::<u64>() else {
                return "?".into();
            };
            let minutes = match unit.trim() {
                "m" | "min" | "minute" | "minutes" => Some(amount),
                "h" | "hr" | "hour" | "hours" => amount.checked_mul(60),
                "d" | "day" | "days" => amount.checked_mul(1440),
                "w" | "week" | "weeks" => amount.checked_mul(10080),
                _ => None,
            };
            window_label(minutes, "?")
        }
    }
}

impl QuotaWindow {
    fn parse(key: &str, value: &Value, observed_ms: i64) -> Self {
        let minutes = value["window_minutes"].as_u64().or(match key {
            "five_hour" => Some(300),
            "seven_day" => Some(10080),
            _ => None,
        });
        let used = value["used_percentage"]
            .as_f64()
            .filter(|used| used.is_finite() && (0.0..=100.0).contains(used))
            .map(|used| used as f32);
        Self {
            label: value["label"].as_str().map_or_else(
                || match minutes {
                    Some(300) => "5-hour".into(),
                    Some(10080) => "Weekly".into(),
                    _ => window_label(minutes, key),
                },
                str::to_owned,
            ),
            ring_label: window_label(minutes, "?").into(),
            used,
            reset_ms: timestamp(&value["resets_at"]),
            requires_reset: true,
            observed_ms: value["observed_at_ms"].as_i64().unwrap_or(observed_ms),
        }
    }

    fn parse_cli(key: &str, value: &Value, observed_ms: i64) -> Self {
        let mut window = Self::parse(key, value, observed_ms);
        if let Some(label) = value["label"].as_str() {
            window.label = label.into();
        }
        if window.ring_label.as_ref() == "?" {
            let duration = value["window"]
                .as_str()
                .unwrap_or_else(|| window.label.rsplit(" · ").next().unwrap_or(key));
            window.ring_label = compact_window_label(duration).into();
        }
        // Omission is allowed; malformed and expired explicit resets are not.
        window.requires_reset = value.get("resets_at").is_some_and(|reset| !reset.is_null());
        window
    }

    fn value(&self, now: i64) -> Option<f32> {
        (self.observed_ms > 0
            && now >= self.observed_ms
            && self
                .reset_ms
                .map_or(!self.requires_reset, |reset| reset > now))
        .then_some(self.used)
        .flatten()
    }

    fn remaining(&self, now: i64) -> Option<f32> {
        self.value(now).map(|used| 100. - used)
    }

    fn row(&self, now: i64, p: &Palette) -> Div {
        let remaining = self.remaining(now);
        let known = self.used.map(|used| 100. - used);
        let color = remaining.map_or(p.faint, |left| remaining_color(left, p));
        let suffix = if remaining.is_none() {
            " · last known".into()
        } else if let Some(reset) = self.reset_ms {
            format!(" · {}", countdown((reset - now + 999) / 1000))
        } else {
            String::new()
        };
        h_flex()
            .w_full()
            .gap(px(8.))
            .child(div().w(px(106.)).flex_none().child(self.label.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(32.))
                    .h(px(10.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(p.ink(8.))
                    .child(
                        div()
                            .h_full()
                            .w(relative(known.unwrap_or(100.) / 100.))
                            .rounded_full()
                            .bg(color),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .justify_end()
                    .min_w(px(150.))
                    .when_some(known, |row, left| {
                        row.child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if remaining.is_some() { p.text } else { p.muted })
                                .child(format!("{left:.0}% left")),
                        )
                        .child(div().text_color(p.muted).child(suffix))
                    })
                    .when(known.is_none(), |row| {
                        row.child(div().text_color(p.muted).child("unavailable"))
                    }),
            )
    }
}

/// "12d 4h", "3h 20m", "45m".
fn countdown(seconds: i64) -> String {
    if seconds >= 86400 {
        format!("{}d {}h", seconds / 86400, seconds % 86400 / 3600)
    } else if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else {
        format!("{}m", (seconds + 59) / 60)
    }
}

/// Codex's banked resets for an account, from its last usage read:
/// how many are left and when the first expires. `None` when Codex has not
/// reported them.
fn banked_resets(usage: Option<&Value>, now_ms: i64) -> Option<String> {
    let credits = usage?.get("reset_credits")?;
    let available = credits["available"].as_i64()?;
    Some(match (available, credits["expires_at"].as_i64()) {
        (0, _) => "No banked resets".into(),
        (n, expiry) => {
            let noun = if n == 1 { "reset" } else { "resets" };
            let expiry = reset_expiry(expiry, now_ms);
            format!("↺ {n} {noun} banked · {expiry}")
        }
    })
}

fn reset_expiry(expires_at: Option<i64>, now_ms: i64) -> String {
    match expires_at {
        Some(expires) if expires > now_ms / 1000 => {
            format!("next expires in {}", countdown(expires - now_ms / 1000))
        }
        Some(_) => "next expiry passed, awaiting update".into(),
        None => "no expiry".into(),
    }
}

impl Account {
    fn from_agent(account: &AgentAccount) -> Self {
        let usage = account.usage.as_ref();
        let observed = usage
            .and_then(|usage| usage["observed_at_ms"].as_i64())
            .unwrap_or(0);
        let windows = usage
            .and_then(|usage| usage["rate_limits"].as_object())
            .map(|windows| {
                windows
                    .iter()
                    .map(|(key, value)| (key.clone(), QuotaWindow::parse_cli(key, value, observed)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            observed_at: observed,
            windows,
        }
    }
}

impl QuotaMonitor {
    pub(crate) fn new(daemon: Rc<Daemon>, host: String, cx: &mut Context<Self>) -> Self {
        let enabled = crate::prefs::quota_providers(&host);
        let manager = cx.new(|cx| AccountManager::new(daemon.clone(), host.clone(), cx));
        cx.observe(&manager, |this, manager, cx| {
            if let Some(agents) = &this.agents {
                if agents.read(cx).accounts == manager.read(cx).accounts {
                    cx.notify();
                    return;
                }
                let accounts = manager.read(cx).accounts.clone();
                agents.update(cx, |agents, cx| {
                    agents.accounts = accounts;
                    cx.notify();
                });
            }
            cx.notify();
        })
        .detach();
        Self {
            daemon,
            host,
            manager,
            agents: None,
            agents_observation: None,
            popup_triggers: crate::popup::PopupTriggers::default(),
            quota_open: None,
            enabled,
            reset_confirm: None,
            reset_pending: None,
            reset_notices: BTreeMap::new(),
        }
    }

    pub(crate) fn set_agents(&mut self, agents: Entity<Agents>, cx: &mut Context<Self>) {
        let accounts = self.manager.read(cx).accounts.clone();
        agents.update(cx, |agents, cx| {
            agents.accounts = accounts;
            cx.notify();
        });
        self.agents_observation = Some(cx.observe(&agents, |_, _, cx| cx.notify()));
        self.agents = Some(agents);
        cx.notify();
    }

    pub(crate) fn terminal_event(&mut self, event: &TerminalEvent, cx: &mut Context<Self>) {
        self.manager
            .update(cx, |manager, cx| manager.terminal_event(event, cx));
    }

    /// Shows what the device last saved; with `kind`, first asks that agent
    /// about each of its accounts.
    fn refresh_accounts(&mut self, kind: Option<&str>, cx: &mut Context<Self>) {
        // Another window may have changed which chips show.
        self.enabled = crate::prefs::quota_providers(&self.host);
        self.manager.update(cx, |manager, cx| {
            let ask = match kind {
                Some(kind) => Ask::Accounts(
                    manager
                        .accounts
                        .iter()
                        .filter(|account| account.kind == kind)
                        .map(|account| account.id.clone())
                        .collect(),
                ),
                None => Ask::Nobody,
            };
            manager.refresh(ask, cx)
        });
    }

    fn shown_account(&self, kind: &str, cx: &App) -> Option<(String, bool)> {
        let agents = self.agents.as_ref()?.read(cx);
        if !matches!(agents.show, Show::Chat | Show::Terminal)
            || agents.account_host(cx) != self.host
        {
            return None;
        }
        let terminal = agents.current().filter(|terminal| terminal.kind == kind)?;
        Some((
            terminal.options.account.as_deref().unwrap_or(kind).into(),
            terminal.running || crate::composer::can_resume(terminal),
        ))
    }

    pub(crate) fn dismiss_popup(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.quota_open.take().is_some();
        self.reset_confirm = None;
        if open {
            cx.notify();
        }
        open
    }

    fn use_reset(&mut self, cx: &mut Context<Self>) {
        if self.reset_pending.is_some() {
            return;
        }
        let Some(confirm) = self.reset_confirm.clone() else {
            return;
        };
        self.reset_pending = Some(confirm.account.clone());
        self.reset_notices.remove(&confirm.account);
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            let result = daemon
                .call::<ResetResponse>(Call::UseAgentReset {
                    id: confirm.account.clone(),
                    credit: Some(confirm.credit),
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.reset_pending = None;
                if this
                    .reset_confirm
                    .as_ref()
                    .is_some_and(|open| open.account == confirm.account)
                {
                    this.reset_confirm = None;
                }
                let notice = match result {
                    Ok(response) => {
                        this.manager.update(cx, |manager, cx| {
                            manager.apply_account(response.account, cx);
                        });
                        match response.outcome.as_str() {
                            "reset" => ("Limits refilled.".into(), false),
                            "nothingToReset" => (
                                "Codex can only use a reset once a limit is mostly used.".into(),
                                true,
                            ),
                            "noCredit" => ("No banked resets left.".into(), true),
                            "alreadyRedeemed" => ("That reset was already used.".into(), true),
                            outcome => (format!("Unexpected reset outcome: {outcome}"), true),
                        }
                    }
                    Err(error) => (format!("Could not use reset: {error:#}"), true),
                };
                this.reset_notices.insert(confirm.account, notice);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn account_card(
        &self,
        entry: &AgentAccount,
        kind: &'static str,
        selection: (&str, bool),
        now: i64,
        p: &Palette,
        cx: &Context<Self>,
    ) -> Div {
        let (account_id, can_switch) = selection;
        let for_new_chats = !can_switch;
        let current = account_id == entry.id;
        let signed_out = entry.method == "login" && entry.signed_in != Some(true);
        let mut header = h_flex().justify_between().gap(px(8.)).child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(entry.name.clone()),
                )
                .when_some(entry.identity.clone(), |row, identity| {
                    row.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(p.muted)
                            .child(identity),
                    )
                }),
        );
        if signed_out {
            let entry = entry.clone();
            header = header.child(
                Button::new(SharedString::from(format!("quota-sign-in-{}", entry.id)))
                    .small()
                    .text_size(px(13.))
                    .label("Sign In")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.dismiss_popup(cx);
                        this.manager
                            .update(cx, |manager, cx| manager.sign_in(entry.clone(), window, cx));
                    })),
            );
        } else if current {
            header = header.child(
                div()
                    .flex_none()
                    .px(px(7.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(p.accent_soft)
                    .text_color(p.accent)
                    .child(if for_new_chats { "New chats" } else { "In use" }),
            );
        } else {
            let entry = entry.clone();
            header = header.child(
                Button::new(SharedString::from(format!("quota-use-{}", entry.id)))
                    .small()
                    .text_size(px(13.))
                    .label(if for_new_chats {
                        "Use for new chats"
                    } else {
                        "Use"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if for_new_chats {
                            if this
                                .shown_account(kind, cx)
                                .is_some_and(|(_, can_switch)| can_switch)
                            {
                                return;
                            }
                            crate::prefs::save_agent_account(&this.host, kind, &entry.id);
                            cx.notify();
                            return;
                        }
                        if !this
                            .shown_account(kind, cx)
                            .is_some_and(|(id, can_switch)| can_switch && id != entry.id)
                        {
                            return;
                        }
                        if let Some(agents) = &this.agents {
                            agents.update(cx, |agents, cx| {
                                agents.switch_account(entry.clone(), window, cx)
                            });
                            this.dismiss_popup(cx);
                        }
                    })),
            );
        }
        let mut card = v_flex()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(p.line)
            .child(header);
        let snapshot = Account::from_agent(entry);
        if entry.method == "api_key" {
            card = card.child(div().text_color(p.muted).child("API key · no plan limits"));
        } else if signed_out {
            card = card.child(div().text_color(p.muted).child("Signed out"));
        } else if snapshot.windows.is_empty() {
            card = card.child(div().text_color(p.muted).child("No usage reported yet"));
        } else {
            for window in snapshot.windows.values() {
                card = card.child(window.row(now, p));
            }
            if snapshot.observed_at > 0 && now >= snapshot.observed_at {
                card = card.child(div().text_color(p.muted).child(format!(
                    "Checked {} ago",
                    countdown((now - snapshot.observed_at) / 1000)
                )));
            }
        }
        if kind == "codex" && entry.method == "login" {
            let credits = entry
                .usage
                .as_ref()
                .and_then(|usage| usage.get("reset_credits"));
            let credit = credits
                .and_then(|credits| credits["next_id"].as_str())
                .map(str::to_owned);
            let available = credits
                .and_then(|credits| credits["available"].as_u64())
                .unwrap_or(0);
            let expires_at = credits.and_then(|credits| credits["expires_at"].as_i64());
            let pending = self.reset_pending.as_deref() == Some(entry.id.as_str());
            let confirm_open = self
                .reset_confirm
                .as_ref()
                .is_some_and(|confirm| confirm.account == entry.id);
            let confirmation = credit.map(|credit| ResetConfirmation {
                account: entry.id.clone(),
                credit,
                expires_at,
            });
            let disabled = signed_out
                || available == 0
                || confirmation.is_none()
                || self.reset_pending.is_some();
            card = card.child(
                h_flex()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(p.line)
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div().flex_1().text_color(p.muted).child(
                            banked_resets(entry.usage.as_ref(), now)
                                .unwrap_or_else(|| "Banked resets: not reported yet".into()),
                        ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("quota-reset-{}", entry.id)))
                            .small()
                            .text_size(px(13.))
                            .label(if pending {
                                "Using reset…"
                            } else {
                                "Use Now…"
                            })
                            .disabled(disabled || confirm_open)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.reset_pending.is_none() {
                                    this.reset_confirm = confirmation.clone();
                                    cx.notify();
                                }
                            })),
                    ),
            );
            if let Some(confirm) = self
                .reset_confirm
                .as_ref()
                .filter(|confirm| confirm.account == entry.id)
            {
                let question = match confirm.expires_at {
                    Some(expires) if expires > now / 1000 => format!(
                        "Use the reset that expires in {} now?",
                        countdown(expires - now / 1000)
                    ),
                    Some(_) => "Use the reset whose expiry has passed now?".into(),
                    None => "Use the banked reset with no expiry now?".into(),
                };
                card = card.child(
                    v_flex().gap(px(8.)).p(px(10.)).rounded(px(6.)).bg(p.ink(4.))
                        .child(question)
                        .child(div().text_color(p.muted).child("Refills the 5-hour and weekly limits now and moves your weekly reset date. This can't be undone."))
                        .child(
                            h_flex().justify_end().gap(px(8.))
                                .child(
                                    Button::new(SharedString::from(format!("quota-reset-cancel-{}", entry.id)))
                                        .small().text_size(px(13.)).label("Cancel").disabled(pending)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.reset_pending.is_none() {
                                                this.reset_confirm = None;
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("quota-reset-confirm-{}", entry.id)))
                                        .small().text_size(px(13.)).primary()
                                        .label(if pending { "Using reset…" } else { "Use Reset" })
                                        .disabled(self.reset_pending.is_some())
                                        .on_click(cx.listener(|this, _, _, cx| this.use_reset(cx))),
                                ),
                        ),
                );
            }
            if let Some((notice, error)) = self.reset_notices.get(&entry.id) {
                card = card.child(
                    div()
                        .text_color(if *error { p.err } else { p.ok })
                        .child(notice.clone()),
                );
            }
        } else if kind == "claude" {
            card = card.child(
                h_flex()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(p.line)
                    .gap(px(4.))
                    .text_color(p.muted)
                    .child("↺ Limit resets: see")
                    .child(
                        div()
                            .id(SharedString::from(format!("claude-resets-{}", entry.id)))
                            .text_color(p.accent)
                            .cursor_pointer()
                            .child("claude.ai")
                            .on_click(|_, _, cx| cx.open_url("https://claude.ai/settings/usage")),
                    ),
            );
        }
        card
    }

    fn provider_enabled(&self, kind: &str) -> bool {
        self.enabled.get(kind).copied().unwrap_or(true)
    }

    fn set_provider(&mut self, kind: &'static str, on: bool, cx: &mut Context<Self>) {
        crate::prefs::save_quota_provider(&self.host, kind, on);
        self.enabled.insert(kind.into(), on);
        if on {
            self.refresh_accounts(None, cx);
        }
        cx.notify();
    }
}

impl Render for QuotaMonitor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let hover = p.ink(7.);
        let now = chrono::Utc::now().timestamp_millis();
        let manager = self.manager.read(cx);
        let managed_accounts = &manager.accounts;
        let manager_refreshing = manager.refreshing;
        let manager_error = &manager.error;
        let mut strip = h_flex()
            .id("account-quota")
            .flex_none()
            .gap(px(5.))
            .text_size(px(look::SMALL))
            .text_color(p.muted);
        let mut visible = false;
        let mut details = crate::composer_view::popup(&p)
            .id("account-quota-details")
            .text_size(px(13.))
            .w(px(460.))
            // The status bar keeps its chips on one line; explanations wrap.
            .whitespace_normal()
            .max_h(px(420.))
            .overflow_y_scroll()
            .when(self.quota_open == Some(QuotaPopup::All), |panel| {
                panel.child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .text_color(p.muted)
                        .child(format!("AI accounts · {}", self.host)),
                )
            });
        for provider in quota_providers() {
            let kind = provider.kind;
            let enabled = self.provider_enabled(kind);
            let unknown_account = Account::default();
            let shown = self.shown_account(kind, cx);
            let selected = managed_accounts.iter().find(|account| {
                account.kind == kind
                    && shown
                        .as_ref()
                        .map_or(account.default, |(id, _)| account.id == *id)
            });
            let managed_snapshot = selected.map(Account::from_agent);
            let account = managed_snapshot.as_ref().unwrap_or(&unknown_account);
            let missing = |label: &str| QuotaWindow {
                label: label.into(),
                ring_label: "?".into(),
                used: None,
                reset_ms: None,
                requires_reset: true,
                observed_ms: 0,
            };
            let unknown = if matches!(kind, "claude" | "codex") {
                let mut windows = [missing("5h"), missing("7d")];
                windows[0].ring_label = "5h".into();
                windows[1].ring_label = "7d".into();
                windows
            } else {
                [
                    missing(if kind == "antigravity" {
                        "Quota"
                    } else {
                        "primary"
                    }),
                    missing("secondary"),
                ]
            };
            let keys = if kind == "claude" {
                ["five_hour", "seven_day"]
            } else {
                ["primary", "secondary"]
            };
            let mut windows = Vec::with_capacity(account.windows.len().max(2));
            if kind == "antigravity" && account.windows.is_empty() {
                windows.push(&unknown[0]);
            } else if kind != "antigravity" {
                for (index, key) in keys.iter().enumerate() {
                    windows.push(account.windows.get(*key).unwrap_or(&unknown[index]));
                }
            }
            windows.extend(
                account
                    .windows
                    .iter()
                    .filter(|(key, _)| !keys.contains(&key.as_str()))
                    .map(|(_, window)| window),
            );
            let provider_popup = QuotaPopup::Provider(kind);
            if enabled {
                visible = true;
                let mut item = h_flex()
                    .id(kind)
                    .flex_none()
                    .h(px(24.))
                    .px(px(6.))
                    .gap(px(5.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(self.quota_open == Some(provider_popup), |d| d.bg(hover))
                    .hover(move |d| d.bg(hover))
                    .on_prepaint(self.popup_triggers.track(kind))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quota_open =
                            (this.quota_open != Some(provider_popup)).then_some(provider_popup);
                        this.reset_confirm = None;
                        this.reset_notices.clear();
                        if this.quota_open.is_some() {
                            this.refresh_accounts(None, cx);
                        }
                        cx.notify();
                    }))
                    .child(crate::agents_view::provider(kind).size(px(14.)));
                for window in windows.iter().take(if kind == "antigravity" {
                    windows.len()
                } else {
                    2
                }) {
                    let value = (selected.is_none_or(|account| {
                        account.method != "api_key" && account.signed_in == Some(true)
                    }) && now >= account.observed_at)
                        .then(|| window.remaining(now))
                        .flatten();
                    item = item.child(crate::meter::labelled_ring(
                        value.map(|left| left / 100.),
                        value.map_or(p.faint, |left| remaining_color(left, &p)),
                        p.faint,
                        window.ring_label.clone(),
                        p.text,
                    ));
                }
                strip = strip.child(item);
            }
            if self.quota_open != Some(QuotaPopup::All) && self.quota_open != Some(provider_popup) {
                continue;
            }
            let refreshing = manager_refreshing;
            let mut section = v_flex()
                .gap(px(10.))
                .px(px(8.))
                .py(px(8.))
                .border_t_1()
                .border_color(p.line)
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .child(crate::agents_view::provider(kind).size(px(18.)))
                                .child(div().font_weight(FontWeight::BOLD).child(provider.short)),
                        )
                        .child(
                            Button::new(SharedString::from(format!("quota-refresh-{kind}")))
                                .small()
                                .icon(
                                    gpui_kit::component::Icon::default()
                                        .path("sandhill/system/refresh.svg"),
                                )
                                .ghost()
                                .loading(refreshing)
                                .disabled(refreshing)
                                .tooltip(if refreshing {
                                    "Refreshing…"
                                } else {
                                    "Refresh quota"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.refresh_accounts(Some(kind), cx)
                                })),
                        ),
                );
            let switchable = shown.as_ref().filter(|(_, can_switch)| *can_switch);
            let default_account = if switchable.is_none() {
                crate::prefs::agent_account(&self.host, kind)
            } else {
                None
            };
            let selection = switchable.map_or(
                (default_account.as_deref().unwrap_or(kind), false),
                |(id, _)| (id.as_str(), true),
            );
            for entry in managed_accounts
                .iter()
                .filter(|account| account.kind == kind)
            {
                section = section.child(self.account_card(entry, kind, selection, now, &p, cx));
            }
            if !managed_accounts.iter().any(|account| account.kind == kind) {
                section = section.child(div().text_color(p.muted).child(if refreshing {
                    "Checking official CLI account data"
                } else {
                    "Account data unavailable from official CLI"
                }));
            }
            if let Some(error) = manager_error {
                section = section.child(div().text_color(p.err).child(error.clone()));
            }
            section = section.child(
                h_flex()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(p.line)
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(
                                div()
                                    .id(SharedString::from(format!("quota-add-{kind}")))
                                    .cursor_pointer()
                                    .text_color(p.accent)
                                    .child("Add Account…")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.dismiss_popup(cx);
                                        this.manager.update(cx, |manager, cx| {
                                            manager.add(kind, window, cx)
                                        });
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("quota-manage-{kind}")))
                                    .cursor_pointer()
                                    .text_color(p.accent)
                                    .child("Manage…")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_popup(cx);
                                        crate::app_window::show_welcome(
                                            Some(crate::start_screen::StartMode::Accounts(
                                                this.host.clone(),
                                            )),
                                            cx,
                                        );
                                    })),
                            ),
                    )
                    .child(
                        h_flex().gap(px(6.)).child("Show in status bar").child(
                            Switch::new(SharedString::from(format!("quota-show-{kind}")))
                                .small()
                                .color(p.accent)
                                .checked(enabled)
                                .accessibility_label(format!(
                                    "Show {} in status bar",
                                    provider.short
                                ))
                                .on_change(cx.listener(move |this, on: &bool, _, cx| {
                                    this.set_provider(kind, *on, cx)
                                })),
                        ),
                    ),
            );
            details = details.child(section);
        }
        if !visible {
            strip = strip.child(
                h_flex()
                    .id("quota-accounts")
                    .h(px(20.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(self.quota_open == Some(QuotaPopup::All), |d| d.bg(hover))
                    .hover(move |d| d.bg(hover))
                    .on_prepaint(self.popup_triggers.track("account-quota"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quota_open =
                            (this.quota_open != Some(QuotaPopup::All)).then_some(QuotaPopup::All);
                        this.reset_confirm = None;
                        this.reset_notices.clear();
                        if this.quota_open.is_some() {
                            this.refresh_accounts(None, cx);
                        }
                        cx.notify();
                    }))
                    .child("AI accounts"),
            );
        }
        if matches!(self.quota_open, Some(QuotaPopup::Provider(_))) {
            details = details.child(
                h_flex()
                    .id("quota-all-accounts")
                    .mx(px(8.))
                    .mt(px(4.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(p.line)
                    .text_color(p.muted)
                    .cursor_pointer()
                    .hover(move |d| d.text_color(p.text))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quota_open = Some(QuotaPopup::All);
                        this.reset_confirm = None;
                        this.refresh_accounts(None, cx);
                        cx.notify();
                    }))
                    .child("All AI accounts…"),
            );
        }
        let popup = details.occlude().on_mouse_down_out(cx.listener(
            |this, event: &MouseDownEvent, _, cx| {
                let trigger = match this.quota_open {
                    Some(QuotaPopup::Provider(kind)) => kind,
                    _ => "account-quota",
                };
                if this.popup_triggers.contains(trigger, event.position) {
                    return;
                }
                this.dismiss_popup(cx);
            },
        ));
        div()
            .relative()
            .child(strip)
            .when(self.quota_open.is_some(), |view| {
                view.child(
                    div().absolute().right_0().bottom(px(26.)).child(deferred(
                        anchored()
                            .anchor(Anchor::BottomRight)
                            .snap_to_window_with_margin(px(8.))
                            .child(popup),
                    )),
                )
            })
            .child(
                self.manager
                    .update(cx, |manager, cx| manager.render_dialogs(cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        QuotaWindow, RemainingLevel, banked_resets, quota_providers, remaining_level, window_label,
    };
    use serde_json::{Value, json};

    #[test]
    fn remaining_percentage_and_thresholds_follow_available_capacity() {
        for (used, remaining, level) in [
            (0., 100., RemainingLevel::Ok),
            (70., 30., RemainingLevel::Ok),
            (70.1, 29.9, RemainingLevel::Warn),
            (90., 10., RemainingLevel::Warn),
            (90.1, 9.9, RemainingLevel::Err),
            (100., 0., RemainingLevel::Err),
        ] {
            let window = QuotaWindow::parse_cli(
                "primary",
                &json!({"used_percentage": used, "window_minutes": 300}),
                1000,
            );
            let actual = window.remaining(1500).unwrap();
            assert!((actual - remaining).abs() < 0.0001);
            assert_eq!(remaining_level(actual), level);
            // The last measurement stays until its window resets.
            assert_eq!(window.remaining(1000 + 30 * 86_400_000), Some(actual));
            assert_eq!(window.remaining(999), None);
        }
    }

    #[test]
    fn banked_resets_say_how_many_and_when_the_first_expires() {
        let now = 1_000_000_000_000;
        let usage = |credits: Value| json!({"reset_credits": credits});
        assert_eq!(
            banked_resets(
                Some(&usage(
                    json!({"available": 2, "expires_at": now / 1000 + 86_400 * 3 + 7_200})
                )),
                now
            )
            .unwrap(),
            "↺ 2 resets banked · next expires in 3d 2h"
        );
        assert_eq!(
            banked_resets(
                Some(&usage(json!({"available": 0, "expires_at": null}))),
                now
            )
            .unwrap(),
            "No banked resets"
        );
        assert_eq!(
            banked_resets(
                Some(&usage(json!({"available": 1, "expires_at": null}))),
                now
            )
            .unwrap(),
            "↺ 1 reset banked · no expiry"
        );
        assert!(banked_resets(Some(&json!({"rate_limits": {}})), now).is_none());
    }

    #[test]
    fn agents_without_a_subscription_quota_have_no_quota_chip() {
        assert!(quota_providers().any(|provider| provider.kind == "codex"));
        for kind in ["openscience", "omp", "pi", "posit"] {
            assert!(!quota_providers().any(|provider| provider.kind == kind));
        }
    }

    #[test]
    fn actual_duration_reset_and_invalid_values_are_preserved() {
        let window = QuotaWindow::parse(
            "primary",
            &json!({"used_percentage": 42.5,
            "window_minutes": 60, "resets_at": "2026-10-12T05:00:00Z", "observed_at_ms": 1000}),
            0,
        );
        assert_eq!(window.label, "1h");
        assert_eq!(window.value(2000), Some(42.5));
        assert_eq!(
            QuotaWindow::parse("secondary", &json!({"used_percentage": -1}), 1000).used,
            None
        );
        assert_eq!(window_label(None, "primary"), "primary");
    }

    #[test]
    fn quota_ring_labels_follow_reported_windows_not_provider_slot_names() {
        for (minutes, label) in [(300, "5h"), (10080, "7d"), (1440, "1d"), (90, "90m")] {
            for key in ["primary", "secondary"] {
                let window = QuotaWindow::parse(key, &json!({"window_minutes": minutes}), 1000);
                assert_eq!(window.ring_label.as_ref(), label);
            }
        }
        for (duration, label) in [
            ("weekly", "7d"),
            ("5h", "5h"),
            ("24 hours", "1d"),
            ("daily", "1d"),
            ("monthly", "1mo"),
            ("unreported", "?"),
        ] {
            let display = format!("Gemini Models · {duration}");
            let window = QuotaWindow::parse_cli("bucket", &json!({"label": display}), 1000);
            assert_eq!(window.ring_label.as_ref(), label);
            assert_eq!(window.label, display);
        }
        let explicit = QuotaWindow::parse_cli(
            "bucket",
            &json!({"label": "Claude and GPT models", "window": "weekly"}),
            1000,
        );
        assert_eq!(explicit.ring_label.as_ref(), "7d");
    }

    #[test]
    fn expired_missing_and_future_measurements_never_become_zero() {
        let make = |reset| {
            QuotaWindow::parse(
                "five_hour",
                &json!({"used_percentage": 0,
            "resets_at": reset, "observed_at_ms": 1000}),
                0,
            )
        };
        assert_eq!(make(json!(2)).value(2000), None);
        assert_eq!(make(Value::Null).value(1500), None);
        assert_eq!(make(json!(10)).value(999), None);
        assert_eq!(make(json!(10)).value(1500), Some(0.));
    }

    #[test]
    fn antigravity_invalid_and_expired_measurements_stay_unknown() {
        for value in [
            json!({"used_percentage": null}),
            json!({"used_percentage": -1}),
            json!({"used_percentage": 101}),
            json!({"used_percentage": 0, "resets_at": "not a time"}),
            json!({"used_percentage": 0, "resets_at": 1}),
        ] {
            let window = QuotaWindow::parse_cli("Gemini Models · 5h", &value, 1000);
            assert_eq!(window.value(1500), None, "{value}");
        }
        assert_eq!(
            QuotaWindow::parse_cli("5h", &json!({"used_percentage": 0}), 1000).value(1500),
            Some(0.)
        );
        // Codex and Claude retain their stricter requirement for a reset time.
        assert_eq!(
            QuotaWindow::parse("five_hour", &json!({"used_percentage": 0}), 1000).value(1500),
            None
        );
    }
}
