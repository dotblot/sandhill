//! Welcome → Usage (Huan, 2026-11-16; `docs/design/done/usage-stats-plan.md`):
//! what the agents in Sandhill's conversations used on one device, like
//! `omp stats`: requests, tokens, cache rate and the agents' own cost
//! estimates for a period, each day's tokens by agent, and tables by agent
//! and model and by project. Read when shown and on Refresh, never on a
//! timer.

use super::*;
use sandhill_proto::{UsageRow, UsageStats, UsageTotals};

/// The periods offered, in days.
const PERIODS: [(u32, &str); 4] = [
    (7, "7 days"),
    (30, "30 days"),
    (90, "90 days"),
    (365, "Year"),
];
impl AppWindow {
    /// Welcome → Usage: one device at a time, as Agents is.
    pub(super) fn usage_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        let host = self.start.selected_host.clone();
        let ready = self
            .start
            .machines
            .get(&host)
            .and_then(|machine| match &machine.link {
                MachineLink::Ready(daemon) => Some((machine.attempt, daemon.clone())),
                _ => None,
            });
        match ready {
            Some((attempt, daemon)) => {
                if !self
                    .start
                    .usage
                    .as_ref()
                    .is_some_and(|(device, attached, _)| device == &host && *attached == attempt)
                {
                    self.start.usage = Some((
                        host.clone(),
                        attempt,
                        cx.new(|cx| UsageView::new(daemon, cx)),
                    ));
                }
            }
            None => self.start.usage = None,
        }
        let mut devices = look::tab_capsule(&p).gap(px(1.));
        for (ix, device) in self.start.hosts().into_iter().enumerate() {
            let selected = device == host;
            devices = devices.child(
                crate::workbench::Workbench::tab(
                    ("usage-device", ix),
                    None,
                    host_label(&device),
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if selected {
                        return;
                    }
                    this.start.selected_host = device.clone();
                    this.start.usage = None;
                    this.ensure_machine(&device, cx);
                    cx.notify();
                })),
            );
        }
        let refresh = self.start.usage.as_ref().map(|(_, _, view)| {
            let view = view.clone();
            let loading = view.read(cx).loading();
            Button::new("usage-refresh")
                .small()
                .label(if loading { "Reading…" } else { "Refresh" })
                .disabled(loading)
                .on_click(cx.listener(move |_, _, _, cx| view.update(cx, |view, cx| view.load(cx))))
        });
        let mut page = v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(
                h_flex()
                    .w_full()
                    .gap(px(look::GAP_8))
                    .child(div().flex_1().text_size(px(look::welcome_title())).font_weight(FontWeight::BOLD).child("Usage"))
                    .children(refresh),
            )
            .child(div().text_size(px(look::tab_text())).text_color(p.muted).child(
                "What the agents in Sandhill's conversations used on this device: requests, tokens and their cost estimates.",
            ))
            .child(h_flex().child(devices));
        if let Some((_, _, view)) = &self.start.usage {
            page = page.child(view.clone());
        } else {
            let message = match self.start.machines.get(&host).map(|machine| &machine.link) {
                Some(MachineLink::Failed(error)) => error.clone(),
                Some(MachineLink::Connecting) => "Connecting…".into(),
                _ => "Connect to this device to see its usage.".into(),
            };
            page = page
                .child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.muted)
                        .child(message),
                )
                .child(self.welcome_connection_button(host, "usage-connect", "Connect", cx));
        }
        page.into_any_element()
    }
}

/// The tallest day bar.
const BAR_HEIGHT: f32 = 110.;

pub(crate) struct UsageView {
    daemon: Rc<Daemon>,
    days: u32,
    stats: Option<Result<UsageStats, String>>,
    loading: bool,
    generation: u64,
}

impl UsageView {
    pub(crate) fn new(daemon: Rc<Daemon>, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            daemon,
            days: 30,
            stats: None,
            loading: false,
            generation: 0,
        };
        view.load(cx);
        view
    }

    pub(crate) fn loading(&self) -> bool {
        self.loading
    }

    /// Asks the device for the period's figures, days counted in this
    /// Mac's time zone.
    pub(crate) fn load(&mut self, cx: &mut Context<Self>) {
        let offset = chrono::Local::now().offset().local_minus_utc() / 60;
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let asked = self.daemon.call::<UsageStats>(Call::UsageStats {
            days: self.days,
            tz_offset_minutes: offset,
        });
        cx.spawn(async move |this, cx| {
            let stats = asked.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.stats = Some(stats);
                    this.loading = false;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// `1,234`, `12.3K`, `4.5M`, `1.2B`.
fn count(n: u64) -> String {
    match n {
        0..=9_999 => {
            let s = n.to_string();
            if s.len() > 3 {
                format!("{},{}", &s[..s.len() - 3], &s[s.len() - 3..])
            } else {
                s
            }
        }
        10_000..=999_999 => format!("{:.0}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

/// `$1,240`, `$12.34`, `<$0.01`, `–` with nothing priced.
fn dollars(totals: &UsageTotals) -> String {
    if totals.requests > 0 && totals.unpriced == totals.requests {
        return "–".into();
    }
    match totals.cost {
        c if c <= 0.0 => "$0".into(),
        c if c < 0.01 => "<$0.01".into(),
        c if c < 100.0 => format!("${c:.2}"),
        c => format!("${}", count(c.round() as u64)),
    }
}

fn percent(rate: f64) -> String {
    format!("{:.0}%", rate * 100.)
}

/// An agent's colour in the day bars.
fn agent_colour(agent: &str, p: &Palette) -> Hsla {
    match agent {
        "claude" => rgb(0xd97757).into(),
        "codex" => rgb(0x10a37f).into(),
        "omp" => rgb(0x8b6cd8).into(),
        "pi" => rgb(0x3b82f6).into(),
        "openscience" => rgb(0xe0a526).into(),
        "antigravity" => rgb(0x4285f4).into(),
        _ => p.muted,
    }
}

fn agent_name(agent: &str) -> String {
    crate::agents::kind_of(agent).map_or_else(|| agent.to_string(), |k| k.name.to_string())
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = welcome_palette(cx);
        let mut periods = look::tab_capsule(&p).gap(px(1.));
        for (days, label) in PERIODS {
            let selected = days == self.days;
            periods = periods.child(
                crate::workbench::Workbench::tab(
                    ("usage-period", days as usize),
                    None,
                    label,
                    selected,
                    &p,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.days != days {
                        this.days = days;
                        this.load(cx);
                    }
                })),
            );
        }
        let body = match &self.stats {
            None => div()
                .text_color(p.muted)
                .child("Reading…")
                .into_any_element(),
            Some(Err(err)) => div()
                .text_color(p.err)
                .child(err.clone())
                .into_any_element(),
            Some(Ok(stats)) if stats.first_ms.is_none() => div()
                .text_color(p.muted)
                .child(
                    "Nothing recorded yet. From now on Sandhill counts what its agents use in its \
                     conversations on this device: each request's tokens and the agent's own cost \
                     estimate.",
                )
                .into_any_element(),
            Some(Ok(stats)) => self.figures(stats, &p).into_any_element(),
        };
        v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(h_flex().child(periods))
            .child(body)
    }
}

impl UsageView {
    fn figures(&self, stats: &UsageStats, p: &Palette) -> Div {
        let t = &stats.totals;
        let active = stats.days.iter().filter(|d| d.totals.requests > 0).count();
        let card = |label: &'static str, value: String, note: Option<String>| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(look::GAP_2))
                .p(px(look::GAP_12))
                .rounded(px(look::RADIUS))
                .border_1()
                .border_color(p.line)
                .bg(p.bg.opacity(0.5))
                .child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.muted)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(look::title()))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value),
                )
                .children(note.map(|note| {
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child(note)
                }))
        };
        let tokens_note = format!(
            "{} in · {} out",
            count(t.input + t.cache_write + t.cache_read),
            count(t.output + t.reasoning)
        );
        let cost_note = (t.unpriced > 0).then(|| format!("{} unpriced", count(t.unpriced)));
        let cards = h_flex()
            .w_full()
            .items_stretch()
            .gap(px(look::GAP_8))
            .child(card("Requests", count(t.requests), None))
            .child(card("Tokens", count(t.tokens()), Some(tokens_note)))
            .child(card("Cache rate", percent(t.cache_rate()), None))
            .child(card("Cost (est.)", dollars(t), cost_note))
            .child(card(
                "Active days",
                format!("{active} of {}", stats.days.len()),
                None,
            ));

        // Each day's tokens, stacked by agent.
        let most = stats
            .days
            .iter()
            .map(|d| d.totals.tokens())
            .max()
            .unwrap_or(0)
            .max(1);
        let mut agents: Vec<String> = Vec::new();
        for day in &stats.days {
            for (agent, _) in &day.agents {
                if !agents.contains(agent) {
                    agents.push(agent.clone());
                }
            }
        }
        let bars = stats.days.iter().enumerate().map(|(ix, day)| {
            let tip = format!(
                "{} · {} tokens · {} requests · {}",
                day.day,
                count(day.totals.tokens()),
                count(day.totals.requests),
                dollars(&day.totals)
            );
            let mut bar = v_flex()
                .flex_1()
                .min_w(px(1.))
                .h(px(BAR_HEIGHT))
                .justify_end();
            for (agent, tokens) in day.agents.iter().rev() {
                let height = (*tokens as f32 / most as f32 * BAR_HEIGHT).max(1.);
                bar = bar.child(div().w_full().h(px(height)).bg(agent_colour(agent, p)));
            }
            div()
                .id(("usage-day", ix))
                .flex_1()
                .min_w(px(1.))
                .h(px(BAR_HEIGHT))
                .flex()
                .items_end()
                .rounded_t(px(2.))
                .hover(|d| d.bg(p.ink(look::ROW_HOVER)))
                .child(bar)
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                })
        });
        let first = stats
            .days
            .first()
            .map(|d| d.day.clone())
            .unwrap_or_default();
        let last = stats.days.last().map(|d| d.day.clone()).unwrap_or_default();
        let chart = v_flex()
            .w_full()
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .w_full()
                    .items_end()
                    .gap(px(look::GAP_2))
                    .children(bars),
            )
            .child(
                h_flex()
                    .w_full()
                    .text_size(px(look::small()))
                    .text_color(p.faint)
                    .child(div().flex_1().child(first))
                    .child(last),
            )
            .child(
                h_flex()
                    .gap(px(look::GAP_12))
                    .children(agents.iter().map(|agent| {
                        h_flex()
                            .gap(px(look::GAP_4))
                            .text_size(px(look::small()))
                            .text_color(p.muted)
                            .child(div().size(px(8.)).rounded_full().bg(agent_colour(agent, p)))
                            .child(agent_name(agent))
                    })),
            );

        let table = |title: &'static str, rows: &[UsageRow], projects: bool| {
            let header = h_flex()
                .gap(px(look::GAP_8))
                .pb(px(look::GAP_4))
                .border_b_1()
                .border_color(p.line)
                .text_size(px(look::small()))
                .text_color(p.faint)
                .child(div().flex_1().child(title))
                .child(div().w(px(70.)).text_right().child("Requests"))
                .child(div().w(px(70.)).text_right().child("Tokens"))
                .child(div().w(px(56.)).text_right().child("Cache"))
                .child(div().w(px(70.)).text_right().child("Cost"));
            let lines = rows.iter().map(|row| {
                let name = if projects {
                    if row.label.is_empty() {
                        "No project".to_string()
                    } else {
                        crate::panes::files::base_name(&row.label).to_string()
                    }
                } else {
                    let model = if row.label.is_empty() {
                        "–"
                    } else {
                        row.label.as_str()
                    };
                    format!("{} · {model}", agent_name(&row.agent))
                };
                h_flex()
                    .min_h(px(look::dense_row()))
                    .gap(px(look::GAP_8))
                    .when(!projects, |d| {
                        d.child(
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(agent_colour(&row.agent, p)),
                        )
                    })
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .w(px(70.))
                            .text_right()
                            .child(count(row.totals.requests)),
                    )
                    .child(
                        div()
                            .w(px(70.))
                            .text_right()
                            .child(count(row.totals.tokens())),
                    )
                    .child(
                        div()
                            .w(px(56.))
                            .text_right()
                            .child(percent(row.totals.cache_rate())),
                    )
                    .child(div().w(px(70.)).text_right().child(dollars(&row.totals)))
            });
            v_flex().w_full().child(header).children(lines)
        };
        let since = stats
            .first_ms
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|at| {
                at.with_timezone(&chrono::Local)
                    .format("%b %-d, %Y")
                    .to_string()
            })
            .unwrap_or_default();
        v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(cards)
            .child(chart)
            .child(table("By agent and model", &stats.models, false))
            .child(table("By project", &stats.projects, true))
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.faint)
                    .child(format!(
                        "Counted by Sandhill since {since}, in its own conversations only. Costs are \
                         the agents' own estimates at list prices; Codex gives none."
                    )),
            )
    }
}
