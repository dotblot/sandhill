//! The window's state and what it does: the connection, the project, the
//! open notebook, the kernel's runs, the Console, Variables and Terminal.
//! Drawing is in `view.rs`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::{EditorState, InputEvent, InputState, Position};
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{
    BLOB_KEY, Blob, CONFLICT_ERROR, Call, CellRef, DirEntry, EVENT_RUN_FINISHED, EVENT_RUN_STARTED,
    EVENT_SESSION_ENDED, Event, FileOutputs, FileText, FileWritten, KernelspecInfo, PingReply,
    ProjectInfo, RunStarted, SessionInfo, SessionStatus, Subscribed, TERMINAL_SHELL, TerminalEvent,
    TerminalInfo, TerminalSubscribed, Variable, Variables,
};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::agents::Agents;
use crate::daemon::{Daemon, Streams};
use crate::explorer::Explorer;
use crate::notebook::{self, CellKind, Format, Notebook};
use crate::pages::PageView;
use crate::shown::{Page, Shown, page_title, show};
use crate::statements;
use crate::terminal::TerminalView;
use crate::tree::{self, Folder, Tree};

#[path = "console.rs"]
pub(crate) mod console;
mod panels;
mod runs;
mod tabs;

/// Web pages kept hidden for drawing again.
const PARKED_PAGES: usize = 24;
/// Runs the Console keeps.
const CONSOLE_RUNS: usize = 200;

pub(crate) enum Link {
    Connecting,
    Failed(String),
    Ready(Rc<Daemon>),
}

#[derive(Clone, Default)]
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
    /// Session language retained even if its kernel later stops.
    pub language: Option<&'static str>,
    code_highlights: RefCell<Option<console::HighlightedCode>>,
    /// The notebook cell it came from: (file, cell id).
    pub cell: Option<(String, String)>,
    /// Who ran it when it wasn't the user (an agent).
    pub by: Option<String>,
    /// Code a tool ran to look around (Variables); not shown.
    pub quiet: bool,
    events: BTreeMap<i64, (String, Value)>,
    pub outputs: Vec<Value>,
    pub finished: bool,
    pub status: Option<String>,
    pub execution_count: Option<u64>,
    /// The kernel session it ran in.
    pub session: String,
    /// When it was sent, began (the kernel runs one thing at a time) and
    /// finished, by the daemon's clock.
    pub queued_ms: Option<i64>,
    pub began_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    /// When it began by this machine's clock, for the time shown while it
    /// runs.
    pub began_at: Option<std::time::Instant>,
    /// Last event applied, and the journal snapshot shown while replay catches up.
    last_seq: i64,
    snapshot_seq: i64,
    /// The output event prefix independently reconstructed from the journal.
    output_replay_seq: i64,
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
    /// Standalone Markdown stays source-first; preview reads the editor value.
    pub(crate) markdown_preview: bool,
    pub(crate) md_images: crate::md_images::Cache,
    /// Notebook editors stay alive while the whole Quarto source is shown.
    pub(crate) quarto_view: Option<QuartoView>,
    pub(crate) delimited_header: Option<Entity<EditorState>>,
    pub(crate) mtime_ms: i64,
    pub kernel: Option<String>,
    pub session: Session,
    /// Cells (by id) waiting for the kernel to start.
    pub waiting: Vec<String>,
    /// Cells sent to the daemon whose run id hasn't come back yet.
    pub sending: HashSet<String>,
    /// Each cell's latest run.
    pub cell_runs: HashMap<String, String>,
    /// Where the notebook was scrolled to while another tab was shown.
    scroll: Point<Pixels>,
    /// Text cells (by id) being edited; the others show their Markdown
    /// formatted.
    pub editing: HashSet<String>,
    /// Cells (by id) whose outputs are folded away.
    pub collapsed: HashSet<String>,
    /// Cells deleted, latest last, with where they were and their text,
    /// for bringing them back.
    pub deleted: Vec<(usize, notebook::Cell, String)>,
    /// Cells (by id) an agent changed, until the user keeps or undoes it.
    pub agent_changes: HashMap<String, crate::cell_ai::AgentChange>,
    /// Cells an agent deleted, shown where they were with Undo.
    pub agent_deleted: Vec<crate::cell_ai::AgentDeleted>,
    /// The cell an agent just added or changed, lit up for a moment.
    pub lit: Option<(String, std::time::Instant)>,
    /// The suggested next cell, and the cell the user ran last, which gets
    /// one when its run ends well.
    pub ghost: Option<crate::ghost::Ghost>,
    pub ghost_after: Option<String>,
    /// A script's next journal slot (`console-<n>`).
    next_slot: usize,
    /// A script's code waiting for the kernel to start.
    queued: Vec<String>,
    /// The cell last edited or clicked into: where the user is.
    pub current: Option<usize>,
    pub(crate) _subscriptions: Vec<Subscription>,
}

pub(crate) struct QuartoView {
    editors: Vec<Entity<EditorState>>,
    current: Option<usize>,
    scroll: Point<Pixels>,
    ids: Vec<String>,
}

impl Open {
    /// The code cells as the # menu names them, with their text as edited.
    pub(crate) fn code_cells(&self, cx: &App) -> Vec<crate::composer::DocCell> {
        let mut chunks = 0;
        self.notebook
            .cells
            .iter()
            .zip(&self.editors)
            .enumerate()
            .filter(|(_, (cell, _))| cell.kind == CellKind::Code)
            .map(|(i, (cell, editor))| {
                chunks += 1;
                let name = match self.notebook.format {
                    Format::Ipynb => format!("cell {}", i + 1),
                    _ if cell.id.starts_with("chunk-") => format!("chunk {chunks}"),
                    _ => cell.id.clone(),
                };
                crate::composer::DocCell {
                    id: cell.id.clone(),
                    name,
                    code: editor.read(cx).value().to_string(),
                }
            })
            .collect()
    }

    pub(crate) fn is_script(&self) -> bool {
        self.notebook.format == Format::Script
    }

    pub(crate) fn single_editor(&self) -> bool {
        self.notebook.single_editor()
    }

    fn sources(&self, cx: &App) -> Vec<String> {
        self.editors
            .iter()
            .map(|e| e.read(cx).value().to_string())
            .collect()
    }
}

/// What a cell's editor needs to know about its document.
pub(crate) struct DocKind {
    pub path: String,
    pub language: Option<String>,
    pub format: Format,
    pub script: bool,
}

impl DocKind {
    pub(crate) fn of(nb: &Notebook) -> Self {
        DocKind {
            path: nb.path.clone(),
            language: nb.language.clone(),
            format: nb.format,
            script: nb.single_editor(),
        }
    }
}

/// A gap between panes that can be dragged to resize them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Split {
    /// Between the left panel and the middle.
    Files,
    /// Under Variables, in the left panel.
    Variables,
    /// Between the middle and the AI pane.
    Agents,
    /// Above the Console, under the editor.
    Console,
}

/// The tabs of the pane under the editor, as in the Tauri app.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsoleTab {
    Console,
    Terminal,
    Jobs,
}

/// The left panel's top tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopTab {
    Variables,
    Kernels,
    Help,
    Packages,
}

/// The left panel's second tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BottomTab {
    Files,
    Git,
    Plots,
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
    pub(crate) rt: Arc<Runtime>,
    /// Counts connections, so a late reply for an old one is dropped.
    pub(crate) generation: u64,
    /// This window, for agents' edits that arrive from the daemon.
    pub(crate) window_handle: AnyWindowHandle,
    /// Where the user is, as last told the daemon (see `agent_edits`).
    pub(crate) focus_sent: Option<sandhill_proto::Focus>,
    pub(crate) focus_generation: u64,
    /// A timer redraws running cells' times.
    pub(crate) ticking: bool,
    pub(crate) target: Target,
    pub(crate) link: Link,
    pub(crate) about: Option<PingReply>,
    pub(crate) kernels: Vec<KernelspecInfo>,
    /// Personal project state from the daemon; preserve unrelated saved keys.
    project_state: Value,
    /// The kernel picker's list is open.
    pub(crate) kernel_menu: bool,
    pub(crate) recent: Vec<ProjectInfo>,
    pub(crate) project: Option<String>,
    /// In-flight document requests. The token rejects replies after close/reopen.
    pub(crate) document_loads: HashMap<String, u64>,
    /// A tab is selectable before its document has arrived from the daemon.
    pub(crate) opening_shown: Option<String>,
    pub(crate) next_document_load: u64,
    /// The project's folders, read as they are opened.
    pub(crate) tree: Tree,
    /// The notebook shown.
    pub(crate) open: Option<Open>,
    /// Notebooks open in other tabs.
    pub(crate) others: Vec<Open>,
    /// The tabs' files, in the order they were opened.
    pub(crate) tabs: Vec<String>,
    /// Quarto presentation preference, including documents closed this session.
    quarto_sources: HashSet<String>,
    pub(crate) media: HashMap<String, Entity<crate::media::MediaView>>,
    pub(crate) media_shown: Option<String>,
    /// The Console's kernel when no notebook is open.
    pub(crate) loose: Session,
    pub(crate) loose_kernel: Option<String>,
    /// Monotonic request token for retained Console starts.
    loose_start: u64,
    pub(crate) console_sessions: console::Sessions,
    /// Every run seen, by id.
    pub(crate) runs: HashMap<String, Run>,
    /// The Console: the active kernel's runs, oldest first.
    pub(crate) console: Vec<String>,
    console_session: Option<String>,
    pub(crate) console_input: Entity<EditorState>,
    pub(crate) language: crate::language::Language,
    pub(crate) console_scroll: ScrollHandle,
    pub(crate) vars: Vars,
    pub(crate) console_tab: ConsoleTab,
    pub(crate) top_tab: TopTab,
    pub(crate) bottom_tab: BottomTab,
    /// The pane under the editor is shown (⌘J).
    pub(crate) show_console: bool,
    pub(crate) console_height: f32,
    /// Normal height shares survive temporary collapse/vertical expansion.
    pub(crate) console_share: f32,
    pub(crate) left_vertical: crate::panes::Vertical,
    pub(crate) middle_vertical: crate::panes::Vertical,
    /// The Plots pane's history.
    pub(crate) plot_history: crate::plots::History,
    pub(crate) help: crate::help::Help,
    pub(crate) packages: crate::packages::Packages,
    pub(crate) jobs: crate::jobs::Jobs,
    pub(crate) irkernel: crate::irkernel::IrkernelOffers,
    pub(crate) git: crate::git_pane::Git,
    pub(crate) diff: Option<crate::git_diff::DiffTab>,
    pub(crate) diff_shown: bool,
    pub(crate) terminal: Option<Entity<TerminalView>>,
    pub(crate) terminal_problem: Option<String>,
    terminal_starting: bool,
    /// A terminal file link waiting for its document to finish opening.
    pending_location: Option<(String, Option<u32>, Option<u32>)>,
    /// Plots by blob hash (or by a hash of an inline value).
    pub(crate) plots: HashMap<String, Plot>,
    /// The web views drawn now, by page key (see `sync_pages`).
    pub(crate) pages: HashMap<String, Entity<PageView>>,
    /// Pages not drawn now, hidden, oldest first.
    parked: Vec<(String, Entity<PageView>)>,
    /// The Viewer's page, counting from the oldest; the latest when none.
    pub(crate) viewer_index: Option<usize>,
    /// The Viewer's own pages: previews and HTML files.
    pub(crate) viewer_extra: Vec<crate::viewer::Extra>,
    /// The notebook's scrolling, and where pages under its cells send the
    /// wheel's turns they don't use.
    pub(crate) notebook_scroll: ScrollHandle,
    pub(crate) notebook_scroll_target: std::cell::Cell<Option<usize>>,
    /// Each cell's output box (by cell id), which scrolls on its own when
    /// its outputs are long.
    pub(crate) output_scrolls: std::cell::RefCell<HashMap<String, ScrollHandle>>,
    /// The notebook's cells take the keys while no cell is being edited.
    pub(crate) cells_focus: FocusHandle,
    /// Where the open notebook's cells were at the last layout, and which
    /// are drawn now: only those near the view (see `place_cells`).
    pub(crate) placed: crate::cell_view::Placed,
    page_wheel: tokio::sync::mpsc::UnboundedSender<f32>,
    /// The Data Explorer, a tab of its own in the middle.
    pub(crate) explorer: Option<Entity<Explorer>>,
    /// Data Explorers moved into windows of their own, which this project's
    /// runs still reach and its closing closes.
    pub(crate) popped_explorers: Vec<(AnyWindowHandle, WeakEntity<Explorer>)>,
    /// The Data Explorer's tab is the one shown.
    pub(crate) explorer_shown: bool,
    pub(crate) objects: crate::object_explorer::ObjectExplorers,
    pub(crate) focus_mode: crate::focus_mode::FocusMode,
    pub(crate) notebook_ai_menu: bool,
    /// The open text file's colouring menu (`colouring.rs`).
    pub(crate) colouring_menu: bool,
    /// The latest problem, shown at the bottom.
    pub(crate) problem: Option<String>,
    /// A short note in the status bar, such as "Saved in …".
    pub(crate) notice: Option<String>,
    /// The project's settings (`.sandhill/project.toml`) and their dialog.
    pub(crate) setup: crate::project_settings::Setup,
    pub(crate) settings_env: Entity<InputState>,
    /// The Variables tab's filter box.
    pub(crate) vars_filter: Entity<InputState>,
    /// Variables' Import menu and dialog.
    pub(crate) import: crate::import_dialog::Import,
    pub(crate) delimited: Option<crate::delimited::DelimitedDialog>,
    pub(crate) popup_triggers: crate::popup::PopupTriggers,
    pub(crate) import_name: Entity<InputState>,
    /// The left panel is shown (the title bar's first button).
    pub(crate) show_files: bool,
    /// Middle-column visibility and full-workspace pane expansion are session-only.
    pub(crate) show_middle: bool,
    pub(crate) maximized_panel: Option<crate::panes::Panel>,
    /// The AI pane, the right panel.
    pub(crate) show_agents: bool,
    pub(crate) agents: Option<Entity<Agents>>,
    /// Whether the open project's folder is trusted (see `trust.rs`).
    pub(crate) trust: crate::trust::FolderTrust,
    /// Account limits belong to the connected device, not an open project.
    pub(crate) quota: Option<Entity<crate::quota::QuotaMonitor>>,
    quota_observation: Option<Subscription>,
    /// The left and right panels' shares of the window's width (a quarter
    /// each; the middle takes the rest), set by dragging the gaps.
    pub(crate) left_share: f32,
    pub(crate) agents_share: f32,
    /// Their widths now, from their shares and the window's width.
    pub(crate) files_width: f32,
    pub(crate) agents_width: f32,
    /// The left panel's top pane, set by dragging the gap under it.
    pub(crate) variables_height: f32,
    /// The left panel's top pane's share of the panel's height (half at
    /// first).
    pub(crate) variables_share: f32,
    /// Whether notebooks suggest the next cell after each run.
    pub(crate) ghost_cells: bool,
    /// An agent's edit is running a cell: no suggestion follows it.
    pub(crate) agent_running: bool,
    /// The gap being dragged.
    pub(crate) dragging: Option<Split>,
    /// Notebooks' runs show in the Console, and their plots in Plots (☰).
    pub(crate) notebook_console: bool,
    pub(crate) notebook_plots: bool,
    /// The files pane's New, Rename and right-click menu.
    pub(crate) files_ui: crate::tree_actions::FilesUi,
    pub(crate) name_input: Entity<InputState>,
    /// ⌘K.
    pub(crate) palette: crate::command_palette::CommandPalette,
    pub(crate) palette_input: Entity<InputState>,
    pub(crate) find: crate::find::Find,
    pub(crate) outline: crate::outline::Outline,
    /// The ☰ menu is open.
    pub(crate) main_menu: bool,
    /// The window's own focus, so its shortcuts work before anything
    /// else is clicked.
    pub(crate) focus: FocusHandle,
    /// The window, with its machine tabs and Start screen.
    pub(crate) shell: WeakEntity<crate::app_window::AppWindow>,
    /// The machine's kernels and resources, and the left pane's menus.
    pub(crate) machine: crate::machine::MachineUi,
    /// The connection's pop-up is open, and the daemon's log dialog.
    pub(crate) connection_menu: bool,
    pub(crate) log: Option<crate::connection::LogView>,
    /// When the connection was made.
    pub(crate) connected_ms: Option<i64>,
    /// The project, kept while disconnected, and the documents to open
    /// again once connected.
    pub(crate) last_project: Option<String>,
    pub(crate) reopen: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(
        rt: Arc<Runtime>,
        target: Target,
        project: Option<String>,
        shell: WeakEntity<crate::app_window::AppWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let console_input = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .line_number(false)
                .folding(false)
                .scroll_beyond_last_line(Some(0))
                .soft_wrap(false)
                .submit_on_enter(true)
                .placeholder("Type code; Enter runs it")
        });
        // The language follows the Console's kernel; brackets follow it.
        crate::rainbow::attach(&console_input, cx);
        let (help, help_subscriptions) = crate::help::new_help(window, cx);
        let (packages, package_subscriptions) = crate::packages::new_packages(window, cx);
        let (jobs, job_subscriptions) = crate::jobs::new_jobs(window, cx);
        let (git, git_subscriptions) = crate::git_pane::new_git(window, cx);
        let (settings_env, settings_subscription) =
            crate::project_settings::new_env_input(window, cx);
        let (import_name, import_subscription) = crate::import_dialog::new_import_input(window, cx);
        let (name_input, name_subscription) = crate::tree_actions::new_name_input(window, cx);
        let (palette_input, palette_subscription) =
            crate::command_palette::new_palette_input(window, cx);
        let (find, find_subscriptions) = crate::find::new_find(window, cx);
        let (outline, outline_subscription) = crate::outline::new_outline(window, cx);
        let vars_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let vars_filter_subscription =
            cx.subscribe(&vars_filter, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        let mut subscriptions = vec![cx.subscribe_in(
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
        )];
        subscriptions.extend(help_subscriptions);
        subscriptions.extend(package_subscriptions);
        subscriptions.extend(job_subscriptions);
        subscriptions.extend(git_subscriptions);
        subscriptions.push(settings_subscription);
        subscriptions.push(import_subscription);
        subscriptions.push(name_subscription);
        subscriptions.push(palette_subscription);
        subscriptions.push(vars_filter_subscription);
        subscriptions.extend(find_subscriptions);
        subscriptions.push(outline_subscription);
        // Quitting saves what was edited first, as closing a tab does.
        subscriptions.push(cx.on_app_quit(|this, cx| {
            let tasks = this.save_all_tasks(cx);
            async move {
                for task in tasks {
                    task.await;
                }
            }
        }));
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
            window_handle: window.window_handle(),
            focus_sent: None,
            focus_generation: 0,
            ticking: false,
            target: target.clone(),
            link: Link::Connecting,
            about: None,
            kernels: Vec::new(),
            project_state: Value::Null,
            kernel_menu: false,
            irkernel: Default::default(),
            recent: Vec::new(),
            project: None,
            document_loads: HashMap::new(),
            opening_shown: None,
            next_document_load: 0,
            delimited: None,
            popup_triggers: Default::default(),
            tree: Tree::new(),
            open: None,
            others: Vec::new(),
            tabs: Vec::new(),
            quarto_sources: HashSet::new(),
            media: HashMap::new(),
            media_shown: None,
            loose: Session::None,
            loose_kernel: None,
            loose_start: 0,
            console_sessions: Default::default(),
            runs: HashMap::new(),
            console: Vec::new(),
            console_session: None,
            console_input,
            language: Default::default(),
            console_scroll: ScrollHandle::new(),
            vars: Vars::default(),
            console_tab: ConsoleTab::Console,
            top_tab: TopTab::Variables,
            bottom_tab: BottomTab::Files,
            show_console: true,
            console_height: 260.,
            console_share: 0.35,
            left_vertical: Default::default(),
            middle_vertical: Default::default(),
            plot_history: Default::default(),
            help,
            packages,
            jobs,
            git,
            diff: None,
            diff_shown: false,
            terminal: None,
            terminal_problem: None,
            terminal_starting: false,
            pending_location: None,
            plots: HashMap::new(),
            pages: HashMap::new(),
            parked: Vec::new(),
            viewer_index: None,
            viewer_extra: Vec::new(),
            notebook_scroll: ScrollHandle::new(),
            notebook_scroll_target: Default::default(),
            output_scrolls: Default::default(),
            placed: Default::default(),
            cells_focus: cx.focus_handle(),
            page_wheel,
            explorer: None,
            popped_explorers: Vec::new(),
            explorer_shown: false,
            objects: Default::default(),
            focus_mode: Default::default(),
            notebook_ai_menu: false,
            colouring_menu: false,
            problem: None,
            notice: None,
            setup: Default::default(),
            settings_env,
            vars_filter,
            import: Default::default(),
            import_name,
            show_files: true,
            show_middle: true,
            maximized_panel: None,
            show_agents: true,
            agents: None,
            trust: Default::default(),
            quota: None,
            quota_observation: None,
            left_share: 0.25,
            agents_share: 0.25,
            files_width: 280.,
            agents_width: 560.,
            variables_height: 250.,
            variables_share: 0.5,
            ghost_cells: crate::prefs::ghost_cells(),
            agent_running: false,
            dragging: None,
            notebook_console: crate::prefs::notebook_console(),
            notebook_plots: crate::prefs::notebook_plots(),
            files_ui: Default::default(),
            name_input,
            palette: Default::default(),
            palette_input,
            find,
            outline,
            main_menu: false,
            focus: cx.focus_handle(),
            shell,
            machine: Default::default(),
            connection_menu: false,
            log: None,
            connected_ms: None,
            last_project: None,
            reopen: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.install_language(&this.console_input.clone(), None, cx);
        this.connect(target, project, cx);
        this
    }

    /// This tab's machine, as the window names it.
    pub(crate) fn host(&self) -> String {
        match &self.target {
            Target::Local => sandhill_link::LOCAL.to_string(),
            Target::Ssh(destination) => destination.to_string(),
        }
    }

    /// Does something to the window, after this event: the window may be
    /// about to change this workbench too.
    pub(crate) fn with_shell(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(
            &mut crate::app_window::AppWindow,
            &mut Window,
            &mut Context<crate::app_window::AppWindow>,
        ) + 'static,
    ) {
        let shell = self.shell.clone();
        window.defer(cx, move |window, cx| {
            shell.update(cx, |shell, cx| f(shell, window, cx)).ok();
        });
    }

    /// Hides the web pages drawn, while another tab is shown: they are
    /// native views over the window. Drawing the tab again shows them.
    pub(crate) fn park_pages(&mut self, cx: &mut Context<Self>) {
        let drawn: Vec<(String, Entity<PageView>)> = self.pages.drain().collect();
        for (key, view) in drawn {
            view.update(cx, |page, cx| page.hide(cx));
            self.parked.push((key, view));
        }
        for view in self.media.values() {
            view.update(cx, |view, _| view.hide(true));
        }
    }

    pub(crate) fn daemon(&self) -> Option<Rc<Daemon>> {
        match &self.link {
            Link::Ready(daemon) => Some(daemon.clone()),
            _ => None,
        }
    }

    /// Leaves the daemon: what was edited is saved, and everything read
    /// from it is let go. Replies still on their way are dropped.
    pub(crate) fn forget_connection(&mut self, cx: &mut Context<Self>) {
        if !crate::resource_manager::host_blocked(&self.host(), cx)
            && let Some(daemon) = self.daemon()
        {
            crate::app_window::quit::retain_daemon(self.host(), daemon, cx);
        }
        if self.project.is_some() {
            self.last_project = self.project.clone();
        }
        for path in self.tabs.clone() {
            if self.open_at(&path).is_some_and(|o| o.dirty)
                && let Some(task) = self.save_file(&path, cx)
            {
                task.detach();
            }
        }
        self.close_language(None, cx);
        self.generation += 1;
        self.about = None;
        self.kernels.clear();
        self.recent.clear();
        self.project = None;
        self.agents = None;
        self.quota = None;
        self.quota_observation = None;
        self.tree = Tree::new();
        self.open = None;
        self.others.clear();
        self.tabs.clear();
        self.media.clear();
        self.media_shown = None;
        self.loose = Session::None;
        self.loose_kernel = None;
        self.console_sessions = Default::default();
        self.runs.clear();
        self.console.clear();
        self.console_session = None;
        self.vars = Vars::default();
        self.forget_jobs();
        self.forget_git();
        self.viewer_extra.clear();
        self.setup = Default::default();
        self.terminal = None;
        self.terminal_problem = None;
        self.terminal_starting = false;
        self.problem = None;
        self.explorer = None;
        self.explorer_shown = false;
        self.clear_object_explorers();
        self.notebook_ai_menu = false;
        self.diff = None;
        self.diff_shown = false;
        self.machine = Default::default();
        self.connection_menu = false;
        self.log = None;
        self.connected_ms = None;
        self.focus_sent = None;
    }

    /// Connects to the daemon on `target`, leaving any earlier one.
    pub(crate) fn connect(
        &mut self,
        target: Target,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if crate::app_window::quit::active(cx) {
            return;
        }
        let host = match &target {
            Target::Local => sandhill_link::LOCAL,
            Target::Ssh(host) => host.as_str(),
        };
        if crate::resource_manager::host_blocked(host, cx) {
            if !matches!(self.link, Link::Ready(_)) {
                self.link = Link::Failed(
                    "Stopped or uninstalled. Connect explicitly to start again.".into(),
                );
            }
            cx.notify();
            return;
        }
        self.forget_connection(cx);
        let generation = self.generation;
        self.target = target.clone();
        self.link = Link::Connecting;
        let connect = Daemon::connect(self.rt.clone(), target);
        let window_handle = self.window_handle;
        self.focus_sent = None;
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
                mut requests,
                mut lsp,
            }) = streams
            else {
                return;
            };
            let language_this = this.clone();
            cx.spawn(async move |cx| {
                while let Some(event) = lsp.recv().await {
                    if language_this
                        .update(cx, |this, cx| {
                            if this.generation == generation {
                                this.on_lsp_message(event, cx);
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            let request_this = this.clone();
            let handle = window_handle;
            cx.spawn(async move |cx| {
                while let Some(request) = requests.recv().await {
                    let this = request_this.clone();
                    let alive = cx.update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            if this.generation == generation {
                                this.on_request(request, window, cx)
                            }
                        })
                    });
                    if !matches!(alive, Ok(Ok(()))) {
                        break;
                    }
                }
            })
            .detach();
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
        let host = self.host();
        let quota = cx.new(|cx| crate::quota::QuotaMonitor::new(daemon.clone(), host, cx));
        self.quota_observation = Some(cx.observe(&quota, |_, _, cx| cx.notify()));
        self.quota = Some(quota);
        self.connected_ms = Some(chrono::Utc::now().timestamp_millis());
        self.watch_machine(self.generation, cx);
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
                    Ok(kernels) if this.setup.kernels.is_empty() => this.kernels = kernels,
                    Ok(_) => {}
                    Err(err) => this.problem = Some(format!("Listing kernels failed: {err:#}")),
                }
                this.recent = recent.unwrap_or_default();
                if this.project.is_none() {
                    this.load_irkernel_offers(cx);
                }
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

    /// The code cells of a document open in a tab, as edited there.
    pub(crate) fn open_cells(&self, path: &str, cx: &App) -> Option<Vec<crate::composer::DocCell>> {
        self.open
            .iter()
            .chain(&self.others)
            .find(|o| o.notebook.path == path)
            .map(|o| o.code_cells(cx))
    }

    fn open_project(&mut self, path: String, cx: &mut Context<Self>) {
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
        let saves = self.save_all_tasks(cx);
        let generation = self.generation;
        let previous = self.project.clone();
        let opened = daemon.call::<ProjectInfo>(Call::OpenProject { path: path.clone() });
        cx.spawn(async move |this, cx| {
            for save in saves {
                save.await;
            }
            let opened = opened.await;
            let (setup, kernels) = if let Ok(project) = &opened {
                let setup = daemon.call::<sandhill_proto::ProjectSetup>(Call::GetProjectSettings {
                    path: project.path.clone(),
                });
                let kernels = daemon.call::<Vec<KernelspecInfo>>(Call::ListProjectKernelspecs {
                    path: project.path.clone(),
                });
                (setup.await.ok(), kernels.await.ok())
            } else {
                (None, None)
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation || this.project != previous || this.any_dirty() {
                    return;
                }
                match opened {
                    Ok(project) if !project.exists => {
                        this.problem = Some(format!("{} doesn't exist", project.path))
                    }
                    Ok(project) => {
                        let answered = this.trust.state == crate::trust::Trust::Restricted
                            && this.last_project.as_deref() == Some(&project.path);
                        this.close_project(cx);
                        this.last_project = Some(project.path.clone());
                        this.project = Some(project.path);
                        this.project_state = project.state;
                        this.trust = crate::trust::FolderTrust::opened(project.trusted, answered);
                        this.setup.setup = setup;
                        if let Some(kernels) = kernels {
                            this.setup.kernels = kernels.clone();
                            this.kernels = kernels;
                        }
                        this.read_folder(String::new(), cx);
                        this.load_irkernel_offers(cx);
                        // After a reconnect, the documents open before.
                        for rel in std::mem::take(&mut this.reopen) {
                            this.open_file(rel, cx);
                        }
                        this.problem = None;
                        // An untrusted folder starts nothing until trusted.
                        if project.trusted {
                            this.start_project(window, cx);
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

    /// What a trusted project starts with: its AI pane, the Console's live
    /// sessions, the kernel question the first time, and its terminal.
    pub(crate) fn start_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(daemon), Some(path)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let workbench = cx.entity().downgrade();
        let host = self.host();
        let agents = cx.new(|cx| Agents::new(daemon, host, path, workbench, window, cx));
        if let Some(quota) = &self.quota {
            quota.update(cx, |quota, cx| quota.set_agents(agents.clone(), cx));
        }
        self.agents = Some(agents);
        self.recover_console_sessions(cx);
        self.read_setup(true, window, cx);
        if self.console_tab == ConsoleTab::Terminal {
            self.ensure_terminal(cx);
        }
        if self.bottom_tab == BottomTab::Git {
            self.read_git_status(cx);
        }
    }

    /// Reads the folders shown in the files pane again.
    pub(crate) fn refresh_files(&mut self, cx: &mut Context<Self>) {
        for dir in self.tree.loaded() {
            self.read_folder(dir, cx);
        }
    }

    /// Reads one of the project's folders ("" for the project itself) into
    /// the tree.
    pub(crate) fn read_folder(&mut self, dir: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.tree
            .folders
            .entry(dir.clone())
            .or_insert(Folder::Loading);
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: tree::join(&project, &dir),
            sizes: self.tree.sort == tree::SortBy::Size,
            marks: false,
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

    /// Sorts the files another way, for every project from now on. Folder
    /// sizes are only worked out when sorting by size.
    pub(crate) fn set_sort(&mut self, by: tree::SortBy, cx: &mut Context<Self>) {
        self.files_ui.sort_menu = false;
        if self.tree.sort == by {
            cx.notify();
            return;
        }
        let sizes = by == tree::SortBy::Size || self.tree.sort == tree::SortBy::Size;
        self.tree.sort = by;
        crate::prefs::save_tree_sort(by.key());
        if sizes {
            self.refresh_files(cx);
        }
        cx.notify();
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
        // The caller must await saves before releasing a project. A conflict
        // keeps the editable document and its unsaved source alive.
        if self.any_dirty() {
            return;
        }
        self.close_language(None, cx);
        self.delimited = None;
        self.open = None;
        self.others.clear();
        self.media.clear();
        self.media_shown = None;
        self.tabs.clear();
        self.document_loads.clear();
        self.opening_shown = None;
        self.explorer = None;
        self.explorer_shown = false;
        self.close_popped_explorers(cx);
        self.clear_object_explorers();
        self.notebook_ai_menu = false;
        self.loose = Session::None;
        self.loose_kernel = None;
        self.console_sessions = Default::default();
        self.console_session = None;
        self.console.clear();
        self.project = None;
        self.agents = None;
        self.trust = Default::default();
        self.tree = Tree::new();
        self.terminal = None;
        self.forget_jobs();
        self.forget_git();
        self.viewer_extra.clear();
        self.setup = Default::default();
        self.project_state = Value::Null;
        self.files_ui = Default::default();
        self.palette.shown = false;
        cx.notify();
    }
}

/// The highlighter for a kernel or file language.
fn editor_language(language: Option<&str>) -> &'static str {
    match language.map(str::to_ascii_lowercase).as_deref() {
        Some("python") => "python",
        Some("r") => "r",
        Some("bash") => "bash",
        Some("markdown") => "markdown",
        Some("json") => "json",
        Some("yaml") => "yaml",
        Some("toml") => "toml",
        _ => "text",
    }
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

/// A script's runs are journaled under `console-0` to `console-99` in turn
/// (as in the Tauri app), so the daemon keeps at most this many.
const SCRIPT_SLOTS: usize = 100;

fn script_slot(cell_id: &str) -> Option<usize> {
    cell_id.strip_prefix("console-")?.parse().ok()
}

/// A script's file (`.R`, `.py`), whose runs are the Console's.
pub(crate) fn is_script(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    lower.ends_with(".r") || lower.ends_with(".py")
}
