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
    mtime_ms: i64,
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

    /// The open notebook with this path, shown or in another tab.
    pub(crate) fn open_at(&mut self, path: &str) -> Option<&mut Open> {
        if self.open.as_ref().is_some_and(|o| o.notebook.path == path) {
            return self.open.as_mut();
        }
        self.others.iter_mut().find(|o| o.notebook.path == path)
    }

    /// Shows the notebook open in another tab.
    pub(crate) fn show_file_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.reveal_middle();
        self.hide_object_explorer();
        self.notebook_ai_menu = false;
        self.explorer_shown = false;
        self.diff_shown = false;
        if self.document_loads.contains_key(path) {
            self.put_away();
            self.opening_shown = Some(path.to_string());
            cx.notify();
            return;
        }
        self.opening_shown = None;
        if self.media.contains_key(path) {
            self.put_away();
            self.media_shown = Some(path.to_string());
            self.session_changed(cx);
            cx.notify();
            return;
        }
        self.media_shown = None;
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
    pub(crate) fn put_away(&mut self) {
        self.opening_shown = None;
        self.hide_object_explorer();
        self.notebook_ai_menu = false;
        self.media_shown = None;
        if let Some(mut open) = self.open.take() {
            open.scroll = self.notebook_scroll.offset();
            self.others.push(open);
        }
    }

    /// Closes a tab, saving its notebook first if it was edited.
    pub(crate) fn close_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.document_loads.remove(path).is_some() {
            self.tabs.retain(|tab| tab != path);
            if self.opening_shown.as_deref() == Some(path) {
                self.opening_shown = None;
                if let Some(next) = self.tabs.last().cloned() {
                    self.show_file_tab(&next, cx);
                }
            }
            cx.notify();
            return;
        }
        if self.close_media(path, cx) {
            return;
        }
        if self.open_at(path).is_none() {
            return;
        }
        if self.tab_dirty(path) {
            let Some(save) = self.save_file(path, cx) else {
                return;
            };
            let path = path.to_string();
            let generation = self.generation;
            let project = self.project.clone();
            cx.spawn(async move |this, cx| {
                save.await;
                this.update(cx, |this, cx| {
                    if this.generation == generation
                        && this.project == project
                        && !this.tab_dirty(&path)
                    {
                        this.close_tab(&path, cx);
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        // A notebook is closed by showing it first. Closing a tab that isn't
        // in view then brings back the document that was, and anything that
        // covered it (Data Explorer, diff, Object Explorer).
        let kept = self
            .active_document_path()
            .filter(|shown| *shown != path)
            .map(str::to_string);
        let (explorer, diff, objects) = (self.explorer_shown, self.diff_shown, self.objects.shown);
        self.show_file_tab(path, cx);
        self.close_notebook(cx);
        if let Some(kept) = kept {
            self.show_file_tab(&kept, cx);
        }
        self.explorer_shown = explorer;
        self.diff_shown = diff;
        self.objects.shown = objects;
    }

    pub(crate) fn open_file(&mut self, rel: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.reveal_middle();
        let path = format!("{project}/{rel}");
        if crate::media::is_media(&path) {
            self.open_media(path, cx);
            return;
        }
        if self.open_at(&path).is_some() || self.document_loads.contains_key(&path) {
            self.show_file_tab(&path, cx);
            return;
        }
        if notebook::format_of(&path) == Some(Format::Delimited) {
            self.offer_delimited(rel, cx);
            return;
        }
        if crate::viewer::is_html(&path) {
            self.open_html(path, rel, cx);
            return;
        }
        let format = notebook::format_of(&path);
        if matches!(format, None | Some(Format::Text | Format::Markdown)) {
            self.open_shell_or_external(path, rel, cx);
            return;
        }
        let generation = self.generation;
        let load = self.next_document_load;
        self.next_document_load += 1;
        self.document_loads.insert(path.clone(), load);
        self.put_away();
        self.explorer_shown = false;
        self.diff_shown = false;
        self.problem = None;
        self.tabs.push(path.clone());
        self.opening_shown = Some(path.clone());
        cx.notify();
        // ReadNotebook leaves big outputs in the daemon's store.
        let read: std::pin::Pin<Box<dyn Future<Output = anyhow::Result<FileText>>>> = match format {
            Some(Format::Ipynb) => {
                Box::pin(daemon.call::<FileText>(Call::ReadNotebook { path: path.clone() }))
            }
            _ => Box::pin(daemon.call::<FileText>(Call::ReadFile { path: path.clone() })),
        };
        let runnable = matches!(
            format,
            Some(Format::Ipynb | Format::Quarto | Format::Script)
        );
        let saved =
            runnable.then(|| daemon.call::<FileOutputs>(Call::FileOutputs { file: path.clone() }));
        // Live sessions are attached to only in a trusted folder.
        let sessions = (runnable && !self.restricted())
            .then(|| daemon.call::<Vec<SessionInfo>>(Call::ListSessions));
        cx.spawn(async move |this, cx| {
            let read = read.await;
            // Parsing can be expensive even though the daemon strips outputs.
            // Only GPUI editor/view construction must stay on the UI thread.
            let read = cx
                .background_spawn(async move {
                    read.and_then(|file| {
                        Ok((notebook::parse(&file.path, &file.text)?, file.mtime_ms))
                    })
                })
                .await;
            let saved = match saved {
                Some(saved) => Some(saved.await),
                None => None,
            };
            let sessions = match sessions {
                Some(sessions) => sessions.await.unwrap_or_default(),
                None => Vec::new(),
            };
            this.update_in(cx, |this, window, cx| {
                if this.document_loads.get(&path) != Some(&load) {
                    return;
                }
                this.document_loads.remove(&path);
                if this.generation != generation
                    || this.project.as_deref() != Some(project.as_str())
                {
                    this.tabs.retain(|tab| tab != &path);
                    if this.opening_shown.as_deref() == Some(&path) {
                        this.opening_shown = None;
                    }
                    cx.notify();
                    return;
                }
                let selected = this.opening_shown.as_deref() == Some(&path);
                match read {
                    Ok((nb, mtime_ms)) => {
                        if this.open_at(&nb.path).is_some() {
                            return;
                        }
                        // A background load must not steal the user's new tab.
                        let previous = this.open.take();
                        if selected {
                            this.opening_shown = None;
                            this.notebook_scroll.set_offset(Point::default());
                        }
                        this.show_notebook(nb, mtime_ms, window, cx);
                        if let Some(saved) = saved {
                            match saved {
                                Ok(saved) => {
                                    this.show_saved_outputs(saved, &sessions, selected, cx)
                                }
                                Err(err) => {
                                    this.problem =
                                        Some(format!("Reading saved outputs failed: {err:#}"))
                                }
                            }
                        }
                        if selected {
                            this.session_changed(cx);
                            this.take_pending_location(window, cx);
                        } else {
                            this.others
                                .push(this.open.take().expect("document just constructed"));
                            this.open = previous;
                        }
                    }
                    Err(err) => {
                        this.tabs.retain(|tab| tab != &path);
                        if selected {
                            this.opening_shown = None;
                            if let Some(next) = this.tabs.last().cloned() {
                                this.show_file_tab(&next, cx);
                            }
                        }
                        this.problem = Some(format!("Opening {rel} failed: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a project file and puts the cursor at a 1-based line and column.
    pub(crate) fn open_file_location(
        &mut self,
        path: String,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let Some(rel) = path
            .strip_prefix(&format!("{project}/"))
            .map(str::to_string)
        else {
            return;
        };
        if self.open_at(&path).is_some() {
            self.show_file_tab(&path, cx);
            self.pending_location = Some((path, line, column));
            self.take_pending_location(window, cx);
            return;
        }
        self.pending_location = Some((path, line, column));
        self.open_file(rel, cx);
    }

    pub(crate) fn take_pending_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line, column)) = self.pending_location.take() else {
            return;
        };
        let Some(open) = self.open.as_ref().filter(|open| open.notebook.path == path) else {
            return;
        };
        let line0 = line.unwrap_or(1).saturating_sub(1) as usize;
        let Some((cell, cell_line)) = open.notebook.source_location(line0) else {
            return;
        };
        let editor = open.editors[cell].clone();
        let offset = column.map(|column| {
            let text = editor.read(cx).value().to_string();
            let start = text
                .split_inclusive('\n')
                .take(cell_line)
                .map(str::len)
                .sum::<usize>();
            let row = text[start..].split('\n').next().unwrap_or("");
            let byte = row
                .char_indices()
                .nth(column.saturating_sub(1) as usize)
                .map_or(row.len(), |(byte, _)| byte);
            start + byte
        });
        self.jump_to_source(cell, cell_line, offset.map(|at| at..at), window, cx);
        self.take_language_definition(window, cx);
    }

    /// The editor that import code would be inserted into: a script, or the
    /// notebook's current code cell.
    fn insertion_editor(&self) -> Option<Entity<EditorState>> {
        if self.objects.is_shown()
            || self.media_shown.is_some()
            || self.explorer_shown
            || self.diff_shown
        {
            return None;
        }
        let open = self.open.as_ref()?;
        if !open.notebook.can_run() {
            return None;
        }
        let cell = if open.single_editor() {
            0
        } else {
            open.current?
        };
        (open.notebook.cells.get(cell)?.kind == CellKind::Code)
            .then(|| open.editors.get(cell).cloned())
            .flatten()
    }

    pub(crate) fn can_insert_into_active_document(&self, _cx: &App) -> bool {
        self.insertion_editor().is_some()
    }

    /// Inserts text at the cursor of the active script or current code cell.
    pub(crate) fn insert_into_active_document(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.insertion_editor() else {
            return false;
        };
        editor.update(cx, |editor, cx| {
            editor.insert(text, window, cx);
            editor.focus(window, cx);
        });
        true
    }

    /// An editor for each cell of a document, and what follows their edits.
    pub(crate) fn cell_editors(
        &mut self,
        nb: &Notebook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<Entity<EditorState>>, Vec<Subscription>) {
        let mut doc = DocKind::of(nb);
        let mut editors = Vec::with_capacity(nb.cells.len());
        let mut subscriptions = Vec::with_capacity(nb.cells.len() + 1);
        for (ix, cell) in nb.cells.iter().enumerate() {
            doc.language = nb.cell_language(ix).map(str::to_string);
            let (editor, subscription) =
                self.cell_editor(&doc, cell.kind, cell.source.clone(), window, cx);
            editors.push(editor);
            subscriptions.push(subscription);
        }
        (editors, subscriptions)
    }

    /// An editor for one cell of a document, and what follows its edits.
    pub(crate) fn cell_editor(
        &mut self,
        nb: &DocKind,
        kind: CellKind,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<EditorState>, Subscription) {
        let language = match kind {
            CellKind::Code => editor_language(nb.language.as_deref()),
            CellKind::Text => "markdown",
        };
        let wrap = editor_wrap_type(nb.format, kind, nb.language.as_deref())
            .is_some_and(crate::prefs::editor_soft_wrap);
        let script = nb.script;
        let editor = cx.new(|cx| {
            let editor = EditorState::new(window, cx)
                .language(language)
                .folding(true)
                .searchable(false)
                .soft_wrap(wrap);
            if script {
                // A script is one editor that scrolls itself.
                editor
                    .line_number(nb.language.is_some())
                    .default_value(source)
            } else {
                // The notebook scrolls, not the cell, and a cell is as
                // tall as its lines.
                editor
                    .line_number(false)
                    .scroll_beyond_last_line(Some(0))
                    .default_value(source)
            }
        });
        if script {
            // Local structural folds parse the complete file. A 50k-line
            // Python script took 278ms here, before its first editor appeared.
            let text = editor.read(cx).text().clone();
            let editor = editor.downgrade();
            cx.spawn(async move |_, cx| {
                let (text, folds) = cx
                    .background_spawn(async move {
                        let folds = crate::outline::fold_ranges(&text.to_string(), language);
                        (text, folds)
                    })
                    .await;
                editor
                    .update(cx, |editor, cx| {
                        if editor.text() == &text {
                            editor.apply_highlighter_fold_candidates(folds, cx);
                        }
                    })
                    .ok();
            })
            .detach();
        } else {
            editor.update(cx, |editor, cx| {
                let text = editor.value();
                editor.apply_highlighter_fold_candidates(
                    crate::outline::fold_ranges(&text, language),
                    cx,
                );
            });
        }
        if kind == CellKind::Code && matches!(language, "r" | "python") {
            self.install_language(&editor, Some(&nb.path), cx);
            crate::rainbow::attach(&editor, cx);
        }
        let path = nb.path.clone();
        let subscription = cx.subscribe(&editor, move |this, editor, event: &InputEvent, cx| {
            let Some(open) = this.open_at(&path) else {
                return;
            };
            // Cells move, so the editor says which cell this is.
            let Some(ix) = open.editors.iter().position(|e| *e == editor) else {
                return;
            };
            match event {
                InputEvent::Change => {
                    if open.notebook.truncated {
                        return;
                    }
                    open.dirty = true;
                    let language = open
                        .notebook
                        .cell_language(ix)
                        .unwrap_or("markdown")
                        .to_string();
                    editor.update(cx, |editor, cx| {
                        let text = editor.value();
                        editor.apply_highlighter_fold_candidates(
                            crate::outline::fold_ranges(&text, &language),
                            cx,
                        );
                    });
                }
                // A text cell shows formatted again.
                InputEvent::Blur => {
                    let id = open.notebook.cells[ix].id.clone();
                    open.editing.remove(&id);
                }
                InputEvent::Focus => {}
                _ => return,
            }
            if matches!(event, InputEvent::Change) {
                this.refresh_find(cx);
            }
            if kind == CellKind::Code {
                this.language_event(&path, event, cx);
            }
            cx.notify();
        });
        (editor, subscription)
    }

    /// Wrap follows the current cell's language in mixed-language notebooks.
    pub(crate) fn editor_wrap_type(open: &Open) -> Option<&'static str> {
        let ix = open.current.unwrap_or(0);
        let cell = open.notebook.cells.get(ix)?;
        editor_wrap_type(
            open.notebook.format,
            cell.kind,
            open.notebook.cell_language(ix),
        )
    }

    pub(crate) fn toggle_editor_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.open.as_ref().and_then(Self::editor_wrap_type) else {
            return;
        };
        let wrap = !crate::prefs::editor_soft_wrap(kind);
        crate::prefs::save_editor_soft_wrap(kind, wrap);
        self.apply_editor_wrap(kind, wrap, window, cx);
        let current = cx.entity_id();
        self.with_shell(window, cx, move |shell, window, cx| {
            for tab in &shell.tabs {
                if tab.workbench.entity_id() != current {
                    tab.workbench.update(cx, |workbench, cx| {
                        workbench.apply_editor_wrap(kind, wrap, window, cx);
                    });
                }
            }
        });
    }

    fn apply_editor_wrap(
        &mut self,
        kind: &str,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for open in self.open.iter().chain(&self.others) {
            let mut apply = |notebook: &Notebook, editors: &[Entity<EditorState>]| {
                for (ix, editor) in editors.iter().enumerate() {
                    let Some(cell) = notebook.cells.get(ix) else {
                        continue;
                    };
                    if editor_wrap_type(notebook.format, cell.kind, notebook.cell_language(ix))
                        == Some(kind)
                    {
                        editor.update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
                    }
                }
            };
            apply(&open.notebook, &open.editors);
            // The hidden notebook editors are reused when leaving Source view.
            if let Some((notebook, view)) = open
                .notebook
                .source_notebook()
                .zip(open.quarto_view.as_ref())
            {
                apply(notebook, &view.editors);
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_quarto_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self
            .open
            .as_ref()
            .filter(|open| open.notebook.format == Format::Quarto)
        else {
            return;
        };
        let source_mode = open.notebook.quarto_source();
        let sources = open.sources(cx);
        let result = if source_mode {
            open.notebook.as_quarto_notebook(&sources[0])
        } else {
            open.notebook
                .as_quarto_source(&sources)
                .map(|nb| (nb, Vec::new()))
        };
        let (nb, matched) = match result {
            Ok(result) => result,
            Err(error) => {
                self.problem = Some(format!("Switching Quarto view failed: {error:#}"));
                cx.notify();
                return;
            }
        };
        let reusable: HashMap<_, _> = if source_mode {
            let open = self.open.as_ref().expect("checked above");
            matched
                .iter()
                .filter_map(|(old, new)| {
                    let editor = open.quarto_view.as_ref()?.editors.get(*old)?;
                    (editor.read(cx).value().as_ref() == nb.cells[*new].source
                        && open
                            .notebook
                            .source_notebook()
                            .and_then(|nb| nb.cell_language(*old))
                            == nb.cell_language(*new))
                    .then(|| (*new, editor.clone()))
                })
                .collect()
        } else {
            HashMap::new()
        };
        let mut editors = Vec::with_capacity(nb.cells.len());
        let mut subscriptions = Vec::new();
        let mut doc = DocKind::of(&nb);
        for (ix, cell) in nb.cells.iter().enumerate() {
            if let Some(editor) = reusable.get(&ix) {
                editors.push(editor.clone());
            } else {
                doc.language = nb.cell_language(ix).map(str::to_string);
                let (editor, subscription) =
                    self.cell_editor(&doc, cell.kind, cell.source.clone(), window, cx);
                editors.push(editor);
                subscriptions.push(subscription);
            }
        }
        let open = self.open.as_mut().expect("checked above");
        let mut renames = HashMap::new();
        let (current, position) = if source_mode {
            let position = open.editors[0].read(cx).cursor_position();
            let location = nb.source_location(position.line as usize).unwrap_or((0, 0));
            if let Some(view) = open.quarto_view.take() {
                for (old, new) in &matched {
                    // Retain the journal association even if this cell was edited.
                    renames.insert(view.ids[*old].clone(), nb.cells[*new].id.clone());
                }
                open.cell_runs
                    .retain(|id, _| renames.contains_key(id) || id.starts_with("deleted-"));
                open.collapsed.retain(|id| renames.contains_key(id));
                open.editing.retain(|id| renames.contains_key(id));
                open.waiting.retain(|id| renames.contains_key(id));
                open.sending.retain(|id| renames.contains_key(id));
                self.notebook_scroll.set_offset(view.scroll);
                let current = (!nb.cells.is_empty())
                    .then_some(location.0)
                    .or(view.current.filter(|ix| *ix < nb.cells.len()));
                (
                    current,
                    Position::new(location.1 as u32, position.character),
                )
            } else {
                (
                    Some(location.0),
                    Position::new(location.1 as u32, position.character),
                )
            }
        } else {
            let current = open
                .current
                .unwrap_or(0)
                .min(open.editors.len().saturating_sub(1));
            let position = open
                .editors
                .get(current)
                .map(|editor| editor.read(cx).cursor_position())
                .unwrap_or_default();
            let line = match &nb.source_notebook() {
                Some(notebook) => notebook.cell_source_line(current),
                None => 0,
            };
            open.quarto_view = Some(QuartoView {
                editors: open.editors.clone(),
                current: open.current,
                scroll: self.notebook_scroll.offset(),
                ids: open
                    .notebook
                    .cells
                    .iter()
                    .map(|cell| cell.id.clone())
                    .collect(),
            });
            (
                Some(0),
                Position::new(line as u32 + position.line, position.character),
            )
        };
        open.notebook = nb;
        open.editors = editors;
        open.current = current;
        open._subscriptions.extend(subscriptions);
        if source_mode {
            self.quarto_sources.remove(&open.notebook.path);
        } else {
            self.quarto_sources.insert(open.notebook.path.clone());
        }
        let editor = current.and_then(|ix| open.editors.get(ix)).cloned();
        self.rename_quarto_cells(&renames);
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| {
                editor.set_cursor_position(position, window, cx);
                editor.focus(window, cx);
            });
        }
        self.refresh_find(cx);
        cx.notify();
    }

    pub(crate) fn show_notebook(
        &mut self,
        nb: Notebook,
        mtime_ms: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_middle();
        let project = self.project_kernel(nb.language.as_deref());
        let kernel = notebook::recover_kernel(
            &nb,
            &self.kernels,
            None,
            self.project_state["documents"][&nb.path]["kernel"].as_str(),
            project.as_deref(),
        );
        let (editors, mut subscriptions) = self.cell_editors(&nb, window, cx);
        if nb.format == Format::Text {
            subscriptions.extend(crate::bio_text::decorate_editor(
                &editors[0],
                nb.language.as_deref(),
                cx,
            ));
        }
        let delimited_header = if nb.format == Format::Delimited {
            let (header, header_subscriptions) =
                crate::delimited::decorate_editor(&editors[0], &nb.path, window, cx);
            subscriptions.extend(header_subscriptions);
            Some(header)
        } else {
            None
        };
        self.open = Some(Open {
            notebook: nb,
            editors,
            dirty: false,
            mtime_ms,
            markdown_preview: false,
            md_images: Default::default(),
            quarto_view: None,
            delimited_header,
            kernel,
            session: Session::None,
            waiting: Vec::new(),
            sending: HashSet::new(),
            cell_runs: HashMap::new(),
            scroll: Point::default(),
            editing: HashSet::new(),
            collapsed: HashSet::new(),
            deleted: Vec::new(),
            agent_changes: HashMap::new(),
            agent_deleted: Vec::new(),
            lit: None,
            ghost: None,
            ghost_after: None,
            next_slot: 0,
            queued: Vec::new(),
            current: None,
            _subscriptions: subscriptions,
        });
        if self.open.as_ref().is_some_and(|open| {
            open.notebook.format == Format::Quarto
                && self.quarto_sources.contains(&open.notebook.path)
        }) {
            self.toggle_quarto_view(window, cx);
        }
        self.problem = None;
    }

    /// Puts each cell's latest journaled run under it, and carries on in the
    /// kernel that ran them if it is still running.
    fn show_saved_outputs(
        &mut self,
        saved: FileOutputs,
        sessions: &[SessionInfo],
        activate: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let latest = saved.cells.iter().max_by_key(|c| c.started_ms).cloned();
        // A script carries on after the slot its last run used.
        if let Some(n) = saved
            .cells
            .iter()
            .filter_map(|c| Some((c.started_ms, script_slot(&c.cell_id)?)))
            .max()
            .map(|(_, n)| n)
        {
            open.next_slot = (n + 1) % SCRIPT_SLOTS;
        }
        let cell_ids: HashSet<&str> = open
            .notebook
            .cells
            .iter()
            .map(|cell| cell.id.as_str())
            .collect();
        for cell in saved.cells {
            if !cell_ids.contains(cell.cell_id.as_str()) {
                continue;
            }
            self.runs.insert(
                cell.run.clone(),
                Run {
                    code: cell.code,
                    language: sessions
                        .iter()
                        .find(|session| session.id == cell.session)
                        .map(|session| editor_language(Some(&session.language))),
                    cell: Some((saved.file.clone(), cell.cell_id.clone())),
                    outputs: cell.outputs,
                    finished: cell.finished_ms.is_some(),
                    status: cell.status,
                    execution_count: cell.execution_count,
                    session: cell.session.clone(),
                    queued_ms: Some(cell.started_ms),
                    finished_ms: cell.finished_ms,
                    snapshot_seq: saved.seq,
                    last_seq: saved.seq,
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
        if let Some(session) = running {
            open.kernel = notebook::recover_kernel(
                &open.notebook,
                &self.kernels,
                Some(&session.kernel),
                open.kernel.as_deref(),
                None,
            );
            open.session = Session::Ready(session.id.clone());
        }
        if let Some(session) = running {
            self.remember_document_kernel(&saved.file, &session.kernel);
            if activate {
                self.follow_console_session(session.id.clone(), cx);
            }
        }
    }

    /// Closes the open notebook, saving it first if it was edited. Its
    /// kernel keeps running, so reopening carries on where it left off.
    pub(crate) fn close_notebook(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self
            .open
            .as_ref()
            .filter(|o| o.dirty)
            .map(|o| o.notebook.path.clone())
        {
            self.close_tab(&path, cx);
            return;
        }
        if let Some(path) = self.open.as_ref().map(|open| open.notebook.path.clone()) {
            self.close_language(Some(&path), cx);
        }
        if let Some(closed) = self.open.take() {
            let ix = self.tabs.iter().position(|p| *p == closed.notebook.path);
            self.tabs.retain(|p| *p != closed.notebook.path);
            // The tab that was next to it, as browsers do.
            let next = ix
                .and_then(|ix| self.tabs.get(ix.min(self.tabs.len().saturating_sub(1))))
                .cloned();
            if let Some(next) = next {
                self.show_file_tab(&next, cx);
            }
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.open.as_ref().map(|o| o.notebook.path.clone())
            && let Some(task) = self.save_file(&path, cx)
        {
            task.detach();
        }
    }

    /// Saves every open document with unsaved changes (⌥⌘S).
    pub(crate) fn save_all(&mut self, cx: &mut Context<Self>) {
        for task in self.save_all_tasks(cx) {
            task.detach();
        }
    }

    /// Saves every edited document; the tasks end when they are written.
    pub(crate) fn save_all_tasks(&mut self, cx: &mut Context<Self>) -> Vec<Task<()>> {
        let dirty: Vec<String> = self
            .open
            .iter()
            .chain(&self.others)
            .filter(|o| o.dirty)
            .map(|o| o.notebook.path.clone())
            .collect();
        dirty
            .iter()
            .filter_map(|path| self.save_file(path, cx))
            .collect()
    }

    /// Whether any open document has unsaved changes.
    pub(crate) fn any_dirty(&self) -> bool {
        self.open.iter().chain(&self.others).any(|o| o.dirty)
    }

    /// How many open documents have unsaved changes.
    pub(crate) fn unsaved_count(&self) -> usize {
        self.open
            .iter()
            .chain(&self.others)
            .filter(|o| o.dirty)
            .count()
    }

    /// The project's running kernels, as the daemon last listed them.
    pub(crate) fn project_kernels(&self) -> Vec<String> {
        let Some(project) = self.project.as_deref() else {
            return Vec::new();
        };
        self.machine
            .sessions
            .iter()
            .filter(|s| {
                s.status != sandhill_proto::SessionStatus::Ended
                    && s.project.as_deref() == Some(project)
            })
            .map(|s| s.id.clone())
            .collect()
    }

    /// Stops every kernel of the project; documents using them see it stop.
    pub(crate) fn shut_down_project_kernels(&mut self, cx: &mut Context<Self>) {
        let ids = self.project_kernels();
        self.shut_down(ids, cx);
    }

    /// Writes one open document; the task ends when it is written.
    pub(crate) fn save_file(&mut self, path: &str, cx: &mut Context<Self>) -> Option<Task<()>> {
        let daemon = self.daemon();
        let open = self
            .open
            .iter()
            .chain(&self.others)
            .find(|o| o.notebook.path == path);
        let (Some(daemon), Some(open)) = (daemon, open) else {
            return None;
        };
        if open.notebook.truncated {
            return None;
        }
        let sources = open.sources(cx);
        let text = match open.notebook.render(&sources) {
            Ok(text) => text,
            Err(err) => {
                self.problem = Some(format!("Saving failed: {err:#}"));
                cx.notify();
                return None;
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
            Format::Quarto
            | Format::Script
            | Format::Markdown
            | Format::Text
            | Format::Delimited => Call::WriteFile {
                path: path.clone(),
                text: text.clone(),
                expected_mtime_ms,
                base64: false,
                create: false,
            },
        };
        let written = daemon.call::<FileWritten>(write);
        let task = cx.spawn(async move |this, cx| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(written) => {
                        if let Some(open) = this.open_at(&path) {
                            open.mtime_ms = written.mtime_ms;
                            let current = open.sources(cx);
                            open.dirty = current != sources
                                && open.notebook.render(&current).map_or(true, |current| current != text);
                            // Later saves start from what is on disk now.
                            // Unless the cells were added to or moved since.
                            let ids = |nb: &Notebook| {
                                nb.cells.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
                            };
                            match notebook::parse_as(&path, &text, open.notebook.format) {
                                Ok(_) if open.notebook.quarto_source() => {}
                                Ok(nb) if ids(&nb) == ids(&open.notebook) => {
                                    // Cleared outputs stay cleared.
                                    open.notebook = nb
                                }
                                // Adjacent Quarto prose editors (including an
                                // empty added one) merge when parsed. Retain
                                // their byte spans and existing editor history.
                                Ok(_) if open.notebook.can_add_cells() => {}
                                _ => {
                                    this.problem = Some(
                                        "Saved. The cells changed shape; reopen the file to edit further."
                                            .into(),
                                    )
                                }
                            }
                        }
                        let saved = this.open_at(&path).is_some_and(|open| !open.dirty);
                        this.sync_language(&path, saved, cx);
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
        });
        Some(task)
    }

    /// The shown document's kernel, else the Console's own: what Restart,
    /// Interrupt and the kernel picker act on.
    pub(crate) fn active_session(&self) -> Option<&str> {
        match &self.open {
            Some(open) => open.session.id(),
            None => self.loose.id(),
        }
    }

    /// The kernel Variables, Help and Packages show: the Console's.
    /// Activating or running a document moves the Console to its kernel; a
    /// document without one (or opening another file) leaves it where it is.
    pub(crate) fn variables_session(&self) -> Option<&str> {
        self.console_target()
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

    /// Prefer the running session's own language over the selected kernelspec.
    pub(crate) fn session_language(&self, session: &str) -> Option<&str> {
        self.console_sessions
            .live
            .iter()
            .chain(&self.machine.sessions)
            .find(|info| info.id == session)
            .map(|info| info.language.as_str())
            .or_else(|| {
                let kernel = self
                    .open
                    .iter()
                    .chain(&self.others)
                    .find(|open| open.session.id() == Some(session))
                    .and_then(|open| open.kernel.as_deref())
                    .or_else(|| {
                        self.console_sessions
                            .states
                            .iter()
                            .find(|(_, state)| state.id() == Some(session))
                            .map(|(kernel, _)| kernel.as_str())
                    })
                    .or_else(|| {
                        (self.loose.id() == Some(session))
                            .then_some(self.loose_kernel.as_deref())
                            .flatten()
                    })?;
                self.kernels
                    .iter()
                    .find(|spec| spec.name == kernel)
                    .map(|spec| spec.language.as_str())
            })
    }

    pub(crate) fn console_language(&self) -> &'static str {
        let language = self
            .console_target()
            .and_then(|session| self.session_language(session))
            .or_else(|| {
                self.loose_kernel
                    .as_deref()
                    .and_then(|kernel| self.kernels.iter().find(|spec| spec.name == kernel))
                    .map(|spec| spec.language.as_str())
            })
            .or_else(|| {
                self.open
                    .as_ref()
                    .and_then(|open| open.notebook.language.as_deref())
            });
        editor_language(language)
    }

    pub(crate) fn run_language(&self, run: &Run) -> &'static str {
        run.language
            .unwrap_or_else(|| editor_language(self.session_language(&run.session)))
    }

    /// Follows a change of the active kernel: the Console shows its runs
    /// and Variables its variables.
    pub(crate) fn session_changed(&mut self, cx: &mut Context<Self>) {
        let document = self.open.as_ref().and_then(|open| {
            open.session
                .id()
                .map(|id| (open.notebook.path.clone(), id.to_string()))
        });
        if document != self.console_sessions.followed_document {
            self.console_sessions.followed_document = document.clone();
            if let Some((_, id)) = document {
                let kernel = self.open.as_ref().and_then(|open| open.kernel.clone());
                if let Some(previous) = &self.loose_kernel {
                    self.console_sessions
                        .states
                        .insert(previous.clone(), self.loose.clone());
                }
                self.loose = Session::Ready(id.clone());
                self.loose_kernel = kernel;
                if let Some(kernel) = &self.loose_kernel {
                    self.console_sessions
                        .states
                        .insert(kernel.clone(), self.loose.clone());
                }
                self.console_sessions.retained.insert(id);
                self.save_console_choice();
            }
        }
        let active = self.console_target().map(str::to_string);
        let language = self.console_language();
        if self.console_input.read(cx).language_name() != language {
            self.console_input
                .update(cx, |input, cx| input.set_highlighter(language, cx));
        }
        if let Some(path) = self.open.as_ref().map(|open| open.notebook.path.clone())
            && self.language_status(&path).is_some()
        {
            self.language_event(&path, &InputEvent::Focus, cx);
        }
        let variables = self.variables_session().map(str::to_string);
        let variables_changed = variables != self.console_sessions.variables_session;
        self.console_sessions.variables_session = variables.clone();
        if active != self.console_session {
            self.console_session = active.clone();
            self.console_sessions.history_open = false;
            self.switch_console_draft(cx);
            self.console.clear();
            self.viewer_index = None;
            if let Some(session) = active {
                self.restore_console_runs(&session);
                self.follow_console_session(session, cx);
            }
        }
        if variables_changed {
            self.vars = Vars::default();
            if self
                .explorer
                .as_ref()
                .is_some_and(|e| Some(e.read(cx).session()) != variables.as_deref())
            {
                self.explorer = None;
                self.explorer_shown = false;
            }
            self.refresh_variables(cx);
        }
        cx.notify();
    }

    /// A remembered missing kernel remains a visible choice, never a fallback.
    pub(crate) fn kernel_unavailable(&self, name: &str) -> Option<String> {
        if self.open.is_none()
            && self
                .console_sessions
                .live
                .iter()
                .any(|session| session.kernel == name && session.status == SessionStatus::Running)
        {
            return None;
        }
        let Some(spec) = self.kernels.iter().find(|spec| spec.name == name) else {
            return Some(format!(
                "Kernel “{name}” is unavailable on this machine. Select an installed kernel to run this document; its remembered choice has not been changed."
            ));
        };
        if let Some(open) = self.open.as_ref().filter(|o| o.is_script())
            && open
                .notebook
                .language
                .as_deref()
                .is_some_and(|language| !spec.language.eq_ignore_ascii_case(language))
        {
            return Some(format!(
                "Kernel “{name}” cannot run this script's language. Select a matching kernel."
            ));
        }
        None
    }

    /// The kernel this document used last time, from the project's state.
    pub(crate) fn remembered_kernel(&self, path: &str) -> Option<&str> {
        self.project_state["documents"][path]["kernel"].as_str()
    }

    fn remember_document_kernel(&mut self, file: &str, kernel: &str) {
        if self.project_state["documents"][file]["kernel"].as_str() == Some(kernel) {
            return;
        }
        if !self.project_state.is_object() {
            self.project_state = serde_json::json!({});
        }
        if !self.project_state["documents"].is_object() {
            self.project_state["documents"] = serde_json::json!({});
        }
        if !self.project_state["documents"][file].is_object() {
            self.project_state["documents"][file] = serde_json::json!({});
        }
        self.project_state["documents"][file]["kernel"] = Value::String(kernel.to_string());
        if let (Some(daemon), Some(path)) = (self.daemon(), self.project.clone()) {
            daemon.fire(Call::SaveProjectState {
                path,
                state: self.project_state.clone(),
            });
        }
    }

    pub(crate) fn choose_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| !o.notebook.can_run()) {
            return;
        }
        if self.open.is_none() {
            let has_live = self
                .console_sessions
                .live
                .iter()
                .any(|s| s.kernel == name && s.status == SessionStatus::Running);
            if !has_live && let Some(reason) = self.kernel_unavailable(&name) {
                self.problem = Some(reason);
                cx.notify();
                return;
            }
            self.choose_console_kernel(name, cx);
            return;
        }
        if let Some(reason) = self.kernel_unavailable(&name) {
            self.problem = Some(reason);
            cx.notify();
            return;
        }
        if let Some(path) = self.open.as_ref().map(|o| o.notebook.path.clone()) {
            self.remember_document_kernel(&path, &name);
        }
        let daemon = self.daemon();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (kernel, session) = (&mut open.kernel, &mut open.session);
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
            open.queued.clear();
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn run_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.objects.is_shown() {
            return;
        }
        self.reveal_middle();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.notebook.can_run() {
            return;
        }
        match open.notebook.cells.get(index).map(|c| c.kind) {
            Some(CellKind::Code) => {}
            // Running a Markdown cell shows it formatted, as in Jupyter.
            Some(CellKind::Text) => {
                let id = open.notebook.cells[index].id.clone();
                if open.editing.remove(&id) {
                    cx.notify();
                }
                return;
            }
            None => return,
        }
        if open.is_script() {
            let code = open.editors[index].read(cx).value().to_string();
            self.run_script(code, cx);
            return;
        }
        match &open.session {
            Session::Ready(_) => {
                let path = open.notebook.path.clone();
                self.execute(&path, index, cx)
            }
            Session::Starting => open.waiting.push(open.notebook.cells[index].id.clone()),
            Session::None | Session::Failed(_) => {
                open.waiting.push(open.notebook.cells[index].id.clone());
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

    /// In a script, runs the selection, or the statement at the cursor and
    /// moves the cursor to the next statement, as RStudio does.
    pub(crate) fn run_statement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.objects.is_shown() {
            return;
        }
        let Some(open) = self.open.as_ref().filter(|o| o.is_script()) else {
            return;
        };
        let language = open.notebook.language.clone().unwrap_or_default();
        let editor = open.editors[0].clone();
        let (text, selected, line) = {
            let editor = editor.read(cx);
            (
                editor.value().to_string(),
                editor.selected_value().to_string(),
                editor.cursor_position().line as usize,
            )
        };
        if !selected.is_empty() {
            if !selected.trim().is_empty() {
                self.run_script(selected, cx);
            }
            return;
        }
        let all = statements::statements(&text, &language);
        let here = all.iter().find(|s| s.from <= line && line <= s.to);
        if let Some(here) = here {
            let code: Vec<&str> = text
                .split('\n')
                .skip(here.from)
                .take(here.to - here.from + 1)
                .collect();
            self.run_script(code.join("\n"), cx);
        }
        let after = here.map_or(line, |s| s.to);
        let lines = text.split('\n').count();
        let target = all
            .iter()
            .find(|s| s.from > after)
            .map_or((after + 1).min(lines.saturating_sub(1)), |s| s.from);
        editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(target as u32, 0), window, cx)
        });
    }

    /// Runs code from the shown script in its kernel, starting it if needed.
    /// The runs show in the Console.
    pub(crate) fn run_script(&mut self, code: String, cx: &mut Context<Self>) {
        self.reveal_middle();
        let collapsed = self.panel_collapsed(crate::panes::Panel::Console);
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.is_script() {
            return;
        }
        self.console_tab = ConsoleTab::Console;
        self.show_console = true;
        if collapsed {
            self.middle_vertical = crate::panes::Vertical::Split;
        }
        match &open.session {
            Session::Ready(_) => {
                let path = open.notebook.path.clone();
                self.send_script_run(&path, code, cx);
            }
            Session::Starting => open.queued.push(code),
            Session::None | Session::Failed(_) => {
                open.queued.push(code);
                self.start_session(cx);
            }
        }
        cx.notify();
    }

    /// Sends a script's code to its kernel, journaled under the script and
    /// its next slot, so reopening the script finds the kernel again.
    fn send_script_run(&mut self, path: &str, code: String, cx: &mut Context<Self>) {
        self.bind_console_document(path, cx);
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(open) = self.open_at(path) else {
            return;
        };
        let Some(session) = open.session.id().map(str::to_string) else {
            return;
        };
        let slot = format!("console-{}", open.next_slot);
        open.next_slot = (open.next_slot + 1) % SCRIPT_SLOTS;
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session,
            code,
            cell: Some(CellRef {
                file: path.to_string(),
                cell_id: slot,
            }),
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = started.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Running the script failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Starts the active kernel: the notebook's (in its folder) or the
    /// Console's (in the project folder).
    fn start_session(&mut self, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| !o.notebook.can_run()) {
            return;
        }
        if self.held_back("run code", cx) {
            if let Some(open) = self.open.as_mut() {
                open.waiting.clear();
                open.queued.clear();
            }
            return;
        }
        if self.open.is_none() {
            self.start_console_session(cx);
            return;
        }
        if let Some(reason) = self
            .active_kernel()
            .and_then(|name| self.kernel_unavailable(name))
        {
            match self.open.as_mut() {
                Some(open) => {
                    open.session = Session::Failed(reason);
                    open.waiting.clear();
                    open.queued.clear();
                }
                None => self.loose = Session::Failed(reason),
            }
            cx.notify();
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let notebook_path = self.open.as_ref().map(|o| o.notebook.path.clone());
        let missing = match self.open.as_ref().filter(|o| o.is_script()) {
            Some(open) if open.notebook.language.as_deref() == Some("python") => {
                "No Python kernel is installed"
            }
            Some(_) => "No R kernel is installed",
            None => "No kernel is installed",
        };
        let documents_in_project = self.documents_in_project();
        let (kernel, session, cwd) = match self.open.as_mut() {
            // Scripts run in the project's folder, as in RStudio.
            Some(open) if open.is_script() => {
                (open.kernel.clone(), &mut open.session, project.clone())
            }
            Some(open) if documents_in_project => {
                (open.kernel.clone(), &mut open.session, project.clone())
            }
            Some(open) => (
                open.kernel.clone(),
                &mut open.session,
                open.notebook.folder().to_string(),
            ),
            None => (self.loose_kernel.clone(), &mut self.loose, project.clone()),
        };
        let Some(kernel) = kernel else {
            *session = Session::Failed(missing.into());
            if let Some(open) = self.open.as_mut() {
                open.waiting.clear();
                open.queued.clear();
            }
            return;
        };
        *session = Session::Starting;
        let kernel_name = kernel.clone();
        let generation = self.generation;
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
                if this.generation != generation {
                    return;
                }
                // The notebook may be in another tab by now.
                let slot = match &notebook_path {
                    Some(path) => match this.open_at(path) {
                        Some(open)
                            if open.kernel.as_deref() != Some(&kernel_name)
                                || !matches!(open.session, Session::Starting) =>
                        {
                            return;
                        }
                        Some(open) => &mut open.session,
                        None => return,
                    },
                    None => &mut this.loose,
                };
                match session {
                    Ok(id) => {
                        *slot = Session::Ready(id);
                        if let Some(path) = &notebook_path {
                            if let Some(kernel) = this.open_at(path).and_then(|o| o.kernel.clone())
                            {
                                this.remember_document_kernel(path, &kernel);
                            }
                            let waiting = this
                                .open_at(path)
                                .map(|o| std::mem::take(&mut o.waiting))
                                .unwrap_or_default();
                            for id in waiting {
                                let index = this.open_at(path).and_then(|o| {
                                    o.notebook
                                        .source_notebook()
                                        .unwrap_or(&o.notebook)
                                        .cells
                                        .iter()
                                        .position(|c| c.id == id)
                                });
                                if let Some(index) = index {
                                    this.execute(path, index, cx);
                                }
                            }
                            let queued = this
                                .open_at(path)
                                .map(|o| std::mem::take(&mut o.queued))
                                .unwrap_or_default();
                            for code in queued {
                                this.send_script_run(path, code, cx);
                            }
                        }
                        this.session_changed(cx);
                    }
                    Err(err) => {
                        *slot = Session::Failed(format!("{err:#}"));
                        if let Some(open) = notebook_path.as_deref().and_then(|p| this.open_at(p)) {
                            open.waiting.clear();
                            open.queued.clear();
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
        self.bind_console_document(path, cx);
        let Some(daemon) = self.daemon() else {
            return;
        };
        let by_user = !self.agent_running;
        let Some(open) = self.open_at(path) else {
            return;
        };
        let (Some(session), Some(cell), Some(editor)) = (
            open.session.id(),
            open.notebook
                .source_notebook()
                .unwrap_or(&open.notebook)
                .cells
                .get(index),
            open.quarto_view
                .as_ref()
                .map_or(&open.editors, |view| &view.editors)
                .get(index),
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
        // A new run takes the place of the last suggestion.
        open.ghost = None;
        open.ghost_after = by_user.then(|| cell_id.clone());
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
        // `?topic` and `help(x)` open the Help pane, as in the Tauri app.
        if let Some(asked) = crate::help::console_help(&code, self.console_language()) {
            self.console_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.ask_help(asked, window, cx);
            return;
        }
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.console_target().map(str::to_string))
        else {
            self.problem =
                Some("Choose a kernel and wait for it to be ready before running code.".into());
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

    /// Stops the shown document's kernel (or the Console's) for good; the
    /// next run starts a new one.
    pub(crate) fn shut_down_session(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() {
            self.stop_console_session(cx);
            return;
        }
        let daemon = self.daemon();
        let slot = match self.open.as_mut() {
            Some(open) => &mut open.session,
            None => &mut self.loose,
        };
        if let (Some(daemon), Some(id)) = (daemon, slot.id()) {
            let id = id.to_string();
            self.machine.sessions.retain(|s| s.id != id);
            daemon.fire(Call::Shutdown { session: id });
        }
        *slot = Session::None;
        self.session_changed(cx);
        cx.notify();
    }

    /// Restarts the shown document's kernel: a new one of the same kind,
    /// with nothing defined.
    pub(crate) fn restart_session(&mut self, cx: &mut Context<Self>) {
        if self.active_session().is_none() {
            return;
        }
        self.shut_down_session(cx);
        self.start_session(cx);
        cx.notify();
    }

    /// Closes a document's tab, asking first when it has unsaved changes.
    pub(crate) fn ask_close(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_media(&path, cx) {
            return;
        }
        let Some(open) = self.open_at(&path) else {
            return;
        };
        if !open.dirty {
            self.close_tab(&path, cx);
            return;
        }
        let name = crate::files::base_name(&path).to_string();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Save changes to {name}?"),
            Some("Running code keeps running either way."),
            &["Save", "Don't save", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| match answer {
                Ok(0) => this.close_tab(&path, cx),
                Ok(1) => {
                    if let Some(open) = this.open_at(&path) {
                        open.dirty = false;
                    }
                    this.close_tab(&path, cx);
                }
                _ => {}
            })
            .ok();
        })
        .detach();
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
            || self.loose.id() == Some(event.session.as_str())
            || self.console_sessions.retained.contains(&event.session);
        if !ours {
            return;
        }
        if !self.accepts_console_replay(&event) {
            return;
        }
        let active = self.console_target() == Some(event.session.as_str());
        if event.msg_type == EVENT_SESSION_ENDED {
            for run in self
                .runs
                .values_mut()
                .filter(|r| !r.finished && r.session == event.session)
            {
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
            for state in self.console_sessions.states.values_mut() {
                if state.id() == Some(event.session.as_str()) {
                    *state = ended();
                }
            }
            if self.console_sessions.retained.remove(&event.session) {
                self.save_console_choice();
            }
            self.console_sessions.live.retain(|s| s.id != event.session);
            cx.notify();
            return;
        }
        // Events can arrive before the run id does, so keep them by run.
        let Some(run_id) = event.run else {
            return;
        };
        let queue_moved = matches!(
            event.msg_type.as_str(),
            EVENT_RUN_STARTED | EVENT_RUN_FINISHED
        );
        let (session, ts_ms) = (event.session.clone(), event.ts_ms);
        if self
            .runs
            .get(&run_id)
            .is_some_and(|run| event.seq <= run.last_seq)
        {
            return;
        }
        if active && !event.msg_type.starts_with("status") {
            self.console_scroll.scroll_to_bottom();
        }
        match event.msg_type.as_str() {
            EVENT_RUN_STARTED if !event.content["quiet"].as_bool().unwrap_or(false) => {
                self.explorer_run(&event.session, true, cx)
            }
            EVENT_RUN_FINISHED if !self.runs.get(&run_id).is_some_and(|r| r.quiet) => {
                self.explorer_run(&event.session, false, cx)
            }
            _ => {}
        }
        let language = self
            .session_language(&event.session)
            .map(|language| editor_language(Some(language)));
        let run = self.runs.entry(run_id.clone()).or_default();
        run.last_seq = event.seq;
        if language.is_some() {
            run.language = language;
        }
        match event.msg_type.as_str() {
            EVENT_RUN_STARTED => {
                run.session = event.session.clone();
                run.queued_ms = Some(event.ts_ms);
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
                    self.console.push(run_id.clone());
                    if self.console.len() > CONSOLE_RUNS {
                        self.console.remove(0);
                    }
                }
                if !run.quiet && event.seq > run.snapshot_seq {
                    let code = run.code.clone();
                    self.record_console_command(&session, run_id.clone(), code, event.seq);
                }
            }
            EVENT_RUN_FINISHED => {
                self.plots_run_finished(&event.session, &run_id, cx);
                let run = self.runs.entry(run_id.clone()).or_default();
                run.finished = true;
                run.status = event.content["status"].as_str().map(str::to_string);
                run.execution_count = event.content["execution_count"].as_u64();
                run.finished_ms = Some(event.ts_ms);
                if !run.quiet && self.variables_session() == Some(event.session.as_str()) {
                    self.refresh_variables(cx);
                    self.packages_run_finished(cx);
                }
                self.suggest_after_run(&run_id, cx);
            }
            _ if run.quiet => {}
            msg_type => {
                let is_plot = matches!(
                    msg_type,
                    "display_data" | "execute_result" | "update_display_data"
                );
                run.append_output(event.seq, event.msg_type, event.content);
                if is_plot {
                    let for_viewer = run.cell.as_ref().is_none_or(|(file, _)| is_script(file));
                    let outputs = run.outputs.clone();
                    let page = outputs.iter().any(|o| matches!(show(o), Shown::Page(_)));
                    if page && for_viewer && active {
                        self.bottom_tab = BottomTab::Viewer;
                        self.viewer_index = None;
                    }
                    self.fetch_plots(&outputs, cx);
                    self.plots_from_run(&event.session, &run_id, cx);
                }
            }
        }
        if queue_moved {
            self.advance_queue(&session, ts_ms, cx);
        }
        cx.notify();
    }

    /// Lists the active kernel's variables again, and the parts open.
    pub(crate) fn refresh_variables(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
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

    pub(crate) fn show_tab(
        &mut self,
        tab: ConsoleTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_middle();
        self.console_tab = tab;
        self.show_console = true;
        if self.panel_collapsed(crate::panes::Panel::Console) {
            self.middle_vertical = crate::panes::Vertical::Split;
        }
        if tab == ConsoleTab::Terminal {
            self.ensure_terminal(cx);
            if let Some(terminal) = &self.terminal {
                let focus = terminal.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    /// The pages the Console's runs (and scripts') showed, oldest first,
    /// with their titles.
    pub(crate) fn run_pages(&self) -> Vec<crate::viewer::ViewerItem> {
        self.console
            .iter()
            .filter_map(|id| self.runs.get(id))
            .filter(|run| run.cell.as_ref().is_none_or(|(file, _)| is_script(file)))
            .flat_map(|run| run.outputs.iter())
            .filter_map(|output| match show(output) {
                Shown::Page(page) => Some(crate::viewer::ViewerItem {
                    page: Some(page),
                    title: page_title(output),
                    note: None,
                    extra: None,
                    stoppable: false,
                }),
                _ => None,
            })
            .collect()
    }

    /// Keeps a web view for each page about to be drawn, and drops the rest:
    /// a web view is a native view over the window, so one left over would
    /// stay on screen.
    pub(crate) fn sync_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_media(cx);
        let mut wanted: Vec<Page> = Vec::new();
        if self.panel_visible(crate::panes::Panel::Editor)
            && !self.panel_collapsed(crate::panes::Panel::Editor)
            && !self.objects.is_shown()
            && self.explorer.is_none()
            && let Some(open) = &self.open
        {
            for (ix, cell) in open.notebook.cells.iter().enumerate() {
                // A page under a cell not drawn would stay where it was.
                if cell.kind != CellKind::Code || !self.placed.shows(ix) {
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
        let viewer_page = (self.panel_visible(crate::panes::Panel::LeftBottom)
            && !self.panel_collapsed(crate::panes::Panel::LeftBottom)
            && self.bottom_tab == BottomTab::Viewer)
            .then(|| self.viewer_page())
            .flatten()
            .and_then(|(item, ..)| item.page);
        let viewer_shown = viewer_page.is_some();
        wanted.extend(viewer_page);
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
        let in_notebook = wanted.len() - usize::from(viewer_shown);
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
    pub(crate) fn open_explorer(
        &mut self,
        path: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_object_explorer();
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            return;
        };
        self.reveal_middle();
        let language = self
            .active_kernel()
            .and_then(|k| self.kernels.iter().find(|s| s.name == k))
            .map_or_else(|| "python".to_string(), |s| s.language.to_ascii_lowercase());
        let project = self.project.clone();
        self.explorer =
            Some(cx.new(|cx| Explorer::new(daemon, session, language, path, project, window, cx)));
        if let Some(explorer) = &self.explorer {
            cx.subscribe_in(
                explorer,
                window,
                |this, _, _: &crate::explorer::FocusChanged, window, cx| {
                    this.report_focus(window, cx);
                },
            )
            .detach();
            cx.subscribe(
                explorer,
                |this, explorer, _: &crate::explorer::PopOut, cx| {
                    this.pop_out_explorer(explorer, cx);
                },
            )
            .detach();
        }
        self.explorer_shown = true;
        self.diff_shown = false;
        cx.notify();
    }

    /// Code starts or ends in a session: the Data Explorer waits for it,
    /// then reads its table again.
    fn explorer_run(&mut self, session: &str, busy: bool, cx: &mut Context<Self>) {
        self.object_explorers_run(session, busy, cx);
        self.popped_explorers
            .retain(|(_, explorer)| explorer.upgrade().is_some());
        let explorers = self
            .explorer
            .iter()
            .cloned()
            .chain(
                self.popped_explorers
                    .iter()
                    .filter_map(|(_, e)| e.upgrade()),
            )
            .collect::<Vec<_>>();
        for explorer in explorers {
            if explorer.read(cx).session() == session {
                explorer.update(cx, |e, cx| e.set_busy(busy, cx));
            }
        }
    }

    pub(crate) fn close_explorer(&mut self, cx: &mut Context<Self>) {
        self.explorer = None;
        self.explorer_shown = false;
        cx.notify();
    }

    /// Moves the Data Explorer into a window of its own; its tab goes.
    fn pop_out_explorer(&mut self, explorer: Entity<Explorer>, cx: &mut Context<Self>) {
        if self.explorer.as_ref() != Some(&explorer) {
            return;
        }
        self.explorer = None;
        self.explorer_shown = false;
        explorer.update(cx, |e, cx| e.set_popped(cx));
        let weak = explorer.downgrade();
        if let Some(handle) = crate::app_window::open_explorer_window(explorer, cx) {
            self.popped_explorers.push((handle, weak));
        }
        cx.notify();
    }

    /// Closes the explorer windows this project opened.
    fn close_popped_explorers(&mut self, cx: &mut Context<Self>) {
        for (handle, _) in std::mem::take(&mut self.popped_explorers) {
            cx.update_window(handle, |_, window, _| window.remove_window())
                .ok();
        }
    }

    /// Finds the project's shell terminal, or starts one, and follows it.
    fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.is_some() || self.terminal_starting || self.restricted() {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.terminal_starting = true;
        self.terminal_problem = None;
        let listed = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
        let generation = self.generation;
        let project_root = project.clone();
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
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.terminal_starting = false;
                match info {
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
                        let handle = this.window_handle;
                        let view = cx.new(|cx| {
                            let mut view = TerminalView::new(id, input, resize, handle, cx);
                            view.set_file_links(
                                daemon.clone(),
                                project_root.clone(),
                                info.cwd.clone(),
                                cx,
                            );
                            view
                        });
                        cx.subscribe_in(
                            &view,
                            window,
                            |this, _, link: &gpui_cn::terminal::FileLink, window, cx| {
                                this.open_file_location(
                                    link.path.clone(),
                                    link.line,
                                    link.column,
                                    window,
                                    cx,
                                );
                            },
                        )
                        .detach();
                        if this.console_tab == ConsoleTab::Terminal {
                            let focus = view.read(cx).focus_handle();
                            window.focus(&focus, cx);
                        }
                        this.terminal = Some(view);
                        // Install the view before subscribing: the daemon's
                        // first replay event may follow its response at once.
                        let subscribed =
                            daemon.call::<TerminalSubscribed>(Call::SubscribeTerminal {
                                terminal: info.id,
                                from: None,
                                chat_from: None,
                            });
                        cx.spawn(async move |this, cx| {
                            if let Err(err) = subscribed.await {
                                this.update(cx, |this, cx| {
                                    if this.generation == generation {
                                        this.terminal_problem = Some(format!("{err:#}"));
                                        cx.notify();
                                    }
                                })
                                .ok();
                            }
                        })
                        .detach();
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
        if let Some(quota) = &self.quota {
            quota.update(cx, |quota, cx| quota.terminal_event(&event, cx));
        }
        if let Some(agents) = self.agents.clone()
            && agents.read(cx).owns(&event.terminal)
        {
            agents.update(cx, |agents, cx| agents.on_terminal_event(event, cx));
            // The title bar's dot shows an agent waiting.
            cx.notify();
            return;
        }
        if self.on_job_event(&event, cx) {
            return;
        }
        let Some(view) = self.terminal.clone() else {
            return;
        };
        if view.read(cx).id != event.terminal {
            return;
        }
        view.update(cx, |view, cx| {
            if let Some(data) = &event.data {
                view.feed(data, event.replay, cx);
            }
            if let Some(exit) = &event.exit {
                view.exited(format!("ended: {exit:?}"), cx);
            }
        });
    }

    /// Offscreen cells keep their output model, but image requests only start
    /// when their views are needed. Opening a notebook must not queue all plots.
    pub(crate) fn fetch_notebook_plots(&mut self, cx: &mut Context<Self>) {
        let mut outputs = Vec::new();
        if let Some(open) = &self.open {
            for (ix, cell) in open.notebook.cells.iter().enumerate() {
                if !self.placed.shows(ix) {
                    continue;
                }
                let run = open
                    .cell_runs
                    .get(&cell.id)
                    .and_then(|id| self.runs.get(id));
                outputs.extend(
                    run.map_or(&cell.outputs, |run| &run.outputs)
                        .iter()
                        .cloned(),
                );
            }
        }
        self.fetch_plots(&outputs, cx);
    }

    /// Starts loading the plots in `outputs` that aren't loaded yet.
    fn fetch_plots(&mut self, outputs: &[Value], cx: &mut Context<Self>) {
        for output in outputs {
            if let Shown::Plot { mime, value } = show(output) {
                self.fetch_image(&mime, &value, cx);
            }
        }
    }

    /// Reads an image (inline, or a blob from the daemon) into the plot
    /// cache, under [`plot_key`], unless it is there.
    pub(crate) fn fetch_image(&mut self, mime: &str, value: &Value, cx: &mut Context<Self>) {
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        let key = plot_key(value);
        if self.plots.contains_key(&key) {
            return;
        }
        let format = match mime {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            _ => ImageFormat::Svg,
        };
        let text = match (value, self.daemon()) {
            (Value::String(text), _) => {
                let text = text.clone();
                Box::pin(async move { anyhow::Ok(text) })
                    as std::pin::Pin<Box<dyn Future<Output = anyhow::Result<String>>>>
            }
            (blob, Some(daemon)) if blob[BLOB_KEY].is_string() => {
                let fetch = daemon.call::<Blob>(Call::GetBlob {
                    hash: blob[BLOB_KEY].as_str().unwrap_or_default().to_string(),
                    near,
                });
                Box::pin(async move { Ok(fetch.await?.data) })
            }
            _ => return,
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

/// CSV/TSV's coloured columns require an unwrapped display. Unknown code
/// languages stay distinct from plain text even when their highlighter is text.
fn editor_wrap_type(
    format: Format,
    kind: CellKind,
    language: Option<&str>,
) -> Option<&'static str> {
    if format == Format::Delimited {
        return None;
    }
    if kind == CellKind::Text || format == Format::Markdown {
        return Some("markdown");
    }
    if format == Format::Text {
        match language {
            Some("fasta") => return Some("fasta"),
            Some("fastq") => return Some("fastq"),
            Some("bio-table") => return Some("bio-table"),
            Some("nextflow") => return Some("nextflow"),
            Some("wdl") => return Some("wdl"),
            Some("bibtex") => return Some("bibtex"),
            _ => {}
        }
    }
    let language = editor_language(language);
    Some(if language == "text" && format != Format::Text {
        "code"
    } else {
        language
    })
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

/// The plot's bytes: base64 for bitmaps, the text itself for SVG.
fn decode(format: ImageFormat, text: String) -> anyhow::Result<Vec<u8>> {
    if format == ImageFormat::Svg {
        return Ok(text.into_bytes());
    }
    // Ark sends PNG data without base64 padding; use the shared lenient
    // decoder the daemon uses for kernel output.
    let clean: String = text.split_whitespace().collect();
    Ok(sandhill_proto::decode_base64(&clean)?)
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

#[cfg(test)]
mod opening_tests {
    use gpui_kit::{AppContext, Point, TestAppContext, WeakEntity, WindowOptions, px};
    use sandhill_proto::{CellOutputs, FileOutputs};
    use serde_json::json;
    use std::sync::Arc;

    use super::{Workbench, editor_wrap_type};
    use crate::notebook::{CellKind, Format};

    #[test]
    fn wrapping_types_follow_cells_and_exclude_delimited_data() {
        assert_eq!(
            editor_wrap_type(Format::Text, CellKind::Code, Some("text")),
            Some("text")
        );
        assert_eq!(
            editor_wrap_type(Format::Text, CellKind::Code, Some("json")),
            Some("json")
        );
        assert_eq!(
            editor_wrap_type(Format::Ipynb, CellKind::Code, Some("julia")),
            Some("code")
        );
        assert_eq!(
            editor_wrap_type(Format::Quarto, CellKind::Text, Some("r")),
            Some("markdown")
        );
        assert_eq!(
            editor_wrap_type(Format::Quarto, CellKind::Code, Some("python")),
            Some("python")
        );
        assert_eq!(
            editor_wrap_type(Format::Delimited, CellKind::Code, None),
            None
        );
    }

    #[gpui_kit::test]
    fn opening_keeps_complete_sources_and_outputs_with_lazy_views(cx: &mut TestAppContext) {
        let cells: Vec<_> = (0..1500)
            .map(|ix| {
                json!({
                    "cell_type": "code", "id": format!("cell-{ix}"), "metadata": {},
                    "source": format!("value = {ix}\nprint(value)\n"),
                    "execution_count": null, "outputs": []
                })
            })
            .collect();
        let text = json!({
            "nbformat": 4, "nbformat_minor": 5,
            "metadata": {"language_info": {"name": "python"}}, "cells": cells
        })
        .to_string();
        let nb = crate::notebook::parse("/large.ipynb", &text).unwrap();
        let saved = FileOutputs {
            file: nb.path.clone(),
            seq: 1,
            cells: nb
                .cells
                .iter()
                .map(|cell| CellOutputs {
                    cell_id: cell.id.clone(),
                    run: cell.id.clone(),
                    session: "saved".into(),
                    language: "python".into(),
                    code: cell.source.clone(),
                    started_ms: 0,
                    finished_ms: Some(1),
                    status: Some("ok".into()),
                    execution_count: Some(1),
                    outputs: vec![json!({
                        "output_type": "stream", "name": "stdout",
                        "text": "saved output\n".repeat(20)
                    })],
                })
                .collect(),
        };
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            // Never connect to or change the user's daemon or project.
            crate::resource_manager::block_host(sandhill_link::LOCAL, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        rt,
                        sandhill_link::Target::Local,
                        None,
                        WeakEntity::new_invalid(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| {
                this.tabs.push(nb.path.clone());
                this.show_notebook(nb, 0, window, cx);
                this.show_saved_outputs(saved, &[], false, cx);
                this.place_cells(window);
                let near = this.placed.near.iter().filter(|near| **near).count();
                assert!(near > 0 && near < 50, "initial cell views: {near}");
                assert!(!this.placed.shows(1499));
                let open = this.open.as_ref().unwrap();
                assert_eq!(open.editors.len(), 1500);
                assert_eq!(open.cell_runs.len(), 1500);
                assert_eq!(this.runs["cell-1499"].outputs.len(), 1);
                // Saving and every editor-based action still see the last cell.
                let sources = open.sources(cx);
                assert_eq!(sources.len(), 1500);
                assert_eq!(sources[1499], "value = 1499\nprint(value)\n");
                let written = open.notebook.render(&sources).unwrap();
                let reopened = crate::notebook::parse("/large.ipynb", &written).unwrap();
                assert_eq!(reopened.cells.len(), 1500);
                assert_eq!(reopened.cells[1499].source, sources[1499]);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        })
        .unwrap();
        // Newly measured output views above the viewport must not move its anchor.
        let anchor = cx
            .update_window(handle, |_, _, cx| {
                workbench.update(cx, |this, cx| {
                    let scroll = &this.notebook_scroll;
                    scroll.set_offset(Point {
                        x: px(0.),
                        y: -scroll.max_offset().y / 2.,
                    });
                    let anchor = scroll.logical_scroll_top();
                    cx.notify();
                    anchor
                })
            })
            .unwrap();
        for _ in 0..3 {
            cx.update_window(handle, |_, window, cx| {
                workbench.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        cx.read(|cx| {
            let scroll = &workbench.read(cx).notebook_scroll;
            let after = scroll.logical_scroll_top();
            assert_eq!(after.0, anchor.0);
            assert!((after.1 - anchor.1).abs() <= px(0.5));
        });
        // Outline/find navigation must still reach the unrendered final cell.
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| {
                this.jump_to_source(1499, 0, None, window, cx);
                cx.notify();
            });
        })
        .unwrap();
        for _ in 0..2 {
            cx.update_window(handle, |_, window, cx| {
                workbench.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        cx.read(|cx| assert!(workbench.read(cx).placed.shows(1499)));
    }
}
