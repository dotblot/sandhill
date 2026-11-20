//! Welcome's Start group (Huan, 2026-11-19; `docs/design/quick-tools-plan.md`,
//! `docs/design/explore-next-plan.md`, `docs/design/python-ready-plan.md`):
//! Explore a Table (a place to drop tables and what can be made from them),
//! Getting Ready (R, where it came from and where packages go, the
//! packages, Python environments for notebooks, Claude Code and Codex,
//! installing what is missing and removing what Sandhill installed), and
//! the tables explored, which Recent lists.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::checkbox::Checkbox;
use sandhill_proto::{
    AgentAccount, InstallPrefs, InstallTool, InstalledKey, NewRuntime, PYTHON_ENV_NAME,
    PYTHON_SETS, PYTHON_VERSIONS, PackageAction, PackageChange, PackagePlan, QuickInstalled,
    QuickInstalling, QuickReadiness, QuickTool, RSource, RuntimePlan, ipykernel_args,
    python_env_name_ok,
};

use crate::install_confirm::{self as review, Choice, Job, Review};
use crate::view::quick_archive::{ArchivePicker, TABLE_TYPES};
use sandhill_proto::{QuickArchiveOpened, QuickArchiveSelection};

use super::*;

use crate::welcome::folder_dialog::choose_local_path;

mod environment;

const NODE: &str = "https://nodejs.org/";

#[derive(Clone, PartialEq, Eq)]
enum PackageRow {
    Plots,
    ChangedGenes,
    Symbols(String),
    Own,
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
    Packages(PackageRow, Vec<String>, Review<PackagePlan>),
    Runtime(RuntimeRow, Review<RuntimePlan>),
}

/// Getting Ready's boxes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    R,
    Python,
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
    Kernel,
    Ipykernel(String),
    RemoveOwn,
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
    /// Whether each agent's own login is signed in, as the Agents page
    /// last saw it; never asked of the agent here.
    signed_in: HashMap<String, Option<bool>>,
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
        }
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
        // What the Agents page last saw: not asking the agents (2026-11-14).
        let accounts = daemon.call::<Vec<AgentAccount>>(Call::ListAgentAccounts {
            refresh: false,
            ids: None,
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            let accounts = accounts.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.start.quick.checking = false;
                this.start.quick.readiness = Some(read.map_err(|err| format!("{err:#}")));
                this.start.quick.signed_in = accounts
                    .into_iter()
                    .filter(|a| a.default)
                    .map(|a| (a.kind, a.signed_in))
                    .collect();
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
            PackageRow::Own => return,
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
        self.quick_request_packages(row, libraries, change, automatic, cx);
    }

    fn quick_request_packages(
        &mut self,
        row: PackageRow,
        libraries: Vec<String>,
        change: PackageChange,
        automatic: Option<InstallTool>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        q.clear_review();
        q.python_form = None;
        q.reviewing = Some(QuickReview::Packages(
            row.clone(),
            libraries.clone(),
            Review::Planning,
        ));
        let id = q.plan_id;
        let pending = daemon.call::<PackagePlan>(Call::PlanPackages { change });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            this.update(cx, |this, cx| {
                if this.start.quick.plan_id != id {
                    return;
                }
                if let (Some(tool), Ok(plan)) = (automatic, &result)
                    && plan.change.tool == Some(tool)
                    && plan.refused.is_none()
                    && !plan.command.is_empty()
                {
                    this.quick_run_packages(plan, true, cx);
                    return;
                }
                this.start.quick.reviewing =
                    Some(QuickReview::Packages(row, libraries, result.into()));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_change_plan(&mut self, choice: Choice, cx: &mut Context<Self>) {
        let Some(QuickReview::Packages(row, libraries, Review::Ready(plan))) =
            &self.start.quick.reviewing
        else {
            return;
        };
        let mut change = plan.change.clone();
        choice.apply(&mut change);
        if change != plan.change {
            self.quick_request_packages(row.clone(), libraries.clone(), change, None, cx);
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
            Some(QuickReview::Packages(_, _, Review::Ready(plan)))
                if plan.refused.is_none() && !plan.command.is_empty() =>
            {
                let plan = plan.clone();
                self.quick_run_packages(&plan, false, cx);
            }
            Some(QuickReview::Runtime(row, Review::Ready(plan)))
                if plan.refused.is_none() && !plan.command.is_empty() =>
            {
                let runtime = plan.runtime.clone();
                let section = match row {
                    RuntimeRow::CranR | RuntimeRow::BrewR | RuntimeRow::CondaR => Section::R,
                    RuntimeRow::Python | RuntimeRow::Miniforge => Section::Python,
                };
                let title = format!("Installing {}", review::runtime_name(&runtime));
                self.quick_start_install(Call::RunRuntime { runtime }, section, title, None, cx);
            }
            _ => {}
        }
    }

    fn quick_run_packages(&mut self, plan: &PackagePlan, automatic: bool, cx: &mut Context<Self>) {
        let section = if matches!(
            self.start.quick.reviewing,
            Some(QuickReview::Packages(PackageRow::Own, ..))
        ) {
            Section::Environment
        } else {
            Section::R
        };
        self.quick_start_install(
            Call::RunPackages {
                change: plan.change.clone(),
            },
            section,
            review::progress_title(&plan.change),
            automatic.then(|| review::auto_notice(plan)),
            cx,
        );
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
                        let job = Job::new(info, log, title);
                        review::watch(daemon, &job, Self::quick_install_status, cx).detach();
                        q.install = Some(Install {
                            section,
                            job,
                            notice,
                        });
                    }
                    Err(err) => {
                        q.install_error = Some((section, format!("{title} didn't start: {err}")));
                        if section == Section::Environment {
                            this.quick_advance_setup_jobs(false, cx);
                            this.quick_check(cx);
                        }
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
        output: Option<String>,
        finished: Option<bool>,
        cx: &mut Context<Self>,
    ) -> bool {
        let q = &mut self.start.quick;
        let Some(install) = q.install.as_mut().filter(|i| i.job.id == id) else {
            return false;
        };
        if let Some(output) = output {
            install.job.output = output;
        }
        install.job.finished = finished;
        if finished.is_some() {
            if self.quick_advance_setup_jobs(finished == Some(true), cx) {
                return true;
            }
            let q = &mut self.start.quick;
            let install = q.install.as_mut().expect("the job just finished");
            if finished == Some(true) {
                q.notice = install
                    .notice
                    .take()
                    .map(|notice| (install.section, notice));
            }
            self.quick_check(cx);
        }
        cx.notify();
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
                                .label("Getting Ready")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_page(Page::Ready, cx)
                                })),
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

    /// Start → Getting Ready: R, its kernel and the packages, and
    /// installing what is missing.
    pub(super) fn ready_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        if self.start.quick.readiness.is_none() && !self.start.quick.checking {
            self.quick_check(cx);
        }
        if let Some(setup) = self.quick_setup_page(&p, cx) {
            return setup.into_any_element();
        }
        v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(page_title(
                "Getting Ready",
                look::mac_or(
                    "What this Mac needs: R and its packages for exploring tables, Python for notebooks, and the AI helpers. Sandhill checks them and installs what is missing.",
                    "What this computer needs: R and its packages for exploring tables, Python for notebooks, and the AI helpers. Sandhill checks them and installs what is missing.",
                ),
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
        let mut agents: Vec<AnyElement> = readiness
            .agents
            .iter()
            .map(|agent| self.quick_agent_row(agent, busy, p, cx))
            .collect();
        agents.extend(self.quick_install_in(Section::Ai, p, cx));
        let mut record = self.quick_record_rows(readiness, busy, p, cx);
        record.extend(self.quick_install_in(Section::Record, p, cx));
        v_flex()
            .gap(px(look::GAP_6))
            .child(self.quick_mode_card(readiness, busy, p, cx))
            .when_some(
                self.quick_install_in(Section::Environment, p, cx),
                |d, progress| d.child(look::settings_box(vec![progress], p)),
            )
            .when(own, |d| {
                d.child(self.quick_own_card(readiness, busy, p, cx))
            })
            .when(!own, |d| {
                d.child(self.quick_existing_cards(readiness, busy, p, cx))
            })
            .child(heading("AI helpers", p))
            .child(look::settings_box(agents, p))
            .child(heading("What Sandhill Installed", p))
            .child(look::settings_box(record, p))
            .into_any_element()
    }

    fn quick_existing_cards(
        &self,
        readiness: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let q = &self.start.quick;
        let (r_ok, r_text) = r_line(readiness);
        let missing: Vec<&str> = readiness
            .packages
            .iter()
            .filter(|p| p.version.is_none() && p.tool == QuickTool::Plot)
            .map(|p| p.name.as_str())
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        let r_row = look::settings_row(
            if r_ok { r_text.clone() } else { "R".into() },
            Some(look::settings_detail(
                match &readiness.r {
                    Some(r) if r_ok => {
                        let from = match r.source {
                            RSource::Cran => "From CRAN's installer",
                            RSource::Homebrew => "From Homebrew",
                            RSource::Conda => "From conda",
                            RSource::Other => "Installed another way",
                        };
                        let builds = if r.binaries {
                            "It installs ready-built packages."
                        } else {
                            look::mac_or(
                                "It builds packages on this Mac from their source.",
                                "It builds packages on this computer from their source.",
                            )
                        };
                        format!("{from}: {}. {builds}", r.path)
                    }
                    _ => r_text.clone(),
                },
                p,
            )),
        );
        rows.push(r_row.into_any_element());
        if !r_ok {
            rows.extend(self.quick_r_offers(readiness, busy, p, cx));
        }
        if let Some(r) = readiness.r.as_ref().filter(|_| r_ok) {
            let writable = r.libraries.iter().any(|l| l.writable);
            let listed = r.libraries.iter().map(|lib| {
                let path = lib.path.clone();
                h_flex()
                    .gap(px(look::GAP_8))
                    .text_size(px(look::small()))
                    .child(div().flex_1().min_w_0().truncate().child(lib.path.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_color(p.muted)
                            .child(if lib.writable {
                                format!("{} packages", lib.packages)
                            } else {
                                format!("{} packages, can't be written to", lib.packages)
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("quick-show-lib-{}", lib.path)))
                            .small()
                            .label(look::mac_or("Show in Finder", "Show in Folder"))
                            .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&path))),
                    )
            });
            rows.push(
                v_flex()
                    .child(look::settings_row(
                        "R's libraries",
                        Some(look::settings_detail(
                            if writable {
                                "Where this R keeps packages, in the order it looks. Sandhill shows which one before it installs anything."
                            } else {
                                "None of these can be written to, and Sandhill doesn't create libraries. Make one yourself (for example, set R_LIBS_USER in ~/.Renviron and create that folder), then Check Again."
                            },
                            p,
                        )),
                    ))
                    .child(
                        v_flex()
                            .px(px(look::PANE_PAD_X))
                            .pb(px(look::PANE_PAD_Y))
                            .gap(px(look::GAP_2))
                            .children(listed),
                    )
                    .into_any_element(),
            );
            let kernel_row = look::settings_row(
                "R kernel",
                Some(look::settings_detail(
                    match &readiness.kernel {
                        Some(name) => format!("{name}: Sandhill runs R through it."),
                        None => "Lets Sandhill run R. Installing it takes a minute.".into(),
                    },
                    p,
                )),
            );
            rows.push(
                if readiness.kernel.is_some() {
                    kernel_row
                } else {
                    kernel_row.child(
                        Button::new("quick-install-kernel")
                            .small()
                            .primary()
                            .label("Install…")
                            .disabled(busy || !writable)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.quick.clear_review();
                                this.start.quick.confirm = Some(Confirm::Kernel);
                                cx.notify();
                            })),
                    )
                }
                .into_any_element(),
            );
            if q.confirm == Some(Confirm::Kernel) {
                rows.push(self.quick_kernel_review(r, busy, p, cx));
            }
            rows.push(self.quick_packages_row(readiness, &missing, busy, p, cx));
        }
        rows.extend(self.quick_install_in(Section::R, p, cx));
        let mut python = self.quick_python_rows(readiness, busy, p, cx);
        python.extend(self.quick_install_in(Section::Python, p, cx));
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
        v_flex()
            .gap(px(look::GAP_6))
            .child(look::settings_box(
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
            ))
            .child(
                h_flex()
                    .px(px(look::PANE_PAD_X))
                    .child(section_title("R and its packages", p))
                    .child(check_again(cx)),
            )
            .child(look::settings_box(rows, p))
            .child(heading("Python for notebooks", p))
            .child(look::settings_box(python, p))
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
        if let Some(tool) = &readiness.python.tool {
            rows.push(self.quick_runtime_offer(
                RuntimeRow::CondaR,
                NewRuntime::RConda {
                    prefix: format!("{}/envs/r", tool.root),
                },
                "Create an R Environment…",
                busy,
                p,
                cx,
            ));
        }
        rows
    }

    /// Python for notebooks (Huan, 2026-11-19; `python-ready-plan.md`): the
    /// kernels found, the mamba used (or Install Miniforge), environments
    /// that could get a kernel, and Create a Python Environment. Each
    /// action shows its command in the row before it runs.
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
        rows.push(
            look::settings_row(
                "Python kernels",
                Some(look::settings_detail(
                    if python.kernels.is_empty() {
                        "None yet. Create an environment below, or add ipykernel to one you have."
                            .to_string()
                    } else {
                        python.kernels.join(", ")
                    },
                    p,
                )),
            )
            .into_any_element(),
        );
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
                "Use the library or environment and tool you chose last time, without asking. Sandhill tells you what it installed afterwards.", p,
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
            RuntimeRow::CondaR => "Keep R and IRkernel together in a new folder.",
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
            PackageRow::Own => "quick-own-package".into(),
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
                std::slice::from_ref(planned),
                &[],
                libraries,
                Self::quick_review_buttons(cx),
                |this: &mut Self, _, choice, cx| this.quick_change_plan(choice, cx),
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
                            this.quick_start_install(
                                call.clone(),
                                Section::Python,
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

    // Registering IRkernel still uses its dedicated setup job.
    fn quick_kernel_review(
        &self,
        r: &sandhill_proto::QuickR,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = look::settings_row(
            "Install IRkernel?",
            Some(look::settings_detail(
                "Choose the library IRkernel goes into if it isn't there yet. Sandhill then registers it as a kernel.",
                p,
            )),
        ).child(review::cancel_button().on_click(cx.listener(|this, _, _, cx| {
            this.start.quick.clear_review();
            cx.notify();
        })));
        let choices = r
            .libraries
            .iter()
            .filter(|lib| lib.writable)
            .map(|lib| lib.path.clone())
            .collect::<Vec<_>>();
        v_flex()
            .child(summary)
            .children(choices.into_iter().enumerate().map(|(index, library)| {
                let program = r.path.clone();
                look::settings_row(library.clone(), None).child(
                    Button::new(("quick-kernel-install", index))
                        .small()
                        .primary()
                        .label("Install")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_start_install(
                                Call::InstallIrkernel {
                                    r: program.clone(),
                                    project: None,
                                    library: Some(library.clone()),
                                },
                                Section::R,
                                "Installing IRkernel".into(),
                                None,
                                cx,
                            );
                        })),
                )
            }))
            .into_any_element()
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

    /// The running install's progress when it belongs in `section`'s box.
    fn quick_install_in(
        &self,
        section: Section,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let q = &self.start.quick;
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
        let install = q.install.as_ref().filter(|i| i.section == section)?;
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
                .child(install.job.render(details, p))
                .into_any_element(),
        )
    }

    /// Claude Code or Codex: installed or not, signed in as the Agents
    /// page last saw, and Install, which first shows the agent's own
    /// command.
    fn quick_agent_row(
        &self,
        agent: &sandhill_proto::QuickAgent,
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
        let detail = match (&agent.path, q.signed_in.get(&kind).copied().flatten()) {
            (Some(path), Some(true)) => format!("{path}. Signed in."),
            (Some(path), Some(false)) => {
                format!("{path}. Not signed in: sign in from Agents in the sidebar.")
            }
            (Some(path), None) => {
                format!("{path}. Agents in the sidebar shows whether it is signed in.")
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
                                Call::QuickInstallAgent { kind: kind.clone() },
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
