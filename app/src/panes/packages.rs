//! The Packages pane, as the Tauri app has it (`PackagesPane.tsx`,
//! `model/help.ts`): what's installed for the active session's language (R's
//! libraries, or the Python environment), with what's attached or imported
//! first. A click opens a package's help. Installing, updating and removing
//! first show what changes, with which tool, in which folder and the exact
//! command, then run as a job, and the list is read again after. Updates
//! are checked when asked, as that reaches the repositories from the
//! session: CRAN and Bioconductor for R, PyPI and conda's channels for
//! Python.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, InstallPrefs, InstallTarget, InstallTool, Package, PackageAction, PackageChange,
    PackagePlan, PackageUpdate, PackageUpdates, Packages as Listed, QuickInstalling,
};

use crate::install_confirm::{self as review, Choice, Job, Review};
use crate::look::{self, Palette, icon, icon_button};
use crate::panes::help::HelpRequest;
use crate::workbench::{TopTab, Workbench};

/// How long after a run finishes the list is read again.
const REFRESH: Duration = Duration::from_millis(250);
/// What the daemon says when the session runs other code.
const BUSY: &str = "the session is busy running code";

/// The pane's changes, the libraries an install can go into, and what
/// can't be changed here, in plain words.
type PaneChanges = (Vec<PackageChange>, Vec<String>, Vec<String>);

fn filter_height() -> f32 {
    look::control_card() + 2. * look::PANE_PAD_Y
}

pub(crate) struct Packages {
    filter: Entity<InputState>,
    names: Entity<InputState>,
    /// The session the list is for.
    session: Option<String>,
    listed: Option<Listed>,
    error: Option<String>,
    /// The list needs reading again once the pane shows.
    stale: bool,
    loading: bool,
    installing: bool,
    invalid: bool,
    /// What checking for updates found, or why it couldn't.
    updates: Option<Result<PackageUpdates, String>>,
    checking: bool,
    /// The changes being shown before they run.
    review: Option<PaneReview>,
    plan_id: u64,
    starting: bool,
    job: Option<PaneJob>,
    /// Planned changes that run after the job, one after another.
    queue: Vec<PackagePlan>,
    job_error: Option<String>,
    generation: u64,
}

/// Changes shown before they run: one per R library for an update, else
/// one.
struct PaneReview {
    plans: Vec<Review<PackagePlan>>,
    /// The answer each plan waits for: a later one replaces it.
    ids: Vec<u64>,
    /// Packages that can't change here, in plain words.
    skipped: Vec<String>,
    /// The package whose row it shows under; none: at the top.
    row: Option<String>,
    /// The R libraries a single change can go into.
    libraries: Vec<String>,
}

struct PaneJob {
    job: Job,
    /// Said once it ends: what Install Without Asking did, that the
    /// session must restart to use an updated module, or what didn't run
    /// after it failed.
    notice: Option<String>,
}

/// Makes the Packages pane's state, with its filter and install boxes.
pub(crate) fn new_packages(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Packages, Vec<Subscription>) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter packages"));
    let names = cx.new(|cx| InputState::new(window, cx).placeholder("Packages to install"));
    let subscriptions = vec![
        cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        }),
        cx.subscribe_in(
            &names,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.packages.invalid = false;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.install_packages(window, cx),
                _ => {}
            },
        ),
    ];
    let packages = Packages {
        filter,
        names,
        session: None,
        listed: None,
        error: None,
        stale: true,
        loading: false,
        installing: false,
        invalid: false,
        updates: None,
        checking: false,
        review: None,
        queue: Vec::new(),
        plan_id: 0,
        starting: false,
        job: None,
        job_error: None,
        generation: 0,
    };
    (packages, subscriptions)
}

fn is_python(language: &str) -> bool {
    language.to_ascii_lowercase().starts_with("python")
}

/// What a package's row says of it: attached (or imported), or loaded.
pub(crate) fn package_status(pkg: &Package, language: &str) -> &'static str {
    if pkg.attached {
        if language.eq_ignore_ascii_case("r") {
            "attached"
        } else {
            "imported"
        }
    } else if pkg.loaded {
        "loaded"
    } else {
        ""
    }
}

/// Packages whose name or title has `text`, names first.
pub(crate) fn filter_packages<'a>(packages: &'a [Package], text: &str) -> Vec<&'a Package> {
    let q = text.trim().to_lowercase();
    if q.is_empty() {
        return packages.iter().collect();
    }
    let named = |p: &Package| p.name.to_lowercase().contains(&q);
    let mut out: Vec<&Package> = packages.iter().filter(|p| named(p)).collect();
    out.extend(
        packages
            .iter()
            .filter(|p| !named(p) && p.title.to_lowercase().contains(&q)),
    );
    out
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// A pip requirement: `name`, `name[extra,other]`, then version clauses
/// like `>=1.10,<2`.
fn is_requirement(text: &str) -> bool {
    let mut rest = text;
    let Some(first) = rest.chars().next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let end = rest
        .find(|c: char| !(is_word(c) || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest = &rest[end..];
    if let Some(after) = rest.strip_prefix('[') {
        let Some(close) = after.find(']') else {
            return false;
        };
        let extras = &after[..close];
        if extras.is_empty()
            || !extras
                .chars()
                .all(|c| is_word(c) || matches!(c, '.' | ',' | '-'))
        {
            return false;
        }
        rest = &after[close + 1..];
    }
    if rest.is_empty() {
        return true;
    }
    rest.split(',').all(|clause| {
        let Some(op) = clause.chars().next().filter(|c| "<>=!~".contains(*c)) else {
            return false;
        };
        let version = clause[op.len_utf8()..]
            .strip_prefix('=')
            .unwrap_or(&clause[op.len_utf8()..]);
        !version.is_empty()
            && version
                .chars()
                .all(|c| is_word(c) || matches!(c, '.' | '*' | '+' | '-'))
    })
}

fn is_r_package(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// A GitHub repository: `owner/repo`.
fn is_github(name: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    name.split_once('/')
        .is_some_and(|(owner, repo)| part(owner) && part(repo))
}

/// The names typed to install: R's package names or GitHub `owner/repo`
/// (separated by spaces or commas), or pip's requirements
/// (`scanpy[leiden]>=1.10`). None when one isn't a name.
pub(crate) fn package_names(text: &str, language: &str) -> Option<Vec<String>> {
    let python = is_python(language);
    let names: Vec<String> = text
        .split(|c: char| c.is_whitespace() || (!python && c == ','))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    let valid = |n: &String| {
        if python {
            is_requirement(n)
        } else {
            is_r_package(n) || is_github(n)
        }
    };
    (!names.is_empty() && names.iter().all(valid)).then_some(names)
}

/// The topic to open for a package: R's package overview, Python's module.
pub(crate) fn package_topic(pkg: &Package, language: &str) -> HelpRequest {
    if language.eq_ignore_ascii_case("r") {
        return HelpRequest {
            topic: String::new(),
            package: Some(pkg.name.clone()),
            search: false,
        };
    }
    let wanted = pkg.name.to_lowercase().replace('-', "_");
    let module = pkg
        .modules
        .iter()
        .find(|m| m.to_lowercase() == wanted)
        .or_else(|| pkg.modules.iter().find(|m| !m.starts_with('_')))
        .cloned()
        .unwrap_or_else(|| pkg.name.replace('-', "_"));
    HelpRequest {
        topic: module,
        package: None,
        search: false,
    }
}

/// Updates for what's installed now, by name: one updated since drops out.
fn pending<'a>(
    updates: &'a PackageUpdates,
    listed: Option<&Listed>,
) -> Vec<(&'a str, &'a PackageUpdate)> {
    let now: HashMap<&str, &str> = listed
        .map(|l| {
            l.packages
                .iter()
                .map(|p| (p.name.as_str(), p.version.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<(&str, &PackageUpdate)> = Vec::new();
    for u in &updates.updates {
        if now.get(u.name.as_str()) == Some(&u.installed.as_str())
            && !out.iter().any(|(n, _)| *n == u.name)
        {
            out.push((u.name.as_str(), u));
        }
    }
    out
}

fn short_error(err: &str) -> String {
    let err = err.strip_prefix("daemon error: ").unwrap_or(err);
    err.strip_prefix("could not check for updates: ")
        .unwrap_or(err)
        .to_string()
}

impl Workbench {
    fn language_name(&self) -> &'static str {
        if self.active_language().is_some_and(|l| is_python(&l)) {
            "Python"
        } else {
            "R"
        }
    }

    /// Reads the list when the pane shows and it's out of date, or the
    /// session changed. Called while drawing.
    pub(crate) fn sync_packages(&mut self, cx: &mut Context<Self>) {
        let session = self.variables_session().map(str::to_string);
        if session != self.packages.session {
            self.packages.generation += 1;
            self.packages.session = session;
            self.packages.listed = None;
            self.packages.error = None;
            self.packages.updates = None;
            self.packages.review = None;
            self.packages.plan_id += 1;
            self.packages.job_error = None;
            if self
                .packages
                .job
                .as_ref()
                .is_some_and(|j| j.job.finished.is_some())
            {
                self.packages.job = None;
            }
            self.packages.stale = true;
        }
        if self.packages.stale
            && !self.packages.loading
            && self.show_files
            && self.top_tab == TopTab::Packages
        {
            self.refresh_packages(cx);
        }
    }

    /// A run in the session finished: the list may have changed.
    pub(crate) fn packages_run_finished(&mut self, cx: &mut Context<Self>) {
        self.packages.stale = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    pub(crate) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        self.packages.stale = false;
        self.packages.loading = true;
        self.packages.generation += 1;
        let mine = self.packages.generation;
        let read = daemon.call::<Listed>(Call::ListPackages { session });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.packages.generation != mine {
                    return;
                }
                this.packages.loading = false;
                match read {
                    Ok(read) => {
                        this.packages.listed = Some(read);
                        this.packages.error = None;
                    }
                    // Busy with other code: read again when it finishes.
                    Err(err) if format!("{err:#}").contains(BUSY) => {}
                    Err(err) => this.packages.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        self.packages.checking = true;
        let name = self.language_name();
        let found = daemon.call::<PackageUpdates>(Call::PackageUpdates {
            session: session.clone(),
        });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                this.packages.checking = false;
                if this.packages.session.as_deref() != Some(&session) {
                    return;
                }
                this.packages.updates = Some(found.map_err(|err| {
                    let err = format!("{err:#}");
                    if err.contains(BUSY) {
                        format!("{name} is busy; check again once it finishes.")
                    } else {
                        format!("Could not check for updates: {}", short_error(&err))
                    }
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn package_busy(&self) -> bool {
        self.packages.starting
            || self
                .packages
                .job
                .as_ref()
                .is_some_and(|j| j.job.finished.is_none())
    }

    /// The changes from the pane: the session's R and one of its libraries
    /// (the last chosen when it is one, else the first it can write to),
    /// or its Python environment. An R update changes each package in the
    /// library it is in, one change per library, and leaves those in a
    /// library the session can't write to (`skipped`, in plain words).
    /// Also the libraries a single install can go into.
    fn package_changes(
        &self,
        action: PackageAction,
        packages: Vec<String>,
        prefs: &InstallPrefs,
    ) -> Result<PaneChanges, String> {
        let listed = self
            .packages
            .listed
            .as_ref()
            .ok_or("The package list isn't ready yet.")?;
        let name = self.language_name();
        let unknown = format!(
            "Sandhill couldn't tell which {name} this session runs. Refresh the list and try again."
        );
        if listed.program.is_empty() {
            return Err(unknown);
        }
        let change = |target, packages| PackageChange {
            action,
            target,
            packages,
            tool: None,
            reason: "From the Packages pane".into(),
        };
        if name == "Python" {
            if listed.prefix.is_empty() {
                return Err(unknown);
            }
            let target = InstallTarget::Python {
                prefix: listed.prefix.clone(),
                python: listed.program.clone(),
            };
            return Ok((vec![change(target, packages)], Vec::new(), Vec::new()));
        }
        let writable = &listed.writable_libraries;
        let library_of = |package: &str| {
            listed
                .packages
                .iter()
                .find(|p| p.name == package)
                .map(|p| p.library.clone())
                .unwrap_or_default()
        };
        let at = |library: String| InstallTarget::R {
            r: listed.program.clone(),
            library,
        };
        Ok(match action {
            PackageAction::Install => {
                let target = review::r_target(listed.program.clone(), writable, prefs);
                (vec![change(target, packages)], writable.clone(), Vec::new())
            }
            // Removed from where it is.
            PackageAction::Remove => {
                let library = packages.first().map(|p| library_of(p)).unwrap_or_default();
                (vec![change(at(library), packages)], Vec::new(), Vec::new())
            }
            // Updated where it is, so no second copy goes elsewhere.
            PackageAction::Update => {
                let mut groups: Vec<(String, Vec<String>)> = Vec::new();
                for package in packages {
                    let library = library_of(&package);
                    match groups.iter_mut().find(|(l, _)| *l == library) {
                        Some((_, names)) => names.push(package),
                        None => groups.push((library, vec![package])),
                    }
                }
                let (open, closed): (Vec<_>, Vec<_>) =
                    groups.into_iter().partition(|(l, _)| writable.contains(l));
                let skipped = closed
                    .into_iter()
                    .map(|(library, names)| {
                        let library = if library.is_empty() {
                            "a library Sandhill couldn't find".to_string()
                        } else {
                            library
                        };
                        format!(
                            "Can't be updated here: {} in {library}, which this R can't write to.",
                            review::names(&names)
                        )
                    })
                    .collect();
                let changes = open
                    .into_iter()
                    .map(|(library, names)| change(at(library), names))
                    .collect();
                (changes, Vec::new(), skipped)
            }
        })
    }

    /// Works out what installing, updating or removing `packages` does,
    /// shown under `row`'s package (else at the top) before it runs; an
    /// install runs at once when Install Without Asking applies.
    fn ask_packages(
        &mut self,
        action: PackageAction,
        packages: Vec<String>,
        row: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        if packages.is_empty() || self.package_busy() {
            return;
        }
        self.packages.plan_id += 1;
        let id = self.packages.plan_id;
        self.packages.review = Some(PaneReview {
            plans: vec![Review::Planning],
            ids: vec![id],
            skipped: Vec::new(),
            row,
            libraries: Vec::new(),
        });
        self.packages.job_error = None;
        let prefs = daemon.call::<InstallPrefs>(Call::InstallPrefs);
        cx.spawn(async move |this, cx| {
            // Without saved choices, everything asks.
            let prefs = prefs.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                let changes = this.package_changes(action, packages, &prefs);
                let Some(shown) = this.packages.review.as_mut().filter(|r| r.ids == [id]) else {
                    return;
                };
                let (changes, libraries, skipped) = match changes {
                    Ok(found) => found,
                    Err(err) => {
                        shown.plans = vec![Review::Error(err)];
                        cx.notify();
                        return;
                    }
                };
                shown.plans = changes.iter().map(|_| Review::Planning).collect();
                shown.ids = vec![0; changes.len()];
                shown.skipped = skipped;
                shown.libraries = libraries;
                let single = changes.len() == 1;
                for (ix, mut change) in changes.into_iter().enumerate() {
                    let automatic = review::automatic_tool(&change, &prefs).filter(|_| single);
                    change.tool = automatic;
                    this.plan_packages(ix, change, automatic, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Works out the change at `ix` of the review.
    fn plan_packages(
        &mut self,
        ix: usize,
        change: PackageChange,
        automatic: Option<InstallTool>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        self.packages.plan_id += 1;
        let id = self.packages.plan_id;
        let Some(shown) = self.packages.review.as_mut().filter(|r| ix < r.plans.len()) else {
            return;
        };
        shown.plans[ix] = Review::Planning;
        shown.ids[ix] = id;
        let planned = daemon.call::<PackagePlan>(Call::PlanPackages { change });
        cx.spawn(async move |this, cx| {
            let planned = planned.await;
            this.update(cx, |this, cx| {
                let current = this
                    .packages
                    .review
                    .as_ref()
                    .is_some_and(|r| r.ids.get(ix) == Some(&id));
                if !current {
                    return;
                }
                if let (Some(tool), Ok(plan)) = (automatic, &planned)
                    && plan.change.tool == Some(tool)
                    && review::runnable(plan)
                {
                    let plan = plan.clone();
                    this.run_plans(vec![plan], true, cx);
                    return;
                }
                if let Some(shown) = &mut this.packages.review {
                    shown.plans[ix] = planned.into();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Another tool or library for the change at `ix`: worked out again.
    fn change_package_plan(&mut self, ix: usize, choice: Choice, cx: &mut Context<Self>) {
        let Some(Review::Ready(plan)) = self.packages.review.as_ref().and_then(|r| r.plans.get(ix))
        else {
            return;
        };
        let mut change = plan.change.clone();
        choice.apply(&mut change);
        if change != plan.change {
            self.plan_packages(ix, change, None, cx);
        }
    }

    fn cancel_package_review(&mut self, cx: &mut Context<Self>) {
        self.packages.review = None;
        cx.notify();
    }

    /// Runs the shown changes that can run.
    fn run_reviewed_packages(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = &self.packages.review else {
            return;
        };
        let plans: Vec<PackagePlan> = shown
            .plans
            .iter()
            .filter_map(|r| match r {
                Review::Ready(plan) if review::runnable(plan) => Some(plan.clone()),
                _ => None,
            })
            .collect();
        self.run_plans(plans, false, cx);
    }

    /// Runs `plans` as jobs, each starting when the one before finishes.
    fn run_plans(&mut self, plans: Vec<PackagePlan>, automatic: bool, cx: &mut Context<Self>) {
        if self.package_busy() {
            return;
        }
        let mut plans = plans.into_iter();
        let Some(first) = plans.next() else {
            return;
        };
        self.packages.queue = plans.collect();
        self.run_packages(&first, automatic, cx);
    }

    fn run_packages(&mut self, plan: &PackagePlan, automatic: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        if self.package_busy() {
            return;
        }
        self.packages.review = None;
        self.packages.job = None;
        self.packages.job_error = None;
        self.packages.starting = true;
        // An updated module already imported stays the old one until the
        // session restarts.
        let restart = plan.change.action == PackageAction::Update
            && matches!(plan.change.target, InstallTarget::Python { .. })
            && self.packages.listed.as_ref().is_some_and(|l| {
                l.packages
                    .iter()
                    .any(|p| p.loaded && plan.change.packages.contains(&p.name))
            });
        let notice = if automatic {
            Some(review::auto_notice(plan))
        } else if restart {
            Some("Restart the session to use the new version.".to_string())
        } else {
            None
        };
        let mut title = review::progress_title(&plan.change);
        if plan.change.action == PackageAction::Update
            && matches!(plan.change.target, InstallTarget::R { .. })
        {
            title = format!("{title} in {}", plan.folder);
        }
        let started = daemon.call::<QuickInstalling>(Call::RunPackages {
            change: plan.change.clone(),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                this.packages.starting = false;
                match started {
                    Ok(started) => {
                        let job = Job::new(started.job, Some(started.log), title);
                        review::watch(daemon, &job, Self::package_job_status, cx).detach();
                        this.packages.job = Some(PaneJob { job, notice });
                    }
                    Err(err) => {
                        let rest = this.packages_left();
                        this.packages.job_error =
                            Some(format!("{title} didn't start: {err:#}{rest}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Drops the changes waiting to run, saying which, after one failed.
    fn packages_left(&mut self) -> String {
        let left: Vec<String> = std::mem::take(&mut self.packages.queue)
            .iter()
            .map(|plan| {
                format!(
                    "{} in {}",
                    review::names(&plan.change.packages),
                    plan.folder
                )
            })
            .collect();
        if left.is_empty() {
            String::new()
        } else {
            format!(" So these weren't updated: {}.", left.join("; "))
        }
    }

    /// A package job's output and end; the list is read again when it ends,
    /// and the next change waiting starts after one that worked.
    fn package_job_status(
        &mut self,
        id: &str,
        output: Option<String>,
        finished: Option<bool>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.packages.job.as_ref().is_some_and(|j| j.job.id == id) {
            return false;
        }
        let left = if finished == Some(false) {
            self.packages_left()
        } else {
            String::new()
        };
        if let Some(running) = &mut self.packages.job {
            if let Some(output) = output {
                running.job.output = output;
            }
            running.job.finished = finished;
            if finished == Some(false) {
                running.notice = (!left.is_empty()).then(|| left.trim_start().to_string());
            }
        }
        if finished.is_some() {
            self.refresh_packages(cx);
            if finished == Some(true) && !self.packages.queue.is_empty() {
                let next = self.packages.queue.remove(0);
                self.run_packages(&next, false, cx);
            }
        }
        cx.notify();
        true
    }

    fn update_packages(&mut self, names: Vec<String>, row: Option<String>, cx: &mut Context<Self>) {
        self.ask_packages(PackageAction::Update, names, row, cx);
    }

    fn install_packages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.active_language().unwrap_or_default();
        let typed = self.packages.names.read(cx).value().to_string();
        let Some(picked) = package_names(&typed, &language) else {
            self.packages.invalid = true;
            cx.notify();
            return;
        };
        if self.packages.listed.is_none() || self.package_busy() {
            return;
        }
        self.ask_packages(PackageAction::Install, picked, None, cx);
        self.packages
            .names
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.packages.invalid = false;
        self.packages.installing = false;
        cx.notify();
    }

    fn remove_package(&mut self, name: String, cx: &mut Context<Self>) {
        self.ask_packages(PackageAction::Remove, vec![name.clone()], Some(name), cx);
    }

    /// The changes being shown, with Cancel and the verb.
    fn package_review(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let shown = self.packages.review.as_ref()?;
        let cancel = review::cancel_button()
            .on_click(cx.listener(|this, _, _, cx| this.cancel_package_review(cx)));
        let run = review::run_button()
            .on_click(cx.listener(|this, _, _, cx| this.run_reviewed_packages(cx)));
        Some(
            review::packages(
                &shown.plans,
                &shown.skipped,
                &shown.libraries,
                (cancel, run),
                |this: &mut Self, ix, choice, cx: &mut Context<Self>| {
                    this.change_package_plan(ix, choice, cx)
                },
                &p,
                cx,
            )
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .into_any_element(),
        )
    }

    /// The running or last package job, with Show Details.
    fn package_job(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        if self.packages.starting {
            return Some(
                div()
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::PANE_PAD_Y))
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child("Starting…")
                    .into_any_element(),
            );
        }
        if let Some(err) = &self.packages.job_error {
            return Some(
                div()
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::PANE_PAD_Y))
                    .text_size(px(look::small()))
                    .text_color(p.err)
                    .child(err.clone())
                    .into_any_element(),
            );
        }
        let running = self.packages.job.as_ref()?;
        let details = Button::new("pkg-job-details")
            .xsmall()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(running) = &mut this.packages.job {
                    running.job.details = !running.job.details;
                }
                cx.notify();
            }));
        let notice = running
            .notice
            .clone()
            .filter(|_| running.job.finished.is_some());
        Some(
            running
                .job
                .render(details, &p)
                .when_some(notice, |d, notice| {
                    d.child(div().text_color(p.text).child(notice))
                })
                .child(
                    h_flex()
                        .justify_end()
                        .when(running.job.finished.is_some(), |d| {
                            d.child(
                                Button::new("pkg-job-close")
                                    .xsmall()
                                    .label("Done")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.packages.job = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .border_b(px(0.5))
                .border_color(p.glass_edge)
                .into_any_element(),
        )
    }

    /// The buttons on the right of the Packages tab's strip.
    pub(crate) fn package_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let listed = self.packages.listed.is_some();
        let mut tools = vec![
            icon_button("check-updates", "doc.download", &p, false)
                .when(!listed || self.packages.checking, |d| d.opacity(0.35))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Check for updates")
                        .build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.packages.listed.is_some() && !this.packages.checking {
                        this.check_updates(cx)
                    }
                }))
                .into_any_element(),
        ];
        tools.push(
            icon_button("install-packages", "add", &p, false)
                .when(self.packages.installing, |d| d.bg(p.ink(10.)))
                .when(!listed || self.package_busy(), |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.packages.listed.is_some() && !this.package_busy() {
                        this.packages.installing = !this.packages.installing;
                        if this.packages.installing {
                            this.packages
                                .names
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    }
                }))
                .into_any_element(),
        );
        tools.push(
            icon_button("refresh-packages", "refresh", &p, false)
                .when(self.variables_session().is_none(), |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.refresh_packages(cx)))
                .into_any_element(),
        );
        tools
    }

    /// The Packages tab's body.
    pub(crate) fn render_packages(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let language = self.active_language().unwrap_or_default();
        let python = is_python(&language);
        let name = self.language_name();
        let listed = self.packages.listed.as_ref();
        let count = listed.map(|l| {
            let n = l.packages.len();
            format!("{n} installed")
        });
        let filter_bar = h_flex()
            .flex_none()
            .h(px(filter_height()))
            .gap(px(look::GAP_8))
            .px(px(look::PANE_PAD_X))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(
                div().flex_1().child(
                    Input::new(&self.packages.filter)
                        .small()
                        .prefix(icon("filter").size(px(look::ICON))),
                ),
            )
            .when_some(count, |d, c| {
                d.child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child(c),
                )
            });
        let mut top: Vec<AnyElement> = Vec::new();
        if self.packages.installing && listed.is_some() {
            top.push(self.render_install(python, cx));
        }
        if self
            .packages
            .review
            .as_ref()
            .is_some_and(|r| r.row.is_none())
            && let Some(shown) = self.package_review(cx)
        {
            top.push(shown);
        }
        if let Some(job) = self.package_job(cx) {
            top.push(job);
        }
        if let Some(bar) = self.render_updates(cx) {
            top.push(bar);
        }
        let mut body: Vec<AnyElement> = Vec::new();
        let muted_note = |text: String, color: Hsla| {
            div()
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        if self.variables_session().is_none() {
            body.push(muted_note(
                "Start R or Python in the Console, and its installed packages show here.".into(),
                p.muted,
            ));
        } else {
            if let Some(err) = &self.packages.error {
                body.push(muted_note(err.clone(), p.err));
            }
            if listed.is_none() && self.packages.loading {
                body.push(muted_note(format!("Reading {name}'s packages…"), p.muted));
            }
            if let Some(listed) = listed {
                let filter = self.packages.filter.read(cx).value().to_string();
                let shown = filter_packages(&listed.packages, &filter);
                if shown.is_empty() {
                    body.push(muted_note(
                        format!("No packages match \"{}\".", filter.trim()),
                        p.muted,
                    ));
                }
                let updates: Vec<(&str, &PackageUpdate)> = match &self.packages.updates {
                    Some(Ok(found)) => pending(found, Some(listed)),
                    _ => Vec::new(),
                };
                let (used, rest): (Vec<&Package>, Vec<&Package>) =
                    shown.into_iter().partition(|p| p.attached);
                for (title, group) in [
                    (if python { "Imported" } else { "Attached" }, used),
                    ("Installed", rest),
                ] {
                    if group.is_empty() {
                        continue;
                    }
                    body.push(
                        div()
                            .px(px(look::PANE_PAD_X))
                            .pt(px(look::PANE_PAD_Y))
                            .pb(px(look::GAP_2))
                            .text_size(px(look::small()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.faint)
                            .child(title)
                            .into_any_element(),
                    );
                    for pkg in group {
                        let update = updates
                            .iter()
                            .find(|(n, _)| *n == pkg.name)
                            .map(|(_, u)| *u);
                        body.push(self.package_row(pkg, &language, update, cx));
                        if self.packages.review.as_ref().and_then(|r| r.row.as_deref())
                            == Some(pkg.name.as_str())
                            && let Some(shown) = self.package_review(cx)
                        {
                            body.push(shown);
                        }
                    }
                }
            }
        }
        v_flex()
            .relative()
            .size_full()
            .child(look::PaneScroll::new(
                "packages-scroll",
                look::strip() + filter_height(),
                0.,
                div()
                    .id("packages")
                    .pb(px(look::PANE_PAD_Y))
                    .children(top)
                    .children(body),
            ))
            .child(look::floating_header(&p, look::strip(), false, filter_bar))
            .into_any_element()
    }

    fn package_row(
        &self,
        pkg: &Package,
        language: &str,
        update: Option<&PackageUpdate>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let hover = p.ink(look::ROW_HOVER);
        let asked = package_topic(pkg, language);
        let remove_name = pkg.name.clone();
        let update_name = pkg.name.clone();
        let status = package_status(pkg, language);
        h_flex()
            .id(SharedString::from(format!("pkg-{}", pkg.name)))
            .mx(px(look::GAP_4))
            .px(px(look::GAP_8))
            .gap(px(look::GAP_4))
            .h(px(look::row()))
            .rounded(px(look::CONTROL_RADIUS))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .on_click(
                cx.listener(move |this, _, window, cx| this.ask_help(asked.clone(), window, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(p.faint)
                    .child(icon("package").size(px(look::ICON))),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(pkg.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(p.faint)
                    .child(pkg.version.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .italic()
                    .text_color(p.muted)
                    .child(pkg.title.clone()),
            )
            .map(|d| match update {
                Some(u) => d.child(
                    h_flex()
                        .id(SharedString::from(format!("pkg-update-{}", pkg.name)))
                        .flex_none()
                        .h(px(look::control_row()))
                        .px(px(look::GAP_6))
                        .rounded(px(look::CONTROL_RADIUS))
                        .text_size(px(look::small()))
                        .text_color(p.accent)
                        .hover(|d| d.bg(p.ink(look::BUTTON_HOVER)))
                        .child(format!("update {}", u.available))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.update_packages(
                                vec![update_name.clone()],
                                Some(update_name.clone()),
                                cx,
                            )
                        })),
                ),
                None => d.child(
                    div()
                        .flex_none()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child(status),
                ),
            })
            .when(!pkg.base, |d| {
                d.child(
                    icon_button(
                        SharedString::from(format!("pkg-remove-{}", pkg.name)),
                        "delete",
                        &p,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_package(remove_name.clone(), cx)
                    })),
                )
            })
            .into_any_element()
    }

    fn render_install(&self, python: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let how = if python {
            "Type pip names, like scanpy or scikit-learn>=1.5. Sandhill shows the tool, the folder and the command before anything runs."
        } else {
            "Type package names or a GitHub owner/repo. Sandhill shows the tool, the library and the command before anything runs."
        };
        let typed = !self.packages.names.read(cx).value().trim().is_empty();
        v_flex()
            .flex_none()
            .gap(px(look::GAP_4))
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.packages.names).small()),
                    )
                    .child(
                        Button::new("install-go")
                            .small()
                            .primary()
                            .label("Install…")
                            .disabled(!typed || self.package_busy())
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.install_packages(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(if self.packages.invalid {
                        p.err
                    } else {
                        p.faint
                    })
                    .child(if self.packages.invalid {
                        if python {
                            "Type package names, separated by spaces."
                        } else {
                            "Type package names or GitHub owner/repo, separated by spaces."
                        }
                    } else {
                        how
                    }),
            )
            .into_any_element()
    }

    fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        if !self.packages.checking && self.packages.updates.is_none() {
            return None;
        }
        let listed = self.packages.listed.as_ref();
        let mut failed = false;
        let mut names: Vec<String> = Vec::new();
        let python = self.active_language().is_some_and(|l| is_python(&l));
        let text = if self.packages.checking {
            if python && listed.is_some_and(|l| l.conda) {
                "Checking conda's channels and PyPI for updates…".to_string()
            } else if python {
                "Checking PyPI for updates…".to_string()
            } else if listed.is_some_and(|l| l.bioconductor) {
                "Checking CRAN and Bioconductor for updates…".to_string()
            } else {
                "Checking CRAN for updates…".to_string()
            }
        } else {
            match self.packages.updates.as_ref()? {
                Err(err) => {
                    failed = true;
                    err.clone()
                }
                Ok(found) => {
                    names = pending(found, listed)
                        .iter()
                        .map(|(n, _)| n.to_string())
                        .collect();
                    let missed = if found.unreached.is_empty() {
                        String::new()
                    } else {
                        format!(" Couldn't reach {}.", found.unreached.join(", "))
                    };
                    let n = names.len();
                    if n == 0 {
                        format!("Everything is up to date.{missed}")
                    } else {
                        format!(
                            "{n} update{}: {}.{missed}",
                            if n == 1 { "" } else { "s" },
                            names.join(", ")
                        )
                    }
                }
            }
        };
        let checking = self.packages.checking;
        Some(
            h_flex()
                .flex_none()
                .gap(px(look::GAP_8))
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .border_b(px(0.5))
                .border_color(p.glass_edge)
                .text_size(px(look::small()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if failed { p.err } else { p.muted })
                        .child(text),
                )
                .when(!checking && !names.is_empty(), |d| {
                    d.child(
                        Button::new("update-all")
                            .xsmall()
                            .label("Update All")
                            .disabled(self.package_busy())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_packages(names.clone(), None, cx)
                            })),
                    )
                })
                .when(!checking, |d| {
                    d.child(
                        icon_button("hide-updates", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.packages.updates = None;
                                cx.notify();
                            },
                        )),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{filter_packages, package_names, package_topic};
    use sandhill_proto::Package;

    fn pkg(name: &str, title: &str) -> Package {
        serde_json::from_value(serde_json::json!({
            "name": name, "version": "1.0", "title": title
        }))
        .unwrap()
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(
            package_names("DESeq2, harmony", "R"),
            Some(vec!["DESeq2".into(), "harmony".into()])
        );
        assert_eq!(package_names("DESeq2 ;rm", "R"), None);
        assert_eq!(
            package_names("satijalab/seurat-data", "R"),
            Some(vec!["satijalab/seurat-data".into()])
        );
        assert_eq!(package_names("a/b/c", "R"), None);
        assert_eq!(
            package_names("scanpy[leiden]>=1.10,<2 numpy", "python"),
            Some(vec!["scanpy[leiden]>=1.10,<2".into(), "numpy".into()])
        );
        assert_eq!(package_names("scanpy;import os", "python"), None);
        assert_eq!(package_names("  ", "python"), None);
    }

    #[test]
    fn filter_and_topic() {
        let list = vec![
            pkg("ggplot2", "Grammar of graphics"),
            pkg("grid", "The grid ggplot uses"),
        ];
        let found: Vec<&str> = filter_packages(&list, "ggplot")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(found, ["ggplot2", "grid"]);
        let mut sk = pkg("scikit-learn", "");
        sk.modules = vec!["_distributor".into(), "sklearn".into()];
        assert_eq!(package_topic(&sk, "python").topic, "sklearn");
        assert_eq!(
            package_topic(&sk, "R").package.as_deref(),
            Some("scikit-learn")
        );
    }
}
