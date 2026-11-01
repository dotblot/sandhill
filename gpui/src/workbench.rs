//! The window: a project's files on the left, the open notebook in the
//! middle, with an editor and a Run button on each cell.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_daemon::outputs::OutputBuilder;
use sandhill_link::Target;
use sandhill_proto::{
    BLOB_KEY, Blob, CONFLICT_ERROR, Call, CellRef, EVENT_RUN_FINISHED, EVENT_SESSION_ENDED, Event,
    FileOutputs, FileText, FileWritten, KernelspecInfo, PingReply, ProjectInfo, RunStarted,
    SessionInfo, SessionStatus, Subscribed,
};
use serde_json::Value;

use crate::daemon::Daemon;
use crate::notebook::{self, CellKind, Format, Notebook};
use crate::shown::{Shown, show};
use crate::{RunAll, RunCell, Save};

const MAX_FILES: u32 = 5000;
/// Height of one line in a cell's editor, and its padding.
const LINE_HEIGHT: f32 = 20.;
const EDITOR_PADDING: f32 = 20.;
const PLOT_MAX_WIDTH: f32 = 720.;

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

enum Plot {
    Loading,
    Ready(Arc<Image>),
    Failed(String),
}

struct Open {
    notebook: Notebook,
    /// One editor per cell, text cells included.
    editors: Vec<Entity<EditorState>>,
    /// Edited since the last save.
    dirty: bool,
    mtime_ms: i64,
    kernel: Option<String>,
    session: Session,
    /// Cells waiting for the kernel to start.
    waiting: Vec<usize>,
    /// Cells sent to the daemon whose run id hasn't come back yet.
    sending: HashSet<String>,
    /// Each cell's latest run.
    cell_runs: HashMap<String, String>,
    runs: HashMap<String, Run>,
    _subscriptions: Vec<Subscription>,
}

impl Open {
    fn session_id(&self) -> Option<&str> {
        match &self.session {
            Session::Ready(id) => Some(id),
            _ => None,
        }
    }

    fn sources(&self, cx: &App) -> Vec<String> {
        self.editors
            .iter()
            .map(|e| e.read(cx).value().to_string())
            .collect()
    }

    /// Forgets the kernel, for a switch to another.
    fn reset_session(&mut self) {
        self.session = Session::None;
        self.waiting.clear();
        self.sending.clear();
    }
}

pub struct Workbench {
    link: Link,
    about: Option<PingReply>,
    kernels: Vec<KernelspecInfo>,
    project: Option<String>,
    files: Vec<String>,
    open: Option<Open>,
    /// Plots by blob hash (or by a hash of an inline value).
    plots: HashMap<String, Plot>,
    /// The latest problem, shown at the bottom.
    problem: Option<String>,
}

impl Workbench {
    pub fn new(
        rt: Arc<tokio::runtime::Runtime>,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let connect = Daemon::connect(rt, Target::Local);
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
            link: Link::Connecting,
            about: None,
            kernels: Vec::new(),
            project: None,
            files: Vec::new(),
            open: None,
            plots: HashMap::new(),
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
                "{rel}: the trial opens only notebooks and Quarto documents"
            ));
            cx.notify();
            return;
        };
        // ReadNotebook leaves big outputs in the daemon's store, so a large
        // notebook travels light.
        let read = match format {
            Format::Ipynb => Call::ReadNotebook { path: path.clone() },
            Format::Quarto => Call::ReadFile { path: path.clone() },
        };
        let read = daemon.call::<FileText>(read);
        let saved = daemon.call::<FileOutputs>(Call::FileOutputs { file: path });
        let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let saved = saved.await;
            let sessions = sessions.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                match read
                    .and_then(|file| Ok((notebook::parse(&file.path, &file.text)?, file.mtime_ms)))
                {
                    Ok((nb, mtime_ms)) => {
                        this.close_notebook(cx);
                        this.show_notebook(nb, mtime_ms, window, cx);
                        match saved {
                            Ok(saved) => this.show_saved_outputs(saved, &sessions, cx),
                            Err(err) => {
                                this.problem =
                                    Some(format!("Reading saved outputs failed: {err:#}"))
                            }
                        }
                    }
                    Err(err) => this.problem = Some(format!("Opening {rel} failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_notebook(
        &mut self,
        nb: Notebook,
        mtime_ms: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kernel = notebook::pick_kernel(&nb, &self.kernels);
        let code_language = match nb.language.as_deref() {
            Some("python") => "python",
            Some("r") => "r",
            _ => "text",
        };
        let mut subscriptions = Vec::new();
        let editors = nb
            .cells
            .iter()
            .map(|cell| {
                let language = match cell.kind {
                    CellKind::Code => code_language,
                    CellKind::Text => "markdown",
                };
                let source = cell.source.clone();
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language)
                        .line_number(false)
                        .folding(false)
                        // The notebook scrolls, not the cell.
                        .scroll_beyond_last_line(Some(0))
                        .default_value(source)
                });
                subscriptions.push(cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
                    if let InputEvent::Change = event
                        && let Some(open) = this.open.as_mut()
                    {
                        open.dirty = true;
                        cx.notify();
                    }
                }));
                editor
            })
            .collect();
        let file_outputs: Vec<Value> = nb.cells.iter().flat_map(|c| c.outputs.clone()).collect();
        self.open = Some(Open {
            notebook: nb,
            editors,
            dirty: false,
            mtime_ms,
            kernel,
            session: Session::None,
            waiting: Vec::new(),
            sending: HashSet::new(),
            cell_runs: HashMap::new(),
            runs: HashMap::new(),
            _subscriptions: subscriptions,
        });
        self.problem = None;
        self.fetch_plots(&file_outputs, cx);
    }

    /// Puts each cell's latest journaled run under it, and carries on in the
    /// kernel that ran them if it is still running.
    fn show_saved_outputs(
        &mut self,
        saved: FileOutputs,
        sessions: &[SessionInfo],
        cx: &mut Context<Self>,
    ) {
        let daemon = self.daemon();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let mut outputs = Vec::new();
        let latest = saved.cells.iter().max_by_key(|c| c.started_ms).cloned();
        for cell in saved.cells {
            if !open.notebook.cells.iter().any(|c| c.id == cell.cell_id) {
                continue;
            }
            outputs.extend(cell.outputs.iter().cloned());
            open.runs.insert(
                cell.run.clone(),
                Run {
                    events: Vec::new(),
                    outputs: cell.outputs,
                    finished: cell.finished_ms.is_some(),
                    status: cell.status,
                    execution_count: cell.execution_count,
                },
            );
            open.cell_runs.insert(cell.cell_id, cell.run);
        }
        let running = latest.and_then(|latest| {
            sessions
                .iter()
                .find(|s| s.id == latest.session && s.status == SessionStatus::Running)
        });
        if let (Some(session), Some(daemon)) = (running, daemon)
            && self.kernels.iter().any(|k| k.name == session.kernel)
        {
            open.kernel = Some(session.kernel.clone());
            open.session = Session::Ready(session.id.clone());
            let subscribe = daemon.call::<Subscribed>(Call::Subscribe {
                session: session.id.clone(),
                since_seq: Some(saved.seq),
            });
            cx.spawn(async move |this, cx| {
                if let Err(err) = subscribe.await {
                    this.update(cx, |this, cx| {
                        this.problem = Some(format!("Following the kernel failed: {err:#}"));
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        self.fetch_plots(&outputs, cx);
    }

    /// Closes the open notebook, saving it first if it was edited. Its
    /// kernel keeps running, so reopening carries on where it left off.
    fn close_notebook(&mut self, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| o.dirty) {
            self.save(cx);
        }
        self.open = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(open)) = (self.daemon(), self.open.as_ref()) else {
            return;
        };
        let sources = open.sources(cx);
        let text = match open.notebook.render(&sources) {
            Ok(text) => text,
            Err(err) => {
                self.problem = Some(format!("Saving failed: {err:#}"));
                cx.notify();
                return;
            }
        };
        let path = open.notebook.path.clone();
        let expected_mtime_ms = Some(open.mtime_ms);
        let write = match open.notebook.format {
            Format::Ipynb => Call::WriteNotebook {
                path: path.clone(),
                text: text.clone(),
                expected_mtime_ms,
            },
            Format::Quarto => Call::WriteFile {
                path: path.clone(),
                text: text.clone(),
                expected_mtime_ms,
                base64: false,
                create: false,
            },
        };
        let written = daemon.call::<FileWritten>(write);
        cx.spawn(async move |this, cx| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(written) => {
                        if let Some(open) = this.open.as_mut().filter(|o| o.notebook.path == path)
                        {
                            open.mtime_ms = written.mtime_ms;
                            open.dirty = open.sources(cx) != sources;
                            // Later saves start from what is on disk now.
                            match notebook::parse(&path, &text) {
                                Ok(nb) if nb.cells.len() == open.editors.len() => {
                                    open.notebook = nb
                                }
                                _ => {
                                    this.problem = Some(
                                        "Saved. The cells changed shape; reopen the file to edit further."
                                            .into(),
                                    )
                                }
                            }
                        }
                    }
                    Err(err) if format!("{err:#}").contains(CONFLICT_ERROR) => {
                        this.problem = Some(
                            "Not saved: the file changed on disk. Reopen it to see the change."
                                .into(),
                        )
                    }
                    Err(err) => this.problem = Some(format!("Saving failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn choose_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        let daemon = self.daemon();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.kernel.as_deref() == Some(&name) {
            return;
        }
        if let (Some(daemon), Some(session)) = (daemon, open.session_id()) {
            let stop = daemon.call::<Value>(Call::Shutdown {
                session: session.to_string(),
            });
            cx.background_spawn(async move {
                if let Err(err) = stop.await {
                    tracing::warn!("stopping a kernel failed: {err:#}");
                }
            })
            .detach();
        }
        open.reset_session();
        open.kernel = Some(name);
        cx.notify();
    }

    fn run_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.notebook.cells.get(index).map(|c| c.kind) != Some(CellKind::Code) {
            return;
        }
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

    fn run_all(&mut self, cx: &mut Context<Self>) {
        let count = self.open.as_ref().map_or(0, |o| o.notebook.cells.len());
        for index in 0..count {
            self.run_cell(index, cx);
        }
    }

    /// Runs the cell whose editor has focus, then moves to the next code
    /// cell, as Shift+Enter does in Jupyter.
    fn run_and_advance(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.run_cell(index, cx);
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let next = (index + 1..open.notebook.cells.len())
            .find(|&i| open.notebook.cells[i].kind == CellKind::Code);
        if let Some(next) = next {
            open.editors[next].update(cx, |editor, cx| editor.focus(window, cx));
        }
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
        let (Some(session), Some(cell), Some(editor)) = (
            open.session_id(),
            open.notebook.cells.get(index),
            open.editors.get(index),
        ) else {
            return;
        };
        let cell_id = cell.id.clone();
        let started = daemon.call::<RunStarted>(Call::Execute {
            session: session.to_string(),
            code: editor.read(cx).value().to_string(),
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
            let is_plot = matches!(
                event.msg_type.as_str(),
                "display_data" | "execute_result" | "update_display_data"
            );
            run.events.push((event.msg_type, event.content));
            let mut builder = OutputBuilder::new();
            for (msg_type, content) in &run.events {
                builder.push(msg_type, content);
            }
            run.outputs = builder.finish();
            if is_plot {
                let outputs = run.outputs.clone();
                self.fetch_plots(&outputs, cx);
            }
        }
        cx.notify();
    }

    /// Starts loading the plots in `outputs` that aren't loaded yet.
    fn fetch_plots(&mut self, outputs: &[Value], cx: &mut Context<Self>) {
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        for output in outputs {
            let Shown::Plot { mime, value } = show(output) else {
                continue;
            };
            let key = plot_key(&value);
            if self.plots.contains_key(&key) {
                continue;
            }
            let format = match mime.as_str() {
                "image/png" => ImageFormat::Png,
                "image/jpeg" => ImageFormat::Jpeg,
                "image/gif" => ImageFormat::Gif,
                _ => ImageFormat::Svg,
            };
            let text = match (&value, self.daemon()) {
                (Value::String(text), _) => {
                    let text = text.clone();
                    Box::pin(async move { anyhow::Ok(text) })
                        as std::pin::Pin<Box<dyn Future<Output = anyhow::Result<String>>>>
                }
                (blob, Some(daemon)) if blob[BLOB_KEY].is_string() => {
                    let fetch = daemon.call::<Blob>(Call::GetBlob {
                        hash: blob[BLOB_KEY].as_str().unwrap_or_default().to_string(),
                        near: near.clone(),
                    });
                    Box::pin(async move { Ok(fetch.await?.data) })
                }
                _ => continue,
            };
            self.plots.insert(key.clone(), Plot::Loading);
            cx.spawn(async move |this, cx| {
                let plot = match text.await.and_then(|text| decode(format, text)) {
                    Ok(bytes) => Plot::Ready(Arc::new(Image::from_bytes(format, bytes))),
                    Err(err) => Plot::Failed(format!("{err:#}")),
                };
                this.update(cx, |this, cx| {
                    this.plots.insert(key, plot);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }
}

/// The plot's bytes: base64 for bitmaps, the text itself for SVG.
fn decode(format: ImageFormat, text: String) -> anyhow::Result<Vec<u8>> {
    if format == ImageFormat::Svg {
        return Ok(text.into_bytes());
    }
    let clean: String = text.split_whitespace().collect();
    Ok(base64::engine::general_purpose::STANDARD.decode(clean)?)
}

fn plot_key(value: &Value) -> String {
    match value[BLOB_KEY].as_str() {
        Some(hash) => hash.to_string(),
        None => {
            let mut hasher = DefaultHasher::new();
            value.to_string().hash(&mut hasher);
            format!("inline-{:x}", hasher.finish())
        }
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
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
            .on_action(cx.listener(|this, _: &RunAll, _, cx| this.run_all(cx)))
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
        let cells = open
            .notebook
            .cells
            .iter()
            .enumerate()
            .map(|(ix, cell)| match cell.kind {
                CellKind::Text => div()
                    .px_2()
                    .child(self.render_editor(open, ix, cx))
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

    /// A cell's editor, as tall as its text, so the notebook scrolls and
    /// the cell doesn't.
    fn render_editor(&self, open: &Open, ix: usize, cx: &mut Context<Self>) -> AnyElement {
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
            .and_then(|id| open.runs.get(id));
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
            .map(|o| self.render_output(show(o), cx))
            .collect();
        let failed = run
            .and_then(|r| r.status.as_deref())
            .filter(|s| *s != "ok" && *s != "error")
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
                            .child(self.render_editor(open, ix, cx)),
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

    fn render_output(&self, shown: Shown, cx: &App) -> AnyElement {
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
                    .child(img(image.clone()).max_w(px(PLOT_MAX_WIDTH)))
                    .into_any_element(),
                Some(Plot::Failed(err)) => note(format!("The plot could not be shown: {err}")),
                _ => note("Loading the plot…".into()),
            },
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
                "GPUI trial · milestone 2 · Shift+Enter runs a cell, ⌘S saves".into()
            }))
    }
}
