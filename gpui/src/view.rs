//! Drawing the window: the top bar, files, the notebook, and on the right
//! the Console or Terminal above Variables.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, Input};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::Variable;
use serde_json::Value;

use crate::notebook::{self, CellKind};
use crate::shown::{Shown, show};
use crate::workbench::{Link, Open, Plot, RightTab, Run, Session, Workbench, plot_key};
use crate::{Interrupt, RunAll, RunCell, Save};

/// Height of one line in an editor, and its padding.
const LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING: f32 = 20.;
const PLOT_MAX_WIDTH: f32 = 720.;
const RIGHT_WIDTH: f32 = 460.;
const VARIABLES_HEIGHT: f32 = 250.;

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run_all(cx)))
            .on_action(cx.listener(|this, _: &Interrupt, _, _| this.interrupt()))
            .child(self.render_top_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_files(cx))
                    .child(self.render_notebook(cx))
                    .child(self.render_right(cx)),
            )
            .child(self.render_status(cx))
    }
}

impl Workbench {
    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let project = self
            .project
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| "No project".into());
        let local = self.target == Target::Local;
        h_flex()
            .w_full()
            .h(px(40.))
            .px_3()
            .gap_3()
            .border_b_1()
            .border_color(theme.border)
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Sandhill"))
            .child(div().text_color(theme.muted_foreground).child(project))
            .child(
                Button::new("open-project")
                    .small()
                    .label("Open Project…")
                    .disabled(self.daemon().is_none() || !local)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_project(cx))),
            )
            .when(self.project.is_some(), |d| {
                d.child(
                    Button::new("close-project")
                        .small()
                        .ghost()
                        .label("Other Project")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_notebook(cx);
                            this.project = None;
                            this.files.clear();
                            this.terminal = None;
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Machine"),
            )
            .child(
                div()
                    .w(px(180.))
                    .child(Input::new(&self.host_input).small()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.connection_text()),
            )
    }

    fn connection_text(&self) -> String {
        match (&self.link, &self.about) {
            (Link::Connecting, _) => "Connecting…".into(),
            (Link::Failed(_), _) => "Not connected".into(),
            (Link::Ready(_), Some(about)) => {
                format!("{} · sandhilld {}", about.hostname, about.daemon_version)
            }
            (Link::Ready(_), None) => "Connected".into(),
        }
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let open_path = self.open.as_ref().map(|o| o.notebook.path.clone());
        let project = self.project.clone().unwrap_or_default();
        let list = uniform_list(
            "files",
            self.files.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let theme = cx.theme();
                range
                    .map(|ix| {
                        let rel = this.files[ix].clone();
                        let is_notebook = notebook::format_of(&rel).is_some();
                        let is_open = open_path.as_deref() == Some(&format!("{project}/{rel}"));
                        div()
                            .id(("file", ix))
                            .w_full()
                            .h(px(24.))
                            .px_3()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .when(!is_notebook, |d| d.text_color(theme.muted_foreground))
                            .when(is_open, |d| d.bg(theme.list_active))
                            .hover(|d| d.bg(theme.list_hover))
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_file(rel.clone(), cx)),
                            )
                            .child(this.files[ix].clone())
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .size_full();
        v_flex()
            .w(px(240.))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if self.project.is_some() {
                        format!("Files ({})", self.files.len())
                    } else {
                        "Files".into()
                    })
                    .child(div().flex_1())
                    .when(self.project.is_some(), |d| {
                        d.child(
                            Button::new("refresh-files")
                                .xsmall()
                                .ghost()
                                .label("Refresh")
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx))),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().child(list))
    }

    fn render_notebook(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(open) = &self.open else {
            return self.render_start(cx);
        };
        let cells = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| match cell.kind {
                CellKind::Text => div()
                    .px_2()
                    .child(self.render_cell_editor(open, ix, cx))
                    .into_any_element(),
                CellKind::Code => self.render_code_cell(open, ix, cx),
            })
            .collect::<Vec<_>>();
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(self.render_notebook_bar(open, cx))
            .child(
                div()
                    .id("cells")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().p_4().gap_3().children(cells)),
            )
            .border_r_1()
            .border_color(theme.border)
            .into_any_element()
    }

    /// What the middle shows with no notebook open: how to open a project,
    /// and the projects opened before on this machine.
    fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let body: AnyElement = match &self.link {
            Link::Connecting => div().child("Connecting…").into_any_element(),
            Link::Failed(err) => v_flex()
                .gap_2()
                .max_w(px(520.))
                .child(format!("Could not reach sandhilld: {err}"))
                .child(
                    div()
                        .text_xs()
                        .child("Type another machine in the box at the top right and press Enter."),
                )
                .into_any_element(),
            Link::Ready(_) if self.project.is_some() => div()
                .child("Pick a notebook (.ipynb, .qmd or .Rmd) on the left, or use the Console.")
                .into_any_element(),
            Link::Ready(_) => {
                let recent = self
                    .recent
                    .iter()
                    .filter(|p| p.exists)
                    .take(8)
                    .enumerate()
                    .map(|(ix, project)| {
                        let path = project.path.clone();
                        div()
                            .id(("recent", ix))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_project(path.clone(), cx)
                            }))
                            .child(
                                div()
                                    .text_color(theme.foreground)
                                    .child(project.name.clone()),
                            )
                            .child(div().text_xs().child(project.path.clone()))
                    });
                v_flex()
                    .gap_3()
                    .w(px(460.))
                    .child(div().text_color(theme.foreground).child("Open a project"))
                    .child(Input::new(&self.path_input))
                    .child(div().text_xs().child(
                        "Type a folder and press Enter. On this Mac, Open Project… picks one.",
                    ))
                    .when(!self.recent.is_empty(), |d| {
                        d.child(div().pt_2().text_color(theme.foreground).child("Recent"))
                            .children(recent)
                    })
                    .into_any_element()
            }
        };
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .px_6()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.muted_foreground)
            .border_r_1()
            .border_color(theme.border)
            .child(body)
            .into_any_element()
    }

    fn kernel_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let chosen = self.active_kernel().map(str::to_string);
        self.kernels
            .iter()
            .enumerate()
            .map(|(ix, spec)| {
                let name = spec.name.clone();
                let button = Button::new(("kernel", ix))
                    .xsmall()
                    .label(spec.display_name.clone())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.choose_kernel(name.clone(), cx)),
                    );
                if chosen.as_deref() == Some(&spec.name) {
                    button.primary().into_any_element()
                } else {
                    button.ghost().into_any_element()
                }
            })
            .collect()
    }

    fn session_text(session: &Session) -> String {
        match session {
            Session::None => "Kernel not started".into(),
            Session::Starting => "Starting kernel…".into(),
            Session::Ready(_) => "Kernel ready".into(),
            Session::Failed(err) => format!("Kernel: {err}"),
        }
    }

    fn render_notebook_bar(&self, open: &Open, cx: &mut Context<Self>) -> impl IntoElement {
        let kernels = self.kernel_buttons(cx);
        let theme = cx.theme().clone();
        let name = if open.dirty {
            format!("{} •", open.notebook.name())
        } else {
            open.notebook.name().to_string()
        };
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_2()
            .flex_wrap()
            .border_b_1()
            .border_color(theme.border)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
            .child(
                Button::new("save")
                    .xsmall()
                    .ghost()
                    .label("Save")
                    .disabled(!open.dirty)
                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            )
            .child(
                Button::new("run-all")
                    .xsmall()
                    .ghost()
                    .label("Run All")
                    .on_click(cx.listener(|this, _, _, cx| this.run_all(cx))),
            )
            .child(
                Button::new("close")
                    .xsmall()
                    .ghost()
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_notebook(cx))),
            )
            .child(div().w(px(12.)))
            .children(kernels)
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Self::session_text(&open.session)),
            )
    }

    /// A cell's editor, as tall as its text, so the notebook scrolls and
    /// the cell doesn't.
    fn render_cell_editor(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let editor = &open.editors[ix];
        let lines = editor.read(cx).value().split('\n').count().max(1);
        div()
            .w_full()
            .on_action(cx.listener(move |this, _: &RunCell, window, cx| {
                this.run_and_advance(ix, window, cx)
            }))
            .child(
                Editor::new(editor)
                    .bordered(false)
                    .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
            )
            .into_any_element()
    }

    fn render_code_cell(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let cell = &open.notebook.cells[ix];
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id));
        let busy = open.sending.contains(&cell.id)
            || open.waiting.contains(&ix)
            || run.is_some_and(|r| !r.finished);
        let count = match run {
            Some(run) => run.execution_count,
            None => cell.execution_count,
        };
        let label = match count {
            _ if busy => "[*]".to_string(),
            Some(n) => format!("[{n}]"),
            None => "[ ]".into(),
        };
        let outputs: &[Value] = match run {
            Some(run) => &run.outputs,
            None => &cell.outputs,
        };
        let outputs: Vec<AnyElement> = outputs
            .iter()
            .map(|o| self.render_output(show(o), PLOT_MAX_WIDTH, cx))
            .collect();
        let failed = run.and_then(failed_status);
        v_flex()
            .w_full()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .p_1()
                    .bg(theme.secondary)
                    .rounded_t_md()
                    .child(
                        v_flex()
                            .w(px(44.))
                            .pt_1()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(("run", ix))
                                    .xsmall()
                                    .ghost()
                                    .label("Run")
                                    .loading(busy)
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.run_cell(ix, cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(label),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_cell_editor(open, ix, cx)),
                    ),
            )
            .when(!outputs.is_empty() || failed.is_some(), |d| {
                d.child(
                    v_flex()
                        .p_2()
                        .gap_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .children(outputs)
                        .when_some(failed, |d, s| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.danger)
                                    .child(format!("Run ended: {s}")),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn render_output(&self, shown: Shown, max_width: f32, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let text = |s: String| {
            div()
                .font_family(mono.clone())
                .text_size(px(12.5))
                .child(s.trim_end_matches('\n').to_string())
        };
        let note = |s: String| {
            div()
                .text_xs()
                .italic()
                .text_color(theme.muted_foreground)
                .child(s)
                .into_any_element()
        };
        match shown {
            Shown::Text(s) => text(s).into_any_element(),
            Shown::Warning(s) => text(s).text_color(theme.warning).into_any_element(),
            Shown::Error(s) => text(s).text_color(theme.danger).into_any_element(),
            Shown::Note(s) => note(s),
            Shown::Plot { value, .. } => match self.plots.get(&plot_key(&value)) {
                Some(Plot::Ready(image)) => div()
                    .child(img(image.clone()).max_w(px(max_width)))
                    .into_any_element(),
                Some(Plot::Failed(err)) => note(format!("The plot could not be shown: {err}")),
                _ => note("Loading the plot…".into()),
            },
        }
    }

    fn render_right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tab =
            |id: &'static str, label: &'static str, which: RightTab, cx: &mut Context<Self>| {
                let button = Button::new(id).xsmall().label(label).on_click(
                    cx.listener(move |this, _, window, cx| this.show_tab(which, window, cx)),
                );
                if self.right_tab == which {
                    button.primary()
                } else {
                    button.ghost()
                }
            };
        let top: AnyElement = match self.right_tab {
            RightTab::Console => self.render_console(cx),
            RightTab::Terminal => self.render_terminal(cx),
        };
        v_flex()
            .w(px(RIGHT_WIDTH))
            .h_full()
            .flex_none()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(tab("tab-console", "Console", RightTab::Console, cx))
                    .child(tab("tab-terminal", "Terminal", RightTab::Terminal, cx)),
            )
            .child(div().flex_1().min_h_0().child(top))
            .child(self.render_variables(cx))
    }

    fn render_console(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mono = theme.mono_font_family.clone();
        let entries = self
            .console
            .iter()
            .filter_map(|id| self.runs.get(id).map(|run| (id, run)))
            .map(|(_, run)| {
                let source = match (&run.cell, &run.by) {
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
                        d.child(div().text_xs().text_color(theme.muted_foreground).child(s))
                    })
                    .child(
                        h_flex()
                            .items_start()
                            .gap_1()
                            .font_family(mono.clone())
                            .text_size(px(12.5))
                            .child(div().text_color(theme.primary).child(">"))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(run.code.trim_end().to_string()),
                            ),
                    )
                    .children(
                        run.outputs
                            .iter()
                            .map(|o| self.render_output(show(o), RIGHT_WIDTH - 40., cx)),
                    )
                    .when_some(failed, |d, s| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(format!("Run ended: {s}")),
                        )
                    })
                    .when(!run.finished, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("running…"),
                        )
                    })
            })
            .collect::<Vec<_>>();
        let header = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .flex_wrap()
            .text_xs()
            .text_color(theme.muted_foreground)
            .when(self.open.is_none() && self.project.is_some(), |d| {
                d.children(self.kernel_buttons(cx)).when(
                    matches!(self.loose, Session::None | Session::Failed(_)),
                    |d| {
                        d.child(
                            Button::new("start-console")
                                .xsmall()
                                .primary()
                                .label("Start")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.start_console_kernel(cx)),
                                ),
                        )
                    },
                )
            })
            .when_some(self.open.as_ref(), |d, open| {
                d.child(format!("Following {}", open.notebook.name()))
            })
            .child(div().flex_1())
            .child(Self::session_text(self.active_state()))
            .when(self.active_session().is_some(), |d| {
                d.child(
                    Button::new("interrupt")
                        .xsmall()
                        .ghost()
                        .label("Stop")
                        .on_click(cx.listener(|this, _, _, _| this.interrupt())),
                )
            });
        let lines = self
            .console_input
            .read(cx)
            .value()
            .split('\n')
            .count()
            .clamp(1, 8);
        v_flex()
            .size_full()
            .child(header)
            .child(
                div()
                    .id("console")
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .overflow_y_scroll()
                    .track_scroll(&self.console_scroll)
                    .when(entries.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Code run in this kernel shows here."),
                        )
                    })
                    .children(entries),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(theme.border)
                    .on_action(
                        cx.listener(|this, _: &RunCell, window, cx| this.run_console(window, cx)),
                    )
                    .child(
                        Editor::new(&self.console_input)
                            .bordered(false)
                            .h(px(lines as f32 * LINE_HEIGHT + EDITOR_PADDING)),
                    ),
            )
            .into_any_element()
    }

    fn render_terminal(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        match (&self.terminal, &self.terminal_problem) {
            (Some(view), _) => div().size_full().child(view.clone()).into_any_element(),
            (None, Some(err)) => div()
                .p_2()
                .text_color(theme.danger)
                .child(format!("The terminal could not start: {err}"))
                .into_any_element(),
            (None, None) if self.project.is_none() => div()
                .p_2()
                .text_color(theme.muted_foreground)
                .child("Open a project to start a terminal in it.")
                .into_any_element(),
            (None, None) => div()
                .p_2()
                .text_color(theme.muted_foreground)
                .child("Starting the terminal…")
                .into_any_element(),
        }
    }

    fn render_variables(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut rows = Vec::new();
        self.variable_rows(&self.vars.list, &[], &mut rows, cx);
        let status = if self.active_session().is_none() {
            Some("No kernel running.".to_string())
        } else if let Some(err) = &self.vars.error {
            Some(err.clone())
        } else if self.vars.list.is_empty() && !self.vars.loading {
            Some("No variables yet.".into())
        } else {
            None
        };
        v_flex()
            .h(px(VARIABLES_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Variables")
                    .when(self.vars.loading, |d| {
                        d.child(div().pl_2().child("updating…"))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("refresh-variables")
                            .xsmall()
                            .ghost()
                            .label("Refresh")
                            .disabled(self.active_session().is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_variables(cx))),
                    ),
            )
            .child(
                div()
                    .id("variables")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when_some(status, |d, s| {
                        d.child(
                            div()
                                .px_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(s),
                        )
                    })
                    .children(rows),
            )
    }

    fn variable_rows(
        &self,
        list: &[Variable],
        parent: &[String],
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let theme = cx.theme().clone();
        for variable in list {
            let mut path = parent.to_vec();
            path.push(variable.name.clone());
            let open = self.vars.expanded.contains(&path);
            let toggle_path = path.clone();
            rows.push(
                h_flex()
                    .id(SharedString::from(format!("var-{}", path.join("\u{1f}"))))
                    .px_2()
                    .pl(px(8. + 14. * parent.len() as f32))
                    .gap_2()
                    .h(px(22.))
                    .text_xs()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .hover(|d| d.bg(theme.list_hover))
                    .when(variable.children, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_variable(toggle_path.clone(), cx)
                            }))
                    })
                    .child(div().w(px(10.)).text_color(theme.muted_foreground).child(
                        match (variable.children, open) {
                            (false, _) => "",
                            (true, false) => "▸",
                            (true, true) => "▾",
                        },
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(variable.name.clone()),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(variable.class.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(variable.summary.clone()),
                    )
                    .into_any_element(),
            );
            if open && let Some(children) = self.vars.children.get(&path) {
                self.variable_rows(children, &path, rows, cx);
            }
        }
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .h(px(24.))
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(if self.problem.is_some() {
                theme.danger
            } else {
                theme.muted_foreground
            })
            .child(self.problem.clone().unwrap_or_else(|| {
                "GPUI trial · milestone 3 · Shift+Enter runs a cell, ⌘S saves".into()
            }))
    }
}

/// How a run ended, when that wasn't fine (or a plain error, which its
/// output already shows).
fn failed_status(run: &Run) -> Option<String> {
    run.status
        .as_deref()
        .filter(|s| *s != "ok" && *s != "error")
        .map(str::to_string)
}
