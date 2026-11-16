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
        let hover = p.ink(7.);
        h_flex()
            .id("connection-status")
            .on_prepaint(self.popup_triggers.track("connection"))
            .min_w_0()
            .max_w(px(300.))
            .flex_shrink(1.)
            .h(px(20.))
            .px(px(6.))
            .gap(px(6.))
            .rounded(px(6.))
            .text_size(px(look::SMALL))
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
                .size(px(12.)),
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
        self.connect(self.target.clone(), project, cx);
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
                Ok(_) => this.reconnect(cx),
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

    /// Above the status bar's device item: what the connection is, and what
    /// can be done to it.
    pub(crate) fn render_connection_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.connection_menu {
            return None;
        }
        let p = *Palette::get(cx);
        let local = self.local();
        let host = self.host();
        let mut rows: Vec<(&str, String)> = Vec::new();
        let info = self.about.as_ref();
        if !local {
            let (user, address) = match host.split_once('@') {
                Some((user, address)) => (Some(user.to_string()), address.to_string()),
                None => (None, host.clone()),
            };
            let named = info
                .map(|i| i.hostname.clone())
                .filter(|h| !h.is_empty() && h.split('.').next() != address.split('.').next())
                .map(|h| format!(" ({h})"))
                .unwrap_or_default();
            rows.push(("Address", format!("{address}{named}")));
            let as_user = info
                .map(|i| i.user.clone())
                .filter(|u| !u.is_empty())
                .or(user)
                .unwrap_or_else(|| "you".into());
            rows.push(("Via", format!("ssh, as {as_user}")));
        }
        if let Some(info) = info {
            let by = match info.started_by.as_str() {
                "app" => " · started by Sandhill",
                "launchd" => " · started by launchd",
                "systemd" => " · started by systemd",
                _ => "",
            };
            rows.push(("sandhilld", format!("{}{by}", info.daemon_version)));
        }
        match &self.link {
            Link::Failed(err) => rows.push(("Error", err.clone())),
            Link::Ready(_) => {
                let since = if local {
                    info.map(|i| i.started_ms).filter(|ms| *ms > 0)
                } else {
                    self.connected_ms
                };
                if let Some(since) = since {
                    rows.push((
                        if local {
                            "Running for"
                        } else {
                            "Connected for"
                        },
                        duration(now_ms() - since),
                    ));
                }
                let kernels = self
                    .machine
                    .sessions
                    .iter()
                    .filter(|s| s.status != sandhill_proto::SessionStatus::Ended)
                    .count();
                rows.push((
                    "Kernels",
                    if kernels > 0 {
                        format!("{kernels} running")
                    } else {
                        "none running".into()
                    },
                ));
            }
            Link::Connecting => {}
        }
        rows.push((
            "Sandhill",
            format!(
                "{}{}",
                env!("CARGO_PKG_VERSION"),
                option_env!("SANDHILL_COMMIT")
                    .map(|c| format!(" · {c}"))
                    .unwrap_or_default()
            ),
        ));
        let hover = p.ink(8.);
        let item = |id: &'static str,
                    name: &'static str,
                    label: &'static str,
                    enabled: bool,
                    cx: &mut Context<Self>,
                    run: fn(&mut Workbench, &mut Context<Workbench>)| {
            h_flex()
                .id(id)
                .h(px(26.))
                .px(px(8.))
                .gap(px(8.))
                .rounded(px(look::RADIUS - 4.))
                .when(!enabled, |d| d.opacity(0.45))
                .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
                .child(icon(name).size(px(13.)).text_color(p.muted))
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
        let ready = matches!(self.link, Link::Ready(_));
        let failed = matches!(self.link, Link::Failed(_));
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
        let body = crate::agents::composer_view::popup(&p)
            .id("connection-menu")
            .occlude()
            .w(px(320.))
            .child(
                h_flex()
                    .px(px(8.))
                    .py(px(4.))
                    .gap(px(8.))
                    .child(
                        icon(if local {
                            "device.laptop"
                        } else {
                            "device.cloud"
                        })
                        .size(px(14.)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::welcome::start_screen::host_label(&host)),
                    )
                    .child(div().ml_auto().text_color(p.faint).child(self.state_word())),
            )
            .child(
                v_flex()
                    .px(px(8.))
                    .pb(px(4.))
                    .gap(px(2.))
                    .children(rows.into_iter().map(|(name, value)| {
                        h_flex()
                            .gap(px(10.))
                            .items_start()
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_none()
                                    .text_color(p.faint)
                                    .child(name.to_string()),
                            )
                            .child(div().flex_1().min_w_0().child(value))
                    })),
            )
            .child(crate::panes::tree_actions::separator(&p))
            .children(actions)
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
            .gap(px(8.))
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
                    .p(px(8.))
                    .bg(p.chunk)
                    .rounded(px(6.))
                    .font_family(look::MONO)
                    .text_size(px(look::SMALL))
                    .child(text),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
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
