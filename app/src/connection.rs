//! A workbench's connection: the status bar's device item opens a pop-up
//! with what the connection is and what can be done to it (Reconnect,
//! Disconnect, Connect, or on this machine Restart sandhilld), and the
//! daemon's log.

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{Call, LogTail};

use crate::look::{self, Palette, icon};
use crate::workbench::{Link, Workbench};

/// The log dialog: the end of the daemon's log, once read.
pub(crate) struct LogView {
    pub tail: Option<Result<LogTail, String>>,
    pub scroll: ScrollHandle,
}

/// "2 h 5 min", "3 min", "12 s".
fn duration(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else if s < 86_400 {
        format!("{} h {} min", s / 3600, s % 3600 / 60)
    } else {
        format!("{} d {} h", s / 86_400, s % 86_400 / 3600)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Workbench {
    fn local(&self) -> bool {
        self.target == Target::Local
    }

    /// "running" (this machine), "connected", "connecting" or "disconnected".
    pub(crate) fn state_word(&self) -> &'static str {
        match &self.link {
            Link::Ready(_) if self.local() => "running",
            Link::Ready(_) => "connected",
            Link::Connecting => "connecting",
            Link::Failed(_) => "disconnected",
        }
    }

    /// The current workbench's device and link state at the status bar's left.
    pub(crate) fn render_connection_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let host = self.host();
        let name = crate::welcome::start_screen::host_label(&host);
        let state = self.state_word();
        let link = match &self.link {
            Link::Ready(_) => p.ok,
            Link::Connecting => p.busy,
            Link::Failed(_) => p.err,
        };
        let hover = p.ink(look::BUTTON_HOVER);
        h_flex()
            .id("connection-status")
            .on_prepaint(self.popup_triggers.track("connection"))
            .min_w_0()
            .max_w(px(300.))
            .flex_shrink(1.)
            .h(px(16.))
            .px(px(look::GAP_6))
            .gap(px(look::GAP_4))
            .rounded_full()
            .text_size(px(look::small()))
            .text_color(p.text)
            .cursor_pointer()
            .when(self.connection_menu, |d| d.bg(hover))
            .hover(move |d| d.bg(hover))
            .child(
                icon(if self.local() {
                    "device.laptop"
                } else {
                    "device.server"
                })
                .size(px(look::ICON_SMALL)),
            )
            .child(div().min_w_0().overflow_hidden().child(name))
            .child(crate::look::dot(link).size(px(5.)).flex_none())
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(format!(
                    "{host}: {state}. Connection details"
                ))
                .build(w, cx)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.connection_menu = !this.connection_menu;
                this.main_menu = false;
                // The popup shows the usage, measured or not in the bar.
                if this.connection_menu && matches!(this.link, Link::Ready(_)) {
                    this.read_stats(cx);
                }
                cx.notify();
            }))
    }

    /// Connects again, reopening the project and the documents open now.
    pub(crate) fn reconnect(&mut self, cx: &mut Context<Self>) {
        if crate::shell::app_window::quit::active(cx)
            || !crate::welcome::resource_manager::allow_host(&self.host(), cx)
        {
            return;
        }
        let project = self.project.clone().or_else(|| self.last_project.clone());
        if let Some(project) = &project {
            let prefix = format!("{project}/");
            self.reopen = self
                .tabs
                .iter()
                .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
                .collect();
        }
        self.connect(self.target.clone(), project, true, cx);
    }

    /// Leaves the daemon. Kernels keep running there; Connect comes back.
    pub(crate) fn disconnect(&mut self, cx: &mut Context<Self>) {
        let project = self.project.clone().or_else(|| self.last_project.clone());
        if let Some(project) = &project {
            let prefix = format!("{project}/");
            self.reopen = self
                .tabs
                .iter()
                .filter_map(|p| p.strip_prefix(&prefix).map(str::to_string))
                .collect();
        }
        self.forget_connection(cx);
        self.last_project = project;
        self.link = Link::Failed("Disconnected".into());
        cx.notify();
    }

    /// Stops this machine's daemon and connects again, which starts a new
    /// one; kernels and agents keep running.
    fn restart_daemon(&mut self, cx: &mut Context<Self>) {
        if crate::shell::app_window::quit::active(cx)
            || crate::welcome::resource_manager::host_blocked(&self.host(), cx)
        {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        let stopped = daemon.call::<serde_json::Value>(Call::StopDaemon);
        cx.spawn(async move |this, cx| {
            let stopped = stopped.await;
            this.update(cx, |this, cx| match stopped {
                Ok(_) if !crate::welcome::resource_manager::host_blocked(&this.host(), cx) => {
                    this.reconnect(cx)
                }
                Ok(_) => {}
                Err(err) => {
                    this.problem = Some(format!("Could not restart sandhilld: {err:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn read_log(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let scroll = self.log.take().map(|l| l.scroll).unwrap_or_default();
        self.log = Some(LogView { tail: None, scroll });
        let tail = daemon.call::<LogTail>(Call::ReadLog {
            bytes: Some(256 * 1024),
        });
        cx.spawn(async move |this, cx| {
            let tail = tail.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                if let Some(log) = &mut this.log {
                    log.tail = Some(tail);
                    log.scroll.scroll_to_bottom();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Welcome's Resources and Storage page, for this device first.
    pub(crate) fn manage_resources(&mut self, cx: &mut Context<Self>) {
        let host = self.host();
        let shell = self.shell.clone();
        let handle = self.window_handle;
        // The manager reads every tab; release this workbench's update first.
        cx.defer(move |cx| {
            cx.update_window(handle, |_, window, cx| {
                shell
                    .update(cx, |shell, cx| {
                        shell.show_start(
                            crate::welcome::start_screen::StartMode::Resources(host),
                            window,
                            cx,
                        )
                    })
                    .ok();
            })
            .ok();
        });
    }

    /// Above the status bar's device item (Huan, 2026-11-16): the device and
    /// its state, its usage as rings, its running kernels, the switch for
    /// usage in the status bar, and what can be done to the connection.
    pub(crate) fn render_connection_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.connection_menu {
            return None;
        }
        let p = *Palette::get(cx);
        let local = self.local();
        let host = self.host();
        let info = self.about.as_ref();
        let ready = matches!(self.link, Link::Ready(_));
        let failed = matches!(self.link, Link::Failed(_));
        let heading = |text: &'static str| {
            div()
                .px(px(8.))
                .pt(px(6.))
                .pb(px(2.))
                .text_size(px(look::small()))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.faint)
                .child(text)
        };

        // The device, its state, and one grey line of what it runs.
        let mut about: Vec<String> = Vec::new();
        if !local {
            let (user, address) = match host.split_once('@') {
                Some((user, address)) => (Some(user.to_string()), address.to_string()),
                None => (None, host.clone()),
            };
            let as_user = info
                .map(|i| i.user.clone())
                .filter(|u| !u.is_empty())
                .or(user)
                .map_or_else(String::new, |u| format!("{u}@"));
            about.push(format!("{as_user}{address} over ssh"));
        }
        if let Some(info) = info {
            about.push(format!("sandhilld {}", info.daemon_version));
        }
        if ready {
            let since = if local {
                info.map(|i| i.started_ms).filter(|ms| *ms > 0)
            } else {
                self.connected_ms
            };
            if let Some(since) = since {
                about.push(format!("up {}", duration(now_ms() - since)));
            }
        }
        let (dot, state) = match &self.link {
            Link::Ready(_) => (p.ok, if local { "Running" } else { "Connected" }),
            Link::Connecting => (p.busy, "Connecting"),
            Link::Failed(_) => (p.err, "Disconnected"),
        };
        let header = v_flex()
            .px(px(look::GAP_8))
            .pt(px(look::GAP_4))
            .pb(px(look::GAP_6))
            .gap(px(look::GAP_2))
            .child(
                h_flex()
                    .gap(px(look::GAP_4))
                    .child(
                        icon(if local {
                            "device.laptop"
                        } else {
                            "device.cloud"
                        })
                        .size(px(look::ICON_LARGE)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(look::tab_text()))
                            .child(crate::welcome::start_screen::host_label(&host)),
                    )
                    .child(
                        h_flex()
                            .ml_auto()
                            .flex_none()
                            .gap(px(look::GAP_4))
                            .text_color(p.muted)
                            .child(crate::look::dot(dot).size(px(6.)))
                            .child(state),
                    ),
            )
            .when(!about.is_empty(), |d| {
                d.child(
                    div()
                        .pl(px(look::ICON_LARGE + look::GAP_4))
                        .text_color(p.faint)
                        .child(about.join(" · ")),
                )
            })
            .when_some(
                match &self.link {
                    Link::Failed(err) => Some(err.clone()),
                    _ => None,
                },
                |d, err| d.child(div().pl(px(23.)).text_color(p.err).child(err)),
            );

        // Usage: a ring each, with the memory and the kernels under them.
        let usage = ready.then(|| {
            let body = match &self.machine.stats {
                None => div()
                    .px(px(8.))
                    .text_color(p.faint)
                    .child("Measuring…")
                    .into_any_element(),
                Some(stats) => {
                    let rows = crate::panes::machine::resource_rows(
                        stats,
                        &self.machine.sessions,
                        &self.kernels,
                    );
                    let memory = rows
                        .iter()
                        .find(|r| r.key == "mem")
                        .map(|r| format!("Memory: {}", r.main));
                    let tiles = rows.into_iter().map(|r| {
                        let key = r.key;
                        let high = r.percent.is_some_and(|v| v >= 80);
                        let hover = p.ink(look::ROW_HOVER);
                        let main = r.main.clone();
                        h_flex()
                            .id(SharedString::from(format!("usage-{key}")))
                            .w(relative(0.5))
                            .gap(px(look::GAP_4))
                            .px(px(look::GAP_8))
                            .py(px(look::GAP_4))
                            .rounded(px(look::CONTROL_RADIUS))
                            .hover(move |d| d.bg(hover))
                            .child(crate::shell::meter::ring_of(
                                r.percent.map(|v| v as f32 / 100.),
                                if high { p.err } else { p.accent },
                                p.ink(18.),
                                26.,
                            ))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_color(p.muted).child(r.name.clone()))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if high { p.err } else { p.text })
                                            .child(
                                                r.percent
                                                    .map_or("–".to_string(), |v| format!("{v}%")),
                                            ),
                                    ),
                            )
                            .tooltip(move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(main.clone())
                                    .build(w, cx)
                            })
                            .cursor_pointer()
                            // Its details, as the status bar's number opens them.
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.connection_menu = false;
                                this.machine.resource = Some(key);
                                cx.notify();
                            }))
                    });
                    v_flex()
                        .child(h_flex().flex_wrap().children(tiles))
                        .when_some(memory, |d, memory| {
                            d.child(
                                div()
                                    .px(px(8.))
                                    .pt(px(2.))
                                    .text_color(p.faint)
                                    .child(memory),
                            )
                        })
                        .into_any_element()
                }
            };
            v_flex().child(heading("USAGE")).child(body)
        });

        // The device's running kernels, and this window's files using them.
        let sessions: Vec<_> = self
            .machine
            .sessions
            .iter()
            .filter(|s| s.status != sandhill_proto::SessionStatus::Ended)
            .collect();
        let kernels = (ready && !sessions.is_empty()).then(|| {
            let shown = 5;
            let lines = sessions.iter().take(shown).map(|s| {
                let files = self.used_by(&s.id).join(", ");
                h_flex()
                    .h(px(look::dense_row()))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .flex_none()
                            .child(crate::panes::machine::session_label(s, &self.kernels)),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .min_w_0()
                            .truncate()
                            .text_color(p.faint)
                            .child(files),
                    )
            });
            v_flex()
                .gap(px(2.))
                .child(heading("KERNELS"))
                .children(lines)
                .when(sessions.len() > shown, |d| {
                    d.child(
                        div()
                            .px(px(8.))
                            .text_color(p.faint)
                            .child(format!("and {} more", sessions.len() - shown)),
                    )
                })
        });

        // The status bar's usage, on or off.
        let usage_switch = h_flex()
            .px(px(8.))
            .py(px(4.))
            .gap(px(8.))
            .child(div().flex_1().child("Show usage in status bar"))
            .child(
                gpui_kit::component::switch::Switch::new("usage-in-status-bar")
                    .small()
                    .color(p.accent)
                    .checked(crate::prefs::usage_in_status_bar())
                    .accessibility_label("Show usage in status bar")
                    .on_change(|on: &bool, _, cx| {
                        crate::shell::choices::choose(crate::shell::choices::Choice::Usage(*on), cx)
                    }),
            );

        let hover = p.ink(look::ROW_HOVER);
        let item = |id: &'static str,
                    name: &'static str,
                    label: &'static str,
                    enabled: bool,
                    cx: &mut Context<Self>,
                    run: fn(&mut Workbench, &mut Context<Workbench>)| {
            h_flex()
                .id(id)
                .h(px(look::row()))
                .px(px(look::GAP_8))
                .gap(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .when(!enabled, |d| d.opacity(0.45))
                .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
                .child(icon(name).text_color(p.muted))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if enabled {
                        this.connection_menu = false;
                        run(this, cx);
                        cx.notify();
                    }
                }))
                .into_any_element()
        };
        let mut actions = Vec::new();
        if local && ready {
            actions.push(item(
                "c-restart",
                "refresh",
                "Restart sandhilld",
                true,
                cx,
                |this, cx| this.restart_daemon(cx),
            ));
        } else if failed {
            actions.push(item(
                "c-connect",
                if local { "refresh" } else { "device.cloud" },
                "Connect",
                true,
                cx,
                |this, cx| this.reconnect(cx),
            ));
        } else {
            actions.push(item(
                "c-reconnect",
                "refresh",
                "Reconnect",
                true,
                cx,
                |this, cx| this.reconnect(cx),
            ));
            actions.push(item(
                "c-disconnect",
                "close",
                "Disconnect",
                true,
                cx,
                |this, cx| this.disconnect(cx),
            ));
        }
        actions.push(item(
            "c-resources",
            "device.server",
            "Manage resources and storage…",
            true,
            cx,
            Self::manage_resources,
        ));
        actions.push(item(
            "c-log",
            "file.new",
            "Open the sandhilld log",
            ready,
            cx,
            |this, cx| this.read_log(cx),
        ));
        let version = format!(
            "Sandhill {}{}",
            env!("CARGO_PKG_VERSION"),
            option_env!("SANDHILL_COMMIT")
                .map(|c| format!(" · {}", &c[..c.len().min(7)]))
                .unwrap_or_default()
        );
        let separator = || crate::panes::tree_actions::separator(&p);
        let body = crate::agents::composer_view::popup(&p)
            .id("connection-menu")
            .occlude()
            .w(px(340.))
            .child(header)
            .when_some(usage, |d, usage| d.child(separator()).child(usage))
            .when_some(kernels, |d, kernels| d.child(kernels))
            .child(separator())
            .child(usage_switch)
            .child(separator())
            .children(actions)
            .child(
                div()
                    .px(px(8.))
                    .pt(px(4.))
                    .text_size(px(look::small()))
                    .text_color(p.faint)
                    .child(version),
            )
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("connection", event.position) {
                    return;
                }
                this.connection_menu = false;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .bottom(px(look::STATUS_BAR + 2.))
                .left(px(14.))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// The end of the daemon's log, scrolled to its last line.
    pub(crate) fn render_log_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let log = self.log.as_ref()?;
        let p = *Palette::get(cx);
        let host = crate::welcome::start_screen::host_label(&self.host());
        let (detail, text) = match &log.tail {
            None => ("Loading…".to_string(), String::new()),
            Some(Err(err)) => (err.clone(), String::new()),
            Some(Ok(tail)) => (
                tail.path.clone(),
                if tail.text.is_empty() {
                    "The log is empty.".into()
                } else {
                    // The log's colours are for a terminal.
                    strip_ansi(&tail.text)
                },
            ),
        };
        let card = v_flex()
            .id("log-card")
            .w(px(760.))
            .max_w(relative(0.9))
            .p(px(16.))
            .gap(px(look::GAP_12))
            .bg(p.bg)
            .border_1()
            .border_color(p.glass_edge)
            .rounded(px(look::RADIUS))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("sandhilld log on {host}")),
            )
            .child(div().text_color(p.muted).child(detail))
            .child(
                div()
                    .id("log-text")
                    .h(px(420.))
                    .overflow_y_scroll()
                    .track_scroll(&log.scroll)
                    .p(px(look::GAP_8))
                    .bg(p.chunk)
                    .rounded(px(look::CODE_RADIUS))
                    .font_family(look::mono())
                    .text_size(px(look::small()))
                    .child(text),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .child(
                        Button::new("log-refresh")
                            .small()
                            .label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.read_log(cx))),
                    )
                    .child(
                        Button::new("log-close")
                            .small()
                            .primary()
                            .label("Close")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.log = None;
                                cx.notify();
                            })),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("log-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.log = None;
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// The text without terminal colour codes.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{duration, strip_ansi};

    #[test]
    fn says_durations_and_strips_colours() {
        assert_eq!(duration(12_000), "12 s");
        assert_eq!(duration(185_000), "3 min");
        assert_eq!(duration(7_500_000), "2 h 5 min");
        assert_eq!(strip_ansi("\u{1b}[2mtime\u{1b}[0m ok"), "time ok");
    }
}
