//! The first-run R and Python setup and kit review for Sandhill's own folder.

use sandhill_proto::{
    CORE_KIT, CopyReport, EnvCard, EnvMode, InstallTarget, KITS, Kit, OWN_FOLDER, OwnRuntime,
    OwnSetup, OwnSetupPlan, RSource,
};

use super::*;

const EXISTING: &str = "Use what's on this computer";
const OWN: &str = "Everything in one folder";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Source,
    Kits,
    Review,
}

#[derive(Default, PartialEq, Eq)]
enum FirstRun {
    #[default]
    Done,
    CheckMode,
    CheckingMode,
    CheckReadiness,
}

struct ExistingReview {
    plans: Vec<Review<PackagePlan>>,
    libraries: Vec<String>,
    quarto: bool,
}

enum SetupReview {
    Own(Review<OwnSetupPlan>),
    Existing(ExistingReview),
}

struct Flow {
    step: Step,
    mode: EnvMode,
    setup: OwnSetup,
    back_to: Option<Page>,
    adding: bool,
    plan: Option<SetupReview>,
    details: bool,
    copy: bool,
    copy_details: bool,
    choosing: bool,
    requesting_tools: bool,
    tools_message: Option<(String, bool)>,
}

#[derive(Default)]
pub(super) struct State {
    first_run: FirstRun,
    flow: Option<Flow>,
    generation: u64,
    pub(super) switching: bool,
    switch_error: Option<String>,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            first_run: if crate::prefs::environment_setup_deferred() {
                FirstRun::Done
            } else {
                FirstRun::CheckMode
            },
            ..Self::default()
        }
    }
}

fn mode_label(mode: EnvMode) -> &'static str {
    match mode {
        EnvMode::Existing => EXISTING,
        EnvMode::Own => OWN,
    }
}

fn suggested(found: &QuickReadiness) -> EnvMode {
    if found.r.is_some() && (found.python.found.is_some() || !found.python.bare.is_empty()) {
        EnvMode::Existing
    } else {
        EnvMode::Own
    }
}

fn detected(found: &QuickReadiness) -> String {
    let r = found.r.as_ref().map(|r| {
        let source = match r.source {
            RSource::Cran => " (CRAN)",
            RSource::Homebrew => " (Homebrew)",
            RSource::Conda => " (conda)",
            RSource::Other => "",
        };
        let version = if r.version.is_empty() {
            String::new()
        } else {
            format!(" {}", r.version)
        };
        format!("R{version}{source}")
    });
    let python = found
        .python
        .found
        .as_ref()
        .map(|python| python_label(&python.version, &python.description))
        .or_else(|| {
            let env = found.python.bare.first()?;
            Some(python_label(&env.python, &format!("conda, {}", env.name)))
        });
    match (r, python) {
        (Some(r), Some(python)) => format!("We found {r} and {python} on this computer."),
        (Some(r), None) => format!("We found {r} on this computer, but no Python."),
        (None, Some(python)) => format!("We found {python} on this computer, but no R."),
        (None, None) => "No R or Python was found.".into(),
    }
}

fn python_label(version: &str, description: &str) -> String {
    let mut label = "Python".to_owned();
    if !version.is_empty() {
        label.push(' ');
        label.push_str(version);
    }
    if !description.is_empty() {
        label.push_str(" (");
        label.push_str(description);
        label.push(')');
    }
    label
}

pub(super) fn own_r_target(r: &OwnRuntime) -> InstallTarget {
    InstallTarget::R {
        r: format!("{}/bin/R", r.prefix),
        library: format!("{}/lib/R/library", r.prefix),
    }
}

fn kit_packages(kit: &Kit, mode: EnvMode, r: bool, python: bool) -> String {
    kit.r
        .iter()
        .filter(|_| r)
        .map(|&(name, _)| name)
        .chain(kit.python.iter().filter(|_| python).map(|&(conda, pypi)| {
            if mode == EnvMode::Own { conda } else { pypi }
        }))
        .chain(
            kit.conda_only
                .iter()
                .filter(|_| r)
                .map(|&name| if name == "quarto" { "Quarto" } else { name }),
        )
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn kit_choice(tick: Checkbox, about: &str, packages: String, p: &Palette) -> Div {
    v_flex()
        .px(px(look::PANE_PAD_X))
        .py(px(look::PANE_PAD_Y))
        .gap(px(look::GAP_2))
        .child(tick)
        .child(look::settings_detail(about.to_owned(), p))
        .child(look::settings_detail(packages, p))
}

fn existing_changes(
    found: &QuickReadiness,
    selected: &[String],
) -> (Vec<PackageChange>, Vec<String>) {
    let kits = || {
        std::iter::once(&CORE_KIT).chain(
            KITS.iter()
                .filter(|kit| selected.iter().any(|id| id == kit.id)),
        )
    };
    let mut changes = Vec::new();
    let mut gsva = None;
    let mut libraries = Vec::new();
    let make = |target, packages| PackageChange {
        action: PackageAction::Install,
        target,
        packages,
        tool: None,
        reason: "Selected for R and Python setup".into(),
    };
    if let Some(r) = &found.r {
        libraries = r
            .libraries
            .iter()
            .filter(|lib| lib.writable)
            .map(|lib| lib.path.clone())
            .collect();
        let target = review::r_target(r.path.clone(), &libraries, &found.prefs);
        let (source_package, packages): (Vec<_>, Vec<_>) = kits()
            .flat_map(|kit| kit.r.iter().map(|&(name, _)| name.to_owned()))
            .partition(|name| name == "GSVA");
        if !source_package.is_empty() {
            // GSVA has no conda build on Apple silicon. Its choice of tool
            // must not prevent the other selected packages from being installed.
            gsva = Some(make(target.clone(), source_package));
        }
        changes.push(make(target, packages));
    }
    let python = found
        .python
        .found
        .as_ref()
        .map(|python| InstallTarget::Python {
            prefix: python.prefix.clone(),
            python: python.program.clone(),
        })
        .or_else(|| {
            found.python.bare.first().map(|env| InstallTarget::Python {
                prefix: env.prefix.clone(),
                python: format!("{}/bin/python", env.prefix),
            })
        });
    if let Some(target) = python {
        let mut seen = HashSet::new();
        let packages = kits()
            .flat_map(|kit| kit.python.iter().map(|&(_, pypi)| pypi))
            .filter(|name| seen.insert(*name))
            .map(str::to_owned)
            .collect();
        changes.push(make(target, packages));
    }
    changes.extend(gsva);
    (changes, libraries)
}

impl AppWindow {
    pub(in crate::welcome::start_screen) fn quick_offer_first_setup(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let q = &mut self.start.quick;
        if self.start.page_chosen
            || !matches!(
                self.start.page,
                Page::Projects | Page::Explore | Page::Ready
            )
        {
            q.environment.first_run = FirstRun::Done;
            return;
        }
        if q.environment.first_run == FirstRun::Done {
            return;
        }
        if let Some(Ok(found)) = &q.readiness {
            q.environment.first_run = FirstRun::Done;
            if found.prefs.mode.is_none() {
                self.quick_begin_setup(None, None, Some(self.start.page.clone()), cx);
            }
            return;
        }
        if q.environment.first_run == FirstRun::CheckReadiness {
            if q.readiness.is_none() {
                self.quick_check(cx);
            }
            return;
        }
        if q.environment.first_run != FirstRun::CheckMode {
            return;
        }
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        self.start.quick.environment.first_run = FirstRun::CheckingMode;
        let request = daemon.call::<InstallPrefs>(Call::InstallPrefs);
        cx.spawn(async move |this, cx| {
            let request = request.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.environment;
                if state.first_run != FirstRun::CheckingMode {
                    return;
                }
                state.first_run = match request {
                    Ok(prefs) if prefs.mode.is_none() => FirstRun::CheckReadiness,
                    _ => FirstRun::Done,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn quick_begin_own_kits(&mut self, python: bool, cx: &mut Context<Self>) {
        self.quick_begin_setup(Some(EnvMode::Own), Some(python), None, cx);
    }

    fn quick_begin_setup(
        &mut self,
        mode: Option<EnvMode>,
        adding_for_python: Option<bool>,
        back_to: Option<Page>,
        cx: &mut Context<Self>,
    ) {
        let q = &mut self.start.quick;
        if q.busy() || q.checking || q.setting_prefs || q.environment.switching {
            return;
        }
        let Some(Ok(found)) = &q.readiness else {
            return;
        };
        let home = self
            .start
            .machines
            .get(sandhill_link::LOCAL)
            .and_then(|m| m.home.clone())
            .or_else(|| std::env::var("HOME").ok());
        let default_folder =
            home.filter(|home| !home.is_empty())
                .map_or_else(String::new, |home| {
                    std::path::Path::new(&home)
                        .join(OWN_FOLDER)
                        .to_string_lossy()
                        .into_owned()
                });
        let (setup, step) = if let Some(python) = adding_for_python {
            let Some(own) = &found.own else { return };
            (
                OwnSetup {
                    folder: own.folder.clone(),
                    r: !python,
                    python,
                    kits: own
                        .kits
                        .iter()
                        .filter(|id| KITS.iter().any(|kit| kit.id == id.as_str()))
                        .cloned()
                        .collect(),
                    copy_from: Vec::new(),
                },
                Step::Kits,
            )
        } else {
            (
                OwnSetup {
                    folder: found
                        .own
                        .as_ref()
                        .map_or(default_folder, |own| own.folder.clone()),
                    r: true,
                    python: true,
                    kits: vec!["bulk".into(), "plots".into()],
                    copy_from: Vec::new(),
                },
                Step::Source,
            )
        };
        let mode = mode.unwrap_or_else(|| suggested(found));
        q.clear_review();
        q.python_form = None;
        q.environment.switch_error = None;
        q.environment.generation += 1;
        q.environment.flow = Some(Flow {
            step,
            mode,
            setup,
            back_to,
            adding: adding_for_python.is_some(),
            plan: None,
            details: false,
            copy: false,
            copy_details: false,
            choosing: false,
            requesting_tools: false,
            tools_message: None,
        });
        self.show_page(Page::Ready, cx);
    }

    fn quick_leave_setup(&mut self, defer: bool, cx: &mut Context<Self>) {
        let state = &mut self.start.quick.environment;
        if state.switching {
            return;
        }
        let Some(flow) = state.flow.take() else {
            return;
        };
        if defer {
            crate::prefs::defer_environment_setup();
        }
        state.generation += 1;
        self.start.quick.clear_review();
        self.start.quick.python_form = None;
        if let Some(page) = flow.back_to {
            self.show_page(page, cx);
        }
        cx.notify();
    }

    fn quick_choose_setup_folder(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.start.quick.environment;
        let Some(flow) = &mut state.flow else { return };
        if flow.choosing {
            return;
        }
        flow.choosing = true;
        let id = state.generation;
        let choose = choose_local_path("Choose Sandhill's Environment Folder", false, true, cx);
        cx.spawn(async move |this, cx| {
            let folder = choose.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.environment;
                if state.generation == id
                    && let Some(flow) = &mut state.flow
                {
                    flow.choosing = false;
                    if let Some(folder) = folder {
                        flow.setup.folder = folder;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_continue_setup(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = &mut self.start.quick.environment.flow else {
            return;
        };
        match flow.step {
            Step::Source => flow.step = Step::Kits,
            Step::Kits => {
                flow.step = Step::Review;
                if flow.mode == EnvMode::Own {
                    self.quick_plan_own_setup(cx);
                } else {
                    self.quick_plan_existing_setup(cx);
                }
            }
            Step::Review => {}
        }
        cx.notify();
    }

    fn quick_back_setup(&mut self, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        let state = &mut q.environment;
        let Some(flow) = &mut state.flow else { return };
        flow.step = match flow.step {
            Step::Review => Step::Kits,
            Step::Kits if !flow.adding => Step::Source,
            _ => return,
        };
        flow.plan = None;
        flow.details = false;
        flow.copy_details = false;
        flow.tools_message = None;
        flow.requesting_tools = false;
        state.generation += 1;
        q.clear_review();
        q.python_form = None;
        cx.notify();
    }

    fn quick_plan_own_setup(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let state = &mut self.start.quick.environment;
        let Some(flow) = &mut state.flow else { return };
        let pending = daemon.call::<OwnSetupPlan>(Call::PlanOwnSetup {
            setup: flow.setup.clone(),
        });
        state.generation += 1;
        let id = state.generation;
        flow.plan = Some(SetupReview::Own(Review::Planning));
        flow.tools_message = None;
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.environment;
                if state.generation == id
                    && let Some(flow) = &mut state.flow
                {
                    flow.plan = Some(SetupReview::Own(result.into()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_plan_existing_setup(&mut self, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        let (Some(Ok(found)), Some(flow)) = (&q.readiness, &mut q.environment.flow) else {
            return;
        };
        let (changes, libraries) = existing_changes(found, &flow.setup.kits);
        let quarto = flow.setup.kits.iter().any(|id| id == "plots");
        flow.plan = Some(SetupReview::Existing(ExistingReview {
            plans: changes.iter().map(|_| Review::Planning).collect(),
            libraries,
            quarto,
        }));
        q.environment.generation += 1;
        for (ix, change) in changes.into_iter().enumerate() {
            self.quick_request_setup_package(ix, change, cx);
        }
        cx.notify();
    }

    fn quick_request_setup_package(
        &mut self,
        ix: usize,
        change: PackageChange,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let id = self.start.quick.environment.generation;
        let pending = daemon.call::<PackagePlan>(Call::PlanPackages { change });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.environment;
                if state.generation == id
                    && let Some(SetupReview::Existing(review)) =
                        state.flow.as_mut().and_then(|flow| flow.plan.as_mut())
                    && let Some(plan) = review.plans.get_mut(ix)
                {
                    *plan = result.into();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn quick_resume_setup(&mut self, cx: &mut Context<Self>) {
        if self
            .start
            .quick
            .environment
            .flow
            .as_ref()
            .is_some_and(|flow| flow.step == Step::Review && flow.mode == EnvMode::Existing)
        {
            self.quick_plan_existing_setup(cx);
        }
    }

    fn quick_pick_setup_package(&mut self, ix: usize, choice: Choice, cx: &mut Context<Self>) {
        let Some(SetupReview::Existing(review)) = self
            .start
            .quick
            .environment
            .flow
            .as_mut()
            .and_then(|flow| flow.plan.as_mut())
        else {
            return;
        };
        let Some(at @ Review::Ready(_)) = review.plans.get_mut(ix) else {
            return;
        };
        let Review::Ready(plan) = at else {
            unreachable!()
        };
        let mut change = plan.change.clone();
        choice.apply(&mut change);
        if change != plan.change {
            *at = Review::Planning;
            self.quick_request_setup_package(ix, change, cx);
            cx.notify();
        }
    }

    fn quick_setup_developer_tools(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let state = &mut self.start.quick.environment;
        let Some(flow) = &mut state.flow else { return };
        if flow.requesting_tools {
            return;
        }
        flow.requesting_tools = true;
        flow.tools_message = None;
        let id = state.generation;
        let request = daemon.call::<()>(Call::InstallDeveloperTools);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.environment;
                if state.generation == id
                    && let Some(flow) = &mut state.flow
                {
                    flow.requesting_tools = false;
                    flow.tools_message = Some(match result {
                        Ok(()) => (
                            "Finish in Apple's installer, then choose Check Again.".into(),
                            false,
                        ),
                        Err(error) => (error.to_string(), true),
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn quick_run_setup(&mut self, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        if q.busy() || q.checking || q.setting_prefs || q.environment.switching {
            return;
        }
        let Some(flow) = &q.environment.flow else {
            return;
        };
        match &flow.plan {
            Some(SetupReview::Own(Review::Ready(plan))) if plan.refused.is_none() => {
                let setup = plan.setup.clone();
                let title = if flow.adding {
                    "Adding kits"
                } else if !setup.copy_from.is_empty() {
                    "Setting up and copying packages"
                } else {
                    "Setting up Sandhill's own R and Python"
                };
                q.environment.flow = None;
                self.quick_start_install(
                    Call::RunOwnSetup { setup },
                    Section::Environment,
                    title.into(),
                    None,
                    cx,
                );
            }
            Some(SetupReview::Existing(review))
                if !review
                    .plans
                    .iter()
                    .any(|plan| matches!(plan, Review::Planning | Review::Error(_))) =>
            {
                let jobs = review
                    .plans
                    .iter()
                    .filter_map(|plan| match plan {
                        Review::Ready(plan) if review::runnable(plan) => Some(plan.change.clone()),
                        _ => None,
                    })
                    .collect();
                self.quick_change_mode(EnvMode::Existing, Some(jobs), cx);
            }
            _ => {}
        }
    }

    fn quick_change_mode(
        &mut self,
        mode: EnvMode,
        jobs: Option<VecDeque<PackageChange>>,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.busy() || q.checking || q.setting_prefs || q.environment.switching {
            return;
        }
        if q.readiness.as_ref().is_none_or(Result::is_err) {
            return;
        }
        if mode == EnvMode::Own {
            self.quick_begin_setup(Some(EnvMode::Own), None, None, cx);
            return;
        }
        q.clear_review();
        q.python_form = None;
        q.environment.switching = true;
        q.environment.switch_error = None;
        let set = daemon.call::<InstallPrefs>(Call::SetEnvMode { mode: Some(mode) });
        cx.spawn(async move |this, cx| {
            let result = set.await;
            this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                q.environment.switching = false;
                match result {
                    Ok(prefs) => {
                        if let Some(jobs) = jobs {
                            q.environment.flow = None;
                            q.waiting = jobs;
                            if let Some(Ok(found)) = &mut q.readiness {
                                found.prefs = prefs;
                            }
                            if this.quick_advance_jobs(Section::Environment, true, cx) {
                                return;
                            }
                        }
                        this.start.quick.readiness = None;
                        this.quick_check(cx);
                        this.quick_invalidate_cards(cx);
                    }
                    Err(error) => q.environment.switch_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn quick_mode_card(
        &self,
        found: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.start.quick.environment;
        let mut rows =
            vec![look::settings_row("Sandhill's R and Python come from", None).into_any_element()];
        rows.push(
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_6))
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .children([EnvMode::Existing, EnvMode::Own].into_iter().map(|mode| {
                    Button::new(mode_label(mode))
                        .small()
                        .label(mode_label(mode))
                        .when(found.prefs.mode == Some(mode), |b| b.primary())
                        .disabled(busy || state.switching || found.prefs.mode == Some(mode))
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.quick_change_mode(mode, None, cx)
                            }),
                        )
                }))
                .into_any_element(),
        );
        if found.prefs.mode.is_none() {
            rows.push(
                look::settings_row(
                    "Set up R and Python",
                    Some(look::settings_detail(
                        "Choose where they come from and which kits to install.",
                        p,
                    )),
                )
                .child(
                    Button::new("ready-start-setup")
                        .small()
                        .label("Set Up…")
                        .disabled(busy || state.switching)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.quick_begin_setup(None, None, None, cx)
                        })),
                )
                .into_any_element(),
            );
        }
        if let Some(error) = &state.switch_error {
            rows.push(
                look::settings_row(
                    "Couldn't save this choice",
                    Some(look::settings_detail(error.clone(), p)),
                )
                .into_any_element(),
            );
        }
        look::settings_box(rows, p)
    }

    pub(super) fn quick_setup_page(&self, p: &Palette, cx: &mut Context<Self>) -> Option<Div> {
        let q = &self.start.quick;
        let state = &q.environment;
        let flow = state.flow.as_ref()?;
        let found = q.readiness.as_ref().and_then(|result| result.as_ref().ok());
        let card_list = q.cards.list.as_ref();
        let cards = card_list.and_then(|result| result.as_ref().ok());
        let sources = cards.into_iter().flatten().filter(|card| {
            card.kind != "own"
                && match card.language.as_str() {
                    "r" => flow.setup.r,
                    "python" => flow.setup.python,
                    _ => false,
                }
        });
        let busy =
            q.busy() || q.checking || state.switching || flow.choosing || flow.requesting_tools;
        let (step, title) = match flow.step {
            Step::Source => (1, "Where should Sandhill's R and Python come from?"),
            Step::Kits => (2, "What will you work on?"),
            Step::Review => (3, "Review"),
        };
        let step_label = if flow.adding {
            format!("Add Kits · Step {} of 2", step - 1)
        } else {
            format!("Set up R and Python · Step {step} of 3")
        };
        let mut panel = v_flex()
            .w_full()
            .gap(px(look::GAP_12))
            .child(look::settings_detail(step_label, p))
            .child(
                div()
                    .text_size(px(look::welcome_title()))
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            );
        let mut can_continue = !busy;
        match flow.step {
            Step::Source => {
                can_continue &= flow.mode == EnvMode::Existing
                    || ((flow.setup.r || flow.setup.python) && !flow.setup.folder.is_empty());
                let suggestion = found.map(suggested);
                let mut rows = [EnvMode::Existing, EnvMode::Own].into_iter().map(|mode| {
                    let detail = if mode == EnvMode::Own {
                        "Sandhill sets up its own R and Python, separate from the rest of this computer."
                    } else {
                        "Sandhill uses the R, Python and tools already here, and asks before adding anything."
                    };
                    let detail = if suggestion == Some(mode) { format!("Suggested. {detail}") } else { detail.into() };
                    look::settings_row(mode_label(mode), Some(look::settings_detail(detail, p)))
                        .child(Button::new(("setup-source", usize::from(mode == EnvMode::Own))).small()
                            .label(if flow.mode == mode { "Selected" } else { "Choose" })
                            .when(flow.mode == mode, |b| b.primary())
                            .disabled(busy || flow.mode == mode)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(flow) = &mut this.start.quick.environment.flow {
                                    flow.mode = mode;
                                    cx.notify();
                                }
                            })))
                        .into_any_element()
                }).collect::<Vec<_>>();
                if flow.mode == EnvMode::Own {
                    rows.push(
                        look::settings_row(
                            "Folder",
                            Some(look::settings_detail(flow.setup.folder.clone(), p)),
                        )
                        .child(
                            Button::new("setup-folder")
                                .small()
                                .label("Choose Folder…")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.quick_choose_setup_folder(cx)
                                    }),
                                ),
                        )
                        .into_any_element(),
                    );
                    rows.push(
                        self.quick_setup_languages(flow, busy, p, cx)
                            .into_any_element(),
                    );
                }
                panel = panel
                    .children(found.map(|found| look::settings_detail(detected(found), p)))
                    .child(look::settings_detail(
                        "You can also set up later from R and Python in Welcome.",
                        p,
                    ))
                    .child(look::settings_box(rows, p));
            }
            Step::Kits => {
                let mut rows = vec![
                    look::settings_row(
                        "Core is always included",
                        Some(look::settings_detail(CORE_KIT.about, p)),
                    )
                    .into_any_element(),
                ];
                rows.extend(KITS.iter().enumerate().filter_map(|(ix, kit)| {
                    let (r, python) = if flow.mode == EnvMode::Own {
                        (flow.setup.r, flow.setup.python)
                    } else {
                        (true, true)
                    };
                    let packages = kit_packages(kit, flow.mode, r, python);
                    if packages.is_empty() {
                        return None;
                    }
                    let checked = flow.setup.kits.iter().any(|id| id == kit.id);
                    let tick = Checkbox::new(("setup-kit", ix))
                        .label(kit.label)
                        .checked(checked)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            if let Some(flow) = &mut this.start.quick.environment.flow {
                                flow.setup.kits.retain(|id| id != kit.id);
                                if *checked {
                                    flow.setup.kits.push(kit.id.into());
                                }
                                cx.notify();
                            }
                        }));
                    Some(kit_choice(tick, kit.about, packages, p).into_any_element())
                }));
                panel = panel.child(look::settings_box(rows, p));
                if flow.mode == EnvMode::Own && !flow.adding {
                    let sources: Vec<_> = sources.collect();
                    if !sources.is_empty() || flow.copy {
                        can_continue &= !flow.copy
                            || (!q.cards.loading
                                && !flow.setup.copy_from.is_empty()
                                && flow
                                    .setup
                                    .copy_from
                                    .iter()
                                    .all(|id| sources.iter().any(|card| &card.id == id)));
                        panel = panel.child(self.quick_copy_options(flow, &sources, busy, p, cx));
                    } else if let Some(Err(error)) = card_list {
                        panel = panel.child(look::settings_box(
                            vec![
                                look::settings_row(
                                    "Couldn't list environments to copy from",
                                    Some(look::settings_detail(error.clone(), p)),
                                )
                                .child(
                                    Button::new("setup-copy-retry")
                                        .small()
                                        .label("Check Again")
                                        .disabled(q.cards.loading)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.quick_list_cards(cx)),
                                        ),
                                )
                                .into_any_element(),
                            ],
                            p,
                        ));
                    } else if q.cards.loading {
                        can_continue &= found.is_none_or(|found| {
                            found.r.is_none()
                                && found.python.found.is_none()
                                && found.python.bare.is_empty()
                        });
                        panel = panel.child(look::settings_detail(
                            "Checking for environments you can copy from…",
                            p,
                        ));
                    }
                }
            }
            Step::Review => match &flow.plan {
                Some(SetupReview::Own(plan)) => {
                    can_continue &= matches!(plan, Review::Ready(plan) if plan.refused.is_none());
                    let details = Button::new("setup-details").small().on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(flow) = &mut this.start.quick.environment.flow {
                                flow.details = !flow.details;
                                cx.notify();
                            }
                        },
                    ));
                    let developer_tools = v_flex()
                        .gap(px(look::GAP_4))
                        .children(flow.tools_message.as_ref().map(|(text, error)| {
                            look::settings_detail(text.clone(), p)
                                .when(*error, |d| d.text_color(p.err))
                        }))
                        .child(
                            h_flex()
                                .gap(px(look::GAP_6))
                                .child(
                                    Button::new("setup-developer-tools")
                                        .small()
                                        .label("Install Developer Tools")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.quick_setup_developer_tools(cx)
                                        })),
                                )
                                .child(
                                    Button::new("setup-check-developer-tools")
                                        .small()
                                        .label("Check Again")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.quick_plan_own_setup(cx)
                                        })),
                                ),
                        );
                    let copy_details =
                        Button::new("setup-copy-details")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(flow) = &mut this.start.quick.environment.flow {
                                    flow.copy_details = !flow.copy_details;
                                    cx.notify();
                                }
                            }));
                    let content = v_flex()
                        .child(review::own_setup(
                            plan,
                            flow.details,
                            details,
                            developer_tools,
                            p,
                        ))
                        .children(review::own_copy(plan, flow.copy_details, copy_details, p));
                    panel = panel.child(look::settings_box(vec![content.into_any_element()], p));
                }
                Some(SetupReview::Existing(review)) => {
                    can_continue &= found.is_some()
                        && !review
                            .plans
                            .iter()
                            .any(|plan| matches!(plan, Review::Planning | Review::Error(_)));
                    panel =
                        panel.children(found.map(|found| {
                            self.quick_existing_setup_review(review, found, busy, p, cx)
                        }));
                }
                None => can_continue = false,
            },
        }
        Some(
            panel
                .children(
                    state
                        .switch_error
                        .as_ref()
                        .map(|error| look::settings_detail(error.clone(), p).text_color(p.err)),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap(px(look::GAP_8))
                        .child(
                            Button::new("setup-later")
                                .small()
                                .label(if flow.adding {
                                    "Cancel"
                                } else {
                                    "Set up later"
                                })
                                .disabled(state.switching)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let defer = this
                                        .start
                                        .quick
                                        .environment
                                        .flow
                                        .as_ref()
                                        .is_some_and(|flow| !flow.adding);
                                    this.quick_leave_setup(defer, cx);
                                })),
                        )
                        .child(div().flex_1())
                        .when(step > 1 && !(flow.adding && step == 2), |d| {
                            d.child(
                                Button::new("setup-back")
                                    .small()
                                    .label("Back")
                                    .disabled(state.switching)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.quick_back_setup(cx)),
                                    ),
                            )
                        })
                        .child(
                            Button::new("setup-continue")
                                .small()
                                .primary()
                                .label(if step < 3 {
                                    "Continue"
                                } else if flow.adding {
                                    "Add Kits"
                                } else if flow.mode == EnvMode::Own && flow.copy {
                                    "Set Up and Copy"
                                } else {
                                    "Set Up"
                                })
                                .disabled(!can_continue)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this
                                        .start
                                        .quick
                                        .environment
                                        .flow
                                        .as_ref()
                                        .is_some_and(|flow| flow.step == Step::Review)
                                    {
                                        this.quick_run_setup(cx);
                                    } else {
                                        this.quick_continue_setup(cx);
                                    }
                                })),
                        ),
                ),
        )
    }

    fn quick_setup_languages(
        &self,
        flow: &Flow,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        look::settings_row(
            "Set up",
            Some(look::settings_detail(
                "Only the selected languages and their packages are installed.",
                p,
            )),
        )
        .child(
            Checkbox::new("setup-r")
                .label("R")
                .checked(flow.setup.r)
                .disabled(busy)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    if let Some(flow) = &mut this.start.quick.environment.flow {
                        flow.setup.r = *on;
                        if !on {
                            flow.setup.copy_from.retain(|id| !id.starts_with("r:"));
                        }
                        cx.notify();
                    }
                })),
        )
        .child(
            Checkbox::new("setup-python")
                .label("Python")
                .checked(flow.setup.python)
                .disabled(busy)
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    if let Some(flow) = &mut this.start.quick.environment.flow {
                        flow.setup.python = *on;
                        if !on {
                            flow.setup.copy_from.retain(|id| !id.starts_with("python:"));
                        }
                        cx.notify();
                    }
                })),
        )
    }

    fn quick_copy_options(
        &self,
        flow: &Flow,
        sources: &[&EnvCard],
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut rows = vec![
            look::settings_row(
                "What should Sandhill's own R and Python start with?",
                Some(look::settings_detail(
                    "Sandhill only reads the environments you choose. It doesn't change them.",
                    p,
                )),
            )
            .into_any_element(),
        ];
        rows.push(
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_6))
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .children(
                    [
                        (false, "Only what Sandhill needs"),
                        (true, "Also the packages in my current R / Python"),
                    ]
                    .into_iter()
                    .map(|(copy, label)| {
                        Button::new(("setup-copy-choice", usize::from(copy)))
                            .small()
                            .label(label)
                            .when(flow.copy == copy, |b| b.primary())
                            .disabled(busy || flow.copy == copy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(flow) = &mut this.start.quick.environment.flow {
                                    flow.copy = copy;
                                    flow.setup.copy_from.clear();
                                    cx.notify();
                                }
                            }))
                    }),
                )
                .into_any_element(),
        );
        if flow.copy {
            rows.push(
                look::settings_row(
                    if sources.is_empty() {
                        "No environments available to copy from"
                    } else {
                        "Choose up to one R and one Python environment"
                    },
                    None,
                )
                .into_any_element(),
            );
            rows.extend(sources.iter().enumerate().map(|(ix, card)| {
                let id = card.id.clone();
                let language = card.language.clone();
                let checked = flow.setup.copy_from.contains(&id);
                v_flex()
                    .min_w_0()
                    .gap(px(look::GAP_4))
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::PANE_PAD_Y))
                    .child(
                        Checkbox::new(("setup-copy-source", ix))
                            .label(card.name.clone())
                            .checked(checked)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                if let Some(flow) = &mut this.start.quick.environment.flow {
                                    let prefix = if language == "r" { "r:" } else { "python:" };
                                    flow.setup.copy_from.retain(|id| !id.starts_with(prefix));
                                    if *on {
                                        flow.setup.copy_from.push(id.clone());
                                    }
                                    cx.notify();
                                }
                            })),
                    )
                    .child(look::settings_detail(card.program.clone(), p))
                    .into_any_element()
            }));
        }
        look::settings_box(rows, p)
    }

    pub(super) fn quick_copy_report(
        &self,
        folder: &str,
        report: &CopyReport,
        busy: bool,
        progress: Option<AnyElement>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut rows = vec![
            look::settings_row(
                "Sandhill's own folder",
                Some(look::settings_detail(folder.to_owned(), p)),
            )
            .into_any_element(),
        ];
        rows.extend(progress);
        rows.push(
            look::settings_row(
                "Package Copy Report",
                Some(look::settings_detail(
                    format!("Copy finished on {}.", report.date),
                    p,
                )),
            )
            .into_any_element(),
        );
        rows.push(
            review::copy_report(
                report,
                busy,
                |this: &mut Self, change, cx| {
                    this.quick_request_packages(
                        PackageRow::Retry(Section::Environment),
                        Vec::new(),
                        vec![change],
                        None,
                        cx,
                    );
                },
                p,
                cx,
            )
            .into_any_element(),
        );
        look::settings_box(rows, p)
    }

    fn quick_existing_setup_review(
        &self,
        review: &ExistingReview,
        found: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let missing_r = found.r.is_none();
        let missing_python = found.python.found.is_none() && found.python.bare.is_empty();
        let managed_python = review.plans.iter().any(|plan| matches!(plan, Review::Ready(plan)
            if plan.change.target.language() == "python" && plan.refused.is_some() && plan.tools.is_empty()));
        let mut rows = vec![
            look::settings_row(
                EXISTING,
                Some(look::settings_detail(
                    "These are the changes Sandhill will make for the kits you chose.",
                    p,
                )),
            )
            .into_any_element(),
        ];
        if !review.plans.is_empty() {
            rows.push(
                review::setup_packages(
                    &review.plans,
                    &review.libraries,
                    Self::quick_pick_setup_package,
                    p,
                    cx,
                )
                .into_any_element(),
            );
        }
        if review
            .plans
            .iter()
            .any(|plan| matches!(plan, Review::Ready(plan) if plan.refused.is_some()))
        {
            rows.push(
                look::settings_row("Plans that can't run will be skipped", None).into_any_element(),
            );
        }
        if review.quarto {
            rows.push(look::settings_row("Get Quarto from quarto.org", Some(look::settings_detail("Quarto is part of Plots and reports. Sandhill won't install it in this mode.", p)))
                .child(Button::new("setup-quarto").small().label("Open quarto.org").on_click(|_, _, cx| cx.open_url("https://quarto.org/")))
                .into_any_element());
        }
        if missing_r {
            rows.push(
                look::settings_row(
                    "No R was found",
                    Some(look::settings_detail(
                        "Set up R here, then review the R packages, or continue without them.",
                        p,
                    )),
                )
                .into_any_element(),
            );
            rows.extend(self.quick_r_offers(found, busy, p, cx));
        }
        if missing_python || managed_python {
            rows.push(look::settings_row("Choose a Python environment", Some(look::settings_detail("Set up a Python environment here, then review its packages, or continue without them.", p))).into_any_element());
            rows.extend(self.quick_python_rows(found, busy, p, cx));
        }
        rows.extend(self.quick_install_in(Section::Cards, p, cx));
        look::settings_box(rows, p)
    }
}

#[cfg(test)]
mod tests {
    use super::{detected, existing_changes, suggested};
    use sandhill_proto::{EnvMode, InstallTarget, QuickReadiness};
    use serde_json::json;

    fn found() -> QuickReadiness {
        serde_json::from_value(json!({
            "packages": [],
            "r": {
                "path": "/example/R/bin/R",
                "version": "4.6.2",
                "new_enough": true,
                "source": "homebrew",
                "libraries": [
                    {"path": "/example/system-library", "packages": 20, "writable": false},
                    {"path": "/example/library", "packages": 5, "writable": true}
                ]
            },
            "python": {
                "found": {
                    "program": "/example/py/bin/python",
                    "prefix": "/example/py",
                    "version": "3.12",
                    "description": "conda, omicverse"
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn suggest_the_own_folder_when_either_language_is_missing() {
        let mut ready = found();
        assert_eq!(suggested(&ready), EnvMode::Existing);
        assert_eq!(
            detected(&ready),
            "We found R 4.6.2 (Homebrew) and Python 3.12 (conda, omicverse) on this computer."
        );
        ready.python.found = None;
        assert_eq!(suggested(&ready), EnvMode::Own);
        ready.r = None;
        assert_eq!(detected(&ready), "No R or Python was found.");
    }

    #[test]
    fn existing_plans_keep_core_and_only_target_discovered_installations() {
        let (changes, libraries) = existing_changes(&found(), &["plots".into(), "pathways".into()]);
        assert_eq!(libraries, ["/example/library"]);
        let [r, python, gsva] = changes.as_slice() else {
            panic!("Expected three plans")
        };
        assert_eq!(
            r.target,
            InstallTarget::R {
                r: "/example/R/bin/R".into(),
                library: "/example/library".into(),
            }
        );
        assert!(r.packages.iter().any(|name| name == "IRkernel"));
        assert!(r.packages.iter().any(|name| name == "ComplexHeatmap"));
        assert!(
            !r.packages
                .iter()
                .any(|name| name.eq_ignore_ascii_case("quarto"))
        );
        assert!(!r.packages.iter().any(|name| name == "GSVA"));
        assert_eq!(
            python.target,
            InstallTarget::Python {
                prefix: "/example/py".into(),
                python: "/example/py/bin/python".into(),
            }
        );
        assert!(python.packages.iter().any(|name| name == "ipykernel"));
        assert!(python.packages.iter().any(|name| name == "decoupler"));
        assert_eq!(gsva.packages, ["GSVA"]);
        assert_eq!(gsva.target, r.target);
    }
}
