//! Welcome's Start group (Huan, 2026-11-19; `docs/design/quick-tools-plan.md`):
//! Explore a Table (a place to drop tables and what can be made from them),
//! Getting Ready (what this Mac still needs, and installing it), and the
//! tables explored, which Recent lists. Opening tables first asks where to
//! keep the session's files.

use std::time::Duration;

use sandhill_proto::{FileText, QuickInstalling, QuickReadiness, QuickTool};

use super::*;

/// How often a running install is looked at.
const INSTALL_POLL: Duration = Duration::from_secs(2);
const CRAN_MAC: &str = "https://cloud.r-project.org/bin/macosx/";

/// What is being installed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Installing {
    Packages,
    Kernel,
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

/// Tables waiting for the "Keep results in" answer.
struct Pending {
    tables: Vec<String>,
    parent: String,
    problem: Option<String>,
    saving: bool,
}

#[derive(Default)]
pub(crate) struct QuickPage {
    readiness: Option<Result<QuickReadiness, String>>,
    checking: bool,
    install: Option<Install>,
    details: bool,
    pending: Option<Pending>,
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
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                this.start.quick.checking = false;
                this.start.quick.readiness = Some(read.map_err(|err| format!("{err:#}")));
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
        let Some(r) = readiness.r.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        let started = match what {
            Installing::Packages => {
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
                let call = daemon.call::<TerminalInfo>(Call::InstallIrkernel { r, project: None });
                cx.background_spawn(async move { call.await.map(|job| (job.id, None)) })
            }
        };
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

    /// Tables chosen or dropped: ask where their session's files go.
    pub(super) fn quick_ask_folder(&mut self, tables: Vec<String>, cx: &mut Context<Self>) {
        let tables: Vec<String> = tables
            .into_iter()
            .filter(|t| crate::quick::is_table(t))
            .collect();
        let Some(first) = tables.first() else {
            self.start.problem = Some("Sandhill explores CSV, TSV, text and Excel tables.".into());
            cx.notify();
            return;
        };
        self.start.page = Page::Explore;
        self.start.quick.pending = Some(Pending {
            parent: parent_of(first),
            tables,
            problem: None,
            saving: false,
        });
        cx.notify();
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
            this.update(cx, |this, cx| this.quick_ask_folder(tables, cx))
                .ok();
        })
        .detach();
    }

    fn quick_change_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Keep Results Here".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(pending) = &mut this.start.quick.pending {
                    pending.parent = folder.to_string_lossy().into_owned();
                    pending.problem = None;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// OK: makes the session folder, trusted, and opens its window.
    fn quick_create(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.local_daemon() else {
            return;
        };
        let Some(pending) = &mut self.start.quick.pending else {
            return;
        };
        if pending.saving {
            return;
        }
        pending.saving = true;
        let made = daemon.call::<String>(Call::QuickCreate {
            tables: pending.tables.clone(),
            parent: pending.parent.clone(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let made = made.await;
            this.update(cx, |this, cx| {
                match made {
                    Ok(folder) => {
                        this.start.quick.pending = None;
                        cx.defer(move |cx| crate::shell::app_window::open_quick(folder, cx));
                    }
                    Err(err) => {
                        if let Some(pending) = &mut this.start.quick.pending {
                            pending.saving = false;
                            pending.problem = Some(format!("{err:#}"));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
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
        let kernel_ok = readiness.kernel.is_some();
        if r_ok && kernel_ok && missing.is_empty() && q.install.is_none() {
            return div()
                .px(px(look::PANE_PAD_X))
                .text_color(p.ok)
                .child(format!(
                    "Ready: {r_text} and every package for exploring tables."
                ))
                .into_any_element();
        }
        let installing = q.install.as_ref().filter(|i| !i.failed);
        let mut rows: Vec<AnyElement> = Vec::new();
        let r_row = look::settings_row(
            if r_ok { r_text.clone() } else { "R".into() },
            Some(look::settings_detail(
                if r_ok {
                    look::mac_or("Found on this Mac.", "Found on this computer.").to_string()
                } else {
                    let chip = if cfg!(target_arch = "aarch64") {
                        "the file ending in arm64.pkg (Apple chip)"
                    } else {
                        "the file ending in x86_64.pkg (Intel chip)"
                    };
                    format!(
                        "{r_text} Get R opens R's download page: choose {chip}, open it and follow the installer, then come back here."
                    )
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
        if r_ok {
            let kernel_row = look::settings_row(
                "R kernel",
                Some(look::settings_detail(
                    if kernel_ok {
                        "Installed: Sandhill runs R through it."
                    } else {
                        "Lets Sandhill run R. Installing it takes a minute."
                    },
                    p,
                )),
            );
            rows.push(
                if kernel_ok {
                    kernel_row
                } else {
                    kernel_row.child(
                        Button::new("quick-install-kernel")
                            .small()
                            .primary()
                            .label("Install")
                            .disabled(installing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.quick_install(Installing::Kernel, cx)
                            })),
                    )
                }
                .into_any_element(),
            );
            let packages_row = look::settings_row(
                "R packages",
                Some(look::settings_detail(
                    if missing.is_empty() {
                        "Every package for exploring tables is installed.".to_string()
                    } else {
                        format!(
                            "Missing: {}. They go into your own R library, ready-built, and nothing you already have changes.",
                            missing.join(", ")
                        )
                    },
                    p,
                )),
            );
            rows.push(
                if missing.is_empty() {
                    packages_row
                } else {
                    packages_row.child(
                        Button::new("quick-install-packages")
                            .small()
                            .primary()
                            .label("Install Missing Packages")
                            .disabled(installing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.quick_install(Installing::Packages, cx)
                            })),
                    )
                }
                .into_any_element(),
            );
        }
        if let Some(install) = &q.install {
            let what = match install.what {
                Installing::Packages => "packages",
                Installing::Kernel => "IRkernel",
            };
            let status = if install.failed {
                format!("Installing {what} didn't finish. Show details has R's messages.")
            } else {
                match &install.last {
                    Some(line) => format!("Installing {what}: {line}"),
                    None => format!("Installing {what}…"),
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
            rows.push(row.into_any_element());
        }
        v_flex()
            .gap(px(look::GAP_6))
            .child(
                h_flex()
                    .px(px(look::PANE_PAD_X))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(look::small()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.muted)
                            .child("Getting ready"),
                    )
                    .child(check_again(cx)),
            )
            .child(look::settings_box(rows, p))
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
                this.quick_ask_folder(tables, cx);
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
                // its daemon.
                let found = std::path::Path::new(&path).is_dir();
                let name = crate::quick::session_name(&path);
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

    /// "Keep results in": where the session's folder is made (Huan,
    /// 2026-11-19), in the trust card's style.
    pub(super) fn render_quick_folder_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.start.quick.pending.as_ref()?;
        let p = *Palette::get(cx);
        let first = pending.tables.first()?;
        let stem = std::path::Path::new(first)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let count = pending.tables.len();
        let title = if count == 1 {
            format!("Open {}?", crate::panes::files::base_name(first))
        } else {
            format!("Open {count} tables?")
        };
        let wide = |button: Button| button.w_full();
        let card = v_flex()
            .id("quick-folder-card")
            .occlude()
            .w(px(420.))
            .max_w(relative(0.95))
            .p(px(20.))
            .gap(px(look::GAP_12))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap(px(look::GAP_12))
                    .child(icon("table").size(px(look::ICON_LARGE)))
                    .child(
                        div()
                            .text_size(px(look::title()))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(look::GAP_2))
                    .child(div().text_color(p.muted).child("Keep results in"))
                    .child(
                        h_flex()
                            .gap(px(look::GAP_8))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(pending.parent.clone()),
                            )
                            .child(
                                Button::new("quick-change-folder")
                                    .small()
                                    .label("Change…")
                                    .disabled(pending.saving)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.quick_change_folder(cx)
                                    })),
                            ),
                    )
                    .child(div().text_size(px(look::small())).text_color(p.muted).child(format!(
                        "Sandhill makes the folder “{stem} (Sandhill)” there, with copies of the tables and your plots and results. Your files stay as they are."
                    ))),
            )
            .when_some(pending.problem.clone(), |d, problem| {
                d.child(div().text_color(p.err).child(problem))
            })
            .child(
                v_flex()
                    .gap(px(look::GAP_8))
                    .child(wide(
                        Button::new("quick-folder-ok")
                            .primary()
                            .label(if pending.saving { "Opening…" } else { "OK" })
                            .disabled(pending.saving || pending.problem.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.quick_create(cx))),
                    ))
                    .child(wide(
                        Button::new("quick-folder-cancel")
                            .label("Cancel")
                            .disabled(pending.saving)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start.quick.pending = None;
                                cx.notify();
                            })),
                    )),
            );
        Some(
            deferred(
                div()
                    .id("quick-folder-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// Enter opens, Escape cancels, while the card shows.
    pub(crate) fn quick_folder_keys(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.start.quick.pending.is_none() {
            return false;
        }
        match key {
            "enter" => self.quick_create(cx),
            "escape" => {
                self.start.quick.pending = None;
                cx.notify();
            }
            _ => return false,
        }
        true
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
