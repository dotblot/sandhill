//! One card per R or Python environment on this computer.

use std::path::Path;
use std::time::Duration;

use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use sandhill_proto::{
    CORE_KIT, EnvCard, EnvMode, InstallTarget, KITS, Kit, PackageHit, PackageUpdate,
};

use super::*;

const SEARCH_PAUSE: Duration = Duration::from_millis(250);
const CARD_REASON: &str = "Installed from R and Python";

#[derive(Default)]
pub(super) struct State {
    pub(super) list: Option<Result<Vec<EnvCard>, String>>,
    generation: u64,
    pub(super) loading: bool,
    pub(super) focus_explore: bool,
    open: Option<OpenCard>,
    job_card: Option<String>,
    new_offers: bool,
    destination: Option<DestinationForm>,
}

struct OpenCard {
    id: String,
    panel: Panel,
    input: Entity<InputState>,
    _watch: Subscription,
    search: Option<Task<()>>,
    hits: Option<Result<Vec<PackageHit>, String>>,
    selected: Vec<PackageHit>,
    update_request: Option<Task<()>>,
    updates: Option<Result<Vec<PackageUpdate>, String>>,
    kits: HashSet<&'static str>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Panel {
    #[default]
    Add,
    Updates,
    Check,
    Kits,
}

#[derive(Clone, Copy)]
enum More {
    New,
    Duplicate,
    Export,
    Import,
    Delete,
}

#[derive(Clone)]
enum Destination {
    NewR,
    Duplicate { env: String },
    Import { file: String },
}

struct DestinationForm {
    action: Destination,
    parent: String,
    name: Entity<InputState>,
    _watch: Subscription,
}

fn is_r(card: &EnvCard) -> bool {
    card.language == "r"
}

fn conda_source(source: &str) -> bool {
    !matches!(source, "" | "CRAN" | "Bioconductor" | "PyPI")
}

fn writable_libraries(card: &EnvCard) -> Vec<String> {
    card.libraries
        .iter()
        .filter(|lib| lib.writable)
        .map(|lib| lib.path.clone())
        .collect()
}

fn card_target(card: &EnvCard, prefs: &InstallPrefs, libraries: &[String]) -> InstallTarget {
    if is_r(card) {
        review::r_target(card.program.clone(), libraries, prefs)
    } else {
        InstallTarget::Python {
            prefix: card.folder.clone(),
            python: card.program.clone(),
        }
    }
}

fn update_changes(card: &EnvCard, updates: &[PackageUpdate]) -> Vec<PackageChange> {
    let mut groups: Vec<PackageChange> = Vec::new();
    for update in updates {
        let target = if is_r(card) {
            InstallTarget::R {
                r: card.program.clone(),
                library: update.library.clone(),
            }
        } else {
            InstallTarget::Python {
                prefix: card.folder.clone(),
                python: card.program.clone(),
            }
        };
        let tool = if conda_source(&update.source) {
            Some(InstallTool::Conda)
        } else if is_r(card) {
            Some(InstallTool::R)
        } else {
            card.tools
                .iter()
                .map(|info| info.tool)
                .find(|tool| matches!(tool, InstallTool::Uv | InstallTool::Pip))
        };
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.target == target && group.tool == tool)
        {
            group.packages.push(update.name.clone());
        } else {
            groups.push(PackageChange {
                action: PackageAction::Update,
                target,
                tool,
                packages: vec![update.name.clone()],
                reason: "Updated from R and Python".into(),
            });
        }
    }
    groups
}

fn package_names_for_kit(kit: &Kit, r: bool, conda: bool) -> impl Iterator<Item = &'static str> {
    kit.r
        .iter()
        .filter(move |_| r)
        .map(|&(name, _)| name)
        .chain(kit.python.iter().filter(move |_| !r).map(
            move |&(conda_name, pypi)| {
                if conda { conda_name } else { pypi }
            },
        ))
}

fn explore_program(found: &QuickReadiness) -> Option<&str> {
    found.r.as_ref().map(|r| r.path.as_str())
}

fn card_body() -> Div {
    v_flex()
        .w_full()
        .min_w_0()
        .gap(px(look::GAP_6))
        .px(px(look::PANE_PAD_X))
        .py(px(look::PANE_PAD_Y))
}

fn package_list(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .max_h(px(look::row() * 6.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

impl AppWindow {
    pub(super) fn quick_show_explore_r(&mut self, cx: &mut Context<Self>) {
        self.start.quick.cards.focus_explore = true;
        self.show_page(Page::Ready, cx);
    }

    pub(super) fn quick_focus_explore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = &self.start.quick;
        if !q.cards.focus_explore || q.busy() {
            return;
        }
        let (Some(Ok(cards)), Some(Ok(found))) = (&q.cards.list, &q.readiness) else {
            return;
        };
        let id = explore_program(found)
            .and_then(|program| {
                cards
                    .iter()
                    .find(|card| is_r(card) && card.program == program)
            })
            .map(|card| card.id.clone());
        self.start.quick.cards.focus_explore = false;
        if let Some(id) = id {
            if self
                .start
                .quick
                .cards
                .open
                .as_ref()
                .is_none_or(|open| open.id != id)
            {
                self.quick_toggle_card(id, window, cx);
            }
        } else {
            self.start.quick.cards.new_offers = true;
        }
    }

    pub(super) fn quick_list_cards(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let state = &mut self.start.quick.cards;
        if state.loading {
            return;
        }
        state.loading = true;
        let generation = state.generation;
        let pending = daemon.call::<Vec<EnvCard>>(Call::ListEnvironments);
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.start.quick.cards;
                if state.generation != generation {
                    return;
                }
                state.loading = false;
                if let Ok(cards) = &result
                    && state
                        .open
                        .as_ref()
                        .is_some_and(|open| !cards.iter().any(|card| card.id == open.id))
                {
                    state.open = None;
                    state.destination = None;
                    this.start.quick.clear_review();
                }
                this.start.quick.cards.list = Some(result.map_err(|err| err.to_string()));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn quick_invalidate_cards(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.start.quick.cards;
        state.generation += 1;
        state.loading = false;
        if let Some(card) = &mut state.open {
            card.updates = None;
            card.update_request = None;
            card.selected.clear();
        }
        if self.start.page == Page::Ready {
            self.quick_list_cards(cx);
            self.quick_search_cards(cx);
            if self
                .start
                .quick
                .cards
                .open
                .as_ref()
                .is_some_and(|card| card.panel == Panel::Updates)
            {
                self.quick_load_updates(cx);
            }
        } else {
            state.list = None;
        }
    }

    fn quick_toggle_card(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        q.clear_review();
        q.python_form = None;
        q.cards.new_offers = false;
        q.cards.destination = None;
        if q.cards.open.as_ref().is_some_and(|card| card.id == id) {
            q.cards.open = None;
        } else {
            let input =
                cx.new(|cx| InputState::new(window, cx).placeholder("Search for a package"));
            let watch = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.quick_search_cards(cx);
                }
            });
            q.cards.open = Some(OpenCard {
                id,
                panel: Panel::Add,
                input,
                _watch: watch,
                search: None,
                hits: None,
                selected: Vec::new(),
                update_request: None,
                updates: None,
                kits: HashSet::new(),
            });
        }
        cx.notify();
    }

    fn quick_search_cards(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let Some(card) = &mut self.start.quick.cards.open else {
            return;
        };
        card.search = None;
        card.hits = None;
        let query = card.input.read(cx).value().trim().to_owned();
        if !query.is_empty() {
            let env = card.id.clone();
            let input = card.input.downgrade();
            card.search = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SEARCH_PAUSE).await;
                let result = daemon
                    .call::<Vec<PackageHit>>(Call::SearchPackages { env, query })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(card) = &mut this.start.quick.cards.open
                        && card.input.downgrade() == input
                    {
                        card.hits = Some(result.map_err(|err| err.to_string()));
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }

    fn quick_load_updates(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let Some(card) = &mut self.start.quick.cards.open else {
            return;
        };
        card.updates = None;
        let input = card.input.downgrade();
        let request = daemon.call::<Vec<PackageUpdate>>(Call::EnvUpdates {
            env: card.id.clone(),
        });
        card.update_request = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(card) = &mut this.start.quick.cards.open
                    && card.input.downgrade() == input
                {
                    card.updates = Some(result.map_err(|err| err.to_string()));
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn quick_select_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        q.clear_review();
        q.python_form = None;
        q.cards.destination = None;
        q.cards.new_offers = false;
        if let Some(card) = &mut q.cards.open {
            card.panel = panel;
            if panel == Panel::Updates && card.update_request.is_none() {
                self.quick_load_updates(cx);
            }
        }
        cx.notify();
    }

    fn quick_card(&self) -> Option<&EnvCard> {
        let state = &self.start.quick.cards;
        let open = state.open.as_ref()?;
        state
            .list
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|card| card.id == open.id)
    }

    fn quick_install_selected_hits(&mut self, cx: &mut Context<Self>) {
        let q = &self.start.quick;
        let (Some(card), Some(open), Some(Ok(found))) =
            (self.quick_card(), &q.cards.open, &q.readiness)
        else {
            return;
        };
        let libraries = writable_libraries(card);
        let target = card_target(card, &found.prefs, &libraries);
        let mut changes: Vec<PackageChange> = Vec::new();
        for hit in &open.selected {
            if let Some(change) = changes
                .iter_mut()
                .find(|change| change.tool == Some(hit.tool))
            {
                change.packages.push(hit.name.clone());
            } else {
                changes.push(PackageChange {
                    action: PackageAction::Install,
                    target: target.clone(),
                    packages: vec![hit.name.clone()],
                    tool: Some(hit.tool),
                    reason: CARD_REASON.into(),
                });
            }
        }
        self.quick_request_packages(PackageRow::Card, libraries, changes, None, cx);
    }

    fn quick_plan_updates(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        let (Some(card), Some(open)) = (self.quick_card(), &self.start.quick.cards.open) else {
            return;
        };
        let Some(Ok(updates)) = &open.updates else {
            return;
        };
        let updates = match row {
            Some(ix) => updates.get(ix).map_or(&[][..], std::slice::from_ref),
            None => updates.as_slice(),
        };
        let changes = update_changes(card, updates);
        self.quick_request_packages(PackageRow::Card, Vec::new(), changes, None, cx);
    }

    fn quick_install_package_file(&mut self, cx: &mut Context<Self>) {
        let (Some(card), Some(Ok(found))) = (self.quick_card(), &self.start.quick.readiness) else {
            return;
        };
        let libraries = writable_libraries(card);
        let target = card_target(card, &found.prefs, &libraries);
        let plan_id = self.start.quick.plan_id;
        let choose = choose_local_path("Choose a Downloaded Package", true, false, cx);
        cx.spawn(async move |this, cx| {
            let Some(file) = choose.await else { return };
            let _ = this.update(cx, |this, cx| {
                if this.start.quick.plan_id == plan_id {
                    this.quick_request_packages(
                        PackageRow::Card,
                        libraries,
                        vec![PackageChange {
                            action: PackageAction::Install,
                            target,
                            packages: vec![file],
                            tool: None,
                            reason: CARD_REASON.into(),
                        }],
                        None,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn quick_install_selected_kits(&mut self, cx: &mut Context<Self>) {
        let q = &self.start.quick;
        let (Some(card), Some(open), Some(Ok(found))) =
            (self.quick_card(), &q.cards.open, &q.readiness)
        else {
            return;
        };
        if open.kits.is_empty() {
            return;
        }
        let libraries = writable_libraries(card);
        let target = card_target(card, &found.prefs, &libraries);
        let r = is_r(card);
        let conda = card
            .tools
            .first()
            .is_some_and(|tool| tool.tool == InstallTool::Conda);
        let mut seen = HashSet::new();
        let (source, other): (Vec<_>, Vec<_>) = std::iter::once(&CORE_KIT)
            .chain(KITS.iter().filter(|kit| open.kits.contains(kit.id)))
            .flat_map(|kit| package_names_for_kit(kit, r, conda))
            .filter(|name| seen.insert(*name))
            .map(str::to_owned)
            .partition(|package| {
                r && conda
                    && cfg!(all(target_os = "macos", target_arch = "aarch64"))
                    && package == "GSVA"
            });
        let changes = [(other, None), (source, Some(InstallTool::R))]
            .into_iter()
            .filter(|(packages, _)| !packages.is_empty())
            .map(|(packages, tool)| PackageChange {
                action: PackageAction::Install,
                target: target.clone(),
                packages,
                tool,
                reason: "Added kits from R and Python".into(),
            })
            .collect();
        self.quick_request_packages(PackageRow::Card, libraries, changes, None, cx);
    }

    pub(super) fn quick_start_card_job(
        &mut self,
        call: Call,
        title: String,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.start.quick.busy() {
            return;
        }
        let cards = &mut self.start.quick.cards;
        cards.job_card = cards.open.as_ref().map(|open| open.id.clone());
        self.quick_start_install(call, Section::Cards, title, notice, cx);
    }

    fn quick_ask_environment(&mut self, action: EnvAction, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.busy() {
            return;
        }
        q.clear_review();
        q.python_form = None;
        q.cards.destination = None;
        q.reviewing = Some(QuickReview::Environment(Review::Planning));
        let id = q.plan_id;
        let pending = daemon.call::<EnvActionPlan>(Call::PlanEnvAction { action });
        cx.spawn(async move |this, cx| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                if q.plan_id == id {
                    q.reviewing = Some(QuickReview::Environment(result.into()));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn quick_more_action(&mut self, action: More, window: &mut Window, cx: &mut Context<Self>) {
        let q = &mut self.start.quick;
        q.clear_review();
        q.python_form = None;
        q.cards.destination = None;
        q.cards.new_offers = matches!(action, More::New);
        let at = q.cards.open.as_ref().map(|card| card.id.clone());
        match (action, at) {
            (More::New, _) => cx.notify(),
            (More::Import, env) => self.quick_choose_import(env, window, cx),
            (More::Duplicate, Some(env)) => {
                self.quick_choose_destination(Destination::Duplicate { env }, window, cx)
            }
            (More::Export, Some(env)) => self.quick_choose_export(env, cx),
            (More::Delete, Some(env)) => self.quick_ask_environment(EnvAction::Delete { env }, cx),
            _ => {}
        }
    }

    fn quick_choose_import(
        &mut self,
        env: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan_id = self.start.quick.plan_id;
        let choose = choose_local_path("Choose a Package List", true, false, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(file) = choose.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.start.quick.plan_id != plan_id {
                    return;
                }
                let yaml = Path::new(&file)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
                    });
                if yaml {
                    this.quick_choose_destination(Destination::Import { file }, window, cx);
                } else {
                    this.quick_ask_environment(
                        EnvAction::Import {
                            file,
                            env,
                            prefix: None,
                        },
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn quick_choose_destination(
        &mut self,
        action: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let q = &mut self.start.quick;
        q.clear_review();
        q.python_form = None;
        q.cards.destination = None;
        let plan_id = self.start.quick.plan_id;
        let default = match &action {
            Destination::NewR => "r-environment".into(),
            Destination::Import { .. } => "imported-environment".into(),
            Destination::Duplicate { .. } => self
                .quick_card()
                .and_then(|card| Path::new(&card.folder).file_name())
                .map_or_else(
                    || "environment-copy".into(),
                    |name| format!("{}-copy", name.to_string_lossy()),
                ),
        };
        let choose = choose_local_path("Choose Where the New Folder Goes", false, true, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(parent) = choose.await else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.start.quick.plan_id != plan_id {
                    return;
                }
                let name = cx.new(|cx| InputState::new(window, cx).default_value(default));
                let watch = cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify());
                name.update(cx, |input, cx| input.focus(window, cx));
                this.start.quick.cards.destination = Some(DestinationForm {
                    action,
                    parent,
                    name,
                    _watch: watch,
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn quick_choose_r_destination(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quick_choose_destination(Destination::NewR, window, cx);
    }

    fn quick_choose_export(&mut self, env: String, cx: &mut Context<Self>) {
        let Some(card) = self.quick_card() else {
            return;
        };
        let filename = if is_r(card) {
            "r-packages.csv"
        } else if matches!(card.kind.as_str(), "own" | "conda") {
            "environment.yml"
        } else {
            "requirements.txt"
        };
        let folder = self
            .start
            .machines
            .get(sandhill_link::LOCAL)
            .and_then(|machine| machine.home.as_ref())
            .map_or_else(|| Path::new(&card.folder), |home| Path::new(home));
        let plan_id = self.start.quick.plan_id;
        let choose = cx.prompt_for_new_path(folder, Some(filename));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = choose.await {
                let _ = this.update(cx, |this, cx| {
                    if this.start.quick.plan_id == plan_id {
                        this.quick_ask_environment(
                            EnvAction::Export {
                                env,
                                path: path.to_string_lossy().into_owned(),
                            },
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn quick_environment_cards(
        &self,
        found: &QuickReadiness,
        busy: bool,
        own: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let q = &self.start.quick;
        let state = &q.cards;
        let open = state.open.as_ref();
        let cards = state.list.as_ref().and_then(|result| result.as_ref().ok());
        let show_new = !q.cards.new_offers && open.is_none() && cards.is_some();
        let mut head = h_flex()
            .gap(px(look::GAP_6))
            .px(px(look::PANE_PAD_X))
            .pt(px(look::GAP_12))
            .child(section_title("Environments", p))
            .child(check_again(cx));
        if show_new {
            head = head.child(
                Button::new("env-new-top")
                    .small()
                    .label("New Environment…")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.quick_more_action(More::New, window, cx)
                    })),
            );
        }
        if cards.is_some_and(Vec::is_empty) {
            head = head.child(
                Button::new("env-import-empty")
                    .small()
                    .label("Import…")
                    .disabled(busy || q.reviewing.is_some())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.quick_more_action(More::Import, window, cx)
                    })),
            );
        }
        let mut content = v_flex().gap(px(look::GAP_6)).child(head);
        if open.is_none() || state.job_card.as_deref() != open.map(|card| card.id.as_str()) {
            content = content.children(self.quick_install_in(Section::Cards, p, cx));
        }
        if open.is_none() {
            if state.new_offers {
                content = content.child(self.quick_new_environment(found, busy, own, p, cx));
            }
            if let Some(panel) = self.quick_card_confirmation("the environment", busy, p, cx) {
                content = content.child(look::settings_box(vec![panel.into_any_element()], p));
            }
        }
        match &state.list {
            None => content.child(look::settings_box(vec![look::settings_row("Looking for R and Python…", None).into_any_element()], p)),
            Some(Err(error)) => content.child(look::settings_box(vec![look::settings_row("Couldn't list environments", Some(look::settings_detail(error.clone(), p))).into_any_element()], p)),
            Some(Ok(cards)) if cards.is_empty() => content.child(look::settings_box(vec![look::settings_row(
                "No environments found",
                Some(look::settings_detail(if own { "Choose where R and Python come from above to set up Sandhill's folder." }
                    else { "Set up R or add a Python environment to get started." }, p)),
            ).into_any_element()], p)),
            Some(Ok(cards)) => content.children(cards.iter().map(|card| {
                self.quick_environment_card(card, open.filter(|open| open.id == card.id), found, busy, p, cx)
            })),
        }
    }

    fn quick_new_environment(
        &self,
        found: &QuickReadiness,
        busy: bool,
        own: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut rows = vec![look::settings_row("New Environment", own.then(|| look::settings_detail(
            "To see other environments on this page, choose Use what's on this computer above.", p,
        ))).child(Button::new("env-close-new").small().label("Close").on_click(cx.listener(|this, _, _, cx| {
            this.start.quick.cards.new_offers = false;
            this.start.quick.python_form = None;
            this.start.quick.cards.destination = None;
            this.start.quick.clear_review();
            cx.notify();
        }))).into_any_element()];
        if own && let Some(folder) = &found.own {
            rows.extend(
                [
                    (false, "R", folder.r.is_none()),
                    (true, "Python", folder.python.is_none()),
                ]
                .into_iter()
                .filter(|(_, _, missing)| *missing)
                .map(|(python, language, _)| {
                    look::settings_row(format!("{language} in Sandhill's folder"), None)
                        .child(
                            Button::new(("env-new-own-runtime", usize::from(python)))
                                .small()
                                .label("Set Up…")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_begin_own_kits(python, cx);
                                })),
                        )
                        .into_any_element()
                }),
            );
        }
        rows.extend(self.quick_r_offers(found, busy, p, cx));
        rows.extend(self.quick_python_rows(found, busy, p, cx));
        look::settings_box(rows, p)
    }

    fn quick_environment_card(
        &self,
        card: &EnvCard,
        open: Option<&OpenCard>,
        found: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = card.id.clone();
        let explore = is_r(card) && explore_program(found) == Some(card.program.as_str());
        let size = if card.size_mb == 0 {
            "Less than 1 MB".into()
        } else {
            format!("{} MB", card.size_mb)
        };
        let head = look::settings_row(
            card.name.clone(),
            Some(
                v_flex()
                    .gap(px(look::GAP_2))
                    .child(look::settings_detail(card.status.clone(), p))
                    .when(explore, |d| {
                        let (ready, reason) = r_line(found);
                        d.child(look::settings_detail(
                            if ready {
                                "Used by Explore a Table".to_owned()
                            } else {
                                reason
                            },
                            p,
                        ))
                    })
                    .child(look::settings_detail(
                        format!("{size} · {}", card.folder),
                        p,
                    )),
            ),
        )
        .id(SharedString::from(format!("environment-{}", card.id)))
        .cursor_pointer()
        .hover(|d| d.bg(p.ink(look::ROW_HOVER)))
        .on_click(
            cx.listener(move |this, _, window, cx| this.quick_toggle_card(id.clone(), window, cx)),
        )
        .child(
            Button::new("env-disclosure")
                .small()
                .ghost()
                .accessibility_label(if open.is_some() {
                    "Close environment"
                } else {
                    "Open environment"
                })
                .child(
                    icon(if open.is_some() {
                        "chevron.down"
                    } else {
                        "chevron.right"
                    })
                    .size(px(look::ICON)),
                )
                .on_click({
                    let id = card.id.clone();
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.quick_toggle_card(id.clone(), window, cx);
                    })
                }),
        );
        let mut rows = vec![head.into_any_element()];
        if let Some(open) = open {
            let q = &self.start.quick;
            let reviewing = q.reviewing.is_some() || q.cards.destination.is_some();
            let blocked = busy || reviewing;
            rows.extend(self.quick_card_details(card, busy, p, cx));
            rows.push(self.quick_card_actions(open, blocked, cx));
            if let Some(undo) = &card.undo {
                let id = card.id.clone();
                rows.push(
                    look::settings_row(format!("Undo: {} ({})", undo.what, undo.date), None)
                        .child(
                            Button::new("env-undo")
                                .small()
                                .label("Undo Last Change")
                                .disabled(blocked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_ask_environment(
                                        EnvAction::Undo { env: id.clone() },
                                        cx,
                                    )
                                })),
                        )
                        .into_any_element(),
                );
            }
            if q.cards.job_card.as_deref() == Some(card.id.as_str()) {
                rows.extend(self.quick_install_in(Section::Cards, p, cx));
            }
            rows.extend(
                self.quick_card_confirmation(&card.name, busy, p, cx)
                    .map(IntoElement::into_any_element),
            );
            if q.cards.new_offers {
                rows.push(
                    card_body()
                        .child(self.quick_new_environment(
                            found,
                            busy,
                            found.prefs.mode == Some(EnvMode::Own),
                            p,
                            cx,
                        ))
                        .into_any_element(),
                );
            } else {
                let panel = match open.panel {
                    Panel::Add => {
                        self.quick_add_packages(card, open, found.prefs.mode, blocked, p, cx)
                    }
                    Panel::Updates => self.quick_env_updates(card, open, blocked, p, cx),
                    Panel::Check => self.quick_env_check(card, blocked, p, cx),
                    Panel::Kits => self.quick_env_kits(card, open, blocked, p, cx),
                };
                rows.push(panel.into_any_element());
            }
            if explore {
                let missing = found
                    .packages
                    .iter()
                    .filter(|pkg| pkg.tool == QuickTool::Plot && pkg.version.is_none())
                    .map(|pkg| pkg.name.as_str())
                    .collect::<Vec<_>>();
                rows.push(self.quick_packages_row(found, &missing, blocked, p, cx));
            }
        }
        look::settings_box(rows, p)
    }

    fn quick_card_details(
        &self,
        card: &EnvCard,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let kernel = look::settings_row(
            "Kernel",
            Some(look::settings_detail(
                card.kernel
                    .as_deref()
                    .unwrap_or("No kernel found")
                    .to_owned(),
                p,
            )),
        );
        let reviewing = &self.start.quick.reviewing;
        let needs_kernel = is_r(card) && card.kernel.is_none() && card.kind != "own";
        let kernel = if needs_kernel && !matches!(reviewing, Some(QuickReview::Kernel(_))) {
            let program = card.program.clone();
            let canonical = card.id.strip_prefix("r:").unwrap_or(&program).to_owned();
            kernel.child(
                Button::new("env-install-irkernel")
                    .small()
                    .label("Install IRkernel…")
                    .disabled(busy || reviewing.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_plan_kernel(program.clone(), canonical.clone(), cx);
                    })),
            )
        } else {
            kernel
        };
        let mut details = card_body().child(kernel.px(px(0.)).py(px(0.)));
        if is_r(card) {
            details = details.child(section_title("Where R keeps packages", p));
            if card.libraries.is_empty() {
                details = details.child(look::settings_detail("No package folders found.", p));
            }
            details = details.children(card.libraries.iter().enumerate().map(|(ix, lib)| {
                let folder = lib.path.clone();
                h_flex()
                    .gap(px(look::GAP_6))
                    .child(
                        look::settings_detail(
                            format!(
                                "{} · {} packages{}",
                                lib.path,
                                lib.packages,
                                if lib.writable { "" } else { ", read only" }
                            ),
                            p,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        Button::new(("env-reveal-library", ix))
                            .small()
                            .label(look::mac_or("Show in Finder", "Show in Folder"))
                            .on_click(move |_, _, cx| cx.reveal_path(Path::new(&folder))),
                    )
            }));
        } else {
            let folder = card.folder.clone();
            details = details.child(
                h_flex()
                    .gap(px(look::GAP_6))
                    .child(
                        look::settings_detail(format!("Folder: {}", card.folder), p)
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        Button::new("env-reveal-folder")
                            .small()
                            .label(look::mac_or("Show in Finder", "Show in Folder"))
                            .on_click(move |_, _, cx| cx.reveal_path(Path::new(&folder))),
                    ),
            );
        }
        let kits = std::iter::once(&CORE_KIT)
            .chain(KITS)
            .filter(|kit| card.kits.iter().any(|id| id == kit.id))
            .map(|kit| kit.label)
            .collect::<Vec<_>>();
        if !kits.is_empty() {
            details = details.child(look::settings_detail(
                format!("Kits installed: {}", kits.join(", ")),
                p,
            ));
        }
        let mut rows = vec![details.into_any_element()];
        if let Some(QuickReview::Kernel(plan)) = reviewing {
            rows.push(self.quick_kernel_review(card, plan, busy, p, cx));
        }
        rows
    }

    fn quick_card_actions(
        &self,
        card: &OpenCard,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let env = card.id.clone();
        card_body()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(px(look::GAP_6))
                    .children(
                        [(Panel::Add, "Add Packages"), (Panel::Updates, "Updates")]
                            .into_iter()
                            .map(|(panel, label)| {
                                Button::new(label)
                                    .small()
                                    .label(label)
                                    .disabled(busy)
                                    .when(
                                        !self.start.quick.cards.new_offers
                                            && (card.panel == panel
                                                || panel == Panel::Add
                                                    && card.panel == Panel::Kits),
                                        |b| b.primary(),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.quick_select_panel(panel, cx)
                                    }))
                            }),
                    )
                    .child(
                        Button::new("env-check")
                            .small()
                            .label("Check and Repair")
                            .disabled(busy)
                            .when(
                                card.panel == Panel::Check && !self.start.quick.cards.new_offers,
                                |b| b.primary(),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.quick_select_panel(Panel::Check, cx);
                                this.quick_ask_environment(
                                    EnvAction::Check { env: env.clone() },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("env-more")
                            .small()
                            .label("More")
                            .disabled(busy)
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                [
                                    (More::New, "New Environment…"),
                                    (More::Duplicate, "Duplicate…"),
                                    (More::Export, "Export…"),
                                    (More::Import, "Import…"),
                                    (More::Delete, "Delete…"),
                                ]
                                .into_iter()
                                .fold(
                                    menu,
                                    |menu, (action, label)| {
                                        let owner = owner.clone();
                                        menu.item(PopupMenuItem::new(label).on_click(
                                            move |_, window, cx| {
                                                let _ = owner.update(cx, |this, cx| {
                                                    this.quick_more_action(action, window, cx)
                                                });
                                            },
                                        ))
                                    },
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn quick_card_confirmation(
        &self,
        name: &str,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if let Some(QuickReview::Environment(plan)) = &self.start.quick.reviewing {
            return Some(review::environment(
                plan,
                name,
                Self::quick_review_buttons(cx),
                p,
            ));
        }
        if let Some(plan) = self.quick_package_panel(&PackageRow::Card, p, cx) {
            return Some(plan);
        }
        self.quick_destination_form(false, busy, p, cx)
    }

    pub(super) fn quick_destination_form(
        &self,
        new_r: bool,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let form = self.start.quick.cards.destination.as_ref()?;
        if matches!(&form.action, Destination::NewR) != new_r {
            return None;
        }
        let name = form.name.read(cx).value();
        let name = name.trim();
        let valid = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        let prefix = Path::new(&form.parent)
            .join(name)
            .to_string_lossy()
            .into_owned();
        let action = form.action.clone();
        let label = match action {
            Destination::NewR => "Create…",
            Destination::Duplicate { .. } => "Duplicate…",
            Destination::Import { .. } => "Import…",
        };
        Some(
            card_body()
                .child(section_title("Name the new folder", p))
                .child(Input::new(&form.name).small())
                .child(look::settings_detail(prefix.clone(), p))
                .child(
                    h_flex()
                        .justify_end()
                        .gap(px(look::GAP_6))
                        .child(
                            Button::new("env-destination-cancel")
                                .small()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start.quick.cards.destination = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("env-destination-plan")
                                .small()
                                .primary()
                                .label(label)
                                .disabled(busy || !valid)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let prefix = prefix.clone();
                                    match &action {
                                        Destination::NewR => {
                                            this.start.quick.cards.destination = None;
                                            this.quick_plan_runtime(
                                                RuntimeRow::CondaR,
                                                NewRuntime::RConda { prefix },
                                                cx,
                                            );
                                        }
                                        Destination::Duplicate { env } => {
                                            this.quick_ask_environment(
                                                EnvAction::Duplicate {
                                                    env: env.clone(),
                                                    prefix,
                                                },
                                                cx,
                                            );
                                        }
                                        Destination::Import { file } => {
                                            this.quick_ask_environment(
                                                EnvAction::Import {
                                                    file: file.clone(),
                                                    env: None,
                                                    prefix: Some(prefix),
                                                },
                                                cx,
                                            );
                                        }
                                    }
                                })),
                        ),
                ),
        )
    }

    fn quick_add_packages(
        &self,
        card: &EnvCard,
        open: &OpenCard,
        mode: Option<EnvMode>,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let value = open.input.read(cx).value();
        let own = card.kind == "own" && mode == Some(EnvMode::Own);
        let python = !is_r(card);
        let mut body = card_body().child(Input::new(&open.input).small())
            .child(look::settings_detail(if python { "Search by package name, or enter the full path to a downloaded package." }
                else { "Search by package name, enter a GitHub owner/repo, or enter the full path to a downloaded package." }, p))
            .child(h_flex().flex_wrap().gap(px(look::GAP_6))
                .child(Button::new("env-package-file").small().label("Choose a File…").disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.quick_install_package_file(cx))))
                .child(Button::new("env-package-kits").small().label("Add Kits…").disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if own { this.quick_begin_own_kits(python, cx) } else { this.quick_select_panel(Panel::Kits, cx) }
                    }))));
        if !open.selected.is_empty() {
            body = body.child(
                h_flex()
                    .flex_wrap()
                    .gap(px(look::GAP_6))
                    .child(
                        look::settings_detail(
                            format!(
                                "Selected: {}",
                                open.selected
                                    .iter()
                                    .map(|hit| hit.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            p,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        Button::new("env-clear-hits")
                            .small()
                            .label("Clear Selection")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(open) = &mut this.start.quick.cards.open {
                                    open.selected.clear();
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        Button::new("env-install-hits")
                            .small()
                            .label("Install…")
                            .primary()
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.quick_install_selected_hits(cx)),
                            ),
                    ),
            );
        }
        match &open.hits {
            None if !value.trim().is_empty() => body.child(look::settings_detail("Searching…", p)),
            None => body,
            Some(Err(error)) => body.child(
                look::settings_detail(format!("Couldn't search: {error}"), p).text_color(p.err),
            ),
            Some(Ok(hits)) if hits.is_empty() => {
                body.child(look::settings_detail("No packages found.", p))
            }
            Some(Ok(hits)) => body.child(package_list("env-search-results").children(
                hits.iter().enumerate().map(|(ix, hit)| {
                    let checked = open
                        .selected
                        .iter()
                        .any(|picked| picked.name == hit.name && picked.tool == hit.tool);
                    let meta = [
                        hit.version.as_deref(),
                        Some(hit.source.as_str()),
                        hit.installed.as_ref().map(|_| "installed"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    let checkbox = Checkbox::new(("env-package-hit", ix))
                        .label(hit.name.clone())
                        .checked(checked)
                        .disabled(busy)
                        .on_click({
                            let hit = hit.clone();
                            cx.listener(move |this, on: &bool, _, cx| {
                                if let Some(open) = &mut this.start.quick.cards.open {
                                    open.selected.retain(|picked| picked.name != hit.name);
                                    if *on {
                                        open.selected.push(hit.clone());
                                    }
                                    cx.notify();
                                }
                            })
                        });
                    v_flex()
                        .border_t_1()
                        .border_color(p.line)
                        .py(px(look::GAP_6))
                        .pr(px(look::GAP_8))
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap(px(look::GAP_8))
                                .justify_between()
                                .child(checkbox)
                                .child(look::settings_detail(meta, p)),
                        )
                        .when(!hit.title.is_empty(), |d| {
                            d.child(look::settings_detail(hit.title.clone(), p).truncate())
                        })
                }),
            )),
        }
    }

    fn quick_env_kits(
        &self,
        card: &EnvCard,
        open: &OpenCard,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let r = is_r(card);
        let conda = card
            .tools
            .first()
            .is_some_and(|tool| tool.tool == InstallTool::Conda);
        let mut body =
            card_body()
                .child(section_title("Add Kits", p))
                .child(look::settings_detail(
                    "Core packages are included with every kit.",
                    p,
                ));
        for (ix, kit) in KITS.iter().enumerate() {
            let packages = package_names_for_kit(kit, r, conda).collect::<Vec<_>>();
            if packages.is_empty() {
                continue;
            }
            let installed = card.kits.iter().any(|id| id == kit.id);
            body = body.child(environment::kit_choice(
                Checkbox::new(("env-kit", ix))
                    .label(kit.label)
                    .checked(installed || open.kits.contains(kit.id))
                    .disabled(busy || installed)
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        if let Some(open) = &mut this.start.quick.cards.open {
                            if *on {
                                open.kits.insert(kit.id);
                            } else {
                                open.kits.remove(kit.id);
                            }
                            cx.notify();
                        }
                    })),
                if installed {
                    "Already installed."
                } else {
                    kit.about
                },
                packages.join(", "),
                p,
            ));
        }
        if r && open.kits.contains("plots") {
            body = body.child(look::settings_detail(
                "Quarto is installed separately. You can get it from quarto.org.",
                p,
            ));
        }
        body.child(
            h_flex()
                .justify_end()
                .gap(px(look::GAP_6))
                .child(
                    Button::new("env-kits-back")
                        .small()
                        .label("Back")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _, _, cx| this.quick_select_panel(Panel::Add, cx)),
                        ),
                )
                .child(
                    Button::new("env-kits-review")
                        .small()
                        .primary()
                        .label("Add Kits…")
                        .disabled(busy || open.kits.is_empty())
                        .on_click(
                            cx.listener(|this, _, _, cx| this.quick_install_selected_kits(cx)),
                        ),
                ),
        )
    }

    fn quick_env_updates(
        &self,
        card: &EnvCard,
        open: &OpenCard,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = card_body();
        let refresh = Button::new("env-recheck-updates")
            .small()
            .label("Check Again")
            .disabled(busy || open.updates.is_none())
            .on_click(cx.listener(|this, _, _, cx| this.quick_load_updates(cx)));
        let updates = match &open.updates {
            None => return body.child(look::settings_detail("Checking for updates…", p)),
            Some(Err(error)) => {
                return body
                    .child(
                        look::settings_detail(format!("Couldn't check for updates: {error}"), p)
                            .text_color(p.err),
                    )
                    .child(h_flex().child(refresh));
            }
            Some(Ok(updates)) if updates.is_empty() => {
                return body
                    .child(look::settings_detail("No updates found.", p))
                    .child(h_flex().child(refresh));
            }
            Some(Ok(updates)) => updates,
        };
        let mut libraries = updates
            .iter()
            .map(|update| update.library.as_str())
            .collect::<Vec<_>>();
        libraries.sort_unstable();
        libraries.dedup();
        let mut list = package_list("env-update-results");
        for library in libraries {
            if is_r(card) {
                list = list.child(
                    look::settings_detail(library.to_owned(), p)
                        .py(px(look::GAP_4))
                        .font_weight(FontWeight::SEMIBOLD),
                );
            }
            for (ix, update) in updates
                .iter()
                .enumerate()
                .filter(|(_, update)| update.library == library)
            {
                let detail = if update.source.is_empty() {
                    format!("{} to {}", update.installed, update.available)
                } else {
                    format!(
                        "{} to {} · {}",
                        update.installed, update.available, update.source
                    )
                };
                list = list.child(
                    look::settings_row(update.name.clone(), Some(look::settings_detail(detail, p)))
                        .border_t_1()
                        .border_color(p.line)
                        .pl(px(0.))
                        .pr(px(look::GAP_8))
                        .child(
                            Button::new(("env-update-one", ix))
                                .small()
                                .label("Update…")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.quick_plan_updates(Some(ix), cx)
                                })),
                        ),
                );
            }
        }
        body.child(
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_6))
                .child(refresh)
                .child(
                    Button::new("env-update-all")
                        .small()
                        .primary()
                        .label("Update All…")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.quick_plan_updates(None, cx))),
                ),
        )
        .child(list)
    }

    fn quick_env_check(
        &self,
        card: &EnvCard,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = card_body();
        let Some(check) = &card.check else {
            return body.child(look::settings_detail("No check results yet.", p));
        };
        let mut problems = package_list("env-check-results");
        for (ix, problem) in check.problems.iter().enumerate() {
            let problem_row = look::settings_row(
                problem.package.clone(),
                Some(look::settings_detail(problem.reason.clone(), p)),
            )
            .pl(px(0.))
            .pr(px(look::GAP_8))
            .border_t_1()
            .border_color(p.line);
            problems =
                problems.child(problem_row.when_some(problem.fix.as_ref(), |row, change| {
                    let change = change.clone();
                    let libraries = if change.action == PackageAction::Install {
                        writable_libraries(card)
                    } else {
                        Vec::new()
                    };
                    row.child(
                        Button::new(("env-repair", ix))
                            .small()
                            .label("Repair…")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.quick_request_packages(
                                    PackageRow::Card,
                                    libraries.clone(),
                                    vec![change.clone()],
                                    None,
                                    cx,
                                )
                            })),
                    )
                }));
        }
        body.child(look::settings_detail(
            format!("Checked {} packages on {}.", check.checked, check.date),
            p,
        ))
        .child(
            look::settings_detail(
                match &check.kernel {
                    Some(reason) => format!("The kernel didn't start: {reason}"),
                    None if card.kernel.is_some() => "The kernel started.".into(),
                    None => "There was no kernel to start.".into(),
                },
                p,
            )
            .when(check.kernel.is_some(), |d| d.text_color(p.err)),
        )
        .when(check.problems.is_empty(), |d| {
            d.child(look::settings_detail("No package problems found.", p))
        })
        .when(!check.problems.is_empty(), |d| d.child(problems))
    }
}

#[cfg(test)]
mod tests {
    use sandhill_proto::{EnvCard, InstallTarget, InstallTool, PackageUpdate};

    use super::update_changes;

    fn environment(language: &str) -> EnvCard {
        serde_json::from_value(serde_json::json!({
            "id": "test", "language": language, "name": "Test",
            "kind": "conda", "program": "/test/bin/runtime", "folder": "/test",
            "status": "Ready", "tools": [
                {"tool": "conda", "path": "/test/bin/conda"},
                {"tool": "pip", "path": "/test/bin/python"}
            ]
        }))
        .unwrap()
    }

    fn update(name: &str, library: &str, source: &str) -> PackageUpdate {
        PackageUpdate {
            name: name.into(),
            installed: "1.0".into(),
            available: "2.0".into(),
            library: library.into(),
            bioconductor: source == "Bioconductor",
            source: source.into(),
        }
    }

    #[test]
    fn r_updates_stay_in_their_library() {
        let card = environment("r");
        let changes = update_changes(
            &card,
            &[
                update("readxl", "/one", "CRAN"),
                update("DESeq2", "/two", "Bioconductor"),
                update("writexl", "/one", "CRAN"),
            ],
        );
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].packages, ["readxl", "writexl"]);
        assert_eq!(changes[1].packages, ["DESeq2"]);
        assert!(
            matches!(&changes[1].target, InstallTarget::R { library, .. } if library == "/two")
        );
    }

    #[test]
    fn python_updates_keep_conda_and_pypi_separate() {
        let card = environment("python");
        let changes = update_changes(
            &card,
            &[
                update("numpy", "/test", "conda-forge"),
                update("gseapy", "/test", "some-other-conda-source"),
                update("scanpy", "/test", "PyPI"),
            ],
        );
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].packages, ["numpy", "gseapy"]);
        assert_eq!(changes[0].tool, Some(InstallTool::Conda));
        assert_eq!(changes[1].packages, ["scanpy"]);
        assert_eq!(changes[1].tool, Some(InstallTool::Pip));
    }
}
