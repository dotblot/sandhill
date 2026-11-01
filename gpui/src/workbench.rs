//! The window's state and what it does: the connection, the project, the
//! open notebook, the kernel's runs, the Console, Variables and Terminal.
//! Drawing is in `view.rs`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use gpui_kit::component::input::{EditorState, InputEvent, InputState};
use gpui_kit::*;
use sandhill_daemon::outputs::OutputBuilder;
use sandhill_link::Target;
use sandhill_proto::{
    BLOB_KEY, Blob, CONFLICT_ERROR, Call, CellRef, DirEntry, EVENT_RUN_FINISHED, EVENT_RUN_STARTED,
    EVENT_SESSION_ENDED, Event, FileOutputs, FileText, FileWritten, KernelspecInfo, PingReply,
    ProjectInfo, RunStarted, SessionInfo, SessionRuns, SessionStatus, Subscribed, TERMINAL_SHELL,
    TerminalEvent, TerminalInfo, TerminalSubscribed, Variable, Variables,
};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::daemon::{Daemon, Streams};
use crate::explorer::Explorer;
use crate::notebook::{self, CellKind, Format, Notebook};
use crate::pages::PageView;
use crate::shown::{Page, Shown, page_title, show};
use crate::terminal::TerminalView;
use crate::tree::{self, Folder, Tree};

/// Web pages kept hidden for drawing again.
const PARKED_PAGES: usize = 24;
/// Runs the Console keeps.
const CONSOLE_RUNS: usize = 200;

pub(crate) enum Link {
    Connecting,
    Failed(String),
    Ready(Rc<Daemon>),
}

#[derive(Default)]
pub(crate) enum Session {
    #[default]
    None,
    Starting,
    Ready(String),
    Failed(String),
}

impl Session {
    pub(crate) fn id(&self) -> Option<&str> {
        match self {
            Session::Ready(id) => Some(id),
            _ => None,
        }
    }
}

/// One piece of code a kernel ran, from a cell, the Console or an agent.
#[derive(Default)]
pub(crate) struct Run {
    pub code: String,
    /// The notebook cell it came from: (file, cell id).
    pub cell: Option<(String, String)>,
    /// Who ran it when it wasn't the user (an agent).
    pub by: Option<String>,
    /// Code a tool ran to look around (Variables); not shown.
    pub quiet: bool,
    events: Vec<(String, Value)>,
    pub outputs: Vec<Value>,
    pub finished: bool,
    pub status: Option<String>,
    pub execution_count: Option<u64>,
}

pub(crate) enum Plot {
    Loading,
    Ready(Arc<Image>),
    Failed(String),
}

pub(crate) struct Open {
    pub notebook: Notebook,
    /// One editor per cell, text cells included.
    pub editors: Vec<Entity<EditorState>>,
    /// Edited since the last save.
    pub dirty: bool,
    mtime_ms: i64,
    pub kernel: Option<String>,
    pub session: Session,
    /// Cells waiting for the kernel to start.
    pub waiting: Vec<usize>,
    /// Cells sent to the daemon whose run id hasn't come back yet.
    pub sending: HashSet<String>,
    /// Each cell's latest run.
    pub cell_runs: HashMap<String, String>,
    /// Where the notebook was scrolled to while another tab was shown.
    scroll: Point<Pixels>,
    _subscriptions: Vec<Subscription>,
}

impl Open {
    fn sources(&self, cx: &App) -> Vec<String> {
        self.editors
            .iter()
            .map(|e| e.read(cx).value().to_string())
            .collect()
    }
}

/// A gap between panes that can be dragged to resize them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Split {
    /// Between the files pane and the middle.
    Files,
    /// Between the middle and the right column.
    Right,
    /// Above Variables.
    Variables,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightTab {
    Console,
    Terminal,
    Viewer,
}

/// The Variables pane: the active kernel's variables, and the parts of
/// those opened.
#[derive(Default)]
pub(crate) struct Vars {
    pub session: Option<String>,
    pub list: Vec<Variable>,
    pub children: HashMap<Vec<String>, Vec<Variable>>,
    pub expanded: HashSet<Vec<String>>,
    pub loading: bool,
    pub error: Option<String>,
}

pub struct Workbench {
    rt: Arc<Runtime>,
    /// Counts connections, so a late reply for an old one is dropped.
    generation: u64,
    pub(crate) target: Target,
    pub(crate) link: Link,
    pub(crate) about: Option<PingReply>,
    pub(crate) host_input: Entity<InputState>,
    pub(crate) path_input: Entity<InputState>,
    pub(crate) kernels: Vec<KernelspecInfo>,
    pub(crate) recent: Vec<ProjectInfo>,
    pub(crate) project: Option<String>,
    /// The project's folders, read as they are opened.
    pub(crate) tree: Tree,
    /// The notebook shown.
    pub(crate) open: Option<Open>,
    /// Notebooks open in other tabs.
    others: Vec<Open>,
    /// The tabs' files, in the order they were opened.
    pub(crate) tabs: Vec<String>,
    /// The Console's kernel when no notebook is open.
    pub(crate) loose: Session,
    pub(crate) loose_kernel: Option<String>,
    /// Every run seen, by id.
    pub(crate) runs: HashMap<String, Run>,
    /// The Console: the active kernel's runs, oldest first.
    pub(crate) console: Vec<String>,
    console_session: Option<String>,
    pub(crate) console_input: Entity<EditorState>,
    pub(crate) console_scroll: ScrollHandle,
    pub(crate) vars: Vars,
    pub(crate) right_tab: RightTab,
    pub(crate) terminal: Option<Entity<TerminalView>>,
    pub(crate) terminal_problem: Option<String>,
    terminal_starting: bool,
    /// Plots by blob hash (or by a hash of an inline value).
    pub(crate) plots: HashMap<String, Plot>,
    /// The web views drawn now, by page key (see `sync_pages`).
    pub(crate) pages: HashMap<String, Entity<PageView>>,
    /// Pages not drawn now, hidden, oldest first.
    parked: Vec<(String, Entity<PageView>)>,
    /// The Viewer's page, counting from the oldest; the latest when none.
    pub(crate) viewer_index: Option<usize>,
    /// The notebook's scrolling, and where pages under its cells send the
    /// wheel's turns they don't use.
    pub(crate) notebook_scroll: ScrollHandle,
    page_wheel: tokio::sync::mpsc::UnboundedSender<f32>,
    /// The Data Explorer, a tab of its own in the middle.
    pub(crate) explorer: Option<Entity<Explorer>>,
    /// The Data Explorer's tab is the one shown.
    pub(crate) explorer_shown: bool,
    /// The latest problem, shown at the bottom.
    pub(crate) problem: Option<String>,
    /// The files pane is shown (the title bar's first button).
    pub(crate) show_files: bool,
    /// The right column is shown.
    pub(crate) show_right: bool,
    /// Pane sizes, set by dragging the gaps between them.
    pub(crate) files_width: f32,
    pub(crate) right_width: f32,
    pub(crate) variables_height: f32,
    /// The gap being dragged.
    pub(crate) dragging: Option<Split>,
    /// The machine box replaces the machine's tab in the title bar.
    pub(crate) editing_host: bool,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(
        rt: Arc<Runtime>,
        target: Target,
        project: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host = match &target {
            Target::Local => sandhill_link::LOCAL.to_string(),
            Target::Ssh(destination) => destination.to_string(),
        };
        let host_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("local, or an ssh host")
                .default_value(host)
        });
        let path_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("A folder on that machine, like ~/analysis")
        });
        let console_input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("python")
                .line_number(false)
                .folding(false)
                .scroll_beyond_last_line(Some(0))
                .soft_wrap(false)
                .submit_on_enter(true)
                .placeholder("Type code; Enter runs it")
        });
        let subscriptions = vec![
            cx.subscribe(&host_input, |this, input, event: &InputEvent, cx| {
                // Clicking away puts the machine's tab back.
                if let InputEvent::Blur = event {
                    this.editing_host = false;
                    cx.notify();
                }
                if let InputEvent::PressEnter { .. } = event {
                    let host = input.read(cx).value().trim().to_string();
                    match Target::parse(if host.is_empty() { "local" } else { &host }) {
                        Ok(target) => {
                            this.editing_host = false;
                            this.connect(target, None, cx)
                        }
                        Err(err) => {
                            this.problem = Some(err.to_string());
                            cx.notify();
                        }
                    }
                }
            }),
            cx.subscribe(&path_input, |this, input, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let path = input.read(cx).value().trim().to_string();
                    if !path.is_empty() {
                        this.open_project(path, cx);
                    }
                }
            }),
            cx.subscribe_in(
                &console_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    } = event
                    {
                        this.run_console(window, cx);
                    }
                },
            ),
        ];
        let (page_wheel, mut wheel) = tokio::sync::mpsc::unbounded_channel::<f32>();
        cx.spawn(async move |this, cx| {
            while let Some(px) = wheel.recv().await {
                let scrolled = this.update(cx, |this, cx| {
                    let scroll = &this.notebook_scroll;
                    let mut offset = scroll.offset();
                    let max = scroll.max_offset().y;
                    offset.y = (offset.y - gpui_kit::px(px)).clamp(-max, gpui_kit::px(0.));
                    scroll.set_offset(offset);
                    cx.notify();
                });
                if scrolled.is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Workbench {
            rt,
            generation: 0,
            target: target.clone(),
            link: Link::Connecting,
            about: None,
            host_input,
            path_input,
            kernels: Vec::new(),
            recent: Vec::new(),
            project: None,
            tree: Tree::default(),
            open: None,
            others: Vec::new(),
            tabs: Vec::new(),
            loose: Session::None,
            loose_kernel: None,
            runs: HashMap::new(),
            console: Vec::new(),
            console_session: None,
            console_input,
            console_scroll: ScrollHandle::new(),
            vars: Vars::default(),
            right_tab: RightTab::Console,
            terminal: None,
            terminal_problem: None,
            terminal_starting: false,
            plots: HashMap::new(),
            pages: HashMap::new(),
            parked: Vec::new(),
            viewer_index: None,
            notebook_scroll: ScrollHandle::new(),
            page_wheel,
            explorer: None,
            explorer_shown: false,
            problem: None,
            show_files: true,
            show_right: true,
            files_width: 240.,
            right_width: 460.,
            variables_height: 250.,
            dragging: None,
            editing_host: false,
            _subscriptions: subscriptions,
        };
        this.connect(target, project, cx);
        this
    }

    pub(crate) fn daemon(&self) -> Option<Rc<Daemon>> {
        match &self.link {
            Link::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    /// Connects to the daemon on `target`, leaving any earlier one.
    pub(crate) fn connect(
        &mut self,
        target: Target,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.open.as_ref().is_some_and(|o| o.dirty) {
            self.save(cx);
        }
        self.generation += 1;
        let generation = self.generation;
        self.target = target.clone();
        self.link = Link::Connecting;
        self.about = None;
        self.kernels.clear();
        self.recent.clear();
        self.project = None;
        self.tree = Tree::default();
        self.open = None;
        self.others.clear();
        self.tabs.clear();
        self.loose = Session::None;
        self.runs.clear();
        self.console.clear();
        self.console_session = None;
        self.vars = Vars::default();
        self.terminal = None;
        self.terminal_problem = None;
        self.terminal_starting = false;
        self.problem = None;
        let connect = Daemon::connect(self.rt.clone(), target);
        cx.spawn(async move |this, cx| {
            let result = connect.await;
            let streams = this
                .update(cx, |this, cx| {
                    if this.generation != generation {
                        return None;
                    }
                    let streams = this.connected(result, project, cx);
                    cx.notify();
                    streams
                })
                .ok()
                .flatten();
            let Some(Streams {
                mut events,
                mut terminals,
            }) = streams
            else {
                return;
            };
            let terminal_this = this.clone();
            cx.spawn(async move |cx| {
                while let Some(event) = terminals.recv().await {
                    let alive = terminal_this.update(cx, |this, cx| {
                        if this.generation == generation {
                            this.on_terminal_event(event, cx)
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
            .detach();
            while let Some(event) = events.recv().await {
                let alive = this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.on_event(event, cx)
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.link = Link::Failed("The connection to sandhilld ended".into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn connected(
        &mut self,
        result: anyhow::Result<(Daemon, Streams)>,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<Streams> {
        let (daemon, streams) = match result {
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
        let recent = daemon.call::<Vec<ProjectInfo>>(Call::ListRecentProjects);
        cx.spawn(async move |this, cx| {
            let ping = ping.await;
            let kernels = kernels.await;
            let recent = recent.await;
            this.update(cx, |this, cx| {
                match ping {
                    Ok(ping) => this.about = Some(ping),
                    Err(err) => this.problem = Some(format!("{err:#}")),
                }
                match kernels {
                    Ok(kernels) => this.kernels = kernels,
                    Err(err) => this.problem = Some(format!("Listing kernels failed: {err:#}")),
                }
                this.recent = recent.unwrap_or_default();
                cx.notify();
            })
            .ok();
        })
        .detach();
        if let Some(project) = project {
            self.open_project(project, cx);
        }
        Some(streams)
    }

    pub(crate) fn choose_project(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn open_project(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        // A folder typed for a server can start with ~.
        let path = match (path.strip_prefix('~'), &self.about) {
            (Some(rest), Some(about)) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", about.home)
            }
            _ => path,
        };
        let opened = daemon.call::<ProjectInfo>(Call::OpenProject { path: path.clone() });
        cx.spawn(async move |this, cx| {
            let opened = opened.await;
            this.update(cx, |this, cx| {
                match opened {
                    Ok(project) if !project.exists => {
                        this.problem = Some(format!("{} doesn't exist", project.path))
                    }
                    Ok(project) => {
                        this.close_project(cx);
                        this.project = Some(project.path);
                        this.read_folder(String::new(), cx);
                        this.problem = None;
                        if this.right_tab == RightTab::Terminal {
                            this.ensure_terminal(cx);
                        }
                    }
                    Err(err) => this.problem = Some(format!("Opening the project failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the folders shown in the files pane again.
    pub(crate) fn refresh_files(&mut self, cx: &mut Context<Self>) {
        for dir in self.tree.loaded() {
            self.read_folder(dir, cx);
        }
    }

    /// Reads one of the project's folders ("" for the project itself) into
    /// the tree.
    fn read_folder(&mut self, dir: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.tree
            .folders
            .entry(dir.clone())
            .or_insert(Folder::Loading);
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: tree::join(&project, &dir),
            sizes: false,
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                if this.project.as_deref() != Some(&project) {
                    return;
                }
                let folder = match listed {
                    Ok(entries) => Folder::Ready(entries),
                    Err(err) => Folder::Failed(format!("{err:#}")),
                };
                this.tree.folders.insert(dir, folder);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens or closes a folder in the files pane.
    pub(crate) fn toggle_folder(&mut self, rel: String, cx: &mut Context<Self>) {
        if !self.tree.open.remove(&rel) {
            self.tree.open.insert(rel.clone());
            if !matches!(self.tree.folders.get(&rel), Some(Folder::Ready(_))) {
                self.read_folder(rel, cx);
            }
        }
        cx.notify();
    }

    /// Leaves the project: its notebooks are saved and closed (their
    /// kernels keep running), and its terminal is let go.
    pub(crate) fn close_project(&mut self, cx: &mut Context<Self>) {
        while self.open.is_some() {
            self.close_notebook(cx);
        }
        self.explorer = None;
        self.explorer_shown = false;
        self.project = None;
        self.tree = Tree::default();
        self.terminal = None;
        cx.notify();
    }

    /// The open notebook with this path, shown or in another tab.
    fn open_at(&mut self, path: &str) -> Option<&mut Open> {
        if self.open.as_ref().is_some_and(|o| o.notebook.path == path) {
            return self.open.as_mut();
        }
        self.others.iter_mut().find(|o| o.notebook.path == path)
    }

    /// Shows the notebook open in another tab.
    pub(crate) fn show_file_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.explorer_shown = false;
        if self.open.as_ref().is_some_and(|o| o.notebook.path == path) {
            cx.notify();
            return;
        }
        let Some(ix) = self.others.iter().position(|o| o.notebook.path == path) else {
            return;
        };
        let next = self.others.remove(ix);
        self.put_away();
        self.notebook_scroll.set_offset(next.scroll);
        self.open = Some(next);
        self.session_changed(cx);
        cx.notify();
    }

    /// Whether the notebook in this tab has edits not saved.
    pub(crate) fn tab_dirty(&self, path: &str) -> bool {
        self.open
            .iter()
            .chain(&self.others)
            .any(|o| o.notebook.path == path && o.dirty)
    }

    /// Moves the shown notebook to the other tabs.
    fn put_away(&mut self) {
        if let Some(mut open) = self.open.take() {
            open.scroll = self.notebook_scroll.offset();
            self.others.push(open);
        }
    }

    /// Closes a tab, saving its notebook first if it was edited.
    pub(crate) fn close_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.show_file_tab(path, cx);
        self.close_notebook(cx);
    }

    pub(crate) fn open_file(&mut self, rel: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let path = format!("{project}/{rel}");
        if self.open_at(&path).is_some() {
            self.show_file_tab(&path, cx);
            return;
        }
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
                        if this.open_at(&nb.path).is_some() {
                            return;
                        }
                        this.put_away();
                        this.explorer_shown = false;
                        this.tabs.push(nb.path.clone());
                        this.notebook_scroll.set_offset(Point::default());
                        this.show_notebook(nb, mtime_ms, window, cx);
                        match saved {
                            Ok(saved) => this.show_saved_outputs(saved, &sessions, cx),
                            Err(err) => {
                                this.problem =
                                    Some(format!("Reading saved outputs failed: {err:#}"))
                            }
                        }
                        this.session_changed(cx);
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
        let code_language = editor_language(nb.language.as_deref());
        let mut subscriptions = Vec::new();
        let path = nb.path.clone();
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
                        // The notebook scrolls, not the cell, and a cell is as
                        // tall as its lines.
                        .scroll_beyond_last_line(Some(0))
                        .soft_wrap(false)
                        .default_value(source)
                });
                let path = path.clone();
                subscriptions.push(cx.subscribe(
                    &editor,
                    move |this, _, event: &InputEvent, cx| {
                        if let InputEvent::Change = event
                            && let Some(open) = this.open_at(&path)
                        {
                            open.dirty = true;
                            cx.notify();
                        }
                    },
                ));
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
            scroll: Point::default(),
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
            self.runs.insert(
                cell.run.clone(),
                Run {
                    code: cell.code,
                    cell: Some((saved.file.clone(), cell.cell_id.clone())),
                    outputs: cell.outputs,
                    finished: cell.finished_ms.is_some(),
                    status: cell.status,
                    execution_count: cell.execution_count,
                    ..Default::default()
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
    pub(crate) fn close_notebook(&mut self, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| o.dirty) {
            self.save(cx);
        }
        if let Some(closed) = self.open.take() {
            let ix = self.tabs.iter().position(|p| *p == closed.notebook.path);
            self.tabs.retain(|p| *p != closed.notebook.path);
            // The tab that was next to it, as browsers do.
            let next = ix
                .and_then(|ix| self.tabs.get(ix.min(self.tabs.len().saturating_sub(1))))
                .cloned();
            if let Some(next) = next
                && let Some(i) = self.others.iter().position(|o| o.notebook.path == next)
            {
                let next = self.others.remove(i);
                self.notebook_scroll.set_offset(next.scroll);
                self.open = Some(next);
            }
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
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
                        if let Some(open) = this.open_at(&path) {
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

    /// The kernel the Console and Variables follow: the open notebook's,
    /// else the Console's own.
    pub(crate) fn active_session(&self) -> Option<&str> {
        match &self.open {
            Some(open) => open.session.id(),
            None => self.loose.id(),
        }
    }

    pub(crate) fn active_kernel(&self) -> Option<&str> {
        match &self.open {
            Some(open) => open.kernel.as_deref(),
            None => self.loose_kernel.as_deref(),
        }
    }

    pub(crate) fn active_state(&self) -> &Session {
        match &self.open {
            Some(open) => &open.session,
            None => &self.loose,
        }
    }

    /// Follows a change of the active kernel: the Console shows its runs
    /// and Variables its variables.
    fn session_changed(&mut self, cx: &mut Context<Self>) {
        let active = self.active_session().map(str::to_string);
        let language = self
            .active_kernel()
            .and_then(|k| self.kernels.iter().find(|s| s.name == k))
            .map(|s| s.language.to_ascii_lowercase());
        let language = editor_language(language.as_deref());
        if self.console_input.read(cx).language_name() != language {
            self.console_input
                .update(cx, |input, cx| input.set_highlighter(language, cx));
        }
        if active == self.console_session {
            return;
        }
        self.console_session = active.clone();
        self.console.clear();
        self.vars = Vars::default();
        self.viewer_index = None;
        if self
            .explorer
            .as_ref()
            .is_some_and(|e| Some(e.read(cx).session()) != active.as_deref())
        {
            self.explorer = None;
            self.explorer_shown = false;
        }
        let (Some(daemon), Some(session)) = (self.daemon(), active) else {
            cx.notify();
            return;
        };
        let history = daemon.call::<SessionRuns>(Call::SessionRuns {
            session: session.clone(),
            limit: Some(50),
        });
        cx.spawn(async move |this, cx| {
            let Ok(history) = history.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.console_session.as_deref() != Some(&session) {
                    return;
                }
                let mut ids = Vec::new();
                let mut outputs = Vec::new();
                for run in history.runs {
                    ids.push(run.run.clone());
                    outputs.extend(run.outputs.iter().cloned());
                    this.runs.entry(run.run).or_insert_with(|| Run {
                        code: run.code,
                        cell: run.cell.map(|c| (c.file, c.cell_id)),
                        by: run.by,
                        outputs: run.outputs,
                        finished: run.finished_ms.is_some(),
                        status: run.status,
                        execution_count: run.execution_count,
                        ..Default::default()
                    });
                }
                // Runs that started while the history was on its way stay.
                ids.retain(|id| !this.console.contains(id));
                ids.append(&mut this.console);
                this.console = ids;
                this.console_scroll.scroll_to_bottom();
                this.fetch_plots(&outputs, cx);
                this.refresh_variables(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn choose_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        let daemon = self.daemon();
        let (kernel, session) = match self.open.as_mut() {
            Some(open) => (&mut open.kernel, &mut open.session),
            None => (&mut self.loose_kernel, &mut self.loose),
        };
        if kernel.as_deref() == Some(&name) {
            return;
        }
        if let (Some(daemon), Some(id)) = (daemon, session.id()) {
            daemon.fire(Call::Shutdown {
                session: id.to_string(),
            });
        }
        *session = Session::None;
        *kernel = Some(name);
        if let Some(open) = self.open.as_mut() {
            open.waiting.clear();
            open.sending.clear();
        }
        self.session_changed(cx);
        cx.notify();
    }

    /// Starts the Console's own kernel (when no notebook is open).
    pub(crate) fn start_console_kernel(&mut self, cx: &mut Context<Self>) {
        if self.open.is_some() {
            return;
        }
        if self.loose_kernel.is_none() {
            self.loose_kernel = self.kernels.first().map(|k| k.name.clone());
        }
        self.start_session(cx);
    }

    pub(crate) fn run_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.notebook.cells.get(index).map(|c| c.kind) != Some(CellKind::Code) {
            return;
        }
        match &open.session {
            Session::Ready(_) => {
                let path = open.notebook.path.clone();
                self.execute(&path, index, cx)
            }
            Session::Starting => open.waiting.push(index),
            Session::None | Session::Failed(_) => {
                open.waiting.push(index);
                self.start_session(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn run_all(&mut self, cx: &mut Context<Self>) {
        let count = self.open.as_ref().map_or(0, |o| o.notebook.cells.len());
        for index in 0..count {
            self.run_cell(index, cx);
        }
    }

    /// Runs the cell whose editor has focus, then moves to the next code
    /// cell, as Shift+Enter does in Jupyter.
    pub(crate) fn run_and_advance(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    /// Starts the active kernel: the notebook's (in its folder) or the
    /// Console's (in the project folder).
    fn start_session(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let notebook_path = self.open.as_ref().map(|o| o.notebook.path.clone());
        let (kernel, session, cwd) = match self.open.as_mut() {
            Some(open) => (
                open.kernel.clone(),
                &mut open.session,
                open.notebook.folder().to_string(),
            ),
            None => (self.loose_kernel.clone(), &mut self.loose, project.clone()),
        };
        let Some(kernel) = kernel else {
            *session = Session::Failed("No kernel is installed".into());
            if let Some(open) = self.open.as_mut() {
                open.waiting.clear();
            }
            return;
        };
        *session = Session::Starting;
        let started = daemon.call::<SessionInfo>(Call::StartSession {
            kernel,
            cwd: Some(cwd),
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
                // The notebook may be in another tab by now.
                let slot = match &notebook_path {
                    Some(path) => match this.open_at(path) {
                        Some(open) => &mut open.session,
                        None => return,
                    },
                    None => &mut this.loose,
                };
                match session {
                    Ok(id) => {
                        *slot = Session::Ready(id);
                        if let Some(path) = &notebook_path {
                            let waiting = this
                                .open_at(path)
                                .map(|o| std::mem::take(&mut o.waiting))
                                .unwrap_or_default();
                            for index in waiting {
                                this.execute(path, index, cx);
                            }
                        }
                        this.session_changed(cx);
                    }
                    Err(err) => {
                        *slot = Session::Failed(format!("{err:#}"));
                        if let Some(open) = notebook_path.as_deref().and_then(|p| this.open_at(p)) {
                            open.waiting.clear();
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a cell of the notebook with this path, shown or not.
    fn execute(&mut self, path: &str, index: usize, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(open) = self.open_at(path) else {
            return;
        };
        let (Some(session), Some(cell), Some(editor)) = (
            open.session.id(),
            open.notebook.cells.get(index),
            open.editors.get(index),
        ) else {
            return;
        };
        let cell_id = cell.id.clone();
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
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
                let Some(open) = this.open_at(&path) else {
                    return;
                };
                open.sending.remove(&cell_id);
                match started {
                    Ok(run) => {
                        open.cell_runs.insert(cell_id, run.run.clone());
                        this.runs.entry(run.run).or_default();
                    }
                    Err(err) => this.problem = Some(format!("Running the cell failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs what is typed in the Console in the active kernel.
    pub(crate) fn run_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.console_input.read(cx).value().to_string();
        if code.trim().is_empty() {
            return;
        }
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.active_session().map(str::to_string))
        else {
            self.problem = Some("Start a kernel first (the buttons above the Console).".into());
            cx.notify();
            return;
        };
        self.console_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session,
            code,
            cell: None,
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = started.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Running the code failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        self.problem = None;
    }

    pub(crate) fn interrupt(&mut self) {
        if let (Some(daemon), Some(session)) = (self.daemon(), self.active_session()) {
            daemon.fire(Call::Interrupt {
                session: session.to_string(),
            });
        }
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        // Any open notebook's kernel, shown or in another tab, or the
        // Console's own.
        let ours = self
            .open
            .iter()
            .chain(&self.others)
            .any(|o| o.session.id() == Some(event.session.as_str()))
            || self.loose.id() == Some(event.session.as_str());
        if !ours {
            return;
        }
        let active = self.active_session() == Some(event.session.as_str());
        if event.msg_type == EVENT_SESSION_ENDED {
            for run in self.runs.values_mut().filter(|r| !r.finished) {
                run.finished = true;
                run.status = Some("the kernel stopped".into());
            }
            let ended = || Session::Failed("The kernel stopped".into());
            for open in self.open.iter_mut().chain(&mut self.others) {
                if open.session.id() == Some(event.session.as_str()) {
                    open.session = ended();
                }
            }
            if self.loose.id() == Some(event.session.as_str()) {
                self.loose = ended();
            }
            cx.notify();
            return;
        }
        // Events can arrive before the run id does, so keep them by run.
        let Some(run_id) = event.run else {
            return;
        };
        if active && !event.msg_type.starts_with("status") {
            self.console_scroll.scroll_to_bottom();
        }
        let run = self.runs.entry(run_id.clone()).or_default();
        match event.msg_type.as_str() {
            EVENT_RUN_STARTED => {
                let content = &event.content;
                run.code = content["code"].as_str().unwrap_or_default().to_string();
                run.quiet = content["quiet"].as_bool().unwrap_or(false);
                run.by = content["by"].as_str().map(str::to_string);
                run.cell = match (
                    content["cell"]["file"].as_str(),
                    content["cell"]["cell_id"].as_str(),
                ) {
                    (Some(file), Some(id)) => Some((file.to_string(), id.to_string())),
                    _ => None,
                };
                if !run.quiet && active && !self.console.contains(&run_id) {
                    self.console.push(run_id);
                    if self.console.len() > CONSOLE_RUNS {
                        self.console.remove(0);
                    }
                }
            }
            EVENT_RUN_FINISHED => {
                run.finished = true;
                run.status = event.content["status"].as_str().map(str::to_string);
                run.execution_count = event.content["execution_count"].as_u64();
                if !run.quiet && active {
                    self.refresh_variables(cx);
                }
            }
            _ if run.quiet => {}
            msg_type => {
                let is_plot = matches!(
                    msg_type,
                    "display_data" | "execute_result" | "update_display_data"
                );
                run.events.push((event.msg_type, event.content));
                let mut builder = OutputBuilder::new();
                for (msg_type, content) in &run.events {
                    builder.push(msg_type, content);
                }
                run.outputs = builder.finish();
                if is_plot {
                    let for_viewer = run.cell.as_ref().is_none_or(|(file, _)| is_script(file));
                    let outputs = run.outputs.clone();
                    let page = outputs.iter().any(|o| matches!(show(o), Shown::Page(_)));
                    if page && for_viewer && active {
                        self.right_tab = RightTab::Viewer;
                        self.viewer_index = None;
                    }
                    self.fetch_plots(&outputs, cx);
                }
            }
        }
        cx.notify();
    }

    /// Lists the active kernel's variables again, and the parts open.
    pub(crate) fn refresh_variables(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.active_session().map(str::to_string))
        else {
            return;
        };
        self.vars.session = Some(session.clone());
        self.vars.loading = true;
        let mut paths = vec![Vec::new()];
        paths.extend(self.vars.expanded.iter().cloned());
        for path in paths {
            let listed = daemon.call::<Variables>(Call::ListVariables {
                session: session.clone(),
                path: path.clone(),
            });
            let session = session.clone();
            cx.spawn(async move |this, cx| {
                let listed = listed.await;
                this.update(cx, |this, cx| {
                    if this.vars.session.as_deref() != Some(&session) {
                        return;
                    }
                    match listed {
                        Ok(listed) if path.is_empty() => {
                            this.vars.list = listed.variables;
                            this.vars.loading = false;
                            this.vars.error = None;
                        }
                        Ok(listed) => {
                            this.vars.children.insert(path, listed.variables);
                        }
                        // Busy with other code: the list updates when it ends.
                        Err(err) if path.is_empty() => {
                            this.vars.loading = false;
                            if !format!("{err:#}").contains("busy") {
                                this.vars.error = Some(format!("{err:#}"));
                            }
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn toggle_variable(&mut self, path: Vec<String>, cx: &mut Context<Self>) {
        if self.vars.expanded.remove(&path) {
            cx.notify();
            return;
        }
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            return;
        };
        self.vars.expanded.insert(path.clone());
        let listed = daemon.call::<Variables>(Call::ListVariables {
            session,
            path: path.clone(),
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                match listed {
                    Ok(listed) => {
                        this.vars.children.insert(path, listed.variables);
                    }
                    Err(err) => {
                        this.vars.expanded.remove(&path);
                        this.problem = Some(format!("Listing its parts failed: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn show_tab(&mut self, tab: RightTab, window: &mut Window, cx: &mut Context<Self>) {
        self.right_tab = tab;
        if tab == RightTab::Terminal {
            self.ensure_terminal(cx);
            if let Some(terminal) = &self.terminal {
                let focus = terminal.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    /// The Viewer's pages: those the Console's runs (and scripts') showed,
    /// oldest first, with their titles. A notebook's pages stay under its
    /// cells.
    pub(crate) fn viewer_pages(&self) -> Vec<(Page, String)> {
        self.console
            .iter()
            .filter_map(|id| self.runs.get(id))
            .filter(|run| run.cell.as_ref().is_none_or(|(file, _)| is_script(file)))
            .flat_map(|run| run.outputs.iter())
            .filter_map(|output| match show(output) {
                Shown::Page(page) => Some((page, page_title(output))),
                _ => None,
            })
            .collect()
    }

    /// The Viewer's page now, with its place and how many there are.
    pub(crate) fn viewer_page(&self) -> Option<(Page, String, usize, usize)> {
        let pages = self.viewer_pages();
        let count = pages.len();
        let ix = self
            .viewer_index
            .unwrap_or(count.checked_sub(1)?)
            .min(count - 1);
        let (page, title) = pages.into_iter().nth(ix)?;
        Some((page, title, ix, count))
    }

    /// Keeps a web view for each page about to be drawn, and drops the rest:
    /// a web view is a native view over the window, so one left over would
    /// stay on screen.
    pub(crate) fn sync_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut wanted: Vec<Page> = Vec::new();
        if self.explorer.is_none()
            && let Some(open) = &self.open
        {
            for (ix, cell) in open.notebook.cells.iter().enumerate() {
                if cell.kind != CellKind::Code {
                    continue;
                }
                let run = open
                    .cell_runs
                    .get(&cell.id)
                    .and_then(|id| self.runs.get(id));
                let outputs = run.map_or(&open.notebook.cells[ix].outputs, |r| &r.outputs);
                wanted.extend(outputs.iter().filter_map(|o| match show(o) {
                    Shown::Page(page) => Some(page),
                    _ => None,
                }));
            }
        }
        if self.right_tab == RightTab::Viewer
            && let Some((page, ..)) = self.viewer_page()
        {
            wanted.push(page);
        }
        let keys: HashSet<String> = wanted.iter().map(Page::key).collect();
        // Pages no longer drawn are parked, hidden, and come back when they
        // are drawn again (a cell run again with the same output). Dropping
        // a web view soon after it was drawn can end the app in X11
        // (BadWindow), so only the oldest parked ones go, a while later.
        let gone: Vec<(String, Entity<PageView>)> = self
            .pages
            .extract_if(|key, _| !keys.contains(key))
            .collect();
        for (key, view) in gone {
            view.update(cx, |page, cx| page.hide(cx));
            self.parked.push((key, view));
        }
        for key in &keys {
            if self.pages.contains_key(key) {
                continue;
            }
            if let Some(ix) = self.parked.iter().position(|(k, _)| k == key) {
                let (key, view) = self.parked.remove(ix);
                view.update(cx, |page, cx| page.show(cx));
                self.pages.insert(key, view);
            }
        }
        if self.parked.len() > PARKED_PAGES {
            let old: Vec<_> = self
                .parked
                .drain(..self.parked.len() - PARKED_PAGES)
                .collect();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                drop(old);
            })
            .detach();
        }
        let remote = self.target != Target::Local;
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        let in_notebook = wanted.len()
            - usize::from(self.right_tab == RightTab::Viewer && self.viewer_page().is_some());
        for (ix, page) in wanted.into_iter().enumerate() {
            let key = page.key();
            if self.pages.contains_key(&key) {
                continue;
            }
            let daemon = self.daemon();
            let near = near.clone();
            let wheel = (ix < in_notebook).then(|| self.page_wheel.clone());
            let view = cx.new(|cx| PageView::new(page, daemon, remote, near, wheel, window, cx));
            self.pages.insert(key, view);
        }
    }

    pub(crate) fn show_viewer_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.viewer_index = Some(ix);
        cx.notify();
    }

    /// Opens a data frame or matrix from the Variables pane in the Data
    /// Explorer.
    pub(crate) fn open_explorer(&mut self, path: Vec<String>, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            return;
        };
        self.explorer = Some(cx.new(|cx| Explorer::new(daemon, session, path, cx)));
        self.explorer_shown = true;
        cx.notify();
    }

    pub(crate) fn close_explorer(&mut self, cx: &mut Context<Self>) {
        self.explorer = None;
        self.explorer_shown = false;
        cx.notify();
    }

    /// Finds the project's shell terminal, or starts one, and follows it.
    fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.is_some() || self.terminal_starting {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.terminal_starting = true;
        self.terminal_problem = None;
        let listed = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let existing = listed
                .await
                .unwrap_or_default()
                .into_iter()
                .rev()
                .find(|t| {
                    t.kind == TERMINAL_SHELL && t.running && t.project.as_deref() == Some(&project)
                });
            let info = match existing {
                Some(info) => Ok(info),
                None => {
                    daemon
                        .call::<TerminalInfo>(Call::StartTerminal {
                            kind: TERMINAL_SHELL.into(),
                            cwd: Some(project.clone()),
                            project: Some(project),
                            cols: Some(80),
                            rows: Some(24),
                            options: Default::default(),
                            prompt: None,
                        })
                        .await
                }
            };
            let subscribed = match &info {
                Ok(info) => daemon
                    .call::<TerminalSubscribed>(Call::SubscribeTerminal {
                        terminal: info.id.clone(),
                        from: None,
                        chat_from: None,
                    })
                    .await
                    .map(|_| ()),
                Err(_) => Ok(()),
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.terminal_starting = false;
                match info.and_then(|info| subscribed.map(|_| info)) {
                    Ok(info) => {
                        let id = info.id.clone();
                        let input = daemon.ordered({
                            let id = id.clone();
                            move |data| Call::TerminalInput {
                                terminal: id.clone(),
                                data,
                            }
                        });
                        let resize = {
                            let daemon = daemon.clone();
                            let id = id.clone();
                            Box::new(move |cols, rows| {
                                daemon.fire(Call::ResizeTerminal {
                                    terminal: id.clone(),
                                    cols,
                                    rows,
                                })
                            })
                        };
                        let view = cx.new(|cx| TerminalView::new(id, input, resize, cx));
                        if this.right_tab == RightTab::Terminal {
                            let focus = view.read(cx).focus_handle();
                            window.focus(&focus, cx);
                        }
                        this.terminal = Some(view);
                    }
                    Err(err) => this.terminal_problem = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn on_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        let Some(view) = self.terminal.clone() else {
            return;
        };
        if view.read(cx).id != event.terminal {
            return;
        }
        view.update(cx, |view, cx| {
            if let Some(data) = &event.data {
                view.feed(data, cx);
            }
            if let Some(exit) = &event.exit {
                view.exited(format!("ended: {exit:?}"), cx);
            }
        });
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

/// The highlighter for a kernel language.
fn editor_language(language: Option<&str>) -> &'static str {
    match language {
        Some("python") => "python",
        Some("r") => "r",
        _ => "text",
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

pub(crate) fn plot_key(value: &Value) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    match value[BLOB_KEY].as_str() {
        Some(hash) => hash.to_string(),
        None => {
            let mut hasher = DefaultHasher::new();
            value.to_string().hash(&mut hasher);
            format!("inline-{:x}", hasher.finish())
        }
    }
}

/// A script's file (`.R`, `.py`), whose runs are the Console's.
pub(crate) fn is_script(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".r") || lower.ends_with(".py")
}
