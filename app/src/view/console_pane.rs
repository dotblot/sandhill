//! The Console and Terminal panes and the kernel session controls.

use super::*;

impl Workbench {
    /// Restart and Shut down for the shown document's kernel, while it runs.
    pub(super) fn session_buttons(
        &self,
        running: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !running {
            return None;
        }
        let p = *Palette::get(cx);
        Some(
            h_flex()
                .child(
                    icon_button("session-restart", "refresh", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "Restart: a new kernel, with nothing defined",
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.restart_session(cx))),
                )
                .child(
                    icon_button("session-stop", "power", &p, false)
                        .tooltip(|w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "Shut down: the next run starts a new kernel",
                            )
                            .build(w, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.shut_down_session(cx))),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn session_text(session: &Session) -> String {
        match session {
            Session::None => "Kernel not started".into(),
            Session::Starting => "Starting kernel…".into(),
            Session::Ready(_) => "Kernel ready".into(),
            Session::Failed(err) => format!("Kernel: {err}"),
        }
    }

    pub(crate) fn session_dot(session: &Session, p: &Palette) -> Div {
        dot(match session {
            Session::None => p.faint,
            Session::Starting => p.busy,
            Session::Ready(_) => p.ok,
            Session::Failed(_) => p.err,
        })
    }

    pub(crate) fn render_console_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let state = &self.loose;
        h_flex()
            .id("console-tools")
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .justify_end()
            .gap(px(4.))
            .text_color(p.muted)
            .when(self.project.is_some(), |d| {
                d.child(self.console_kernel_picker(cx))
            })
            .when(state.id().is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_none()
                        .child(
                            icon_button("console-restart", "refresh", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Restart: a new kernel, with nothing defined",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.restart_console_session(cx)),
                                ),
                        )
                        .child(
                            icon_button("console-stop", "power", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Shut down this session",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.stop_console_session(cx)),
                                ),
                        )
                        .child(
                            icon_button("console-send-plots", "send", &p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(
                                        "Send all plots to Plots",
                                    )
                                    .build(w, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.send_all_console_plots(cx)),
                                ),
                        ),
                )
            })
            .when(self.console_target().is_some(), |d| {
                d.child(
                    icon_button("interrupt", "interrupt", &p, false).on_click(cx.listener(
                        |this, _, _, _| {
                            if let (Some(daemon), Some(session)) =
                                (this.daemon(), this.console_target())
                            {
                                daemon.fire(sandhill_proto::Call::Interrupt {
                                    session: session.to_string(),
                                });
                            }
                        },
                    )),
                )
            })
    }

    pub(crate) fn render_console(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let focused = self
            .console_input
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        let edge = if focused {
            p.accent.opacity(0.6)
        } else {
            p.ink(if p.dark { 26. } else { 22. })
        };
        let entries = self
            .console
            .iter()
            .filter_map(|id| self.runs.get(id).map(|run| (id, run)))
            .filter(|(_, run)| self.notebook_console || !crate::plots::from_notebook(&run.cell))
            .map(|(run_id, run)| {
                let source = match (&run.cell, &run.by) {
                    // A script's runs are labelled with the script alone.
                    (Some((file, _)), _) if is_script(file) => {
                        Some(file.rsplit('/').next().unwrap_or(file).to_string())
                    }
                    (Some((file, cell)), _) => Some(format!(
                        "{} · {cell}",
                        file.rsplit('/').next().unwrap_or(file)
                    )),
                    (None, Some(by)) => Some(format!("run by {by}")),
                    _ => None,
                };
                let failed = failed_status(run);
                v_flex()
                    .gap_1()
                    .pb_2()
                    .when_some(source, |d, s| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(s),
                        )
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_1()
                            .font_family(look::MONO)
                            .text_size(px(look::CODE))
                            .line_height(px(LINE_HEIGHT))
                            .child(div().text_color(p.accent).child("›"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(p.muted)
                                    .child(run.highlighted_code(self.run_language(run), cx)),
                            ),
                    )
                    .children(run.outputs.iter().enumerate().map(|(n, o)| {
                        let id = SharedString::from(format!("cout-{run_id}-{n}"));
                        match show(o) {
                            Shown::Page(_)
                                if run.cell.as_ref().is_some_and(|(f, _)| !is_script(f)) =>
                            {
                                self.render_output(
                                    Shown::Note("A web page: shown under its cell.".into()),
                                    id,
                                    0.,
                                    None,
                                    Some(run_id),
                                    cx,
                                )
                            }
                            shown => self.render_output(
                                shown,
                                id,
                                self.files_width - 40.,
                                None,
                                Some(run_id),
                                cx,
                            ),
                        }
                    }))
                    .when_some(failed, |d, s| {
                        d.child(div().text_color(p.err).child(format!("Run ended: {s}")))
                    })
                    .when(!run.finished, |d| {
                        d.child(div().text_color(p.busy).child("running…"))
                    })
            })
            .collect::<Vec<_>>();
        let lines = self
            .console_input
            .read(cx)
            .value()
            .split('\n')
            .count()
            .clamp(1, 8);
        v_flex()
            .size_full()
            .child(
                look::PaneScroll::new(
                    "console-scroll",
                    look::STRIP,
                    6.,
                    div()
                        .id("console")
                        .px(px(14.))
                        .when(entries.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_color(p.muted)
                                    .child("Code run in this kernel shows here."),
                            )
                        })
                        .children(entries),
                )
                .handle(&self.console_scroll),
            )
            .child(
                div()
                    .flex_none()
                    .m(px(6.))
                    .rounded(px(RADIUS))
                    .border_1()
                    .border_color(edge)
                    .px(px(4.))
                    .overflow_hidden()
                    .on_action(
                        cx.listener(|this, _: &RunCell, window, cx| this.run_console(window, cx)),
                    )
                    // History walk; otherwise Up/Down reach the editor.
                    .capture_action(cx.listener(|this, _: &input::MoveUp, window, cx| {
                        if this.console_history_step(true, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input::MoveDown, window, cx| {
                        if this.console_history_step(false, window, cx) {
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        Editor::new(&self.console_input)
                            .bordered(false)
                            .appearance(false)
                            .line_height(px(LINE_HEIGHT))
                            .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note = |s: String, color: Hsla| {
            div()
                .px(px(14.))
                .py(px(6.))
                .text_color(color)
                .child(s)
                .into_any_element()
        };
        match (&self.terminal, &self.terminal_problem) {
            (Some(view), _) => div()
                .size_full()
                .bg(p.term)
                .child(view.clone())
                .into_any_element(),
            (None, Some(err)) => note(format!("The terminal could not start: {err}"), p.err),
            (None, None) if self.project.is_none() => {
                note("Open a project to start a terminal in it.".into(), p.muted)
            }
            (None, None) => note("Starting the terminal…".into(), p.muted),
        }
    }
}
