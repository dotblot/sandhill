//! The window: a project's files on the left, the open notebook in the
//! middle, and a Run button on each code cell.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_daemon::outputs::OutputBuilder;
use sandhill_link::Target;
use sandhill_proto::{
    Call, CellRef, EVENT_RUN_FINISHED, EVENT_SESSION_ENDED, Event, FileText, KernelspecInfo,
    PingReply, ProjectInfo, RunStarted, SessionInfo, Subscribed,
};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::daemon::Daemon;
use crate::notebook::{self, CellKind, Notebook};
use crate::shown::{Shown, show};

const MAX_FILES: u32 = 5000;

enum Link {
    Connecting,
    Failed(String),
    Ready(Rc<Daemon>),
}

#[derive(Default)]
enum Session {
    #[default]
    None,
    Starting,
    Ready(String),
    Failed(String),
}

/// A run's events as they arrive, and the outputs built from them.
#[derive(Default)]
struct Run {
    events: Vec<(String, Value)>,
    outputs: Vec<Value>,
    finished: bool,
    status: Option<String>,
    execution_count: Option<u64>,
}

struct Open {
    notebook: Notebook,
    kernel: Option<String>,
    session: Session,
    /// Cells waiting for the kernel to start.
    waiting: Vec<usize>,
    /// Cells sent to the daemon whose run id hasn't come back yet.
    sending: HashSet<String>,
    /// Each cell's latest run.
    cell_runs: HashMap<String, String>,
    runs: HashMap<String, Run>,
}

impl Open {
    fn new(notebook: Notebook, kernel: Option<String>) -> Self {
        Open {
            notebook,
            kernel,
            session: Session::None,
            waiting: Vec::new(),
            sending: HashSet::new(),
            cell_runs: HashMap::new(),
            runs: HashMap::new(),
        }
    }

    fn session_id(&self) -> Option<&str> {
        match &self.session {
            Session::Ready(id) => Some(id),
            _ => None,
        }
    }
}

pub struct Workbench {
    rt: Arc<Runtime>,
    link: Link,
    about: Option<PingReply>,
    kernels: Vec<KernelspecInfo>,
    project: Option<String>,
    files: Vec<String>,
    open: Option<Open>,
    /// The latest problem, shown at the bottom.
    problem: Option<String>,
}

impl Workbench {
    pub fn new(rt: Arc<Runtime>, project: Option<String>, cx: &mut Context<Self>) -> Self {
        let connect = Daemon::connect(rt.clone(), Target::Local);
        cx.spawn(async move |this, cx| {
            let result = connect.await;
            let events = this
                .update(cx, |this, cx| {
                    let events = this.connected(result, project, cx);
                    cx.notify();
                    events
                })
                .ok()
                .flatten();
            let Some(mut events) = events else {
                return;
            };
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| this.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Workbench {
            rt,
            link: Link::Connecting,
            about: None,
            kernels: Vec::new(),
            project: None,
            files: Vec::new(),
            open: None,
            problem: None,
        }
    }

    fn daemon(&self) -> Option<Rc<Daemon>> {
        match &self.link {
            Link::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    fn connected(
        &mut self,
        result: anyhow::Result<(Daemon, tokio::sync::mpsc::UnboundedReceiver<Event>)>,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<tokio::sync::mpsc::UnboundedReceiver<Event>> {
        let (daemon, events) = match result {
            Ok(ok) => ok,
            Err(err) => {
                self.link = Link::Failed(format!("{err:#}"));
                return None;
            }
        };
        let daemon = Rc::new(daemon);
        self.link = Link::Ready(daemon.clone());
        let ping = daemon.call::<PingReply>(Call::Ping);
        let kernels = daemon.call::<Vec<KernelspecInfo>>(Call::ListKernelspecs);
        cx.spawn(async move |this, cx| {
            let ping = ping.await;
            let kernels = kernels.await;
            this.update(cx, |this, cx| {
                match ping {
                    Ok(ping) => this.about = Some(ping),
                    Err(err) => this.problem = Some(format!("{err:#}")),
                }
                match kernels {
                    Ok(kernels) => this.kernels = kernels,
                    Err(err) => this.problem = Some(format!("Listing kernels failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        if let Some(project) = project {
            self.open_project(project, cx);
        }
        Some(events)
    }

    fn choose_project(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Project".into()),
        });
        cx.spawn(async move |this, cx| {
            let path = match picked.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(err)) => {
                    this.update(cx, |this, cx| {
                        this.problem = Some(format!("The folder picker failed: {err:#}"));
                        cx.notify();
                    })
                    .ok();
                    None
                }
                _ => None,
            };
            if let Some(path) = path {
                this.update(cx, |this, cx| {
                    this.open_project(path.to_string_lossy().into_owned(), cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_project(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let opened = daemon.call::<ProjectInfo>(Call::OpenProject { path: path.clone() });
        let files = daemon.call::<Vec<String>>(Call::ListFiles {
            path,
            limit: Some(MAX_FILES),
        });
        cx.spawn(async move |this, cx| {
            let opened = opened.await;
            let files = files.await;
            this.update(cx, |this, cx| {
                match opened {
                    Ok(project) => {
                        this.close_notebook(cx);
                        this.project = Some(project.path);
                        let mut files = files.unwrap_or_default();
                        files.sort();
                        this.files = files;
                        this.problem = None;
                    }
                    Err(err) => this.problem = Some(format!("Opening the project failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_file(&mut self, rel: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let path = format!("{project}/{rel}");
        let Some(format) = notebook::format_of(&path) else {
            self.problem = Some(format!(
                "{rel}: milestone 1 opens only notebooks and Quarto documents"
            ));
            cx.notify();
            return;
        };
        // ReadNotebook leaves big outputs in the daemon's store, so a large
        // notebook travels light.
        let read = match format {
            notebook::Format::Ipynb => Call::ReadNotebook { path },
            notebook::Format::Quarto => Call::ReadFile { path },
        };
        let read = daemon.call::<FileText>(read);
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                match read.and_then(|file| notebook::parse(&file.path, &file.text)) {
                    Ok(nb) => {
                        this.close_notebook(cx);
                        let kernel = notebook::pick_kernel(&nb, &this.kernels);
                        this.open = Some(Open::new(nb, kernel));
                        this.problem = None;
                    }
                    Err(err) => this.problem = Some(format!("Opening {rel} failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Closes the open notebook and stops its kernel.
    fn close_notebook(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.take() {
            self.stop_session(&open);
        }
        cx.notify();
    }

    fn stop_session(&self, open: &Open) {
        if let (Some(daemon), Some(session)) = (self.daemon(), open.session_id()) {
            let stop = daemon.call::<Value>(Call::Shutdown {
                session: session.to_string(),
            });
            self.rt.spawn(async move {
                if let Err(err) = stop.await {
                    tracing::warn!("stopping a kernel failed: {err:#}");
                }
            });
        }
    }

    fn choose_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        if open.kernel.as_deref() != Some(&name) {
            self.stop_session(&open);
            open = Open::new(open.notebook, Some(name));
        }
        self.open = Some(open);
        cx.notify();
    }

    fn run_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match &open.session {
            Session::Ready(_) => self.execute(index, cx),
            Session::Starting => open.waiting.push(index),
            Session::None | Session::Failed(_) => {
                open.waiting.push(index);
                self.start_session(cx);
            }
        }
        cx.notify();
    }

    fn start_session(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(kernel) = open.kernel.clone() else {
            open.session = Session::Failed("No kernel is installed".into());
            open.waiting.clear();
            return;
        };
        open.session = Session::Starting;
        let path = open.notebook.path.clone();
        let started = daemon.call::<SessionInfo>(Call::StartSession {
            kernel,
            cwd: Some(open.notebook.folder().to_string()),
            project: Some(project),
        });
        cx.spawn(async move |this, cx| {
            let session = match started.await {
                Ok(info) => {
                    // Subscribe before running anything, so no output is missed.
                    let subscribed = daemon.call::<Subscribed>(Call::Subscribe {
                        session: info.id.clone(),
                        since_seq: None,
                    });
                    subscribed.await.map(|_| info.id)
                }
                Err(err) => Err(err),
            };
            this.update(cx, |this, cx| {
                let Some(open) = this.open.as_mut().filter(|o| o.notebook.path == path) else {
                    return;
                };
                match session {
                    Ok(id) => {
                        open.session = Session::Ready(id);
                        for index in std::mem::take(&mut open.waiting) {
                            this.execute(index, cx);
                        }
                    }
                    Err(err) => {
                        open.session = Session::Failed(format!("{err:#}"));
                        open.waiting.clear();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn execute(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (Some(session), Some(cell)) = (open.session_id(), open.notebook.cells.get(index))
        else {
            return;
        };
        let cell_id = cell.id.clone();
        let started = daemon.call::<RunStarted>(Call::Execute {
            session: session.to_string(),
            code: cell.source.clone(),
            cell: Some(CellRef {
                file: open.notebook.path.clone(),
                cell_id: cell_id.clone(),
            }),
            quiet: false,
            by: None,
        });
        open.sending.insert(cell_id.clone());
        let path = open.notebook.path.clone();
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                let Some(open) = this.open.as_mut().filter(|o| o.notebook.path == path) else {
                    return;
                };
                open.sending.remove(&cell_id);
                match started {
                    Ok(run) => {
                        open.runs.entry(run.run.clone()).or_default();
                        open.cell_runs.insert(cell_id, run.run);
                    }
                    Err(err) => this.problem = Some(format!("Running the cell failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.session_id() != Some(event.session.as_str()) {
            return;
        }
        if event.msg_type == EVENT_SESSION_ENDED {
            for run in open.runs.values_mut().filter(|r| !r.finished) {
                run.finished = true;
                run.status = Some("the kernel stopped".into());
            }
            open.session = Session::Failed("The kernel stopped".into());
            cx.notify();
            return;
        }
        // Events can arrive before the run id does, so keep them by run.
        let Some(run_id) = event.run else {
            return;
        };
        let run = open.runs.entry(run_id).or_default();
        if event.msg_type == EVENT_RUN_FINISHED {
            run.finished = true;
            run.status = event.content["status"].as_str().map(str::to_string);
            run.execution_count = event.content["execution_count"].as_u64();
        } else {
            run.events.push((event.msg_type, event.content));
            let mut builder = OutputBuilder::new();
            for (msg_type, content) in &run.events {
                builder.push(msg_type, content);
            }
            run.outputs = builder.finish();
        }
        cx.notify();
    }
}

impl Render for Workbench {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .child(self.render_top_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_files(cx))
                    .child(self.render_notebook(cx)),
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
                    .disabled(self.daemon().is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.choose_project(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.connection_text()),
            )
    }

    fn connection_text(&self) -> String {
        match (&self.link, &self.about) {
            (Link::Connecting, _) => "Starting sandhilld…".into(),
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
            .w(px(260.))
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if self.project.is_some() {
                        format!("Files ({})", self.files.len())
                    } else {
                        "Files".into()
                    }),
            )
            .child(div().flex_1().min_h_0().child(list))
    }

    fn render_notebook(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(open) = &self.open else {
            let hint = match &self.link {
                Link::Failed(err) => format!("Could not start sandhilld: {err}"),
                _ if self.project.is_none() => "Open a project folder to begin.".into(),
                _ => "Pick a notebook (.ipynb, .qmd or .Rmd) on the left.".into(),
            };
            return div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(hint)
                .into_any_element();
        };
        let mono = theme.mono_font_family.clone();
        let cells = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| match cell.kind {
                CellKind::Text => div()
                    .px_4()
                    .py_2()
                    .text_color(theme.muted_foreground)
                    .child(cell.source.clone())
                    .into_any_element(),
                CellKind::Code => self.render_code_cell(open, ix, mono.clone(), cx),
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
            .into_any_element()
    }

    fn render_notebook_bar(&self, open: &Open, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let session = match &open.session {
            Session::None => "Kernel not started".to_string(),
            Session::Starting => "Starting kernel…".to_string(),
            Session::Ready(_) => "Kernel ready".to_string(),
            Session::Failed(err) => format!("Kernel: {err}"),
        };
        let kernels = self.kernels.iter().enumerate().map(|(ix, spec)| {
            let name = spec.name.clone();
            let chosen = open.kernel.as_deref() == Some(&spec.name);
            let button = Button::new(("kernel", ix))
                .xsmall()
                .label(spec.display_name.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.choose_kernel(name.clone(), cx)));
            if chosen {
                button.primary()
            } else {
                button.ghost()
            }
        });
        h_flex()
            .w_full()
            .px_4()
            .py_2()
            .gap_2()
            .flex_wrap()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(open.notebook.name().to_string()),
            )
            .child(div().w(px(12.)))
            .children(kernels)
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(session),
            )
    }

    fn render_code_cell(
        &self,
        open: &Open,
        ix: usize,
        mono: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let cell = &open.notebook.cells[ix];
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| open.runs.get(id));
        let busy = open.sending.contains(&cell.id)
            || open.waiting.contains(&ix)
            || run.is_some_and(|r| !r.finished);
        let label = match run {
            Some(Run {
                finished: true,
                execution_count: Some(n),
                ..
            }) => format!("[{n}]"),
            _ if busy => "[*]".into(),
            _ => "[ ]".into(),
        };
        let outputs: Vec<AnyElement> = run
            .map(|r| {
                r.outputs
                    .iter()
                    .map(|o| render_output(show(o), &mono, cx))
                    .collect()
            })
            .unwrap_or_default();
        let failed = run
            .and_then(|r| r.status.as_deref())
            .filter(|s| *s != "ok")
            .map(|s| s.to_string());
        v_flex()
            .w_full()
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .p_2()
                    .bg(theme.secondary)
                    .rounded_t_md()
                    .child(
                        Button::new(("run", ix))
                            .xsmall()
                            .ghost()
                            .label("Run")
                            .loading(busy)
                            .on_click(cx.listener(move |this, _, _, cx| this.run_cell(ix, cx))),
                    )
                    .child(
                        div()
                            .w(px(36.))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(mono.clone())
                            .text_size(px(12.5))
                            .child(cell.source.clone()),
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
                        .when_some(failed.filter(|s| s != "error"), |d, s| {
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
            .child(
                self.problem
                    .clone()
                    .unwrap_or_else(|| "GPUI trial · milestone 1".into()),
            )
    }
}

fn render_output(shown: Shown, mono: &SharedString, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let text = |s: String| {
        div()
            .font_family(mono.clone())
            .text_size(px(12.5))
            .child(s.trim_end_matches('\n').to_string())
    };
    match shown {
        Shown::Text(s) => text(s).into_any_element(),
        Shown::Warning(s) => text(s).text_color(theme.warning).into_any_element(),
        Shown::Error(s) => text(s).text_color(theme.danger).into_any_element(),
        Shown::Note(s) => div()
            .text_xs()
            .italic()
            .text_color(theme.muted_foreground)
            .child(s)
            .into_any_element(),
    }
}
