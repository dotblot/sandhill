//! Host-local agent accounts and quotas. Sign-ins stay in the agent's own CLI.
//! Each provider has its own status chip and account popup; hidden providers
//! remain reachable through the full account list in the same popup frame.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{AgentAccount, Call, TerminalEvent};
use serde::Deserialize;
use serde_json::Value;

use crate::agent_accounts::AccountManager;
use crate::agents::{Agents, Show};
use crate::daemon::Daemon;
use crate::look::{self, Palette};

const FRESH_MS: i64 = 5 * 60 * 1000;

/// OpenScience, Oh My Pi and pi use the user's own model accounts and keys,
/// with no subscription account query.
fn quota_providers() -> impl Iterator<Item = &'static crate::agents::Kind> {
    crate::agents::KINDS
        .iter()
        .filter(|provider| !matches!(provider.kind, "openscience" | "omp" | "pi"))
}

#[derive(Default)]
struct Quotas {
    accounts: BTreeMap<String, Account>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuotaPopup {
    All,
    Provider(&'static str),
}

pub(crate) struct QuotaMonitor {
    daemon: Rc<Daemon>,
    host: String,
    quotas: Quotas,
    manager: Entity<AccountManager>,
    agents: Option<Entity<Agents>>,
    agents_observation: Option<Subscription>,
    popup_triggers: crate::popup::PopupTriggers,
    quota_open: Option<QuotaPopup>,
    enabled: BTreeMap<String, bool>,
    refreshing: BTreeSet<String>,
    refresh_errors: BTreeMap<String, String>,
    reset_confirm: Option<ResetConfirmation>,
    reset_pending: Option<String>,
    reset_notices: BTreeMap<String, (String, bool)>,
    polling: bool,
}

#[derive(Default)]
struct Account {
    identity: Option<Value>,
    identity_at: i64,
    /// Anonymous official CLI snapshots are never joined with account feeds.
    cli_snapshot_at: i64,
    quota_error: Option<String>,
    changed_at: i64,
    /// Full authoritative snapshots also tombstone windows absent from them.
    windows_at: i64,
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
            && now - self.observed_ms <= FRESH_MS
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

impl Quotas {
    /// Account timestamps win over arrival order, including replayed terminal
    /// reports. Null is a sign-out tombstone. Quotas never cross identities.
    pub(crate) fn ingest(&mut self, kind: &str, report: &Value) {
        if !matches!(kind, "codex" | "claude" | "antigravity") {
            return;
        }
        let account = self.accounts.entry(kind.into()).or_default();
        let at = report["observed_at_ms"].as_i64().unwrap_or(0);
        if kind == "antigravity" {
            if report["quota_source"] != "cli_snapshot" || at <= 0 || at < account.cli_snapshot_at {
                return;
            }
            let Some(windows) = report["status"]["rate_limits"].as_object() else {
                return;
            };
            account.identity = report
                .get("account")
                .filter(|identity| !identity.is_null())
                .cloned();
            account.identity_at = 0;
            account.cli_snapshot_at = at;
            account.quota_error = report["quota_error"].as_str().map(str::to_owned);
            account.windows_at = at;
            // Every anonymous result replaces the entire provider snapshot:
            // neither removed groups nor a previous account's windows can leak.
            account.windows = windows
                .iter()
                .map(|(key, value)| (key.clone(), QuotaWindow::parse_cli(key, value, at)))
                .collect();
            return;
        }
        if let Some(identity) = report.get("account")
            && at > 0
            && at >= account.identity_at
        {
            let identity = (!identity.is_null()).then(|| identity.clone());
            if account.identity != identity {
                if account.identity_at != 0 {
                    account.changed_at = at;
                }
                account.windows_at = 0;
                account.windows.clear();
                account.identity = identity;
            }
            account.identity_at = at;
        }
        if account.identity.is_none() {
            return;
        }
        // Explicitly attributed snapshots for a different account cannot update
        // the current account, even if their quota notification arrived later.
        if let Some(identity) = report.get("account")
            && account.identity.as_ref() != Some(identity)
        {
            return;
        }
        let status = &report["status"];
        let observed = status["quota_observed_at_ms"].as_i64().unwrap_or(at);
        let Some(windows) = status["rate_limits"].as_object() else {
            return;
        };
        if kind == "claude"
            && (status.get("quota_account") != account.identity.as_ref()
                || observed < account.identity_at
                || observed - account.identity_at > FRESH_MS)
        {
            return;
        }
        let full = kind == "codex"
            && report.get("account").is_some()
            && report["feed"] != true
            && at >= account.windows_at;
        if full {
            account.windows_at = at;
        }
        for (key, value) in windows {
            let window = QuotaWindow::parse(key, value, observed);
            if window.observed_ms < account.changed_at
                || window.observed_ms < account.windows_at
                || window.observed_ms <= 0
            {
                continue;
            }
            if account
                .windows
                .get(key)
                .is_none_or(|old| old.observed_ms <= window.observed_ms)
            {
                account.windows.insert(key.clone(), window);
            }
        }
        // A full, fresh Codex snapshot can explicitly remove a window.
        if full {
            account
                .windows
                .retain(|key, old| windows.contains_key(key) || old.observed_ms > at);
        }
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
            identity_at: observed,
            cli_snapshot_at: observed,
            windows,
            ..Self::default()
        }
    }

    fn available(&self) -> bool {
        self.identity.is_some() || self.cli_snapshot_at > 0
    }

    fn observed_at(&self) -> i64 {
        self.cli_snapshot_at.max(self.identity_at)
    }
}

fn account_name(account: &Account) -> String {
    let Some(identity) = &account.identity else {
        return if account.cli_snapshot_at > 0 {
            "Official CLI usage · account identity not reported".into()
        } else {
            String::new()
        };
    };
    let email = identity["email"].as_str().filter(|s| !s.is_empty());
    let plan = identity["plan"].as_str().filter(|s| !s.is_empty());
    match (email, plan) {
        (Some(email), Some(plan)) => format!("{email} · {plan}"),
        (Some(email), None) => email.into(),
        (None, Some(plan)) => plan.into(),
        _ => "Signed in through CLI".into(),
    }
}

fn unavailable_reason(_kind: &str, account: &Account) -> &'static str {
    let mode = account
        .identity
        .as_ref()
        .and_then(|identity| identity["type"].as_str())
        .unwrap_or("");
    if matches!(
        mode,
        "apiKey" | "api_key" | "anthropic_api_key" | "bedrock" | "vertex"
    ) {
        "API key · no plan limits"
    } else {
        "No usage reported yet"
    }
}

impl QuotaMonitor {
    pub(crate) fn new(daemon: Rc<Daemon>, host: String, cx: &mut Context<Self>) -> Self {
        Self::start_quota_poll(cx);
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
            quotas: Quotas::default(),
            manager,
            agents: None,
            agents_observation: None,
            popup_triggers: crate::popup::PopupTriggers::default(),
            quota_open: None,
            enabled,
            refreshing: BTreeSet::new(),
            refresh_errors: BTreeMap::new(),
            reset_confirm: None,
            reset_pending: None,
            reset_notices: BTreeMap::new(),
            polling: true,
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

    fn refresh_accounts(&self, refresh: bool, cx: &mut Context<Self>) {
        self.manager
            .update(cx, |manager, cx| manager.refresh(refresh, cx));
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
            self.refresh_provider(kind, false, cx);
        }
        cx.notify();
    }

    fn refresh_provider(&mut self, kind: &'static str, force: bool, cx: &mut Context<Self>) {
        if matches!(kind, "claude" | "codex" | "antigravity") {
            self.refresh_accounts(force, cx);
            return;
        }
        if !self.refreshing.insert(kind.into()) {
            return;
        }
        self.refresh_errors.remove(kind);
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            let result = daemon
                .call_within::<Vec<Value>>(
                    Call::AgentAccountsQuery {
                        kinds: vec![kind.into()],
                        force,
                    },
                    Duration::from_secs(50),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing.remove(kind);
                match result {
                    Ok(reports) => {
                        for report in reports {
                            if report["kind"] == kind {
                                this.quotas.ingest(kind, &report);
                            }
                        }
                    }
                    Err(err) => {
                        this.refresh_errors
                            .insert(kind.into(), format!("Refresh failed: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn start_quota_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok((daemon, kinds)) = this.update(cx, |this, cx| {
                    // Reload device settings before every scheduled request:
                    // another workspace may have disabled a provider.
                    this.enabled = crate::prefs::quota_providers(&this.host);
                    this.polling = true;
                    this.refresh_accounts(true, cx);
                    cx.notify();
                    let kinds = quota_providers()
                        .filter(|provider| this.provider_enabled(provider.kind))
                        .filter(|provider| provider.kind == "posit")
                        .map(|provider| provider.kind.to_string())
                        .collect::<Vec<_>>();
                    (this.daemon.clone(), kinds)
                }) else {
                    break;
                };
                let reports = if kinds.is_empty() {
                    Vec::new()
                } else {
                    daemon
                        .call_within::<Vec<Value>>(
                            Call::AgentAccountsQuery {
                                kinds,
                                force: false,
                            },
                            Duration::from_secs(50),
                        )
                        .await
                        .unwrap_or_default()
                };
                if this
                    .update(cx, |this, cx| {
                        this.polling = false;
                        for report in reports {
                            if let Some(kind) = report["kind"].as_str() {
                                this.quotas.ingest(kind, &report);
                            }
                        }
                        // Even a failed poll must redraw stale measurements unknown.
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
            }
        })
        .detach();
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
            let account = managed_snapshot.as_ref().unwrap_or_else(|| {
                if kind == "posit" {
                    self.quotas.accounts.get(kind).unwrap_or(&unknown_account)
                } else {
                    &unknown_account
                }
            });
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
                    missing(if matches!(kind, "antigravity" | "posit") {
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
            if kind == "posit" || (kind == "antigravity" && account.windows.is_empty()) {
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
                            this.refresh_accounts(true, cx);
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
                    }) && now >= account.observed_at()
                        && now - account.observed_at() <= FRESH_MS)
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
            let managed = matches!(kind, "claude" | "codex" | "antigravity");
            let refreshing = if managed {
                manager_refreshing
            } else {
                self.refreshing.contains(kind) || (self.polling && enabled)
            };
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
                                    this.refresh_provider(kind, true, cx)
                                })),
                        ),
                );
            if managed {
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
            } else if account.available() {
                section = section.child(div().text_color(p.muted).child(account_name(account)));
                if account.windows.is_empty() {
                    section = section.child(
                        div().text_color(p.muted).child(
                            account
                                .quota_error
                                .clone()
                                .unwrap_or_else(|| unavailable_reason(kind, account).into()),
                        ),
                    );
                } else {
                    for window in &windows {
                        section = section.child(window.row(now, &p));
                    }
                }
            } else {
                section = section.child(div().text_color(p.muted).child(if refreshing {
                    "Checking official CLI account data"
                } else {
                    "No supported subscription quota query"
                }));
            }
            if let Some(error) = self.refresh_errors.get(kind) {
                section = section.child(div().text_color(p.err).child(error.clone()));
            }
            section = section.child(
                h_flex()
                    .pt(px(8.))
                    .border_t_1()
                    .border_color(p.line)
                    .justify_between()
                    .gap(px(8.))
                    .child(h_flex().gap(px(10.)).when(managed, |row| {
                        row.child(
                            div()
                                .id(SharedString::from(format!("quota-add-{kind}")))
                                .cursor_pointer()
                                .text_color(p.accent)
                                .child("Add Account…")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.dismiss_popup(cx);
                                    this.manager
                                        .update(cx, |manager, cx| manager.add(kind, window, cx));
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
                        )
                    }))
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
                            this.refresh_accounts(true, cx);
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
                        this.refresh_accounts(true, cx);
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
        FRESH_MS, QuotaWindow, Quotas, RemainingLevel, banked_resets, quota_providers,
        remaining_level, window_label,
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
            assert_eq!(window.remaining(1000 + FRESH_MS), Some(actual));
            assert_eq!(window.remaining(1000 + FRESH_MS + 1), None);
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
    fn bring_your_own_key_agents_have_no_quota_chip_or_account_polling() {
        assert!(quota_providers().any(|provider| provider.kind == "codex"));
        for kind in ["openscience", "omp", "pi"] {
            assert!(!quota_providers().any(|provider| provider.kind == kind));
            let mut quotas = Quotas::default();
            quotas.ingest(
                kind,
                &json!({"observed_at_ms": 1000, "status": {"rate_limits": {}}}),
            );
            assert!(!quotas.accounts.contains_key(kind));
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
        assert_eq!(window.value(1000 + FRESH_MS + 1), None);
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
    fn account_changes_and_signout_defeat_replayed_old_reports() {
        let mut quotas = Quotas::default();
        let report = |email, at, used| {
            json!({"account": {"type": "chatgpt", "email": email},
            "observed_at_ms": at, "status": {"rate_limits": {"primary": {
                "window_minutes": 300, "used_percentage": used, "observed_at_ms": at, "resets_at": 1000}}}})
        };
        quotas.ingest("codex", &report("a@example.test", 1000, 30));
        assert_eq!(quotas.accounts["codex"].windows["primary"].used, Some(30.));
        quotas.ingest("codex", &json!({"account": {"type": "chatgpt", "email": "b@example.test"}, "observed_at_ms": 2000}));
        quotas.ingest("codex", &report("a@example.test", 1500, 99));
        assert!(quotas.accounts["codex"].windows.is_empty());
        quotas.ingest("codex", &report("b@example.test", 3000, 12));
        assert_eq!(quotas.accounts["codex"].windows["primary"].used, Some(12.));
        quotas.ingest("codex", &json!({"account": null, "observed_at_ms": 4000}));
        quotas.ingest("codex", &report("b@example.test", 3000, 12));
        assert!(quotas.accounts["codex"].identity.is_none());
        assert!(quotas.accounts["codex"].windows.is_empty());
    }

    #[test]
    fn claude_feed_is_fresh_only_for_the_current_cli_identity() {
        let mut quotas = Quotas::default();
        let identity = |email, at| {
            json!({"account": {"type": "claude.ai", "email": email},
            "observed_at_ms": at})
        };
        let feed = |email, at| {
            json!({"status": {"quota_observed_at_ms": at,
            "quota_account": {"type": "claude.ai", "email": email},
            "rate_limits": {"five_hour": {"used_percentage": 42, "resets_at": 1000}}}})
        };
        quotas.ingest("claude", &identity("a@example.test", 1000));
        // Discovery can still cache A after an in-session login to B. A newer
        // measurement bound to B must not populate A's account.
        quotas.ingest("claude", &feed("b@example.test", 1400));
        assert!(quotas.accounts["claude"].windows.is_empty());
        quotas.ingest("claude", &feed("a@example.test", 1500));
        assert_eq!(
            quotas.accounts["claude"].windows["five_hour"].used,
            Some(42.)
        );
        quotas.ingest("claude", &identity("b@example.test", 2000));
        quotas.ingest("claude", &feed("a@example.test", 2500));
        assert!(quotas.accounts["claude"].windows.is_empty());
        quotas.ingest("claude", &feed("b@example.test", 2500));
        assert_eq!(
            quotas.accounts["claude"].windows["five_hour"].used,
            Some(42.)
        );
        quotas.ingest("claude", &identity("c@example.test", 3000));
        let mut ambiguous = feed("c@example.test", 3500);
        ambiguous["status"]
            .as_object_mut()
            .unwrap()
            .remove("quota_account");
        quotas.ingest("claude", &ambiguous);
        assert!(quotas.accounts["claude"].windows.is_empty());
        quotas.ingest("claude", &feed("c@example.test", 3000 + FRESH_MS + 1));
        assert!(
            quotas.accounts["claude"].windows.is_empty(),
            "fresh feed cannot join an expired account identity"
        );
    }

    #[test]
    fn api_accounts_and_unqueried_antigravity_do_not_invent_windows() {
        let mut quotas = Quotas::default();
        quotas.ingest(
            "codex",
            &json!({"account": {"type": "apiKey"}, "observed_at_ms": 1000}),
        );
        assert!(quotas.accounts["codex"].windows.is_empty());
        quotas.ingest(
            "antigravity",
            &json!({"account": {"type": "google"}, "observed_at_ms": 1000}),
        );
        assert!(quotas.accounts["antigravity"].windows.is_empty());
    }

    #[test]
    fn authoritative_codex_removal_survives_older_same_account_replay() {
        let mut quotas = Quotas::default();
        let identity = json!({"type": "chatgpt", "email": "a@example.test"});
        let old = json!({"account": identity, "feed": true, "observed_at_ms": 1000,
            "status": {"rate_limits": {"primary": {"window_minutes": 300,
            "used_percentage": 42, "resets_at": 1000, "observed_at_ms": 1500}}}});
        quotas.ingest("codex", &old);
        assert!(quotas.accounts["codex"].windows.contains_key("primary"));
        quotas.ingest(
            "codex",
            &json!({"account": identity, "observed_at_ms": 2000,
            "status": {"rate_limits": {}}}),
        );
        quotas.ingest("codex", &old);
        assert!(quotas.accounts["codex"].windows.is_empty());
        let mut newer = old;
        newer["status"]["rate_limits"]["primary"]["observed_at_ms"] = json!(2500);
        quotas.ingest("codex", &newer);
        assert!(quotas.accounts["codex"].windows.contains_key("primary"));
    }

    #[test]
    fn installation_or_unattributed_feed_does_not_establish_login() {
        let mut quotas = Quotas::default();
        quotas.ingest(
            "claude",
            &json!({"status": {"rate_limits": {"five_hour": {
            "used_percentage": 42, "resets_at": 1000}}, "quota_observed_at_ms": 1000}}),
        );
        assert!(quotas.accounts["claude"].identity.is_none());
        quotas.ingest("antigravity", &json!({"path": "/usr/bin/agy"}));
        assert!(quotas.accounts["antigravity"].identity.is_none());
    }

    fn agy_snapshot(at: i64, windows: Value) -> Value {
        json!({"kind": "antigravity", "quota_source": "cli_snapshot", "observed_at_ms": at,
            "status": {"quota_observed_at_ms": at, "rate_limits": windows}})
    }

    #[test]
    fn antigravity_official_groups_need_no_fabricated_identity_or_reset() {
        let mut quotas = Quotas::default();
        quotas.ingest("antigravity", &agy_snapshot(1000, json!({
            "gemini-weekly": {"label": "Gemini Models · weekly", "used_percentage": 0.6253361701965332,
                "resets_at": "2026-11-17T18:03:02Z"},
            "3p-5h": {"label": "Claude and GPT models · 5h", "used_percentage": 0}
        })));
        let account = &quotas.accounts["antigravity"];
        assert!(account.available());
        assert!(account.identity.is_none());
        assert_eq!(
            account.windows["gemini-weekly"].label,
            "Gemini Models · weekly"
        );
        assert_eq!(account.windows["3p-5h"].value(1500), Some(0.));
        assert_eq!(account.windows["3p-5h"].value(1000 + FRESH_MS + 1), None);
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

    #[test]
    fn antigravity_full_snapshots_remove_stale_groups_and_never_join_accounts() {
        let mut quotas = Quotas::default();
        let old = agy_snapshot(
            1000,
            json!({
                "gemini-weekly": {"label": "Gemini Models · weekly", "used_percentage": 42},
                "3p-5h": {"label": "Claude and GPT models · 5h", "used_percentage": 0}
            }),
        );
        quotas.ingest("antigravity", &old);
        let mut new_account = agy_snapshot(
            2000,
            json!({
                "3p-5h": {"label": "Claude and GPT models · 5h", "used_percentage": 12}
            }),
        );
        new_account["account"] = json!({"email": "explicit@example.test"});
        quotas.ingest("antigravity", &new_account);
        assert_eq!(quotas.accounts["antigravity"].windows.len(), 1);
        quotas.ingest("antigravity", &old);
        assert_eq!(quotas.accounts["antigravity"].windows.len(), 1);
        assert_eq!(
            quotas.accounts["antigravity"].windows["3p-5h"].used,
            Some(12.)
        );
        quotas.ingest(
            "antigravity",
            &agy_snapshot(
                3000,
                json!({
                    "gemini-weekly": {"label": "Gemini Models · weekly", "used_percentage": 5}
                }),
            ),
        );
        let account = &quotas.accounts["antigravity"];
        assert!(account.identity.is_none());
        assert_eq!(account.windows.len(), 1);
        assert!(!account.windows.contains_key("3p-5h"));
        // Unattributed incremental data cannot resurrect another account's window.
        quotas.ingest(
            "antigravity",
            &json!({"observed_at_ms": 4000, "status": {
            "rate_limits": {"3p-5h": {"used_percentage": 99}}}}),
        );
        assert!(!quotas.accounts["antigravity"].windows.contains_key("3p-5h"));
        let mut failed = agy_snapshot(5000, json!({}));
        failed["quota_error"] = json!("Antigravity /usage exited with status 7");
        quotas.ingest("antigravity", &failed);
        assert!(quotas.accounts["antigravity"].windows.is_empty());
        assert_eq!(
            quotas.accounts["antigravity"].quota_error.as_deref(),
            Some("Antigravity /usage exited with status 7")
        );
        quotas.ingest("antigravity", &old);
        assert!(quotas.accounts["antigravity"].windows.is_empty());
    }
}
