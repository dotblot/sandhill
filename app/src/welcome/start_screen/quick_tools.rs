//! Welcome's Explore a Table and R and Python pages, and their shared jobs.

use std::collections::{HashSet, VecDeque};

use gpui_kit::component::checkbox::Checkbox;
use sandhill_proto::{
    EnvAction, EnvActionPlan, FailureFix, InstallPrefs, InstallTool, InstalledKey, NewRuntime,
    PYTHON_ENV_NAME, PYTHON_SETS, PYTHON_VERSIONS, PackageAction, PackageChange, PackagePlan,
    QuickInstalled, QuickInstalling, QuickReadiness, QuickTool, RuntimePlan, ipykernel_args,
    python_env_name_ok,
};

use crate::install_confirm::{self as review, Choice, Job, Review};
use crate::view::quick_archive::{ArchivePicker, TABLE_TYPES};
use sandhill_proto::{QuickArchiveOpened, QuickArchiveSelection};

use super::*;

use crate::welcome::folder_dialog::choose_local_path;

mod cards;
mod environment;

const NODE: &str = "https://nodejs.org/";

#[derive(Clone, PartialEq, Eq)]
enum PackageRow {
    Plots,
    ChangedGenes,
    Symbols(String),
    Card,
    Retry(Section),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeRow {
    CranR,
    BrewR,
    CondaR,
    Python,
    Miniforge,
}

enum QuickReview {
    Packages(PackageRow, Vec<String>, Vec<Review<PackagePlan>>),
    Runtime(RuntimeRow, Review<RuntimePlan>),
    Environment(Review<EnvActionPlan>),
    Kernel(Review<sandhill_proto::RWithoutIrkernel>),
}

/// Where to show a job's progress.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Cards,
    Environment,
    Ai,
    Record,
}

/// The new Python environment's choices, while its form is open.
struct PythonForm {
    name: Entity<InputState>,
    _changed: Subscription,
    version: &'static str,
    sets: Vec<&'static str>,
    tool: InstallTool,
    parent: Option<String>,
}

fn agent_name(kind: &str) -> &'static str {
    if kind == "claude" {
        "Claude Code"
    } else {
        "Codex"
    }
}

struct Install {
    section: Section,
    job: Job,
    notice: Option<String>,
}

/// A question asked in its row before something runs.
#[derive(Clone, PartialEq, Eq)]
enum Confirm {
    Agent(String),
    Remove,
    Ipykernel(String),
}

#[derive(Default)]
pub(crate) struct QuickPage {
    readiness: Option<Result<QuickReadiness, String>>,
    checking: bool,
    install: Option<Install>,
    starting: Option<Section>,
    install_error: Option<(Section, String)>,
    notice: Option<(Section, String)>,
    reviewing: Option<QuickReview>,
    plan_id: u64,
    setting_prefs: bool,
    prefs_error: Option<String>,
    selected: HashSet<InstalledKey>,
    confirm: Option<Confirm>,
    environment: environment::State,
    cards: cards::State,
    waiting: VecDeque<PackageChange>,
    python_form: Option<PythonForm>,
    /// Tables being opened.
    opening: bool,
    archive_picker: Option<ArchivePicker>,
    /// A recent session whose actions are shown, and one asked about
    /// deleting.
    actions: Option<String>,
    deleting: Option<String>,
}

/// A path's folder: where the session folder goes by default.
fn parent_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl QuickPage {
    pub(super) fn new() -> Self {
        Self {
            environment: environment::State::new(),
            ..Self::default()
        }
    }

    fn busy(&self) -> bool {
        self.starting.is_some()
            || self
                .install
                .as_ref()
                .is_some_and(|i| i.job.finished.is_none())
    }

    fn clear_review(&mut self) {
        self.reviewing = None;
        self.confirm = None;
        self.plan_id += 1;
    }

    /// Checks this Mac again the next time the page shows.
    pub(super) fn forget_check(&mut self) {
        if !self.checking && !self.busy() && !self.environment.switching {
            self.readiness = None;
            self.cards.list = None;
        }
    }

    pub(super) fn focus_explore_r(&mut self) {
        self.cards.focus_explore = true;
    }
}

/// The R version line of the card.
fn r_line(readiness: &QuickReadiness) -> (bool, String) {
    match &readiness.r {
        Some(r) if r.new_enough => (true, format!("R {}", r.version)),
        Some(r) => (
            false,
            format!(
                "R {} is too old: exploring tables needs R 4.4 or newer.",
                if r.version.is_empty() {
                    "?"
                } else {
                    &r.version
                }
            ),
        ),
        None => (
            false,
            look::mac_or(
                "R isn't installed on this Mac.",
                "R isn't installed on this computer.",
            )
            .into(),
        ),
    }
}

impl AppWindow {
    fn local_daemon(&self) -> Option<Rc<Daemon>> {
        match self
            .start
            .machines
            .get(sandhill_link::LOCAL)
            .map(|m| &m.link)
        {
            Some(MachineLink::Ready(daemon)) => Some(daemon.clone()),
            _ => None,
        }
    }

    /// Reads what this Mac has for exploring tables.
    pub(super) fn quick_check(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        if self.start.quick.checking {
            return;
        }
        self.start.quick.checking = true;
        let read = daemon.call::<QuickReadiness>(Call::QuickReadiness);
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                this.start.quick.checking = false;
                this.start.quick.readiness = Some(read.map_err(|err| format!("{err:#}")));
                this.quick_resume_setup(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn quick_plan_packages(&mut self, row: PackageRow, cx: &mut Context<Self>) {
        let Some(Ok(found)) = &self.start.quick.readiness else {
            return;
        };
        let (target, libraries) = if found.prefs.mode == Some(sandhill_proto::EnvMode::Own) {
            let Some(r) = found.own.as_ref().and_then(|own| own.r.as_ref()) else {
                return;
            };
            (environment::own_r_target(r), Vec::new())
        } else {
            let Some(r) = &found.r else { return };
            let libraries: Vec<_> = r
                .libraries
                .iter()
                .filter(|l| l.writable)
                .map(|l| l.path.clone())
                .collect();
            (
                review::r_target(r.path.clone(), &libraries, &found.prefs),
                libraries,
            )
        };
        let (tool, reason) = match &row {
            PackageRow::Plots => (QuickTool::Plot, "Explore a Table needs it to make plots"),
            PackageRow::ChangedGenes => (QuickTool::Differential, "Find Changed Genes needs it"),
            PackageRow::Symbols(name) => (
                QuickTool::Symbols,
                if name == "org.Hs.eg.db" {
                    "Gene symbols for human genes"
                } else {
                    "Gene symbols for mouse genes"
                },
            ),
            PackageRow::Card | PackageRow::Retry(_) => return,
        };
        let packages = found
            .packages
            .iter()
            .filter(|p| {
                p.version.is_none()
                    && p.tool == tool
                    && match &row {
                        PackageRow::Symbols(name) => p.name == *name || p.name == "AnnotationDbi",
                        _ => true,
                    }
            })
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        if packages.is_empty() || self.start.quick.busy() {
            return;
        }
        let mut change = PackageChange {
            action: PackageAction::Install,
            target,
            packages,
            tool: None,
            reason: reason.into(),
        };
        let automatic = review::automatic_tool(&change, &found.prefs);
        change.tool = automatic;
        self.quick_request_packages(row, libraries, vec![change], automatic, cx);
    }

    fn quick_request_packages(
        &mut self,
        row: PackageRow,
        libraries: Vec<String>,
        changes: Vec<PackageChange>,
        automatic: Option<InstallTool>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.busy() || changes.is_empty() {
            return;
        }
        q.clear_review();
        q.python_form = None;
        let count = changes.len();
        q.reviewing = Some(QuickReview::Packages(
            row,
            libraries,
            (0..count).map(|_| Review::Planning).collect(),
        ));
        for (ix, change) in changes.into_iter().enumerate() {
            self.quick_plan_package(&daemon, ix, change, automatic.filter(|_| count == 1), cx);
        }
        cx.notify();
    }

    fn quick_plan_package(
        &self,
        daemon: &Daemon,
        ix: usize,
        change: PackageChange,
        automatic: Option<InstallTool>,
        cx: &mut Context<Self>,
    ) {
        let id = self.start.quick.plan_id;
        let pending = daemon.call::<PackagePlan>(Call::PlanPackages { change });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                if q.plan_id != id {
                    return;
                }
                let Some(QuickReview::Packages(_, _, plans)) = &mut q.reviewing else {
                    return;
                };
                if let (Some(tool), Ok(plan)) = (automatic, &result)
                    && plan.change.tool == Some(tool)
                    && review::runnable(plan)
                {
                    this.quick_run_packages(plan, true, cx);
                } else {
                    plans[ix] = result.into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn quick_change_plan(&mut self, ix: usize, choice: Choice, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        let Some(QuickReview::Packages(_, _, plans)) = &mut q.reviewing else {
            return;
        };
        let Some(Review::Ready(plan)) = plans.get(ix) else {
            return;
        };
        let mut change = plan.change.clone();
        choice.apply(&mut change);
        if change != plan.change {
            plans[ix] = Review::Planning;
            self.quick_plan_package(&daemon, ix, change, None, cx);
            cx.notify();
        }
    }

    fn quick_plan_runtime(&mut self, row: RuntimeRow, runtime: NewRuntime, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.busy() {
            return;
        }
        q.clear_review();
        if row != RuntimeRow::Python {
            q.python_form = None;
        }
        q.reviewing = Some(QuickReview::Runtime(row, Review::Planning));
        let id = q.plan_id;
        let pending = daemon.call::<RuntimePlan>(Call::PlanRuntime { runtime });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            this.update(cx, |this, cx| {
                if this.start.quick.plan_id == id {
                    this.start.quick.reviewing = Some(QuickReview::Runtime(row, result.into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_run_review(&mut self, cx: &mut Context<Self>) {
        match &self.start.quick.reviewing {
            Some(QuickReview::Packages(_, _, plans)) => {
                if plans
                    .iter()
                    .any(|plan| matches!(plan, Review::Planning | Review::Error(_)))
                {
                    return;
                }
                let mut runnable = plans.iter().filter_map(|plan| match plan {
                    Review::Ready(plan) if review::runnable(plan) => Some(plan),
                    _ => None,
                });
                if let Some(plan) = runnable.next().cloned() {
                    self.start.quick.waiting = runnable.map(|plan| plan.change.clone()).collect();
                    self.quick_run_packages(&plan, false, cx);
                }
            }
            Some(QuickReview::Runtime(_, Review::Ready(plan)))
                if plan.refused.is_none() && !plan.command.is_empty() =>
            {
                let runtime = plan.runtime.clone();
                let title = format!("Installing {}", review::runtime_name(&runtime));
                self.quick_start_card_job(Call::RunRuntime { runtime }, title, None, cx);
            }
            Some(QuickReview::Environment(Review::Ready(plan)))
                if plan.refused.is_none() && !plan.command.is_empty() =>
            {
                let action = plan.action.clone();
                let title = review::env_job_title(&action).into();
                self.quick_start_card_job(Call::RunEnvAction { action }, title, None, cx);
            }
            _ => {}
        }
    }

    fn quick_run_packages(&mut self, plan: &PackagePlan, automatic: bool, cx: &mut Context<Self>) {
        let call = Call::RunPackages {
            change: plan.change.clone(),
        };
        let title = review::progress_title(&plan.change);
        let notice = automatic.then(|| review::auto_notice(plan));
        if let Some(QuickReview::Packages(PackageRow::Retry(section), _, _)) =
            self.start.quick.reviewing.as_ref()
        {
            self.quick_start_install(call, *section, title, notice, cx);
        } else {
            self.quick_start_card_job(call, title, notice, cx);
        }
    }

    fn quick_start_install(
        &mut self,
        call: Call,
        section: Section,
        title: String,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.busy() {
            return;
        }
        q.clear_review();
        q.python_form = None;
        q.install = None;
        q.install_error = None;
        q.notice = None;
        q.starting = Some(section);
        let kernel = matches!(&call, Call::InstallIrkernel { .. });
        let change = match &call {
            Call::RunPackages { change } => Some(change.clone()),
            _ => None,
        };
        let started = if kernel {
            let call = daemon.call::<TerminalInfo>(call);
            cx.background_spawn(async move { call.await.map(|job| (job, None)) })
        } else {
            let call = daemon.call::<QuickInstalling>(call);
            cx.background_spawn(async move { call.await.map(|i| (i.job, Some(i.log))) })
        };
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                q.starting = None;
                match started {
                    Ok((info, log)) => {
                        let mut job = Job::new(info, log, title);
                        job.change = change;
                        review::watch(
                            daemon,
                            sandhill_link::LOCAL.to_string(),
                            &job,
                            Self::quick_install_status,
                            cx,
                        )
                        .detach();
                        q.install = Some(Install {
                            section,
                            job,
                            notice,
                        });
                    }
                    Err(err) => {
                        q.install_error = Some((section, format!("{title} didn't start: {err}")));
                        this.quick_advance_jobs(section, false, cx);
                        this.quick_refresh_after_job(section, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_install_status(
        &mut self,
        id: &str,
        status: review::JobStatus,
        cx: &mut Context<Self>,
    ) -> bool {
        let q = &mut self.start.quick;
        let Some(install) = q.install.as_mut().filter(|i| i.job.id == id) else {
            return false;
        };
        install.job.update(status);
        let finished = install.job.finished;
        if finished.is_some() {
            let section = install.section;
            if self.quick_advance_jobs(section, finished == Some(true), cx) {
                return true;
            }
            let q = &mut self.start.quick;
            let install = q.install.as_mut().expect("the job just finished");
            if finished == Some(true) && install.notice.is_some() {
                q.notice = install
                    .notice
                    .take()
                    .map(|notice| (install.section, notice));
            }
            self.quick_refresh_after_job(section, cx);
        }
        cx.notify();
        true
    }

    fn quick_refresh_after_job(&mut self, section: Section, cx: &mut Context<Self>) {
        self.quick_check(cx);
        let local = sandhill_link::LOCAL;
        if section == Section::Ai {
            if let Some((_, _, accounts)) = self
                .start
                .accounts
                .as_ref()
                .filter(|(host, _, _)| host == local)
            {
                accounts.update(cx, |accounts, cx| accounts.load_agents(cx));
            }
        } else {
            self.quick_invalidate_cards(cx);
        }
    }

    /// Run a batch one change at a time, stopping if a command fails.
    fn quick_advance_jobs(
        &mut self,
        section: Section,
        success: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let q = &mut self.start.quick;
        if !success {
            if !q.waiting.is_empty() {
                q.waiting.clear();
                q.notice = Some((section, "The remaining package changes didn't run.".into()));
            }
            return false;
        }
        let Some(change) = q.waiting.pop_front() else {
            return false;
        };
        let title = review::progress_title(&change);
        self.quick_start_install(Call::RunPackages { change }, section, title, None, cx);
        true
    }

    /// Tables chosen or dropped open in Sandhill's own storage. Archives
    /// first show which tables can be opened.
    pub(super) fn quick_open(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        if self.start.quick.opening || self.start.quick.archive_picker.is_some() {
            return;
        }
        let (tables, archives): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .filter(|p| crate::quick::is_table(p) || crate::quick::is_archive(p))
            .partition(|p| crate::quick::is_table(p));
        self.start.page = Page::Explore;
        self.start.problem = None;
        if tables.is_empty() && archives.is_empty() {
            self.start.problem = Some(TABLE_TYPES.into());
            cx.notify();
            return;
        }
        if archives.is_empty() {
            self.quick_open_selected(tables, Vec::new(), false, cx);
            return;
        }
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        self.start.quick.opening = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picker = ArchivePicker::read(daemon, tables, archives, false).await;
            this.update(cx, |this, cx| {
                this.start.quick.opening = false;
                match picker {
                    Ok(picker) => this.start.quick.archive_picker = Some(picker),
                    Err(err) => {
                        this.start.problem = Some(format!("Could not read the archive: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn quick_open_archive_selection(&mut self, combine: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.start.quick.archive_picker.take() {
            let (tables, archives, _) = picker.into_selection();
            self.quick_open_selected(tables, archives, combine, cx);
        }
    }

    fn quick_open_selected(
        &mut self,
        tables: Vec<String>,
        archives: Vec<QuickArchiveSelection>,
        combine: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        self.start.quick.opening = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = if archives.is_empty() {
                daemon
                    .call::<String>(Call::QuickStart { tables })
                    .await
                    .map(|folder| (folder, None))
            } else {
                daemon
                    .call::<QuickArchiveOpened>(Call::QuickOpenArchives {
                        project: None,
                        tables,
                        archives,
                        combine,
                    })
                    .await
                    .map(|opened| (opened.folder, opened.combine))
            };
            this.update(cx, |this, cx| {
                this.start.quick.opening = false;
                match started {
                    Ok((folder, counts)) => {
                        cx.defer(move |cx| crate::shell::app_window::open_quick(folder, counts, cx))
                    }
                    Err(err) => this.start.problem = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn quick_choose_tables(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let tables = paths
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            this.update(cx, |this, cx| this.quick_open(tables, cx)).ok();
        })
        .detach();
    }

    fn quick_delete(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let deleted = daemon.call::<sandhill_proto::Deleted>(Call::DeletePath {
            path: path.clone(),
            trash: true,
        });
        cx.spawn(async move |this, cx| {
            let deleted = deleted.await;
            this.update(cx, |this, cx| {
                this.start.quick.deleting = None;
                match deleted {
                    Ok(_) => crate::prefs::forget_quick_session(&path),
                    Err(err) => this.start.problem = Some(format!("Deleting didn't work: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Whether this Mac has everything the plots need; `None` until
    /// checked.
    fn quick_ready(&self) -> Option<bool> {
        let Some(Ok(readiness)) = &self.start.quick.readiness else {
            return None;
        };
        Some(
            r_line(readiness).0
                && readiness.kernel.is_some()
                && readiness
                    .packages
                    .iter()
                    .all(|p| p.version.is_some() || p.tool != QuickTool::Plot),
        )
    }

    /// Start → Explore a Table: a place to drop tables, and what can be
    /// made from them.
    pub(super) fn explore_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        if self.start.quick.readiness.is_none() && !self.start.quick.checking {
            self.quick_check(cx);
        }
        let not_ready = self.quick_ready() == Some(false);
        v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(page_title(
                "Explore a Table",
                "Look at a table of results, counts or enrichment terms and make the usual plots, without setting up a project.",
                &p,
            ))
            .when(not_ready, |d| {
                d.child(
                    h_flex()
                        .px(px(look::PANE_PAD_X))
                        .gap(px(look::GAP_8))
                        .child(div().flex_1().text_color(p.warn).child(look::mac_or(
                            "This Mac needs a few things before tables can be explored.",
                            "This computer needs a few things before tables can be explored.",
                        )))
                        .child(
                            Button::new("explore-get-ready")
                                .small()
                                .label("Open R and Python")
                                .on_click(cx.listener(|this, _, _, cx| this.quick_show_explore_r(cx))),
                        ),
                )
            })
            .child(self.quick_drop_area(&p, cx))
            .child(look::settings_group(
                "Make Plots",
                vec![
                    offer_row(
                        "From differential expression results",
                        "Volcano and MA plots, genes up and down, a top genes table and a heatmap.",
                        &p,
                    ),
                    offer_row(
                        "From counts or normalized expression",
                        "Library sizes, PCA, sample correlation, a heatmap of variable genes and one gene by group.",
                        &p,
                    ),
                    offer_row(
                        "From enrichment results",
                        "Dot and bar plots of the most significant terms.",
                        &p,
                    ),
                ],
                &p,
            ))
            .child(look::settings_group(
                "Find Changed Genes",
                vec![offer_row(
                    "From raw counts and sample factors",
                    "Compare groups with DESeq2, edgeR or limma-voom. Add a sample sheet or enter the factors yourself.",
                    &p,
                )],
                &p,
            ))
            .child(look::settings_group(
                "Coming next",
                vec![
                    offer_row(
                        "Find Enriched Pathways",
                        "GO enrichment from the changed genes.",
                        &p,
                    ),
                ],
                &p,
            ))
            .into_any_element()
    }

    /// Start → R and Python.
    pub(super) fn ready_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        if self.start.quick.readiness.is_none() && !self.start.quick.checking {
            self.quick_check(cx);
        }
        if self.start.quick.cards.list.is_none() {
            self.quick_list_cards(cx);
        }
        if let Some(setup) = self.quick_setup_page(&p, cx) {
            return setup.into_any_element();
        }
        self.quick_focus_explore(window, cx);
        if self.start.show_install_record
            && self
                .start
                .quick
                .readiness
                .as_ref()
                .is_some_and(Result::is_ok)
        {
            self.start.show_install_record = false;
            self.start.record_anchor.scroll_to(window, cx);
        }
        v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(page_title(
                "R and Python",
                "See what is installed, add packages and check that your environments work.",
                &p,
            ))
            .child(self.quick_ready_card(&p, cx))
            .into_any_element()
    }

    fn quick_ready_card(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let q = &self.start.quick;
        let readiness = match &q.readiness {
            None => {
                return look::settings_box(
                    vec![
                        look::settings_row(
                            look::mac_or(
                                "Checking what this Mac has…",
                                "Checking what this computer has…",
                            ),
                            None,
                        )
                        .into_any_element(),
                    ],
                    p,
                )
                .into_any_element();
            }
            Some(Err(err)) => {
                return look::settings_box(
                    vec![
                        look::settings_row(
                            look::mac_or(
                                "Sandhill couldn't check this Mac",
                                "Sandhill couldn't check this computer",
                            ),
                            Some(look::settings_detail(err.clone(), p)),
                        )
                        .child(check_again(cx))
                        .into_any_element(),
                    ],
                    p,
                )
                .into_any_element();
            }
            Some(Ok(readiness)) => readiness,
        };
        let busy = q.busy() || q.checking || q.setting_prefs || q.environment.switching;
        let own = readiness.prefs.mode == Some(sandhill_proto::EnvMode::Own);
        let mut record = self.quick_record_rows(readiness, busy, p, cx);
        record.extend(self.quick_install_in(Section::Record, p, cx));
        let progress = self.quick_install_in(Section::Environment, p, cx);
        let copy_report = readiness
            .own
            .as_ref()
            .and_then(|folder| folder.copy_report.as_ref().map(|report| (folder, report)));
        v_flex()
            .gap(px(look::GAP_6))
            .child(self.quick_mode_card(readiness, busy, p, cx))
            .child(self.quick_tools_box(readiness, p, cx))
            .children(match copy_report {
                Some((folder, report)) => {
                    Some(self.quick_copy_report(&folder.folder, report, busy, progress, p, cx))
                }
                None => progress.map(|row| look::settings_box(vec![row], p)),
            })
            .child(self.quick_environment_cards(readiness, busy || q.cards.loading, own, p, cx))
            .child(
                heading("What Sandhill Installed", p)
                    .id("quick-installed-heading")
                    .anchor_scroll(Some(self.start.record_anchor.clone())),
            )
            .child(look::settings_box(record, p))
            .into_any_element()
    }

    fn quick_tools_box(
        &self,
        readiness: &QuickReadiness,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut tools = readiness
            .tools
            .iter()
            .map(|tool| {
                let name = review::tool_name(tool);
                let folder = parent_of(&tool.path);
                let version = tool.version.trim();
                if version.is_empty() {
                    format!("{name} ({folder})")
                } else if version.starts_with(name) {
                    format!("{version} ({folder})")
                } else {
                    format!("{name} {version} ({folder})")
                }
            })
            .collect::<Vec<_>>();
        if let Some(path) = &readiness.brew {
            let version = readiness
                .brew_version
                .trim()
                .strip_prefix("Homebrew ")
                .unwrap_or(readiness.brew_version.trim());
            tools.push(if version.is_empty() {
                format!("Homebrew ({})", parent_of(path))
            } else {
                format!("Homebrew {version} ({})", parent_of(path))
            });
        }
        look::settings_box(
            vec![
                look::settings_row(
                    "Install tools",
                    Some(look::settings_detail(
                        if tools.is_empty() {
                            "No mamba, conda, uv or Homebrew found.".into()
                        } else {
                            tools.join(", ")
                        },
                        p,
                    )),
                )
                .into_any_element(),
                self.quick_prefs_row(readiness, p, cx),
            ],
            p,
        )
    }

    fn quick_r_offers(
        &self,
        readiness: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if cfg!(target_os = "macos") {
            rows.push(self.quick_runtime_offer(
                RuntimeRow::CranR,
                NewRuntime::RFromCran,
                "Install R from CRAN…",
                busy,
                p,
                cx,
            ));
        }
        if readiness.brew.is_some() {
            rows.push(self.quick_runtime_offer(
                RuntimeRow::BrewR,
                NewRuntime::RFromHomebrew,
                "Install R with Homebrew…",
                busy,
                p,
                cx,
            ));
        }
        if readiness.python.tool.is_some() {
            let row = look::settings_row(
                "R with mamba or conda",
                Some(look::settings_detail(
                    "Keep R and IRkernel together in a new folder.",
                    p,
                )),
            );
            rows.push(
                if let Some(confirm) = self.quick_runtime_review(RuntimeRow::CondaR, p, cx) {
                    v_flex().child(row).child(confirm).into_any_element()
                } else if let Some(destination) = self.quick_destination_form(true, busy, p, cx) {
                    v_flex().child(row).child(destination).into_any_element()
                } else {
                    row.child(
                        Button::new("env-create-r")
                            .small()
                            .label("Create an R Environment…")
                            .disabled(busy || self.start.quick.reviewing.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_choose_r_destination(window, cx)
                            })),
                    )
                    .into_any_element()
                },
            );
        }
        rows
    }

    /// Python creation and kernel registration offers.
    fn quick_python_rows(
        &self,
        readiness: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let python = &readiness.python;
        let q = &self.start.quick;
        let mut rows = Vec::new();
        let uv = readiness
            .tools
            .iter()
            .any(|tool| tool.tool == InstallTool::Uv);
        if python.tool.is_none() && !uv {
            rows.push(self.quick_runtime_offer(
                RuntimeRow::Miniforge,
                NewRuntime::Miniforge,
                "Install Miniforge…",
                busy,
                p,
                cx,
            ));
        }
        for env in &python.bare {
            let asking = q.confirm == Some(Confirm::Ipykernel(env.prefix.clone()));
            let label = if env.python.is_empty() {
                env.name.clone()
            } else {
                format!("{} (Python {})", env.name, env.python)
            };
            let row = look::settings_row(
                label,
                Some(look::settings_detail(
                    "Has Python but no kernel. Adding ipykernel makes it one; nothing else in it changes.",
                    p,
                )),
            );
            rows.push(
                self.quick_confirm_row(
                    row,
                    asking,
                    &format!("ipykernel-{}", env.prefix),
                    "Add ipykernel…",
                    "Add",
                    ipykernel_args(&env.tool, &env.prefix)
                        .iter()
                        .map(|arg| sandhill_daemon::agents::shell_quote(arg))
                        .collect::<Vec<_>>()
                        .join(" "),
                    busy,
                    Confirm::Ipykernel(env.prefix.clone()),
                    Call::QuickAddIpykernel {
                        prefix: env.prefix.clone(),
                    },
                    p,
                    cx,
                ),
            );
        }
        let default_tool = if python.tool.is_some() {
            InstallTool::Conda
        } else if uv {
            InstallTool::Uv
        } else {
            return rows;
        };
        let new_row = look::settings_row(
            "New environment",
            Some(look::settings_detail(
                "Set up Python and ipykernel. The environment then shows in the notebook's kernel picker.",
                p,
            )),
        );
        if let Some(confirm) = self.quick_runtime_review(RuntimeRow::Python, p, cx) {
            rows.push(v_flex().child(new_row).child(confirm).into_any_element());
            return rows;
        }
        let Some(form) = &q.python_form else {
            rows.push(
                new_row
                    .child(
                        Button::new("python-create")
                            .small()
                            .label("Create a Python Environment…")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.quick_python_form(default_tool, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
            return rows;
        };
        let name = form.name.read(cx).value().trim().to_string();
        let name_ok = python_env_name_ok(&name);
        let version = form.version;
        let sets: Vec<String> = form.sets.iter().map(|s| s.to_string()).collect();
        let home = self
            .start
            .machines
            .get(sandhill_link::LOCAL)
            .and_then(|m| m.home.clone())
            .or_else(|| std::env::var("HOME").ok());
        let parent = form.parent.clone().or_else(|| match form.tool {
            InstallTool::Conda => python
                .tool
                .as_ref()
                .map(|tool| format!("{}/envs", tool.root)),
            _ => home.map(|home| format!("{home}/.venvs")),
        });
        let prefix = parent.map(|parent| {
            std::path::Path::new(&parent)
                .join(&name)
                .to_string_lossy()
                .into_owned()
        });
        let runtime = prefix.as_ref().map(|prefix| match form.tool {
            InstallTool::Conda => NewRuntime::PythonConda {
                prefix: prefix.clone(),
                python: version.into(),
                sets: sets.clone(),
            },
            _ => NewRuntime::PythonUv {
                prefix: prefix.clone(),
                python: version.into(),
                sets: sets.clone(),
            },
        });
        let tools = [InstallTool::Conda, InstallTool::Uv]
            .into_iter()
            .filter(|tool| match tool {
                InstallTool::Conda => python.tool.is_some(),
                InstallTool::Uv => uv,
                _ => false,
            })
            .map(|tool| {
                let label = readiness
                    .tools
                    .iter()
                    .find(|found| found.tool == tool)
                    .map_or(tool.label(), review::tool_name)
                    .to_owned();
                Button::new(SharedString::from(format!("python-tool-{}", tool.id())))
                    .small()
                    .label(label)
                    .when(form.tool == tool, |b| b.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = &mut this.start.quick.python_form
                            && form.tool != tool
                        {
                            form.tool = tool;
                            form.parent = None;
                        }
                        cx.notify();
                    }))
            });
        let versions = PYTHON_VERSIONS.iter().map(|&choice| {
            let chosen = version == choice;
            Button::new(SharedString::from(format!("python-version-{choice}")))
                .small()
                .label(choice)
                .when(chosen, |b| b.primary())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = &mut this.start.quick.python_form {
                        form.version = choice;
                    }
                    cx.notify();
                }))
        });
        let checks = PYTHON_SETS.iter().map(|&(id, label, packages)| {
            Checkbox::new(SharedString::from(format!("python-set-{id}")))
                .label(format!("{label}: {}", packages.join(", ")))
                .checked(form.sets.contains(&id))
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    if let Some(form) = &mut this.start.quick.python_form {
                        form.sets.retain(|s| *s != id);
                        if *on {
                            form.sets.push(id);
                        }
                    }
                    cx.notify();
                }))
        });
        rows.push(
            v_flex()
                .child(new_row)
                .child(
                    v_flex()
                        .px(px(look::PANE_PAD_X))
                        .pb(px(look::PANE_PAD_Y))
                        .gap(px(look::GAP_8))
                        .child(
                            h_flex()
                                .gap(px(look::GAP_8))
                                .child(div().flex_none().child("Name"))
                                .child(div().flex_1().child(Input::new(&form.name).small())),
                        )
                        .when(!name_ok, |d| {
                            d.child(
                                div()
                                    .text_size(px(look::small()))
                                    .text_color(p.err)
                                    .child("Use letters, digits, - and _ only."),
                            )
                        })
                        .child(
                            h_flex()
                                .gap(px(look::GAP_6))
                                .child(div().flex_none().child("Python"))
                                .children(versions),
                        )
                        .child(h_flex().gap(px(look::GAP_6)).child("Tool").children(tools))
                        .child(
                            h_flex()
                                .gap(px(look::GAP_8))
                                .child(
                                    look::settings_detail(
                                        prefix.as_ref().map_or_else(
                                            || "Choose a folder for the environment.".into(),
                                            |prefix| format!("Folder: {prefix}"),
                                        ),
                                        p,
                                    )
                                    .flex_1()
                                    .min_w_0(),
                                )
                                .child(
                                    Button::new("python-choose-folder")
                                        .small()
                                        .label("Choose Folder…")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.quick_python_choose_folder(cx)
                                        })),
                                ),
                        )
                        .children(checks)
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(look::GAP_8))
                                .child(
                                    Button::new("python-create-cancel")
                                        .small()
                                        .label("Cancel")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.start.quick.python_form = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("python-create-go")
                                        .small()
                                        .primary()
                                        .label("Create…")
                                        .disabled(busy || !name_ok || runtime.is_none())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(runtime) = &runtime {
                                                this.quick_plan_runtime(
                                                    RuntimeRow::Python,
                                                    runtime.clone(),
                                                    cx,
                                                );
                                            }
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        );
        rows
    }

    /// Opens Create a Python Environment's choices, with the default name,
    /// version and both package sets.
    fn quick_python_form(
        &mut self,
        tool: InstallTool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(PYTHON_ENV_NAME));
        let changed = cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify());
        self.start.quick.clear_review();
        self.start.quick.python_form = Some(PythonForm {
            name,
            _changed: changed,
            version: PYTHON_VERSIONS[0],
            sets: PYTHON_SETS.iter().map(|(id, ..)| *id).collect(),
            tool,
            parent: None,
        });
        cx.notify();
    }

    fn quick_python_choose_folder(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.start.quick.python_form else {
            return;
        };
        let name = form.name.clone();
        let choose = choose_local_path("Choose a Parent Folder", false, true, cx);
        cx.spawn(async move |this, cx| {
            if let Some(parent) = choose.await {
                this.update(cx, |this, cx| {
                    if let Some(form) = &mut this.start.quick.python_form
                        && form.name == name
                    {
                        form.parent = Some(parent);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn quick_prefs_row(
        &self,
        readiness: &QuickReadiness,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let q = &self.start.quick;
        let label = "Install Without Asking";
        let switch = Switch::new("quick-without-asking")
            .small()
            .color(p.accent)
            .checked(readiness.prefs.without_asking)
            .accessibility_label(label)
            .disabled(q.setting_prefs || q.checking || q.busy() || q.reviewing.is_some())
            .on_change(cx.listener(|this, on: &bool, _, cx| this.quick_set_prefs(*on, cx)));
        v_flex()
            .child(look::settings_row(label, Some(look::settings_detail(
                "Only installs that use the same folder and tool as last time can run without asking. Add Packages always shows a plan.", p,
            ))).child(switch))
            .when_some(q.prefs_error.clone(), |d, error| {
                d.child(look::settings_row("Couldn't save this choice", Some(look::settings_detail(error, p))))
            })
            .into_any_element()
    }

    fn quick_set_prefs(&mut self, without_asking: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        self.start.quick.setting_prefs = true;
        self.start.quick.prefs_error = None;
        let saved = daemon.call::<InstallPrefs>(Call::SetInstallPrefs { without_asking });
        cx.spawn(async move |this, cx| {
            let saved = saved.await;
            this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                q.setting_prefs = false;
                match saved {
                    Ok(prefs) => {
                        if let Some(Ok(found)) = &mut q.readiness {
                            found.prefs = prefs;
                        }
                    }
                    Err(err) => q.prefs_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_review_buttons(cx: &mut Context<Self>) -> (Button, Button) {
        (
            review::cancel_button().on_click(cx.listener(|this, _, _, cx| {
                this.start.quick.clear_review();
                cx.notify();
            })),
            review::run_button().on_click(cx.listener(|this, _, _, cx| this.quick_run_review(cx))),
        )
    }

    fn quick_runtime_review(
        &self,
        row: RuntimeRow,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let Some(QuickReview::Runtime(at, planned)) = &self.start.quick.reviewing else {
            return None;
        };
        if *at != row {
            return None;
        }
        let (cancel, run) = Self::quick_review_buttons(cx);
        Some(review::runtime(planned, cancel, run, p))
    }

    fn quick_runtime_offer(
        &self,
        at: RuntimeRow,
        runtime: NewRuntime,
        label: &'static str,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = match at {
            RuntimeRow::CranR => "Use the official R installer for this Mac.",
            RuntimeRow::BrewR => "Use the Homebrew already on this computer.",
            RuntimeRow::CondaR => unreachable!(),
            RuntimeRow::Miniforge => {
                "No mamba, conda or uv found. Miniforge provides mamba and conda."
            }
            RuntimeRow::Python => unreachable!(),
        };
        let row = look::settings_row(
            review::runtime_name(&runtime),
            Some(look::settings_detail(detail, p)),
        );
        if let Some(confirm) = self.quick_runtime_review(at, p, cx) {
            v_flex().child(row).child(confirm).into_any_element()
        } else {
            row.child(
                Button::new(label)
                    .small()
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_plan_runtime(at, runtime.clone(), cx)
                    })),
            )
            .into_any_element()
        }
    }

    fn quick_package_offer(
        &self,
        row: Div,
        at: PackageRow,
        label: &'static str,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match &at {
            PackageRow::Plots => SharedString::from("quick-install-packages"),
            PackageRow::ChangedGenes => "quick-install-differential".into(),
            PackageRow::Symbols(name) => format!("quick-symbols-{name}").into(),
            PackageRow::Card | PackageRow::Retry(_) => unreachable!(),
        };
        if let Some(panel) = self.quick_package_panel(&at, p, cx) {
            v_flex().child(row).child(panel).into_any_element()
        } else {
            row.child(
                Button::new(id)
                    .small()
                    .label(label)
                    .disabled(busy)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.quick_plan_packages(at.clone(), cx)),
                    ),
            )
            .into_any_element()
        }
    }

    fn quick_package_panel(
        &self,
        at: &PackageRow,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let Some(QuickReview::Packages(current, libraries, planned)) = &self.start.quick.reviewing
        else {
            return None;
        };
        (current == at).then(|| {
            review::packages(
                planned,
                &[],
                libraries,
                Self::quick_review_buttons(cx),
                Self::quick_change_plan,
                p,
                cx,
            )
        })
    }

    /// A row whose action asks first: its button opens the question, which
    /// shows the exact command, then Cancel and the action.
    #[allow(clippy::too_many_arguments)]
    fn quick_confirm_row(
        &self,
        row: Div,
        asking: bool,
        id: &str,
        ask_label: &'static str,
        go_label: &'static str,
        command: String,
        busy: bool,
        confirm: Confirm,
        call: Call,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !asking {
            return row
                .child(
                    Button::new(SharedString::from(format!("{id}-ask")))
                        .small()
                        .label(ask_label)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start.quick.clear_review();
                            this.start.quick.python_form = None;
                            this.start.quick.confirm = Some(confirm.clone());
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        v_flex()
            .child(
                row.child(
                    Button::new(SharedString::from(format!("{id}-cancel")))
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start.quick.confirm = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("{id}-go")))
                        .small()
                        .primary()
                        .label(go_label)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_start_card_job(
                                call.clone(),
                                "Adding ipykernel".into(),
                                None,
                                cx,
                            );
                        })),
                ),
            )
            .child(
                div()
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .child(review::command_block(&command, p)),
            )
            .into_any_element()
    }

    // Registering IRkernel uses the same offer and job as the kernel picker.
    fn quick_plan_kernel(&mut self, program: String, canonical: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        q.clear_review();
        q.reviewing = Some(QuickReview::Kernel(Review::Planning));
        let generation = q.plan_id;
        let request =
            daemon.call::<Vec<sandhill_proto::RWithoutIrkernel>>(Call::ListRWithoutIrkernel {
                project: None,
            });
        cx.spawn(async move |this, cx| {
            let plan = match request.await {
                Ok(offers) => offers
                    .into_iter()
                    .find(|offer| offer.r == program || offer.r == canonical)
                    .map_or_else(
                        || {
                            Review::Error(
                                "Sandhill couldn't find an IRkernel setup for this R.".into(),
                            )
                        },
                        Review::Ready,
                    ),
                Err(error) => Review::Error(error.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                if this.start.quick.plan_id == generation {
                    this.start.quick.reviewing = Some(QuickReview::Kernel(plan));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn quick_kernel_review(
        &self,
        r: &sandhill_proto::EnvCard,
        plan: &Review<sandhill_proto::RWithoutIrkernel>,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let install = |index, library: Option<String>, program: String, cx: &mut Context<Self>| {
            Button::new(("quick-kernel-install", index))
                .small()
                .primary()
                .label("Install")
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.quick_start_card_job(
                        Call::InstallIrkernel {
                            r: program.clone(),
                            project: None,
                            library: library.clone(),
                        },
                        "Installing IRkernel".into(),
                        None,
                        cx,
                    );
                }))
        };
        let detail = match plan {
            Review::Planning => "Working out how to install IRkernel…".to_owned(),
            Review::Ready(offer) => offer.plan.clone(),
            Review::Error(error) => error.clone(),
        };
        let mut summary =
            look::settings_row("Install IRkernel?", Some(look::settings_detail(detail, p)))
                .child(Self::quick_review_buttons(cx).0);
        if let Review::Ready(offer) = plan
            && !offer.needs_library
        {
            summary = summary.child(install(0, None, offer.r.clone(), cx));
        }
        let mut body = v_flex().child(summary);
        if let Review::Ready(offer) = plan
            && offer.needs_library
        {
            let choices = r.libraries.iter().filter(|lib| lib.writable).enumerate();
            body = body.children(choices.map(|(index, lib)| {
                look::settings_row(lib.path.clone(), None).child(install(
                    index,
                    Some(lib.path.clone()),
                    offer.r.clone(),
                    cx,
                ))
            }));
        }
        body.into_any_element()
    }

    /// The packages: what is missing, with Install, then each package with
    /// its version, its library, and whether Sandhill installed it.
    fn quick_packages_row(
        &self,
        readiness: &QuickReadiness,
        missing: &[&str],
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let detail = if missing.is_empty() {
            "Every package for reading and plotting tables is installed.".to_string()
        } else {
            format!("Missing: {}.", missing.join(", "))
        };
        let head = look::settings_row("R packages", Some(look::settings_detail(detail, p)));
        let head = if missing.is_empty() {
            head.into_any_element()
        } else {
            self.quick_package_offer(
                head,
                PackageRow::Plots,
                "Install Missing Packages…",
                busy,
                p,
                cx,
            )
        };
        let list = readiness.packages.iter().map(|pkg| {
            let (state, color) = match &pkg.version {
                None if pkg.tool == QuickTool::Plot => ("missing".to_string(), p.err),
                None if pkg.tool == QuickTool::Symbols => (
                    "not installed: for gene symbols, when you need them".to_string(),
                    p.muted,
                ),
                None if pkg.tool == QuickTool::Differential => (
                    "not installed: for finding changed genes, when you need them".to_string(),
                    p.muted,
                ),
                None => ("not installed, needed later".to_string(), p.muted),
                Some(_) if pkg.by_sandhill => ("installed by Sandhill".into(), p.muted),
                Some(_) => ("yours".into(), p.muted),
            };
            let place = pkg
                .library
                .as_ref()
                .map(|lib| format!(", in {lib}"))
                .unwrap_or_default();
            h_flex()
                .gap(px(look::GAP_8))
                .text_size(px(look::small()))
                .child(div().flex_none().child(pkg.name.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(color).child(
                    match &pkg.version {
                        Some(version) => format!("{version}: {state}{place}"),
                        None => state,
                    },
                ))
        });
        // The organism packages are large: each installs on its own.
        let symbols = readiness
            .packages
            .iter()
            .filter(|pkg| pkg.tool == QuickTool::Symbols && pkg.version.is_none())
            .filter(|pkg| pkg.name.starts_with("org."))
            .map(|pkg| {
                let organism = if pkg.name == "org.Hs.eg.db" {
                    "human"
                } else {
                    "mouse"
                };
                let row = look::settings_row(
                    format!("Gene symbols for {organism} genes"),
                    Some(look::settings_detail(
                        format!(
                            "{} gives Explore a Table gene symbols, about 90 MB.",
                            pkg.name
                        ),
                        p,
                    )),
                );
                self.quick_package_offer(
                    row,
                    PackageRow::Symbols(pkg.name.clone()),
                    "Install…",
                    busy,
                    p,
                    cx,
                )
            })
            .collect::<Vec<_>>();
        let differential: Vec<_> = readiness
            .packages
            .iter()
            .filter(|pkg| pkg.tool == QuickTool::Differential && pkg.version.is_none())
            .map(|pkg| pkg.name.as_str())
            .collect();
        v_flex()
            .child(head)
            .child(
                v_flex()
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .gap(px(look::GAP_2))
                    .children(list),
            )
            .children(symbols)
            .child({
                let detail = if differential.is_empty() {
                    "DESeq2, edgeR, limma, apeglm, sva and RUVSeq are installed.".to_string()
                } else {
                    format!(
                        "Missing: {}. Install these when you want to compare raw counts.",
                        differential.join(", ")
                    )
                };
                let row = look::settings_row(
                    "For finding changed genes",
                    Some(look::settings_detail(detail, p)),
                );
                if differential.is_empty() {
                    row.into_any_element()
                } else {
                    self.quick_package_offer(
                        row,
                        PackageRow::ChangedGenes,
                        "Install Missing Packages…",
                        busy,
                        p,
                        cx,
                    )
                }
            })
            .into_any_element()
    }

    fn quick_install_fix(&mut self, fix: FailureFix, window: &mut Window, cx: &mut Context<Self>) {
        match fix {
            FailureFix::InstallDeveloperTools => {
                if let (Some(daemon), Some(install)) =
                    (self.local_daemon(), &mut self.start.quick.install)
                {
                    install.job.request_developer_tools(
                        &daemon,
                        |this: &mut Self, id| {
                            this.start
                                .quick
                                .install
                                .as_mut()
                                .filter(|install| install.job.id == id)
                                .map(|install| &mut install.job)
                        },
                        cx,
                    );
                }
            }
            FailureFix::OtherTool { tool, change } => {
                if let Some(install) = self.start.quick.install.as_ref()
                    && let Some(change) = install.job.alternative(tool, change)
                {
                    self.quick_request_packages(
                        PackageRow::Retry(install.section),
                        Vec::new(),
                        vec![change],
                        None,
                        cx,
                    );
                }
            }
            FailureFix::FreeUpSpace => {
                let mode = StartMode::Resources(sandhill_link::LOCAL.to_string());
                // The manager reads all windows, so leave this window's update first.
                let this = cx.entity().downgrade();
                window.defer(cx, move |window, cx| {
                    let _ = this.update(cx, |this, cx| this.show_start(mode, window, cx));
                });
            }
        }
    }

    /// The running install's progress when it belongs in `section`'s box.
    fn quick_install_in(
        &self,
        section: Section,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let q = &self.start.quick;
        if let Some(panel) = self.quick_package_panel(&PackageRow::Retry(section), p, cx) {
            return Some(panel.into_any_element());
        }
        let notice = q
            .notice
            .as_ref()
            .filter(|(at, _)| *at == section)
            .map(|(_, notice)| look::settings_row(notice.clone(), None));
        if q.starting == Some(section) {
            return Some(look::settings_row("Starting…", None).into_any_element());
        }
        if let Some((_, error)) = q.install_error.as_ref().filter(|(at, _)| *at == section) {
            return Some(
                v_flex()
                    .child(look::settings_row(
                        "Couldn't start",
                        Some(look::settings_detail(error.clone(), p)),
                    ))
                    .children(notice)
                    .into_any_element(),
            );
        }
        let Some(install) = q.install.as_ref().filter(|i| i.section == section) else {
            return notice.map(IntoElement::into_any_element);
        };
        let details = Button::new("quick-install-details")
            .small()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(i) = &mut this.start.quick.install {
                    i.job.details = !i.job.details;
                    cx.notify();
                }
            }));
        Some(
            v_flex()
                .children(notice)
                .child(install.job.render(
                    details,
                    install.job.fix_button(Self::quick_install_fix, cx),
                    p,
                ))
                .into_any_element(),
        )
    }

    pub(super) fn quick_ai_helpers(&mut self, p: &Palette, cx: &mut Context<Self>) -> Div {
        if self.start.quick.readiness.is_none() && !self.start.quick.checking {
            self.quick_check(cx);
        }
        let signed_in = ["claude", "codex"].map(|kind| {
            self.start.accounts.as_ref().and_then(|(_, _, accounts)| {
                accounts
                    .read(cx)
                    .accounts
                    .iter()
                    .find(|account| account.default && account.kind == kind)
                    .and_then(|account| account.signed_in)
            })
        });
        let q = &self.start.quick;
        let mut rows = match &q.readiness {
            Some(Ok(found)) => found
                .agents
                .iter()
                .map(|agent| {
                    let index = usize::from(agent.kind == "codex");
                    self.quick_agent_row(agent, signed_in[index], q.busy() || q.checking, p, cx)
                })
                .collect(),
            Some(Err(_)) => {
                vec![look::settings_row("Couldn't check for AI helpers", None).into_any_element()]
            }
            None => vec![look::settings_row("Checking for AI helpers…", None).into_any_element()],
        };
        rows.extend(self.quick_install_in(Section::Ai, p, cx));
        v_flex()
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .px(px(look::PANE_PAD_X))
                    .child(section_title("AI helpers", p))
                    .child(check_again(cx)),
            )
            .child(look::settings_box(rows, p))
    }

    /// Claude Code or Codex: installed or not, signed in as the Agents
    /// page last saw, and Install, which first shows the agent's own
    /// command.
    fn quick_agent_row(
        &self,
        agent: &sandhill_proto::QuickAgent,
        signed_in: Option<bool>,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let q = &self.start.quick;
        let kind = agent.kind.clone();
        let asking = q.confirm == Some(Confirm::Agent(kind.clone()));
        let label = match &agent.version {
            Some(version) => format!("{} {version}", agent.name),
            None => agent.name.clone(),
        };
        let detail = match (&agent.path, signed_in) {
            (Some(path), Some(true)) => format!("{path}. Signed in."),
            (Some(path), Some(false)) => {
                format!("{path}. Not signed in: sign in below.")
            }
            (Some(path), None) => {
                format!("{path}. See accounts below.")
            }
            (None, _) if asking => format!(
                "Install {}? Sandhill runs its official command:",
                agent.name
            ),
            (None, _) if agent.installable => "Not installed.".into(),
            (None, _) => format!(
                "Not installed. {} installs with npm, which comes with Node.js: get Node.js, then Check Again.",
                agent.name
            ),
        };
        let mut row = look::settings_row(label, Some(look::settings_detail(detail, p)));
        if agent.path.is_none() {
            row = if asking {
                let command = agent.install.clone();
                row.child(
                    Button::new(SharedString::from(format!("quick-agent-cancel-{kind}")))
                        .small()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start.quick.confirm = None;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("quick-agent-go-{kind}")))
                        .small()
                        .primary()
                        .label("Install")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_start_install(
                                Call::QuickInstallAgent {
                                    kind: kind.clone(),
                                    command: Some(command.clone()),
                                },
                                Section::Ai,
                                format!("Installing {}", agent_name(&kind)),
                                None,
                                cx,
                            );
                        })),
                )
            } else if agent.installable {
                row.child(
                    Button::new(SharedString::from(format!("quick-agent-install-{kind}")))
                        .small()
                        .label("Install…")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start.quick.clear_review();
                            this.start.quick.confirm = Some(Confirm::Agent(kind.clone()));
                            cx.notify();
                        })),
                )
            } else {
                row.child(
                    Button::new(SharedString::from(format!("quick-agent-node-{kind}")))
                        .small()
                        .label("Get Node.js…")
                        .on_click(|_, _, cx| cx.open_url(NODE)),
                )
            };
        }
        if !asking {
            return row.into_any_element();
        }
        v_flex()
            .child(row)
            .child(
                div()
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .child(review::command_block(&agent.install, p)),
            )
            .into_any_element()
    }

    fn quick_record_rows(
        &self,
        readiness: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let q = &self.start.quick;
        let asking = q.confirm == Some(Confirm::Remove);
        let available = readiness
            .installed
            .iter()
            .filter(|i| i.ours)
            .map(installed_key)
            .collect::<Vec<_>>();
        if readiness.installed.is_empty() {
            return vec![look::settings_row("No installs recorded yet", None).into_any_element()];
        }
        let count = available
            .iter()
            .filter(|key| q.selected.contains(*key))
            .count();
        let mut rows = Vec::new();
        for group in [
            "R packages",
            "Python packages",
            "Environments and R",
            "Kernels",
            "Agents",
        ] {
            let mut items = readiness
                .installed
                .iter()
                .enumerate()
                .filter(|(_, i)| installed_group(i) == group)
                .peekable();
            if items.peek().is_none() {
                continue;
            }
            rows.push(look::settings_row(group, None).into_any_element());
            for (index, item) in items {
                let key = installed_key(item);
                let version = if item.version.is_empty() {
                    "Version not recorded".into()
                } else {
                    format!("Version {}", item.version)
                };
                let title = if item.kind == "agent" {
                    agent_name(&item.name).into()
                } else if item.kind == "own-environment" {
                    "Sandhill's own R and Python".into()
                } else {
                    item.name.clone()
                };
                let tool = match item.tool.as_str() {
                    "r" => "R's own installer",
                    "conda" => "mamba/conda",
                    "brew" => "Homebrew",
                    "installer" => "Installer",
                    "native" => "Official installer",
                    "" => "Not recorded",
                    tool => tool,
                };
                let where_ = if item.place.is_empty() {
                    "Not recorded"
                } else {
                    &item.place
                };
                let date = if item.date.is_empty() {
                    "Not recorded"
                } else {
                    &item.date
                };
                let mut detail =
                    format!("{version} · Folder: {where_} · Tool: {tool} · Date: {date}");
                if !item.reason.is_empty() {
                    detail.push_str(&format!(" · {}", item.reason));
                }
                if !item.ours {
                    detail.push_str(" · changed since: yours now");
                }
                rows.push(
                    look::settings_row(title, Some(look::settings_detail(detail, p)))
                        .child(
                            Checkbox::new(("quick-record-check", index))
                                .checked(q.selected.contains(&key) && item.ours)
                                .disabled(busy || asking || !item.ours)
                                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                    let selected = &mut this.start.quick.selected;
                                    if *on {
                                        selected.insert(key.clone());
                                    } else {
                                        selected.remove(&key);
                                    }
                                    cx.notify();
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        let action = if asking {
            look::settings_row(
                format!("Remove {count} selected {}?", if count == 1 { "item" } else { "items" }),
                Some(look::settings_detail("Your own work may use these packages too. Removing an environment removes its contents.", p)),
            )
            .child(review::cancel_button().on_click(cx.listener(|this, _, _, cx| {
                this.start.quick.clear_review();
                cx.notify();
            })))
            .child(Button::new("quick-remove-record-go").small().danger().label("Remove")
                .disabled(busy || count == 0).on_click(cx.listener(|this, _, _, cx| this.quick_remove_selected(cx))))
        } else {
            look::settings_row(format!("{count} selected"), None)
                .child(
                    Button::new("quick-record-all")
                        .small()
                        .label("Select All")
                        .disabled(busy || count == available.len())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start.quick.selected = available.iter().cloned().collect();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("quick-record-none")
                        .small()
                        .label("Select None")
                        .disabled(busy || count == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start.quick.selected.clear();
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("quick-record-remove")
                        .small()
                        .label("Remove Selected…")
                        .disabled(busy || count == 0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start.quick.clear_review();
                            this.start.quick.confirm = Some(Confirm::Remove);
                            cx.notify();
                        })),
                )
        };
        rows.push(action.into_any_element());
        rows
    }

    fn quick_remove_selected(&mut self, cx: &mut Context<Self>) {
        let q = &self.start.quick;
        let Some(Ok(readiness)) = &q.readiness else {
            return;
        };
        let items = readiness
            .installed
            .iter()
            .filter(|i| i.ours)
            .map(installed_key)
            .filter(|key| q.selected.contains(key))
            .collect::<Vec<_>>();
        if !items.is_empty() {
            self.start.quick.selected.clear();
            self.quick_start_install(
                Call::RemoveInstalled { items },
                Section::Record,
                "Removing selected items".into(),
                None,
                cx,
            );
        }
    }

    fn quick_drop_area(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        if let Some(picker) = &self.start.quick.archive_picker {
            return picker.render(
                p,
                |this: &mut Self| &mut this.start.quick.archive_picker,
                Self::quick_open_archive_selection,
                cx,
            );
        }
        let opening = self.start.quick.opening;
        let accent = p.accent_soft;
        v_flex()
            .id("quick-drop")
            .w_full()
            .items_center()
            .gap(px(look::GAP_8))
            .py(px(28.))
            .rounded(px(look::RADIUS))
            .border_2()
            .border_dashed()
            .border_color(p.line)
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.bg(accent))
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, _, cx| {
                cx.stop_propagation();
                let tables = dropped
                    .paths()
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                this.quick_open(tables, cx);
            }))
            .child(icon("table").size(px(look::ICON_LARGE)).text_color(p.muted))
            .child(div().child(if opening {
                "Opening files…"
            } else {
                "Drop a table or archive here"
            }))
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child("CSV, TSV, text and Excel, including gzip, ZIP and TAR"),
            )
            .child(
                Button::new("quick-choose")
                    .small()
                    .label("Choose a File…")
                    .disabled(opening)
                    .on_click(cx.listener(|this, _, _, cx| this.quick_choose_tables(cx))),
            )
            .into_any_element()
    }

    /// Recent → Tables you explored: each session folder, or "Not found".
    pub(super) fn quick_recent(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sessions = crate::prefs::quick_sessions();
        if sessions.is_empty() {
            return None;
        }
        let q = &self.start.quick;
        let hover = p.ink(look::ROW_HOVER);
        let rows = sessions
            .into_iter()
            .enumerate()
            .map(|(ix, path)| {
                // Explored tables are on this Mac, which the app shares with
                // its daemon. Only kept sessions are listed: their folder's
                // own name is the one the user gave.
                let found = std::path::Path::new(&path).is_dir();
                let name = crate::panes::files::base_name(&path).to_string();
                let folder = parent_of(&path);
                let open_path = path.clone();
                let mut row = v_flex().child(
                    h_flex()
                        .id(("quick-session", ix))
                        .w_full()
                        .min_h(px(look::row()))
                        .px(px(look::PANE_PAD_X))
                        .py(px(look::GAP_6))
                        .gap(px(look::GAP_8))
                        .when(found, |d| {
                            d.cursor_pointer().hover(move |d| d.bg(hover)).on_click(
                                move |_, _, cx| {
                                    let path = open_path.clone();
                                    cx.defer(move |cx| {
                                        crate::shell::app_window::open_quick(path, None, cx)
                                    });
                                },
                            )
                        })
                        .child(icon("table").size(px(look::ICON)).text_color(p.muted))
                        .child(look::settings_label(
                            name.clone(),
                            Some(look::settings_detail(
                                if found {
                                    folder
                                } else {
                                    "Not found: moved or deleted".into()
                                },
                                p,
                            )),
                        ))
                        .child({
                            let path = path.clone();
                            icon_button(("quick-session-more", ix), "more", p, false)
                                .tooltip(|w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new("More").build(w, cx)
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    let q = &mut this.start.quick;
                                    q.actions = if q.actions.as_deref() == Some(&path) {
                                        None
                                    } else {
                                        Some(path.clone())
                                    };
                                    q.deleting = None;
                                    cx.notify();
                                }))
                        }),
                );
                if q.deleting.as_deref() == Some(&path) {
                    let delete = path.clone();
                    row = row.child(
                        h_flex()
                            .px(px(look::PANE_PAD_X))
                            .pb(px(look::GAP_8))
                            .gap(px(look::GAP_8))
                            .child(div().flex_1().child(format!("Delete {name}?")))
                            .child(
                                Button::new(("quick-delete-cancel", ix))
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start.quick.deleting = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(("quick-delete", ix))
                                    .small()
                                    .danger()
                                    .label("Delete")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.quick_delete(delete.clone(), cx)
                                    })),
                            ),
                    );
                } else if q.actions.as_deref() == Some(&path) {
                    let (reveal, project, forget, delete) =
                        (path.clone(), path.clone(), path.clone(), path.clone());
                    row = row.child(
                        h_flex()
                            .px(px(look::PANE_PAD_X))
                            .pb(px(look::GAP_8))
                            .gap(px(look::GAP_8))
                            .when(found, |d| {
                                d.child(
                                    Button::new(("quick-reveal", ix))
                                        .small()
                                        .label("Show in Finder")
                                        .on_click(move |_, _, cx| {
                                            cx.reveal_path(std::path::Path::new(&reveal))
                                        }),
                                )
                                .child(
                                    Button::new(("quick-project", ix))
                                        .small()
                                        .label("Open as Project")
                                        .on_click(move |_, _, cx| {
                                            let path = project.clone();
                                            cx.defer(move |cx| {
                                                crate::shell::app_window::open_in_main(
                                                    sandhill_link::LOCAL.into(),
                                                    path,
                                                    cx,
                                                )
                                            });
                                        }),
                                )
                            })
                            .child(
                                Button::new(("quick-forget", ix))
                                    .small()
                                    .label("Remove from List")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        crate::prefs::forget_quick_session(&forget);
                                        this.start.quick.actions = None;
                                        cx.notify();
                                    })),
                            )
                            .when(found, |d| {
                                d.child(
                                    Button::new(("quick-ask-delete", ix))
                                        .small()
                                        .label("Delete…")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.start.quick.deleting = Some(delete.clone());
                                            this.start.quick.actions = None;
                                            cx.notify();
                                        })),
                                )
                            }),
                    );
                }
                row.into_any_element()
            })
            .collect();
        Some(look::settings_group("Tables you explored", rows, p).into_any_element())
    }
}

fn offer_row(title: &'static str, detail: &'static str, p: &Palette) -> AnyElement {
    look::settings_row(title, Some(look::settings_detail(detail, p))).into_any_element()
}

fn page_title(title: &'static str, detail: &'static str, p: &Palette) -> Div {
    v_flex()
        .gap(px(look::GAP_6))
        .child(
            div()
                .text_size(px(look::welcome_title()))
                .font_weight(FontWeight::BOLD)
                .child(title),
        )
        .child(
            div()
                .text_size(px(look::tab_text()))
                .text_color(p.muted)
                .child(detail),
        )
}

fn installed_key(item: &QuickInstalled) -> InstalledKey {
    InstalledKey {
        kind: item.kind.clone(),
        name: item.name.clone(),
        place: item.place.clone(),
    }
}

fn installed_group(item: &QuickInstalled) -> &'static str {
    match item.kind.as_str() {
        "r-package" => "R packages",
        "conda-package"
            if item.name.starts_with("r-") || item.name.starts_with("bioconductor-") =>
        {
            "R packages"
        }
        "python-package" | "conda-package" | "ipykernel" => "Python packages",
        "kernel" => "Kernels",
        "agent" => "Agents",
        _ => "Environments and R",
    }
}

fn check_again(cx: &mut Context<AppWindow>) -> Button {
    Button::new("quick-check")
        .small()
        .label("Check Again")
        .on_click(cx.listener(|this, _, _, cx| {
            if this.start.quick.setting_prefs
                || this.start.quick.checking
                || this.start.quick.busy()
            {
                return;
            }
            this.start.quick.clear_review();
            this.start.quick.readiness = None;
            this.quick_check(cx);
            this.quick_invalidate_cards(cx);
        }))
}

/// The small heading above one of the page's boxes.
fn section_title(title: &'static str, p: &Palette) -> Div {
    div()
        .flex_1()
        .text_size(px(look::small()))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.muted)
        .child(title)
}

fn heading(title: &'static str, p: &Palette) -> Div {
    div()
        .pt(px(look::GAP_12))
        .px(px(look::PANE_PAD_X))
        .child(section_title(title, p))
}
