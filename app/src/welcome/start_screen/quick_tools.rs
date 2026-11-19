//! Welcome's Start group (Huan, 2026-11-19; `docs/design/quick-tools-plan.md`,
//! `docs/design/explore-next-plan.md`): Explore a Table (a place to drop
//! tables and what can be made from them), Getting Ready (R, where it came
//! from and where packages go, the packages, Claude Code and Codex,
//! installing what is missing and removing what Sandhill installed), and
//! the tables explored, which Recent lists.

use std::collections::HashMap;
use std::time::Duration;

use sandhill_proto::{AgentAccount, FileText, QuickInstalling, QuickReadiness, QuickTool, RSource};

use super::*;

/// How often a running install is looked at.
const INSTALL_POLL: Duration = Duration::from_secs(2);
const CRAN_MAC: &str = "https://cloud.r-project.org/bin/macosx/";
const NODE: &str = "https://nodejs.org/";

/// What is being installed or removed.
#[derive(Clone, PartialEq, Eq)]
enum Installing {
    Packages,
    Kernel,
    /// Claude Code (`claude`) or Codex (`codex`).
    Agent(String),
    Remove,
}

impl Installing {
    fn what(&self) -> String {
        match self {
            Installing::Packages => "Installing packages".into(),
            Installing::Kernel => "Installing IRkernel".into(),
            Installing::Agent(kind) => format!("Installing {}", agent_name(kind)),
            Installing::Remove => "Removing what Sandhill installed".into(),
        }
    }
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

/// A question asked in its row before something runs.
#[derive(Clone, PartialEq, Eq)]
enum Confirm {
    /// Install this agent with its own command.
    Agent(String),
    /// Remove what Sandhill installed.
    Remove,
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
    /// Tables being opened.
    opening: bool,
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
            Installing::Packages => {
                let Some(r) = r else { return };
                let missing: Vec<String> = readiness
                    .packages
                    .iter()
                    .filter(|p| p.version.is_none())
                    .map(|p| p.name.clone())
                    .collect();
                let call = daemon.call::<QuickInstalling>(Call::QuickInstall {
                    r,
                    packages: missing,
                });
                cx.background_spawn(async move { call.await.map(|i| (i.job.id, Some(i.log))) })
            }
            Installing::Kernel => {
                let Some(r) = r else { return };
                let call = daemon.call::<TerminalInfo>(Call::InstallIrkernel { r, project: None });
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
        };
        self.start.quick.confirm = None;
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

    /// Tables chosen or dropped open at once, in Sandhill's own storage:
    /// nothing is made beside them until the user keeps the session
    /// (Huan, 2026-11-19).
    pub(super) fn quick_open(&mut self, tables: Vec<String>, cx: &mut Context<Self>) {
        let tables: Vec<String> = tables
            .into_iter()
            .filter(|t| crate::quick::is_table(t))
            .collect();
        if tables.is_empty() {
            self.start.problem = Some("Sandhill explores CSV, TSV, text and Excel tables.".into());
            cx.notify();
            return;
        }
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        if self.start.quick.opening {
            return;
        }
        self.start.quick.opening = true;
        self.start.page = Page::Explore;
        let started = daemon.call::<String>(Call::QuickStart { tables });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                this.start.quick.opening = false;
                match started {
                    Ok(folder) => {
                        cx.defer(move |cx| crate::shell::app_window::open_quick(folder, cx))
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
                "Coming next",
                vec![
                    offer_row(
                        "Find Changed Genes",
                        "DESeq2 from raw counts and a sample sheet.",
                        &p,
                    ),
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
                    "What this Mac needs to explore tables: R, its kernel and a few R packages. Sandhill checks and installs them for you.",
                    "What this computer needs to explore tables: R, its kernel and a few R packages. Sandhill checks and installs them for you.",
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
            let library = r.library.clone();
            rows.push(
                look::settings_row(
                    "Packages go in",
                    Some(look::settings_detail(
                        format!(
                            "{}: your own R library, which your own R work uses too.",
                            r.library
                        ),
                        p,
                    )),
                )
                .child(
                    Button::new("quick-show-library")
                        .small()
                        .label(look::mac_or("Show in Finder", "Show in Folder"))
                        .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&library))),
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
                            .label("Install")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.quick_install(Installing::Kernel, cx)
                            })),
                    )
                }
                .into_any_element(),
            );
            rows.push(self.quick_packages_row(readiness, r, &missing, busy, p, cx));
        }
        if let Some(install) = &q.install {
            rows.push(self.quick_install_row(install, p, cx));
        }
        let mut agents: Vec<AnyElement> = readiness
            .agents
            .iter()
            .map(|agent| self.quick_agent_row(agent, busy, p, cx))
            .collect();
        if readiness.installed.iter().any(|i| i.ours) {
            agents.push(self.quick_remove_row(readiness, busy, p, cx));
        }
        v_flex()
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .px(px(look::PANE_PAD_X))
                    .child(section_title("R and its packages", p))
                    .child(check_again(cx)),
            )
            .child(look::settings_box(rows, p))
            .child(
                div()
                    .pt(px(look::GAP_12))
                    .px(px(look::PANE_PAD_X))
                    .child(section_title("AI helpers", p)),
            )
            .child(look::settings_box(agents, p))
            .into_any_element()
    }

    /// The packages: what is missing, with Install, then each package with
    /// its version, its library when it isn't the one packages go in, and
    /// whether Sandhill installed it.
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
            "Every package for exploring tables is installed.".to_string()
        } else if r.binaries {
            format!(
                "Missing: {}. They go into your own R library, ready-built, and nothing you already have changes.",
                missing.join(", ")
            )
        } else if r.can_build {
            format!(
                "Missing: {}. This R builds packages on this computer, which can take a few minutes. They go into your own R library, and nothing you already have changes.",
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
                    .label("Install Missing Packages")
                    .disabled(busy || blocked)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.quick_install(Installing::Packages, cx)),
                    ),
            );
        }
        let list = readiness.packages.iter().map(|pkg| {
            let (state, color) = match &pkg.version {
                None if pkg.tool == QuickTool::Plot => ("missing".to_string(), p.err),
                None => ("not installed, needed later".to_string(), p.muted),
                Some(_) if pkg.by_sandhill => ("installed by Sandhill".into(), p.muted),
                Some(_) => ("yours".into(), p.muted),
            };
            let elsewhere = pkg
                .library
                .as_ref()
                .filter(|lib| **lib != r.library)
                .map(|lib| format!(", in {lib}"))
                .unwrap_or_default();
            h_flex()
                .gap(px(look::GAP_8))
                .text_size(px(look::small()))
                .child(div().flex_none().child(pkg.name.clone()))
                .child(div().flex_1().min_w_0().truncate().text_color(color).child(
                    match &pkg.version {
                        Some(version) => format!("{version}: {state}{elsewhere}"),
                        None => state,
                    },
                ))
        });
        v_flex()
            .child(head)
            .child(
                v_flex()
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .gap(px(look::GAP_2))
                    .children(list),
            )
            .into_any_element()
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
                    .mx(px(look::PANE_PAD_X))
                    .mb(px(look::PANE_PAD_Y))
                    .p(px(look::GAP_8))
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.ink(look::ROW_HOVER))
                    .font_family(look::mono())
                    .text_size(px(look::small()))
                    .whitespace_normal()
                    .child(agent.install.clone()),
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
            .child(div().child("Drop a table here"))
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child("CSV, TSV, text or Excel, from an email, Google Drive or Box"),
            )
            .child(
                Button::new("quick-choose")
                    .small()
                    .label("Choose a File…")
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
                                        crate::shell::app_window::open_quick(path, cx)
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
