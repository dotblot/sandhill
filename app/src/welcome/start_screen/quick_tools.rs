//! Welcome's Start group (Huan, 2026-11-19; `docs/design/quick-tools-plan.md`,
//! `docs/design/explore-next-plan.md`, `docs/design/python-ready-plan.md`):
//! Explore a Table (a place to drop tables and what can be made from them),
//! Getting Ready (R, where it came from and where packages go, the
//! packages, Python environments for notebooks, Claude Code and Codex,
//! installing what is missing and removing what Sandhill installed), and
//! the tables explored, which Recent lists.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::checkbox::Checkbox;
use sandhill_proto::{
    AgentAccount, FileText, MINIFORGE_COMMAND, PYTHON_ENV_NAME, PYTHON_SETS, PYTHON_VERSIONS,
    QuickInstalling, QuickPython, QuickReadiness, QuickTool, RSource, ipykernel_args,
    python_create_args, python_env_name_ok,
};

use crate::view::quick_archive::{ArchivePicker, TABLE_TYPES};
use sandhill_proto::{QuickArchiveOpened, QuickArchiveSelection};

use super::*;

/// How often a running install is looked at.
const INSTALL_POLL: Duration = Duration::from_secs(2);
const CRAN_MAC: &str = "https://cloud.r-project.org/bin/macosx/";
const NODE: &str = "https://nodejs.org/";

/// What is being installed or removed.
#[derive(Clone, PartialEq, Eq)]
enum Installing {
    /// The missing packages for exploring tables, into this library.
    Packages(String, QuickTool),
    Kernel(String),
    /// An organism package for gene symbols (with AnnotationDbi when
    /// missing), into `library`.
    Symbols {
        library: String,
        package: String,
    },
    /// Claude Code (`claude`) or Codex (`codex`).
    Agent(String),
    Remove,
    /// A new Python environment.
    CreatePython {
        name: String,
        python: String,
        sets: Vec<String>,
    },
    /// ipykernel into the environment at this prefix.
    Ipykernel(String),
    Miniforge,
}

impl Installing {
    fn what(&self) -> String {
        match self {
            Installing::Packages(_, QuickTool::Differential) => {
                "Installing packages for finding changed genes".into()
            }
            Installing::Packages(..) => "Installing packages".into(),
            Installing::Kernel(_) => "Installing IRkernel".into(),
            Installing::Symbols { package, .. } => format!("Installing {package}"),
            Installing::Agent(kind) => format!("Installing {}", agent_name(kind)),
            Installing::Remove => "Removing what Sandhill installed".into(),
            Installing::CreatePython { name, .. } => {
                format!("Creating the Python environment {name}")
            }
            Installing::Ipykernel(_) => "Adding ipykernel".into(),
            Installing::Miniforge => "Installing Miniforge".into(),
        }
    }

    /// Which box its progress shows in.
    fn section(&self) -> Section {
        match self {
            Installing::Packages(..) | Installing::Kernel(_) | Installing::Symbols { .. } => {
                Section::R
            }
            Installing::CreatePython { .. } | Installing::Ipykernel(_) | Installing::Miniforge => {
                Section::Python
            }
            Installing::Agent(_) | Installing::Remove => Section::Ai,
        }
    }
}

/// Getting Ready's boxes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    R,
    Python,
    Ai,
}

/// The new Python environment's choices, while its form is open.
struct PythonForm {
    name: Entity<InputState>,
    _changed: Subscription,
    version: &'static str,
    sets: Vec<&'static str>,
}

fn agent_name(kind: &str) -> &'static str {
    if kind == "claude" {
        "Claude Code"
    } else {
        "Codex"
    }
}

struct Install {
    what: Installing,
    job: String,
    log: Option<String>,
    /// The install's latest line, as progress.
    last: Option<String>,
    /// Its whole output, for Show details.
    output: String,
    failed: bool,
}

/// What the library being chosen is for.
#[derive(Clone, PartialEq, Eq)]
enum LibraryFor {
    Kernel,
    /// The missing packages for exploring tables.
    Packages(QuickTool),
    /// An organism package for gene symbols.
    Symbols(String),
}

/// A question asked in its row before something runs.
#[derive(Clone, PartialEq, Eq)]
enum Confirm {
    /// Install this agent with its own command.
    Agent(String),
    /// Remove what Sandhill installed.
    Remove,
    /// Which of R's libraries packages or IRkernel go in: asked every
    /// time, and none is ever created (Huan, 2026-11-19).
    Library(LibraryFor),
    /// Add ipykernel to the environment at this prefix.
    Ipykernel(String),
    Miniforge,
}

#[derive(Default)]
pub(crate) struct QuickPage {
    readiness: Option<Result<QuickReadiness, String>>,
    checking: bool,
    install: Option<Install>,
    details: bool,
    confirm: Option<Confirm>,
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
    /// Checks this Mac again the next time the page shows.
    pub(super) fn forget_check(&mut self) {
        if !self.checking {
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
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn quick_install(&mut self, what: Installing, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let Some(Ok(readiness)) = &self.start.quick.readiness else {
            return;
        };
        let r = readiness.r.as_ref().map(|r| r.path.clone());
        let started = match what.clone() {
            Installing::Packages(library, tool) => {
                let Some(r) = r else { return };
                let missing: Vec<String> = readiness
                    .packages
                    .iter()
                    .filter(|p| p.version.is_none() && p.tool == tool)
                    .map(|p| p.name.clone())
                    .collect();
                let call = daemon.call::<QuickInstalling>(Call::QuickInstall {
                    r,
                    packages: missing,
                    library,
                });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Symbols { library, package } => {
                let Some(r) = r else { return };
                let packages: Vec<String> = readiness
                    .packages
                    .iter()
                    .filter(|p| {
                        p.version.is_none() && (p.name == package || p.name == "AnnotationDbi")
                    })
                    .map(|p| p.name.clone())
                    .collect();
                let call = daemon.call::<QuickInstalling>(Call::QuickInstall {
                    r,
                    packages,
                    library,
                });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Kernel(library) => {
                let Some(r) = r else { return };
                let call = daemon.call::<TerminalInfo>(Call::InstallIrkernel {
                    r,
                    project: None,
                    library: Some(library),
                });
                cx.background_spawn(async move { call.await.map(|job| (job.id, None)) })
            }
            Installing::Agent(kind) => {
                let call = daemon.call::<QuickInstalling>(Call::QuickInstallAgent { kind });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Remove => {
                let call = daemon.call::<QuickInstalling>(Call::QuickUninstall);
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::CreatePython { name, python, sets } => {
                let call =
                    daemon.call::<QuickInstalling>(Call::QuickCreatePython { name, python, sets });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Ipykernel(prefix) => {
                let call = daemon.call::<QuickInstalling>(Call::QuickAddIpykernel { prefix });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Miniforge => {
                let call = daemon.call::<QuickInstalling>(Call::QuickInstallMiniforge);
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
        };
        self.start.quick.confirm = None;
        self.start.quick.python_form = None;
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                match started {
                    Ok((job, log)) => {
                        this.start.quick.install = Some(Install {
                            what,
                            job,
                            log,
                            last: None,
                            output: String::new(),
                            failed: false,
                        });
                        this.quick_watch_install(cx);
                    }
                    Err(err) => {
                        this.start.problem = Some(format!("Installing didn't start: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Follows a running install until it ends, then checks again.
    fn quick_watch_install(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let Some(install) = &self.start.quick.install else {
            return;
        };
        let job = install.job.clone();
        let log = install.log.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(INSTALL_POLL).await;
                let terminals = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals).await;
                let output = match &log {
                    Some(path) => daemon
                        .call::<FileText>(Call::ReadFile { path: path.clone() })
                        .await
                        .map(|f| f.text)
                        .unwrap_or_default(),
                    None => String::new(),
                };
                let ended = terminals.as_ref().ok().and_then(|list| {
                    list.iter()
                        .find(|t| t.id == job)
                        .map(|t| (!t.running).then(|| t.exit.as_ref().and_then(|e| e.code)))
                });
                let done = this
                    .update(cx, |this, cx| {
                        let Some(install) = &mut this.start.quick.install else {
                            return true;
                        };
                        if install.job != job {
                            return true;
                        }
                        install.last = output
                            .lines()
                            .rev()
                            .map(str::trim)
                            .find(|l| !l.is_empty())
                            .map(str::to_string);
                        install.output = output.clone();
                        // Found and ended: its exit code says how; still
                        // running: wait; gone from the list: look again.
                        let finished = match ended {
                            Some(Some(code)) => Some(code == Some(0)),
                            Some(None) => None,
                            None if terminals.is_ok() => Some(true),
                            None => None,
                        };
                        cx.notify();
                        match finished {
                            Some(true) => {
                                this.start.quick.install = None;
                                this.quick_check(cx);
                                true
                            }
                            Some(false) => {
                                install.failed = true;
                                this.quick_check(cx);
                                true
                            }
                            None => false,
                        }
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
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
        let (r_ok, r_text) = r_line(readiness);
        let missing: Vec<&str> = readiness
            .packages
            .iter()
            .filter(|p| p.version.is_none() && p.tool == QuickTool::Plot)
            .map(|p| p.name.as_str())
            .collect();
        let busy = q.install.as_ref().is_some_and(|i| !i.failed);
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
                    _ => {
                        let chip = if cfg!(target_arch = "aarch64") {
                            "the file ending in arm64.pkg (Apple chip)"
                        } else {
                            "the file ending in x86_64.pkg (Intel chip)"
                        };
                        format!(
                            "{r_text} Get R opens R's download page: choose {chip}, open it and follow the installer, then come back here."
                        )
                    }
                },
                p,
            )),
        );
        rows.push(
            if r_ok {
                r_row
            } else {
                r_row.child(
                    Button::new("quick-get-r")
                        .small()
                        .primary()
                        .label("Get R…")
                        .on_click(|_, _, cx| cx.open_url(CRAN_MAC)),
                )
            }
            .into_any_element(),
        );
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
                                "Where this R keeps packages, in the order it looks. Sandhill asks which one before it installs anything."
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
                                this.start.quick.confirm =
                                    Some(Confirm::Library(LibraryFor::Kernel));
                                cx.notify();
                            })),
                    )
                }
                .into_any_element(),
            );
            if q.confirm == Some(Confirm::Library(LibraryFor::Kernel)) {
                rows.push(self.quick_library_choice(r, LibraryFor::Kernel, busy, p, cx));
            }
            rows.push(self.quick_packages_row(readiness, r, &missing, busy || !writable, p, cx));
            if let Some(Confirm::Library(
                target @ (LibraryFor::Packages(_) | LibraryFor::Symbols(_)),
            )) = q.confirm.clone()
            {
                rows.push(self.quick_library_choice(r, target, busy, p, cx));
            }
        }
        rows.extend(self.quick_install_in(Section::R, p, cx));
        let mut python = self.quick_python_rows(&readiness.python, busy, p, cx);
        python.extend(self.quick_install_in(Section::Python, p, cx));
        let mut agents: Vec<AnyElement> = readiness
            .agents
            .iter()
            .map(|agent| self.quick_agent_row(agent, busy, p, cx))
            .collect();
        if readiness.installed.iter().any(|i| i.ours) {
            agents.push(self.quick_remove_row(readiness, busy, p, cx));
        }
        agents.extend(self.quick_install_in(Section::Ai, p, cx));
        let heading = |title| {
            div()
                .pt(px(look::GAP_12))
                .px(px(look::PANE_PAD_X))
                .child(section_title(title, p))
        };
        v_flex()
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .px(px(look::PANE_PAD_X))
                    .child(section_title("R and its packages", p))
                    .child(check_again(cx)),
            )
            .child(look::settings_box(rows, p))
            .child(heading("Python for notebooks"))
            .child(look::settings_box(python, p))
            .child(heading("AI helpers"))
            .child(look::settings_box(agents, p))
            .into_any_element()
    }

    /// Python for notebooks (Huan, 2026-11-19; `python-ready-plan.md`): the
    /// kernels found, the mamba used (or Install Miniforge), environments
    /// that could get a kernel, and Create a Python Environment. Each
    /// action shows its command in the row before it runs.
    fn quick_python_rows(
        &self,
        python: &QuickPython,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
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
        match &python.tool {
            Some(tool) => {
                let program = std::path::Path::new(&tool.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                rows.push(
                    look::settings_row(
                        format!("{program} {}", tool.version).trim().to_string(),
                        Some(look::settings_detail(
                            format!("{}. New environments go in {}/envs.", tool.path, tool.root),
                            p,
                        )),
                    )
                    .into_any_element(),
                );
            }
            None => {
                let asking = q.confirm == Some(Confirm::Miniforge);
                let row = look::settings_row(
                    "mamba",
                    Some(look::settings_detail(
                        if asking {
                            "Install Miniforge? Sandhill runs conda-forge's own installer into ~/miniforge3. Your shell's settings stay as they are; to use mamba in Terminal, run ~/miniforge3/bin/mamba shell init yourself."
                        } else {
                            "No mamba or conda found. Miniforge, from conda-forge, brings both."
                        },
                        p,
                    )),
                );
                rows.push(self.quick_confirm_row(
                    row,
                    asking,
                    "miniforge",
                    "Install Miniforge…",
                    "Install",
                    MINIFORGE_COMMAND.to_string(),
                    busy,
                    Confirm::Miniforge,
                    Installing::Miniforge,
                    p,
                    cx,
                ));
            }
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
            rows.push(self.quick_confirm_row(
                row,
                asking,
                &format!("ipykernel-{}", env.prefix),
                "Add ipykernel…",
                "Add",
                ipykernel_args(&env.tool, &env.prefix).join(" "),
                busy,
                Confirm::Ipykernel(env.prefix.clone()),
                Installing::Ipykernel(env.prefix.clone()),
                p,
                cx,
            ));
        }
        let Some(tool) = &python.tool else {
            return rows;
        };
        let new_row = look::settings_row(
            "New environment",
            Some(look::settings_detail(
                "A conda environment with Python and ipykernel, from conda-forge. Its kernel then shows in every notebook's kernel picker.",
                p,
            )),
        );
        let Some(form) = &q.python_form else {
            rows.push(
                new_row
                    .child(
                        Button::new("python-create")
                            .small()
                            .label("Create a Python Environment…")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_python_form(window, cx)
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
        let prefix = format!("{}/envs/{name}", tool.root);
        let command = python_create_args(&tool.path, &prefix, version, &sets).join(" ");
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
                        .children(checks)
                        .child(command_block(command, p))
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
                                        .label("Create")
                                        .disabled(busy || !name_ok)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.quick_install(
                                                Installing::CreatePython {
                                                    name: name.clone(),
                                                    python: version.to_string(),
                                                    sets: sets.clone(),
                                                },
                                                cx,
                                            )
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
    fn quick_python_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(PYTHON_ENV_NAME));
        let changed = cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify());
        self.start.quick.confirm = None;
        self.start.quick.python_form = Some(PythonForm {
            name,
            _changed: changed,
            version: PYTHON_VERSIONS[0],
            sets: PYTHON_SETS.iter().map(|(id, ..)| *id).collect(),
        });
        cx.notify();
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
        install: Installing,
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
                            this.quick_install(install.clone(), cx)
                        })),
                ),
            )
            .child(
                div()
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .child(command_block(command, p)),
            )
            .into_any_element()
    }

    /// "Install into which library?": each of R's libraries the user can
    /// write to, with Install Here, and Cancel.
    fn quick_library_choice(
        &self,
        r: &sandhill_proto::QuickR,
        target: LibraryFor,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let choices = r.libraries.iter().filter(|l| l.writable).map(|lib| {
            let path = lib.path.clone();
            let target = target.clone();
            h_flex()
                .gap(px(look::GAP_8))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(look::small()))
                        .child(lib.path.clone()),
                )
                .child(
                    Button::new(SharedString::from(format!(
                        "quick-install-here-{}",
                        lib.path
                    )))
                    .small()
                    .primary()
                    .label("Install Here")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let library = path.clone();
                        let what = match &target {
                            LibraryFor::Kernel => Installing::Kernel(library),
                            LibraryFor::Packages(tool) => Installing::Packages(library, *tool),
                            LibraryFor::Symbols(package) => Installing::Symbols {
                                library,
                                package: package.clone(),
                            },
                        };
                        this.quick_install(what, cx)
                    })),
                )
        });
        v_flex()
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(div().flex_1().child(match &target {
                        LibraryFor::Kernel => "Install IRkernel into which library?".to_string(),
                        LibraryFor::Packages(QuickTool::Differential) => {
                            "Install the packages for finding changed genes into which library?".to_string()
                        }
                        LibraryFor::Packages(_) => {
                            "Install the missing packages into which library?".to_string()
                        }
                        LibraryFor::Symbols(package) => {
                            format!("Install {package} into which library?")
                        }
                    }))
                    .child(
                        Button::new("quick-library-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.quick.confirm = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child("Your own R work uses these libraries too. Nothing already in them changes."),
            )
            .children(choices)
            .into_any_element()
    }

    /// The packages: what is missing, with Install, then each package with
    /// its version, its library, and whether Sandhill installed it.
    fn quick_packages_row(
        &self,
        readiness: &QuickReadiness,
        r: &sandhill_proto::QuickR,
        missing: &[&str],
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Homebrew's and conda's R build packages from source, which needs
        // a compiler: Apple's developer tools on a Mac.
        let blocked = !missing.is_empty() && !r.binaries && !r.can_build;
        let detail = if missing.is_empty() {
            "Every package for reading and plotting tables is installed.".to_string()
        } else if r.binaries {
            format!(
                "Missing: {}. They go into the library you choose, ready-built, and nothing you already have changes.",
                missing.join(", ")
            )
        } else if r.can_build {
            format!(
                "Missing: {}. This R builds packages on this computer, which can take a few minutes. They go into the library you choose, and nothing you already have changes.",
                missing.join(", ")
            )
        } else {
            format!(
                "Missing: {}. This R builds packages on this computer, which needs {}",
                missing.join(", "),
                look::mac_or(
                    "Apple's developer tools. Open Terminal, run xcode-select --install, follow its window, then Check Again.",
                    "a C compiler. Install your system's build tools, then Check Again.",
                )
            )
        };
        let mut head = look::settings_row("R packages", Some(look::settings_detail(detail, p)));
        if !missing.is_empty() {
            head = head.child(
                Button::new("quick-install-packages")
                    .small()
                    .primary()
                    .label("Install Missing Packages…")
                    .disabled(busy || blocked)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start.quick.confirm =
                            Some(Confirm::Library(LibraryFor::Packages(QuickTool::Plot)));
                        cx.notify();
                    })),
            );
        }
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
                let package = pkg.name.clone();
                Button::new(SharedString::from(format!("quick-symbols-{package}")))
                    .small()
                    .label(format!("Install {package}…"))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start.quick.confirm =
                            Some(Confirm::Library(LibraryFor::Symbols(package.clone())));
                        cx.notify();
                    }))
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
            .when(!symbols.is_empty(), |d| {
                d.child(
                    look::settings_row(
                        "Gene symbols",
                        Some(look::settings_detail(
                            "Explore a Table names genes by symbol with org.Hs.eg.db (human) or org.Mm.eg.db (mouse), about 90 MB each. Install one when you need it.",
                            p,
                        )),
                    )
                    .children(symbols),
                )
            })
            .child({
                let detail = if differential.is_empty() {
                    "DESeq2, edgeR, limma, apeglm, sva and RUVSeq are installed.".to_string()
                } else {
                    format!("Missing: {}. Install these when you want to compare raw counts.{}", differential.join(", "),
                        if !r.binaries && !r.can_build { " This R needs developer tools to install packages." }
                        else if !r.binaries { " This R builds packages on this computer, which can take a few minutes." }
                        else { "" })
                };
                look::settings_row("For finding changed genes", Some(look::settings_detail(detail, p)))
                    .when(!differential.is_empty(), |row| row.child(
                        Button::new("quick-install-differential")
                            .small()
                            .label("Install Missing Packages…")
                            .disabled(busy || (!r.binaries && !r.can_build))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.quick.confirm = Some(Confirm::Library(
                                    LibraryFor::Packages(QuickTool::Differential),
                                ));
                                cx.notify();
                            })),
                    ))
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
        let install = self.start.quick.install.as_ref()?;
        (install.what.section() == section).then(|| self.quick_install_row(install, p, cx))
    }

    /// A running or failed install's progress, with its output on request.
    fn quick_install_row(
        &self,
        install: &Install,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let q = &self.start.quick;
        let what = install.what.what();
        let status = if install.failed {
            format!("{what} didn't finish. Show Details has the messages.")
        } else {
            match &install.last {
                Some(line) => format!("{what}: {line}"),
                None => format!("{what}…"),
            }
        };
        let mut row = v_flex()
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if install.failed { p.err } else { p.muted })
                            .child(status),
                    )
                    .when(install.log.is_some(), |d| {
                        d.child(
                            Button::new("quick-install-details")
                                .small()
                                .label(if q.details {
                                    "Hide Details"
                                } else {
                                    "Show Details"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start.quick.details = !this.start.quick.details;
                                    cx.notify();
                                })),
                        )
                    }),
            );
        if q.details {
            let tail: Vec<&str> = install.output.lines().rev().take(40).collect();
            let text = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            row = row.child(
                div()
                    .id("quick-install-log")
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .p(px(look::GAP_8))
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.ink(look::ROW_HOVER))
                    .font_family(look::mono())
                    .text_size(px(look::small()))
                    .whitespace_normal()
                    .child(text),
            );
        }
        row.into_any_element()
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
                            this.quick_install(Installing::Agent(kind.clone()), cx)
                        })),
                )
            } else if agent.installable {
                row.child(
                    Button::new(SharedString::from(format!("quick-agent-install-{kind}")))
                        .small()
                        .label("Install…")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
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
                    .child(command_block(agent.install.clone(), p)),
            )
            .into_any_element()
    }

    /// Removes what Sandhill installed and is still as it left it, after
    /// asking in the row (Cancel, then a red Remove).
    fn quick_remove_row(
        &self,
        readiness: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ours: Vec<String> = readiness
            .installed
            .iter()
            .filter(|i| i.ours)
            .map(|i| match i.kind.as_str() {
                "r-package" => format!("{} {}", i.name, i.version),
                "agent" => agent_name(&i.name).to_string(),
                "python-env" => format!("the Python environment {}", i.name),
                "ipykernel" => format!(
                    "ipykernel in {}",
                    std::path::Path::new(&i.place)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                "miniforge" => "Miniforge".into(),
                _ => i.name.clone(),
            })
            .collect();
        let asking = self.start.quick.confirm == Some(Confirm::Remove);
        let row = look::settings_row(
            if asking {
                "Remove what Sandhill installed?"
            } else {
                "Installed by Sandhill"
            },
            Some(look::settings_detail(
                if asking {
                    format!(
                        "{}. Your own R work may use these packages too. Settings and sign-ins of Claude Code and Codex stay.",
                        ours.join(", ")
                    )
                } else {
                    format!(
                        "{}. Anything updated since is yours and stays.",
                        ours.join(", ")
                    )
                },
                p,
            )),
        );
        if asking {
            row.child(
                Button::new("quick-remove-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start.quick.confirm = None;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("quick-remove-go")
                    .small()
                    .danger()
                    .label("Remove")
                    .disabled(busy)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.quick_install(Installing::Remove, cx)),
                    ),
            )
        } else {
            row.child(
                Button::new("quick-remove")
                    .small()
                    .label("Remove What Sandhill Installed…")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start.quick.confirm = Some(Confirm::Remove);
                        cx.notify();
                    })),
            )
        }
        .into_any_element()
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

fn check_again(cx: &mut Context<AppWindow>) -> Button {
    Button::new("quick-check")
        .small()
        .label("Check Again")
        .on_click(cx.listener(|this, _, _, cx| {
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

/// A command shown before it runs, in the code font.
fn command_block(command: String, p: &Palette) -> Div {
    div()
        .p(px(look::GAP_8))
        .rounded(px(look::CONTROL_RADIUS))
        .bg(p.ink(look::ROW_HOVER))
        .font_family(look::mono())
        .text_size(px(look::small()))
        .whitespace_normal()
        .child(command)
}
