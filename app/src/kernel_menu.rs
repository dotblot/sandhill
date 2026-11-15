//! Shared document kernel picker and independent Console session picker.

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::KernelspecInfo;

use crate::look::{self, Palette, icon, picture};
use crate::workbench::Workbench;

/// The groups the list shows, in order, by language.
const GROUPS: [(&str, &str); 2] = [("r", "R"), ("python", "Python")];

/// The kernels the list offers, grouped: R, then Python, then any other
/// language, each in the daemon's order. `only` keeps one language (a
/// script runs in its own).
pub(crate) fn grouped<'a>(
    kernels: &'a [KernelspecInfo],
    only: Option<&str>,
) -> Vec<(String, Vec<(usize, &'a KernelspecInfo)>)> {
    let mut groups: Vec<(String, Vec<(usize, &KernelspecInfo)>)> = Vec::new();
    for (ix, spec) in kernels.iter().enumerate() {
        if only.is_some_and(|l| !spec.language.eq_ignore_ascii_case(l)) {
            continue;
        }
        let language = spec.language.to_ascii_lowercase();
        let title = GROUPS
            .iter()
            .find(|(l, _)| *l == language)
            .map_or(spec.language.clone(), |(_, t)| t.to_string());
        match groups.iter_mut().find(|(t, _)| *t == title) {
            Some((_, list)) => list.push((ix, spec)),
            None => groups.push((title, vec![(ix, spec)])),
        }
    }
    let rank = |title: &str| {
        GROUPS
            .iter()
            .position(|(_, t)| *t == title)
            .unwrap_or(GROUPS.len())
    };
    groups.sort_by_key(|(t, _)| rank(t));
    groups
}

/// One explanation per runnable spec, in the same priority as recovery.
fn recommendation_reason(
    name: &str,
    running: bool,
    remembered: Option<&str>,
    project: Option<&str>,
    project_environment: bool,
    named: Option<&str>,
) -> Option<&'static str> {
    if running {
        Some("Running")
    } else if remembered == Some(name) {
        Some("Used last time")
    } else if project == Some(name) {
        Some("Project default")
    } else if project_environment || name.starts_with("project-") {
        Some("Project environment")
    } else if named == Some(name) {
        Some("Named by notebook")
    } else {
        None
    }
}

impl Workbench {
    /// The button naming the kernel, with its list under it when open.
    pub(crate) fn kernel_picker(&self, show_status: bool, cx: &mut Context<Self>) -> AnyElement {
        self.render_kernel_picker(false, show_status, cx)
    }

    pub(crate) fn console_kernel_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        self.render_kernel_picker(true, true, cx)
    }

    fn render_kernel_picker(
        &self,
        console: bool,
        show_status: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let kernel = if console {
            self.loose_kernel.as_deref()
        } else {
            self.active_kernel()
        };
        let chosen = kernel.and_then(|k| self.kernels.iter().find(|s| s.name == k));
        let unavailable = if console {
            None
        } else {
            kernel.and_then(|name| self.kernel_unavailable(name))
        };
        let mut label = chosen
            .map(|s| s.display_name.clone())
            .or_else(|| kernel.map(str::to_string))
            .unwrap_or_else(|| "Choose a kernel".to_string());
        let bound = console.then(|| self.console_bound_file()).flatten();
        if let Some(file) = bound {
            let name = self
                .open
                .iter()
                .chain(&self.others)
                .find(|open| open.notebook.path == file)
                .map(|open| open.notebook.name())
                .unwrap_or_else(|| crate::files::base_name(file));
            label = format!("{name} · {label}");
        }
        let state = Self::session_text(if console {
            &self.loose
        } else {
            self.active_state()
        });
        let tooltip = match unavailable {
            Some(reason) => format!("{state}\n{reason}"),
            None => state,
        };
        let hover = p.ink(8.);
        let open = self.kernel_menu && self.console_sessions.picker_console == console;
        let button = h_flex()
            .id(if console {
                "console-kernel-picker"
            } else {
                "kernel-picker"
            })
            .on_prepaint(self.popup_triggers.track(if console {
                "console-kernel"
            } else {
                "kernel"
            }))
            .flex_none()
            .when(console || self.open.is_none(), |d| {
                d.min_w_0()
                    .max_w(px(if console { 260. } else { 180. }))
                    .flex_shrink(1.)
            })
            // A capsule like every other in the window: fully round, a
            // faint wash, no edge (Huan, 2026-11-15).
            .h(px(22.))
            .gap(px(5.))
            .pl(px(6.))
            .pr(px(8.))
            .rounded_full()
            .bg(p.cap_bg)
            .text_size(px(look::TAB_TEXT))
            .text_color(if chosen.is_some() { p.text } else { p.muted })
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .map(|d| {
                if let Some(file) = bound {
                    d.child(picture(&look::file_icon(file)).size(px(14.)))
                } else if let Some(lang) = chosen.and_then(|s| look::language_icon(&s.language)) {
                    d.child(picture(lang).size(px(14.)))
                } else {
                    d.pl(px(8.))
                }
            })
            .child(div().min_w_0().text_ellipsis().child(label))
            .when(show_status, |d| {
                d.child(Self::session_dot(
                    if console {
                        &self.loose
                    } else {
                        self.active_state()
                    },
                    &p,
                ))
            })
            .child(icon("chevron.down").size(px(12.)).text_color(p.faint))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.kernel_menu = !open;
                this.console_sessions.picker_console = console;
                if this.kernel_menu {
                    this.refresh_console_sessions(cx);
                }
                this.console_sessions.history_open = false;
                cx.notify();
            }));
        div()
            .relative()
            .flex_none()
            // As wide as its label (truncating within 260px), so it sits
            // against the restart button rather than leaving a gap.
            .when(console, |d| {
                d.flex().min_w_0().max_w(px(260.)).flex_shrink(1.)
            })
            .when(!console && self.open.is_none(), |d| {
                d.min_w_0().flex_shrink(1.)
            })
            .child(button)
            .when(open, |d| {
                d.child(
                    div().absolute().left_0().top(px(24.)).child(
                        deferred(
                            anchored()
                                .snap_to_window_with_margin(px(8.))
                                .child(self.kernel_list(console, cx)),
                        )
                        .with_priority(3),
                    ),
                )
            })
            .into_any_element()
    }

    /// The list of kernels, by language; the chosen one is ticked.
    fn kernel_list(&self, console: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let chosen = if console {
            self.loose_kernel.clone()
        } else {
            self.active_kernel().map(str::to_string)
        };
        // A script offers only kernels for its language.
        let only = self
            .open
            .as_ref()
            .filter(|o| !console && o.is_script())
            .and_then(|o| o.notebook.language.clone());
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(name) = chosen.as_deref()
            && !console
            && let Some(reason) = self.kernel_unavailable(name)
        {
            rows.push(
                v_flex()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(p.err)
                    .child(name.to_string())
                    .child(div().text_color(p.faint).child(reason))
                    .into_any_element(),
            );
        }
        if console {
            let bound = self.console_bound_file();
            let mut live: Vec<_> = self
                .console_sessions
                .live
                .iter()
                .chain(&self.machine.sessions)
                .filter(|session| session.status == sandhill_proto::SessionStatus::Running)
                .collect();
            live.sort_by_key(|session| {
                (
                    self.open
                        .iter()
                        .chain(&self.others)
                        .find(|open| open.session.id() == Some(session.id.as_str()))
                        .is_none_or(|open| Some(open.notebook.path.as_str()) != bound),
                    session.created_ms,
                )
            });
            let mut seen = std::collections::HashSet::new();
            live.retain(|session| seen.insert(session.id.as_str()));
            if !live.is_empty() {
                rows.push(
                    div()
                        .px(px(8.))
                        .pt(px(2.))
                        .pb(px(2.))
                        .text_color(p.faint)
                        .child("Live sessions")
                        .into_any_element(),
                );
            }
            for (ix, session) in live.iter().enumerate() {
                let id = session.id.clone();
                let picked = self.console_target() == Some(id.as_str());
                let file = self
                    .open
                    .iter()
                    .chain(&self.others)
                    .find(|open| open.session.id() == Some(id.as_str()))
                    .map(|open| open.notebook.path.as_str());
                let title = format!(
                    "{} · {}",
                    file.map(crate::files::base_name).unwrap_or("Console"),
                    crate::machine::session_label(session, &self.kernels)
                );
                let created = chrono::DateTime::from_timestamp_millis(session.created_ms)
                    .map(|time| {
                        time.with_timezone(&chrono::Local)
                            .format("%b %-d, %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| "creation time unavailable".into());
                let hover = p.ink(7.);
                let busy = self
                    .runs
                    .values()
                    .any(|r| r.session == session.id && !r.finished)
                    || self
                        .machine
                        .sessions
                        .iter()
                        .any(|s| s.id == session.id && s.pending_runs > 0);
                rows.push(
                    h_flex()
                        .id(("live-console-session", ix))
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_none()
                                .when(picked, |d| d.child(icon("check").size(px(13.)))),
                        )
                        .when_some(look::language_icon(&session.language), |d, language| {
                            d.child(picture(language).size(px(14.)))
                        })
                        .child(v_flex().min_w_0().child(title).child(
                            div().text_color(p.faint).child(format!(
                                "Session {} · {created}{}",
                                ix + 1,
                                if busy { " · busy" } else { "" },
                            )),
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kernel_menu = false;
                            this.select_console_session(&id, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        for (title, specs) in grouped(&self.kernels, only.as_deref()) {
            rows.push(
                div()
                    .px(px(8.))
                    .pt(px(if rows.is_empty() { 2. } else { 8. }))
                    .pb(px(2.))
                    .text_color(p.faint)
                    .child(title)
                    .into_any_element(),
            );
            for (ix, spec) in specs {
                let name = spec.name.clone();
                let picked = chosen.as_deref() == Some(&spec.name);
                let hover = p.ink(7.);
                let remembered = self
                    .open
                    .as_ref()
                    .and_then(|open| self.remembered_kernel(&open.notebook.path));
                let project = self.setup.setup.as_ref().and_then(|setup| {
                    match spec.language.to_ascii_lowercase().as_str() {
                        "r" => setup.settings.r.as_deref(),
                        "python" => setup.settings.python.as_deref(),
                        _ => None,
                    }
                });
                let project_environment = self.setup.setup.as_ref().is_some_and(|setup| {
                    setup
                        .found
                        .iter()
                        .any(|found| found.kernel.as_deref() == Some(&spec.name))
                });
                let named = self
                    .open
                    .as_ref()
                    .and_then(|open| open.notebook.kernel.as_deref());
                let running = (picked && self.active_session().is_some())
                    || self
                        .machine
                        .sessions
                        .iter()
                        .chain(&self.console_sessions.live)
                        .any(|session| {
                            session.kernel == spec.name
                                && session.status != sandhill_proto::SessionStatus::Ended
                                && session.project.as_deref() == self.project.as_deref()
                        });
                let reason = recommendation_reason(
                    &spec.name,
                    running,
                    remembered,
                    project,
                    project_environment,
                    named,
                );
                rows.push(
                    h_flex()
                        .id(("kernel-row", ix))
                        .gap(px(6.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_none()
                                .when(picked && !console, |d| d.child(icon("check").size(px(13.)))),
                        )
                        .map(|d| match look::language_icon(&spec.language) {
                            Some(lang) => d.child(picture(lang).size(px(14.))),
                            None => d,
                        })
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    h_flex()
                                        .gap(px(10.))
                                        .child(if console {
                                            format!("New {}", spec.display_name)
                                        } else {
                                            spec.display_name.clone()
                                        })
                                        .when_some(reason.filter(|_| !console), |d, reason| {
                                            d.child(div().text_color(p.muted).child(reason))
                                        }),
                                )
                                // "Python (omicverse)" already names its environment.
                                .when_some(
                                    spec.env
                                        .clone()
                                        .filter(|env| !spec.display_name.contains(env)),
                                    |d, env| {
                                        d.child(
                                            div()
                                                .text_color(p.faint)
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .whitespace_nowrap()
                                                .child(env),
                                        )
                                    },
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.kernel_menu = false;
                            if console {
                                this.new_console_kernel(name.clone(), cx);
                            } else {
                                this.choose_kernel(name.clone(), cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
        }
        let mut offers = self.irkernel_menu_rows(only.as_deref(), cx);
        if rows.is_empty() && !offers.is_empty() {
            // The divider sets the offers apart from kernels; none are above.
            offers.remove(0);
        }
        rows.extend(offers);
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(p.faint)
                    .child("No kernels found")
                    .into_any_element(),
            );
        }
        crate::agents::composer_view::popup(&p)
            .id("kernel-menu")
            .occlude()
            .min_w(px(240.))
            .max_w(px(420.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains(
                    if console { "console-kernel" } else { "kernel" },
                    event.position,
                ) {
                    return;
                }
                this.kernel_menu = false;
                cx.notify();
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::grouped;
    use sandhill_proto::KernelspecInfo;

    #[test]
    fn recommendation_reasons_follow_recovery_precedence() {
        let name = "project-venv-py";
        let reason = |running, remembered, project, named| {
            super::recommendation_reason(name, running, remembered, project, false, named)
        };
        assert_eq!(
            reason(true, Some(name), Some(name), Some(name)),
            Some("Running")
        );
        assert_eq!(
            reason(false, Some(name), Some(name), Some(name)),
            Some("Used last time")
        );
        assert_eq!(
            reason(false, None, Some(name), Some(name)),
            Some("Project default")
        );
        assert_eq!(
            reason(false, None, None, Some(name)),
            Some("Project environment")
        );
        assert_eq!(
            super::recommendation_reason("python3", false, None, None, false, Some("python3")),
            Some("Named by notebook")
        );
        assert_eq!(
            super::recommendation_reason("python3", false, None, None, false, None),
            None
        );
        assert_eq!(
            super::recommendation_reason("conda-env-lab-py", false, None, None, true, None),
            Some("Project environment")
        );
    }

    fn spec(name: &str, language: &str) -> KernelspecInfo {
        KernelspecInfo {
            name: name.into(),
            display_name: name.into(),
            language: language.into(),
            env: None,
        }
    }

    #[test]
    fn lists_r_kernels_then_python_environments() {
        let kernels = [
            spec("python3", "python"),
            spec("ark", "R"),
            spec("conda-env-omicverse-py", "python"),
            spec("ir", "R"),
            spec("julia", "julia"),
        ];
        let names = |only: Option<&str>| -> Vec<(String, Vec<String>)> {
            grouped(&kernels, only)
                .into_iter()
                .map(|(t, specs)| (t, specs.iter().map(|(_, s)| s.name.clone()).collect()))
                .collect()
        };
        assert_eq!(
            names(None),
            vec![
                ("R".to_string(), vec!["ark".to_string(), "ir".to_string()]),
                (
                    "Python".to_string(),
                    vec!["python3".to_string(), "conda-env-omicverse-py".to_string()]
                ),
                ("julia".to_string(), vec!["julia".to_string()]),
            ]
        );
        assert_eq!(names(Some("r")).len(), 1);
    }
}
